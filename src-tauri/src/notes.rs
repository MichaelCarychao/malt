use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter};

use crate::backlinks::BacklinkIndex;
use crate::embeddings::EmbedIndex;
use crate::index::NoteIndex;
use crate::tagger::Tagger;

#[derive(Serialize, Clone, Debug)]
pub struct NoteSummary {
    pub path: String,
    pub title: String,
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
    // Honor the config override if set and the path exists / can be created.
    if let Some(custom) = crate::config::load().notes_dir.as_deref() {
        let p = PathBuf::from(custom);
        if p.exists() || std::fs::create_dir_all(&p).is_ok() {
            return p;
        }
        // If the configured path is unusable, fall through to the default
        // rather than blocking the app entirely.
    }
    let mut dir = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    dir.push("malt");
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    dir
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

pub fn list_notes() -> Vec<NoteSummary> {
    let dir = notes_dir();
    let mut notes = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return notes,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if should_ignore(filename) || !path.is_file() {
            continue;
        }
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let is_encrypted = crate::encryption::is_encrypted(&content);
        // Encrypted notes contribute only filename to the listing. No
        // snippet, no tags, no emptiness signal — none of that is
        // meaningful (or knowable) without the password.
        let (snippet, tags, is_empty) = if is_encrypted {
            (String::from("(encrypted)"), Vec::new(), false)
        } else {
            let (_fm, body) = crate::frontmatter::split(&content);
            let snip = snippet_from(body);
            let t = crate::tags::extract_tags_full(&content);
            // Strip private markup before checking emptiness so a note
            // that's *only* tags / wikilinks still counts as empty
            // content-wise.
            let stripped = crate::tags::strip_tags_for_ai(body);
            let empty = stripped.trim().is_empty();
            (snip, t, empty)
        };
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let is_conflict = is_conflict_filename(&title);
        notes.push(NoteSummary {
            path: path.to_string_lossy().to_string(),
            title,
            snippet,
            modified,
            tags,
            is_conflict,
            is_empty,
            is_encrypted,
            title_matches: Vec::new(),
            snippet_matches: Vec::new(),
        });
    }
    notes.sort_by(|a, b| b.modified.cmp(&a.modified));
    notes
}

pub fn start_watcher(
    app_handle: AppHandle,
    index: Arc<NoteIndex>,
    tagger: Arc<Tagger>,
    backlinks: Arc<BacklinkIndex>,
    embeddings: Arc<EmbedIndex>,
) -> notify::Result<()> {
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

    std::thread::spawn(move || {
        let _watcher = watcher; // keep alive for the lifetime of this thread
        let debounce = Duration::from_millis(250);
        loop {
            let event = match rx.recv() {
                Ok(e) => e,
                Err(_) => break,
            };
            let relevant = event.paths.iter().any(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| !should_ignore(n))
                    .unwrap_or(false)
            });
            if !relevant {
                continue;
            }
            std::thread::sleep(debounce);
            let mut changed: std::collections::HashSet<PathBuf> = event
                .paths
                .iter()
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| !should_ignore(n))
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            while let Ok(e) = rx.try_recv() {
                for p in e.paths {
                    if p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| !should_ignore(n))
                        .unwrap_or(false)
                    {
                        changed.insert(p);
                    }
                }
            }
            let _ = index.rebuild();
            backlinks.rebuild();
            tagger.enqueue_dir();
            for p in changed {
                embeddings.enqueue_path(p);
            }
            let _ = app_handle.emit("notes_changed", ());
        }
    });

    Ok(())
}
