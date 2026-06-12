use crate::{ai, config, frontmatter, notes, secrets};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const TICK: Duration = Duration::from_secs(5);
const MIN_REAGE: Duration = Duration::from_secs(30);
const MAX_BODY_CHARS: usize = 8000;

pub struct Tagger {
    queue: Mutex<HashSet<PathBuf>>,
    /// path → stable hash of the note body AS LAST TAGGED (post-merge).
    /// Persisted to disk (see `hashes_path`) so a restart doesn't re-send
    /// the entire vault to the AI: previously this map was in-memory only,
    /// and `enqueue_dir` at startup re-tagged every note on every launch —
    /// two API calls and two mtime-bumping rewrites per note (the stored
    /// hash was of the PRE-merge body, so the tagger's own write looked
    /// like a fresh edit on the next pass).
    last_tagged_hash: Mutex<HashMap<String, u64>>,
    last_tagged_at: Mutex<HashMap<PathBuf, Instant>>,
}

/// Where the persisted body-hash map lives. Sidecar config data — never in
/// the notes folder.
fn hashes_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    let _ = std::fs::create_dir_all(&p);
    p.push("tagger_hashes.json");
    p
}

fn load_hashes() -> HashMap<String, u64> {
    let map: HashMap<String, u64> = match crate::config::read_json(&hashes_path()) {
        crate::config::JsonRead::Parsed(m) => m,
        _ => HashMap::new(),
    };
    // Prune entries for files that no longer exist so the map doesn't
    // grow forever across renames/deletes. One stat per entry, startup only.
    map.into_iter()
        .filter(|(p, _)| std::path::Path::new(p).is_file())
        .collect()
}

fn save_hashes(map: &HashMap<String, u64>) {
    // Derived data — best-effort persistence, but atomic so a crash can't
    // leave a truncated file (which read_json would then quarantine).
    if let Ok(json) = serde_json::to_string(map) {
        let _ = notes::write_atomic(hashes_path(), &json);
    }
}

impl Tagger {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(HashSet::new()),
            last_tagged_hash: Mutex::new(load_hashes()),
            last_tagged_at: Mutex::new(HashMap::new()),
        })
    }

    /// Enqueue every `.md` file in the notes dir as a tagging candidate. The
    /// body-hash and recency checks during processing filter out files that
    /// don't actually need an API call.
    pub fn enqueue_dir(&self) {
        let dir = notes::notes_dir();
        let mut q = self.queue.lock().expect("tagger queue lock");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase())
                        == Some("md".to_string())
                {
                    q.insert(path);
                }
            }
        }
    }

    fn next(&self) -> Option<PathBuf> {
        let mut q = self.queue.lock().expect("tagger queue lock");
        let next = q.iter().next().cloned();
        if let Some(p) = &next {
            q.remove(p);
        }
        next
    }

    /// Record `body` as the tagged state for `path` and persist the map.
    fn remember_tagged(&self, path_str: &str, body_trimmed: &str) {
        let mut hashes = self.last_tagged_hash.lock().expect("tagger lock");
        hashes.insert(path_str.to_string(), crate::fnv::fnv1a64_str(body_trimmed));
        save_hashes(&hashes);
    }

    async fn process(&self, path: PathBuf) -> Result<bool, String> {
        // Recency check — don't re-tag the same note within MIN_REAGE.
        {
            let last_at = self.last_tagged_at.lock().expect("tagger lock");
            if let Some(t) = last_at.get(&path) {
                if t.elapsed() < MIN_REAGE {
                    return Ok(false);
                }
            }
        }

        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        // Never tag encrypted notes: we don't have the password and the
        // ciphertext line wouldn't yield meaningful tags anyway.
        if crate::encryption::is_encrypted(&content) {
            return Ok(false);
        }
        let (_fm, body) = frontmatter::split(&content);
        let body_trimmed = body.trim();
        if body_trimmed.is_empty() {
            return Ok(false);
        }
        let path_str = path.to_string_lossy().to_string();

        // Body-hash check — skip if the body is unchanged since we last
        // tagged it (stable hash, persisted across restarts).
        let body_hash = crate::fnv::fnv1a64_str(body_trimmed);
        {
            let hashes = self.last_tagged_hash.lock().expect("tagger lock");
            if hashes.get(&path_str) == Some(&body_hash) {
                return Ok(false);
            }
        }

        let cfg = crate::config::load();
        let provider = cfg.active_provider;
        let key = crate::secrets::get_api_key_for(provider.id()).map_err(|e| e.to_string())?;
        // Strip existing hashtags/canonical line before sending — don't want
        // the AI to anchor on tags we already have when proposing new ones.
        // Prepend `# Title` (from filename stem) so the tagger sees what
        // the note is about even when the body itself is fragmentary.
        let cleaned = crate::tags::strip_tags_for_ai(body_trimmed);
        let truncated: String = cleaned.chars().take(MAX_BODY_CHARS).collect();
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let body_for_api = if title.is_empty() {
            truncated
        } else {
            format!("# {}\n\n{}", title, truncated)
        };
        let new_tags = ai::dispatch_propose_tags(provider, &key, &body_for_api).await?;

        // The AI call took seconds; the user (or a sync tool) may have
        // edited the note meanwhile. Merging into the snapshot we read
        // would clobber those edits on disk — the classic lost update. So
        // re-read and only proceed if the file is byte-identical to what
        // the tags were proposed for; otherwise skip — the edit's own
        // change event re-enqueues the note for a fresh pass.
        let fresh = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if fresh != content {
            return Ok(false);
        }

        // Pivot from YAML to inline: merge_tags_into_file unions the
        // proposed tags with any existing tags (inline + canonical line +
        // legacy YAML), wipes the legacy YAML tag list, and writes a fresh
        // canonical tag line at the bottom of the body. The user's editor
        // will hide that line and surface the tags as pills.
        let new_content = crate::tags::merge_tags_into_file(&content, &new_tags);
        if new_content == content {
            // Nothing to change (AI proposed tags we already have). Record
            // the hash anyway so we don't burn another API call on this
            // body, and skip the write — a no-op rewrite would still bump
            // mtime, reshuffling the modified-date ordering and making
            // sync tools re-upload the file.
            self.remember_tagged(&path_str, body_trimmed);
            return Ok(false);
        }
        crate::notes::write_atomic(&path, &new_content).map_err(|e| e.to_string())?;
        // Keep the note cache current so the notes_changed refresh that
        // follows shows the new tags immediately.
        let _ = crate::notes::refresh_path(&path_str);

        // Record the hash of the body WE JUST WROTE (post-merge), not the
        // pre-merge body — otherwise our own write looks like a fresh edit
        // on the next pass and triggers a second API call + rewrite.
        let (_fm2, new_body) = frontmatter::split(&new_content);
        self.remember_tagged(&path_str, new_body.trim());
        {
            let mut last_at = self.last_tagged_at.lock().expect("tagger lock");
            last_at.insert(path, Instant::now());
        }
        Ok(true)
    }
}

pub fn start(tagger: Arc<Tagger>, app_handle: AppHandle) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return,
        };
        loop {
            std::thread::sleep(TICK);
            // Load config once per tick and gate on the SAME config for both
            // the enabled flag and the key check. `process()` tags with
            // `cfg.active_provider`, so the gate must check that provider's
            // key — not the legacy Anthropic-only `has_api_key()` shim, which
            // would either drain the queue with errors (Anthropic key present
            // but active provider's key missing) or never run (vice versa).
            let cfg = config::load();
            if !cfg.tagging_enabled {
                continue;
            }
            if !secrets::has_api_key_for(cfg.active_provider.id()) {
                continue;
            }
            let Some(path) = tagger.next() else { continue };
            match rt.block_on(tagger.process(path)) {
                Ok(true) => {
                    let _ = app_handle.emit("notes_changed", ());
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("tagger: {e}");
                }
            }
        }
    });
}
