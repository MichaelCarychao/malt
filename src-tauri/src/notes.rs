use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `contents` to `path` atomically AND durably: stage a sibling temp
/// file, `fsync` it, then rename it over the target. Rename is atomic on the
/// same volume (Windows and Unix both), so a concurrent reader — e.g. an
/// external tool sharing the notes folder — never observes a truncated or
/// half-written note; it sees either the old complete file or the new
/// complete file. The temp name is unique per call so racing writers don't
/// collide, and it's cleaned up on rename failure. The `.malt-tmp-*` suffix
/// is ignored by the watcher + listing (see `should_ignore`).
///
/// Durability: `rename` is only crash-safe once the *data* it points at is
/// on stable storage. Without the temp-file `sync_all()` below, a power loss
/// between the metadata commit and the data write-back can resurrect the
/// rename pointing at a zero-length (or partially written) file — i.e. a
/// note silently truncated to empty. We `sync_all()` the temp BEFORE the
/// rename so the bytes are durable first. On Unix we additionally fsync the
/// containing directory after the rename so the *rename itself* survives a
/// crash; Windows has no portable directory fsync, so there the temp-file
/// flush is the load-bearing guarantee. Directory-fsync errors are ignored:
/// best-effort, and the data is already safe regardless.
pub fn write_atomic<P: AsRef<Path>>(path: P, contents: &str) -> std::io::Result<()> {
    write_atomic_bytes(path, contents.as_bytes())
}

/// Bytes-oriented sibling of [`write_atomic`] — same temp-file + fsync +
/// rename guarantee, for callers writing non-UTF-8 payloads (e.g. a binary
/// epub export, where a truncated re-export must never replace a good file).
/// `write_atomic` delegates here so both paths share one implementation; see
/// that function's doc comment for the full atomicity/durability rationale.
pub fn write_atomic_bytes<P: AsRef<Path>>(path: P, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let path = path.as_ref();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(format!(".malt-tmp-{seq}"));
    let tmp = PathBuf::from(tmp_name);

    // Write + flush + fsync the temp file, then close it, before renaming.
    // Scope the File so its handle is dropped (closed) prior to the rename —
    // matters on Windows where an open handle can block the replace.
    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    match std::fs::rename(&tmp, path) {
        Ok(()) => {
            // Best-effort: fsync the parent directory so the rename entry
            // itself is durable. Unix-only — no portable equivalent on
            // Windows, where the temp-file sync_all above is what protects
            // against the zero-length-note failure mode.
            #[cfg(unix)]
            {
                if let Some(parent) = path.parent() {
                    if let Ok(dir) = std::fs::File::open(parent) {
                        let _ = dir.sync_all();
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

use crate::backlinks::BacklinkIndex;
use crate::embeddings::EmbedIndex;
use crate::index::NoteIndex;
use crate::tagger::Tagger;

#[derive(Serialize, Clone, Debug)]
pub struct NoteSummary {
    pub path: String,
    pub title: String,
    /// Display name for the sidebar + pane title bars: the note's first-line
    /// `# H1` heading when present, else the filename (`title`). The
    /// filename is still the note's identity (wikilinks, rename, history) —
    /// this is purely what's shown.
    #[serde(default)]
    pub display_title: String,
    pub snippet: String,
    pub modified: u64,
    #[serde(default)]
    pub tags: Vec<String>,
    /// True when the filename matches a known sync-conflict pattern
    /// (Dropbox "conflicted copy", Syncthing ".sync-conflict-", etc.).
    /// Renders a badge in the sidebar so the user can resolve manually.
    #[serde(default)]
    pub is_conflict: bool,
    /// True when the body has no meaningful content — fresh stub notes
    /// or notes whose entire body has been deleted. We strip malt-private
    /// markup (frontmatter, canonical tag line, inline #tags, [[link]]
    /// brackets) before checking, so a note that's *only* tags counts
    /// as empty.
    #[serde(default)]
    pub is_empty: bool,
    /// True when the file starts with the `MALT-ENC-v1:` envelope. Body
    /// is unreadable without the password; sidebar shows a lock icon and
    /// the indexer skips body fields entirely (search by filename only).
    #[serde(default)]
    pub is_encrypted: bool,
    #[serde(default)]
    pub title_matches: Vec<(usize, usize)>,
    #[serde(default)]
    pub snippet_matches: Vec<(usize, usize)>,
}

/// Detect sync-conflict filenames produced by Dropbox / Syncthing / etc.
/// Conservative — won't false-positive on a note actually named "conflict
/// resolution" because we require either the explicit Syncthing marker or
/// a parenthesized "conflict[ed]" phrase near the end.
pub fn is_conflict_filename(stem: &str) -> bool {
    let lower = stem.to_lowercase();
    // Syncthing pattern: name.sync-conflict-DATE-TIME-XXXXX
    if lower.contains(".sync-conflict-") {
        return true;
    }
    // Dropbox: "name (Michael's conflicted copy 2024-01-15)"
    // Generic parenthesized "(... conflict[ed] ...)" near end of stem.
    if let Some(open) = lower.rfind('(') {
        let inside = &lower[open + 1..];
        if inside.contains("conflict") {
            return true;
        }
    }
    false
}

pub fn notes_dir() -> PathBuf {
    // v0.3.1+: route through the vaults registry. The active vault's
    // path wins; the registry seeds itself from the legacy
    // config.notes_dir on first load so existing installs migrate
    // transparently.
    let p = crate::vaults::active_path();
    if !p.exists() {
        let _ = std::fs::create_dir_all(&p);
    }
    p
}

fn should_ignore(filename: &str) -> bool {
    if filename.starts_with(".~lock") || filename.starts_with("~$") {
        return true;
    }
    if filename == ".DS_Store" || filename == "desktop.ini" {
        return true;
    }
    !filename.to_lowercase().ends_with(".md")
}

fn snippet_from(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .or_else(|| body.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("")
        .chars()
        .take(80)
        .collect()
}

/// The note's display name: the text of a first-line `# H1` heading when
/// the first non-empty body line is one, else None (caller falls back to
/// the filename). Requires `# ` (single hash + space) so an inline `#tag`
/// or an `## H2` doesn't count. Strips an optional ATX closing `#`.
fn h1_title(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let rest = t.strip_prefix("# ")?; // first non-empty line must be H1
        let title = rest.trim().trim_end_matches('#').trim();
        return if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        };
    }
    None
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True if `a` and `b` differ by at most one insertion, deletion, or
/// substitution. Used for typo-tolerant matching to mirror Tantivy's
/// FuzzyTermQuery (edit distance 1 for terms of length ≥ 4).
fn within_edit_distance_one(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    if long.len() == short.len() {
        let mismatches = short.iter().zip(long.iter()).filter(|(x, y)| x != y).count();
        return mismatches <= 1;
    }
    // Lengths differ by 1: walk both, allow one skip in the longer string.
    let mut i = 0usize;
    let mut j = 0usize;
    let mut skipped = false;
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
        } else if !skipped {
            skipped = true;
            j += 1;
        } else {
            return false;
        }
    }
    true
}

/// True if `word_lower` matches `term` either by containing it as a
/// substring (handles prefix, infix, suffix matches) or by being within
/// edit distance 1 (for terms of length ≥ 4, matching Tantivy's behavior).
fn word_matches_term(word_lower: &str, term: &str) -> bool {
    if word_lower.contains(term) {
        return true;
    }
    if term.chars().count() >= 4 && within_edit_distance_one(word_lower, term) {
        return true;
    }
    false
}

/// Find words in `text` that match any of `terms` (substring or fuzzy).
/// Returns (start, end) char ranges spanning whole matched words so the
/// highlight feels like a search result, not a partial-word smear.
pub fn find_matches(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    if terms.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut ranges: Vec<(usize, usize)> = Vec::new();

    let mut i = 0;
    while i < n {
        if !is_word_char(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && is_word_char(chars[i]) {
            i += 1;
        }
        let end = i;
        let word_lower: String = chars[start..end]
            .iter()
            .flat_map(|c| c.to_lowercase())
            .collect();
        for term in terms {
            if term.is_empty() {
                continue;
            }
            if word_matches_term(&word_lower, term) {
                ranges.push((start, end));
                break;
            }
        }
    }

    ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        merged.push((s, e));
    }
    merged
}

/// Locate the byte position of the first word in `body` that matches any
/// term (substring first, then fuzzy). Used to center the snippet on a
/// real match instead of falling back to position 0 for typo'd queries.
fn first_match_byte(body: &str, terms: &[String]) -> Option<usize> {
    // Cheap path: try substring first across all terms.
    let lower = body.to_lowercase();
    let mut best: Option<usize> = None;
    for term in terms {
        if term.is_empty() {
            continue;
        }
        if let Some(idx) = lower.find(term.as_str()) {
            best = Some(best.map_or(idx, |prev| prev.min(idx)));
        }
    }
    if best.is_some() {
        return best;
    }
    // Fallback: scan words for fuzzy matches.
    let mut byte_pos = 0usize;
    let chars = body.chars();
    let mut current_word = String::new();
    let mut current_word_byte_start = 0usize;
    for c in chars {
        if is_word_char(c) {
            if current_word.is_empty() {
                current_word_byte_start = byte_pos;
            }
            current_word.push(c);
        } else if !current_word.is_empty() {
            let wl: String = current_word.to_lowercase();
            for term in terms {
                if term.chars().count() >= 4 && within_edit_distance_one(&wl, term) {
                    return Some(current_word_byte_start);
                }
            }
            current_word.clear();
        }
        byte_pos += c.len_utf8();
    }
    if !current_word.is_empty() {
        let wl: String = current_word.to_lowercase();
        for term in terms {
            if term.chars().count() >= 4 && within_edit_distance_one(&wl, term) {
                return Some(current_word_byte_start);
            }
        }
    }
    None
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(idx) && idx > 0 {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Build a snippet of `body` centered on the first occurrence of any term
/// (substring or fuzzy), roughly `window` chars wide. Returns the snippet
/// and match ranges within it.
pub fn snippet_around_match(body: &str, terms: &[String], window: usize) -> (String, Vec<(usize, usize)>) {
    let first_byte = first_match_byte(body, terms);

    // Show the match near the start of the snippet (small leading context)
    // so even when the row's column truncates with ellipsis, the highlight
    // is visible. ~15 chars before the match leaves room for the ellipsis
    // prefix + a word or two of lead-in.
    let approx_start = match first_byte {
        Some(byte) => byte.saturating_sub(15),
        None => 0,
    };
    let approx_end = approx_start + window;

    let start = floor_char_boundary(body, approx_start);
    let end = ceil_char_boundary(body, approx_end.min(body.len()));

    let raw = &body[start..end];
    // Compact whitespace/newlines for display.
    let compacted: String = raw
        .replace('\r', " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < body.len() { "…" } else { "" };
    let display = format!("{prefix}{compacted}{suffix}");

    let mut matches = find_matches(&compacted, terms);
    // Shift by prefix length (in chars).
    let shift = prefix.chars().count();
    for r in &mut matches {
        r.0 += shift;
        r.1 += shift;
    }
    (display, matches)
}

/// Build a `NoteSummary` for one already-validated note `path` whose
/// mtime (`modified`, unix secs) and full `content` the caller has in
/// hand. Single source of truth for per-note fields — the cache scan,
/// the incremental refresh, and (through them) the search index all see
/// identical summaries. Returns the summary unconditionally — callers
/// pre-filter `should_ignore` / non-files.
fn summarize_content(path: &Path, modified: u64, content: &str) -> NoteSummary {
    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let is_encrypted = crate::encryption::is_encrypted(content);
    // Encrypted notes contribute only filename to the listing. No
    // snippet, no tags, no emptiness signal — none of that is
    // meaningful (or knowable) without the password.
    let (snippet, tags, is_empty, display_title) = if is_encrypted {
        (String::from("(encrypted)"), Vec::new(), false, title.clone())
    } else {
        let (_fm, body) = crate::frontmatter::split(content);
        let snip = snippet_from(body);
        let t = crate::tags::extract_tags_full(content);
        // Strip private markup before checking emptiness so a note
        // that's *only* tags / wikilinks still counts as empty
        // content-wise.
        let stripped = crate::tags::strip_tags_for_ai(body);
        let empty = stripped.trim().is_empty();
        // First-line `# H1` becomes the display name; else the filename.
        let disp = h1_title(body).unwrap_or_else(|| title.clone());
        (snip, t, empty, disp)
    };
    let is_conflict = is_conflict_filename(&title);
    NoteSummary {
        path: path.to_string_lossy().to_string(),
        title,
        display_title,
        snippet,
        modified,
        tags,
        is_conflict,
        is_empty,
        is_encrypted,
        title_matches: Vec::new(),
        snippet_matches: Vec::new(),
    }
}

// ─────────────────────── in-memory note cache ───────────────────────
//
// THE latency keystone. Every keystroke-search, sidebar refresh, tag
// count, backlink rebuild, and index rebuild used to re-read and re-parse
// the entire vault from disk — multiple full passes per keystroke while
// typing. The cache holds each note's summary AND full content (an
// Arc<str>, so handing copies out is a pointer bump) for the active vault,
// invalidated surgically:
//
//   - in-app writes funnel through NoteIndex::upsert/remove, which call
//     refresh_path / forget_path_cache here;
//   - background writers (tagger, rename cascade, link-mention) refresh
//     the paths they rewrite;
//   - the watcher refreshes each path in a change batch before reindexing;
//   - a vault switch is detected by the stored dir mismatching notes_dir()
//     and triggers a full rescan on next access.
//
// Memory: content for every plaintext note. A 10k-note vault of ~2KB notes
// is ~20MB — the nvalt deal (whole corpus in RAM) and exactly what makes
// type-to-filter instant.

struct CacheEntry {
    summary: NoteSummary,
    content: Arc<str>,
}

struct NoteCacheState {
    dir: PathBuf,
    entries: HashMap<String, CacheEntry>,
}

fn note_cache() -> &'static std::sync::Mutex<Option<NoteCacheState>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<NoteCacheState>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Read one note off disk into a cache entry. None if it's missing, not a
/// regular file, or an ignored/non-`.md` name.
fn read_entry(p: &Path) -> Option<CacheEntry> {
    let filename = p.file_name().and_then(|n| n.to_str())?;
    if should_ignore(filename) {
        return None;
    }
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let content = std::fs::read_to_string(p).unwrap_or_default();
    let summary = summarize_content(p, modified, &content);
    Some(CacheEntry {
        summary,
        content: Arc::from(content.as_str()),
    })
}

/// One full pass over `dir` — the only place the whole vault is read.
fn scan_vault(dir: &Path) -> HashMap<String, CacheEntry> {
    let mut out = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(e) = read_entry(&path) {
                out.insert(path.to_string_lossy().to_string(), e);
            }
        }
    }
    out
}

/// Run `f` against the cache for the CURRENT vault, (re)scanning first if
/// the cache is cold or belongs to a different vault. Poison-tolerant —
/// a panic elsewhere must not permanently kill note listing.
fn with_cache<R>(f: impl FnOnce(&mut NoteCacheState) -> R) -> R {
    let mut guard = note_cache().lock().unwrap_or_else(|e| e.into_inner());
    let dir = notes_dir();
    let needs_scan = guard.as_ref().map(|c| c.dir != dir).unwrap_or(true);
    if needs_scan {
        *guard = Some(NoteCacheState {
            entries: scan_vault(&dir),
            dir,
        });
    }
    f(guard.as_mut().expect("cache populated above"))
}

/// Re-read a single file into the cache, or drop it if it's gone. Returns
/// the fresh (summary, content) when the path is a live note. This is the
/// surgical-invalidation entry point for every in-app write.
pub fn refresh_path(path: &str) -> Option<(NoteSummary, Arc<str>)> {
    with_cache(|c| match read_entry(Path::new(path)) {
        Some(e) => {
            let out = (e.summary.clone(), e.content.clone());
            c.entries.insert(path.to_string(), e);
            Some(out)
        }
        None => {
            c.entries.remove(path);
            None
        }
    })
}

/// Drop a path from the cache (after a delete / move-away).
pub fn forget_path_cache(path: &str) {
    with_cache(|c| {
        c.entries.remove(path);
    });
}

/// Cached full content of a note, if present. Pointer-cheap.
pub fn cached_content(path: &str) -> Option<Arc<str>> {
    with_cache(|c| c.entries.get(path).map(|e| e.content.clone()))
}

/// Every note's (summary, content) pair — for whole-vault consumers like
/// the index/backlink rebuilds, replacing their own disk passes.
pub fn list_with_content() -> Vec<(NoteSummary, Arc<str>)> {
    with_cache(|c| {
        c.entries
            .values()
            .map(|e| (e.summary.clone(), e.content.clone()))
            .collect()
    })
}

pub fn list_notes() -> Vec<NoteSummary> {
    let mut notes: Vec<NoteSummary> =
        with_cache(|c| c.entries.values().map(|e| e.summary.clone()).collect());
    notes.sort_by(|a, b| b.modified.cmp(&a.modified));
    notes
}

/// Handle for the live file watcher. Wrapped in a Mutex so vault
/// switching can call `repoint(new_dir)` from another thread to
/// un-watch the old path and watch the new one. Drops naturally on
/// app exit when the last Arc goes out of scope.
pub type WatcherHandle = Arc<std::sync::Mutex<Option<RecommendedWatcher>>>;

/// Swap the directory the watcher is observing. Used on vault
/// switch. Safe to call before `start_watcher` has run (no-op).
pub fn repoint_watcher(handle: &WatcherHandle, old: &PathBuf, new: &PathBuf) {
    let mut guard = handle.lock().expect("watcher handle lock");
    if let Some(w) = guard.as_mut() {
        // unwatch ignores errors — the old path may already be gone if
        // the user moved or deleted it before switching.
        let _ = w.unwatch(old);
        let _ = w.watch(new, RecursiveMode::NonRecursive);
    }
}

/// Collect the `.md` paths from a watcher event into `into`, ignoring
/// lock/temp/non-markdown files.
fn collect_relevant(event: &Event, into: &mut HashSet<PathBuf>) {
    for p in &event.paths {
        if p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| !should_ignore(n))
            .unwrap_or(false)
        {
            into.insert(p.clone());
        }
    }
}

/// Content fingerprint for rename detection. Hash the whole file so a
/// pure rename (identical bytes at a new path) is recognizable even
/// across a sync tool's delete+create event soup.
fn content_hash(content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

/// path → content-hash for every `.md` file in `dir` with non-empty
/// content. Empty files are excluded: too many would share a hash to
/// match unambiguously.
fn snapshot_hashes(dir: &Path) -> HashMap<PathBuf, u64> {
    let mut out = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if should_ignore(name) || !p.is_file() {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&p) {
                if !content.trim().is_empty() {
                    out.insert(p, content_hash(&content));
                }
            }
        }
    }
    out
}

/// Like `snapshot_hashes` but reuses `prev` hashes for files NOT in the
/// `changed` set, re-reading only changed/new files. Keeps per-batch cost
/// proportional to what changed rather than the whole vault.
fn snapshot_hashes_reusing(
    dir: &Path,
    prev: &HashMap<PathBuf, u64>,
    changed: &HashSet<PathBuf>,
) -> HashMap<PathBuf, u64> {
    let mut out = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if should_ignore(name) || !p.is_file() {
                continue;
            }
            if !changed.contains(&p) {
                if let Some(h) = prev.get(&p) {
                    out.insert(p, *h);
                    continue;
                }
            }
            if let Ok(content) = std::fs::read_to_string(&p) {
                if !content.trim().is_empty() {
                    out.insert(p, content_hash(&content));
                }
            }
        }
    }
    out
}

/// Infer external renames from two directory snapshots: a path that
/// disappeared whose content fingerprint reappeared at exactly one new
/// path is treated as a rename. Ambiguous matches (same content at
/// multiple paths) are skipped — we never guess.
fn detect_renames(
    prev: &HashMap<PathBuf, u64>,
    curr: &HashMap<PathBuf, u64>,
) -> Vec<(PathBuf, PathBuf)> {
    // Count each content hash across the FULL snapshots. We only treat a
    // disappear+reappear as a rename when the content is globally unique on
    // both sides — otherwise duplicate content (templates, stubs, repeated
    // boilerplate) could mis-pair and rewrite the wrong [[links]].
    let mut prev_counts: HashMap<u64, usize> = HashMap::new();
    for h in prev.values() {
        *prev_counts.entry(*h).or_default() += 1;
    }
    let mut curr_counts: HashMap<u64, usize> = HashMap::new();
    for h in curr.values() {
        *curr_counts.entry(*h).or_default() += 1;
    }

    let mut removed_by_hash: HashMap<u64, Vec<&PathBuf>> = HashMap::new();
    for (p, h) in prev {
        if !curr.contains_key(p) {
            removed_by_hash.entry(*h).or_default().push(p);
        }
    }
    let mut added_by_hash: HashMap<u64, Vec<&PathBuf>> = HashMap::new();
    for (p, h) in curr {
        if !prev.contains_key(p) {
            added_by_hash.entry(*h).or_default().push(p);
        }
    }
    let mut out = Vec::new();
    for (h, removed) in &removed_by_hash {
        if removed.len() != 1 {
            continue;
        }
        // Content must be unique across both snapshots — no ambiguity.
        if prev_counts.get(h) != Some(&1) || curr_counts.get(h) != Some(&1) {
            continue;
        }
        if let Some(added) = added_by_hash.get(h) {
            if added.len() == 1 {
                out.push((removed[0].clone(), added[0].clone()));
            }
        }
    }
    out
}

fn title_of(path: &Path) -> Option<String> {
    path.file_stem().and_then(|s| s.to_str()).map(String::from)
}

pub fn start_watcher(
    app_handle: AppHandle,
    index: Arc<NoteIndex>,
    tagger: Arc<Tagger>,
    backlinks: Arc<BacklinkIndex>,
    embeddings: Arc<EmbedIndex>,
) -> notify::Result<WatcherHandle> {
    let dir = notes_dir();
    let (tx, rx) = channel::<Event>();

    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        notify::Config::default(),
    )?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;

    // Hand the watcher back to the caller via a Mutex<Option<...>> so
    // it stays alive (it'd otherwise drop when start_watcher returns)
    // AND so vault switching can swap the watched path.
    let handle: WatcherHandle = Arc::new(std::sync::Mutex::new(Some(watcher)));

    std::thread::spawn(move || {
        // No longer need to keep `watcher` alive here — the AppState
        // holds the Arc.
        //
        // Coalescing debounce: after the first event we keep draining until
        // the folder is quiet for QUIET, capped at MAX_WAIT. A burst of
        // writes (e.g. an external tool batch-writing hundreds of files)
        // collapses into ONE reindex instead of one per debounce tick;
        // MAX_WAIT guarantees a continuous stream still flushes a few times
        // a minute so the UI never goes stale.
        const QUIET: Duration = Duration::from_millis(300);
        const MAX_WAIT: Duration = Duration::from_secs(3);

        // path → content fingerprint, the baseline for external-rename
        // detection. Initialized from the current vault so even the first
        // event can detect a rename.
        let mut snapshot_dir = notes_dir();
        let mut prev_hashes = snapshot_hashes(&snapshot_dir);

        loop {
            // Block for the first event of a batch.
            let first = match rx.recv() {
                Ok(e) => e,
                Err(_) => break,
            };
            let mut changed: HashSet<PathBuf> = HashSet::new();
            collect_relevant(&first, &mut changed);
            if changed.is_empty() {
                continue;
            }
            // Coalesce the burst.
            let deadline = Instant::now() + MAX_WAIT;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let wait = QUIET.min(deadline - now);
                match rx.recv_timeout(wait) {
                    Ok(e) => collect_relevant(&e, &mut changed),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }

            let dir = notes_dir();

            // Process the coalesced batch inside catch_unwind so a panic in a
            // downstream helper (e.g. a tokenizer hiccup during rebuild, or
            // rename-cascade link rewriting) can't poison this thread and
            // permanently freeze indexing — the watcher would otherwise be
            // dead for the rest of the session. On panic we log and fall
            // through to the next event. AssertUnwindSafe: the captured Arcs
            // / AppHandle carry interior mutability (so they're not auto
            // UnwindSafe), but the locks they guard recover from poisoning
            // (rebuild() uses `unwrap_or_else(into_inner)`), so resuming is
            // safe. prev_hashes/snapshot_dir are borrowed mutably; if the
            // closure panics partway the borrows simply end and the stale
            // baseline is reused next round — correctness is preserved
            // because rename detection only ever *adds* link rewrites.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if dir != snapshot_dir {
                    // Vault switched out from under us — rebaseline and skip
                    // rename detection this round (cross-vault content
                    // collisions would otherwise masquerade as renames).
                    snapshot_dir = dir.clone();
                    prev_hashes = snapshot_hashes(&dir);
                } else {
                    // Detect + cascade external renames BEFORE reindexing so the
                    // index reflects the rewritten links. A file that vanished
                    // and reappeared verbatim at a new path is a rename; rewrite
                    // every [[old]] → [[new]] in the other notes and repoint its
                    // embedding.
                    let curr_hashes = snapshot_hashes_reusing(&dir, &prev_hashes, &changed);
                    for (old_path, new_path) in detect_renames(&prev_hashes, &curr_hashes) {
                        if let (Some(old_title), Some(new_title)) =
                            (title_of(&old_path), title_of(&new_path))
                        {
                            if old_title != new_title {
                                crate::backlinks::cascade_wikilink_rename(
                                    &dir, &old_title, &new_title, &new_path,
                                );
                                embeddings.rename_path(
                                    &old_path.to_string_lossy(),
                                    &new_path.to_string_lossy(),
                                );
                            }
                        }
                    }
                    prev_hashes = curr_hashes;
                }

                // Refresh the cache for exactly the changed paths (handles
                // create/modify/delete uniformly — a vanished file drops
                // out) so the rebuilds below read fresh in-memory content
                // instead of re-scanning the vault from disk.
                for p in &changed {
                    let _ = refresh_path(&p.to_string_lossy());
                }

                // The watcher rebuild is the external-change path (in-app
                // edits update the index incrementally via the write
                // commands). Log on error instead of swallowing it so a
                // persistently failing rebuild is diagnosable.
                if let Err(e) = index.rebuild() {
                    eprintln!("watcher: index rebuild failed: {e}");
                }
                backlinks.rebuild();
                tagger.enqueue_dir();
                for p in &changed {
                    embeddings.enqueue_path(p.clone());
                }
                let _ = app_handle.emit("notes_changed", ());
            }));
            if result.is_err() {
                eprintln!("watcher: panic while processing change batch; continuing");
            }
        }
    });

    Ok(handle)
}
