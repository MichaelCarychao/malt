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
    last_tagged_hash: Mutex<HashMap<PathBuf, u64>>,
    last_tagged_at: Mutex<HashMap<PathBuf, Instant>>,
}

impl Tagger {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(HashSet::new()),
            last_tagged_hash: Mutex::new(HashMap::new()),
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
        let (_fm, body) = frontmatter::split(&content);
        let body_trimmed = body.trim();
        if body_trimmed.is_empty() {
            return Ok(false);
        }

        // Body-hash check — skip if body unchanged since last tagging.
        let body_hash = hash_str(body_trimmed);
        {
            let hashes = self.last_tagged_hash.lock().expect("tagger lock");
            if hashes.get(&path) == Some(&body_hash) {
                return Ok(false);
            }
        }

        let key = secrets::get_api_key().map_err(|e| e.to_string())?;
        // Strip existing hashtags/canonical line before sending — don't want
        // the AI to anchor on tags we already have when proposing new ones.
        let cleaned = crate::tags::strip_tags_for_ai(body_trimmed);
        let body_for_api: String = cleaned.chars().take(MAX_BODY_CHARS).collect();
        let new_tags = ai::propose_tags(&key, &body_for_api).await?;

        // Pivot from YAML to inline: merge_tags_into_file unions the
        // proposed tags with any existing tags (inline + canonical line +
        // legacy YAML), wipes the legacy YAML tag list, and writes a fresh
        // canonical tag line at the bottom of the body. The user's editor
        // will hide that line and surface the tags as pills.
        let new_content = crate::tags::merge_tags_into_file(&content, &new_tags);
        std::fs::write(&path, &new_content).map_err(|e| e.to_string())?;

        {
            let mut hashes = self.last_tagged_hash.lock().expect("tagger lock");
            hashes.insert(path.clone(), body_hash);
        }
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
            if !config::load().tagging_enabled {
                continue;
            }
            if !secrets::has_api_key() {
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

fn hash_str(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
