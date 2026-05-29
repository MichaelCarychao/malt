use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::RwLock;

#[derive(Serialize, Clone, Debug)]
pub struct BacklinkInfo {
    pub source_path: String,
    pub source_title: String,
    pub snippet: String,
    pub link_text: String,
}

pub struct BacklinkIndex {
    map: RwLock<HashMap<String, Vec<BacklinkInfo>>>,
}

impl BacklinkIndex {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }

    /// Re-scan all notes, parse wikilinks, resolve targets, populate the
    /// reverse map. Called alongside the Tantivy index rebuild.
    pub fn rebuild(&self) {
        let notes = crate::notes::list_notes();

        // Build lookup tables for the resolver — case-insensitive title and
        // slug-normalized title — keyed to canonical paths.
        let mut by_title: HashMap<String, String> = HashMap::new();
        let mut by_slug: HashMap<String, String> = HashMap::new();
        for n in &notes {
            by_title.insert(n.title.to_lowercase(), n.path.clone());
            by_slug.insert(slugify(&n.title), n.path.clone());
        }

        let mut new_map: HashMap<String, Vec<BacklinkInfo>> = HashMap::new();
        for note in &notes {
            let content = std::fs::read_to_string(&note.path).unwrap_or_default();
            let (_fm, body) = crate::frontmatter::split(&content);
            for (link_text, match_start, match_end) in scan_wikilinks(body) {
                if let Some(target_path) = resolve(&link_text, &by_title, &by_slug) {
                    let snippet = snippet_around(body, match_start, match_end);
                    new_map
                        .entry(target_path)
                        .or_default()
                        .push(BacklinkInfo {
                            source_path: note.path.clone(),
                            source_title: note.title.clone(),
                            snippet,
                            link_text,
                        });
                }
            }
        }

        // Deduplicate: a single source linking the same target multiple times
        // should still produce multiple entries (each occurrence has its own
        // snippet), but we sort by source title for stable order.
        for entries in new_map.values_mut() {
            entries.sort_by(|a, b| {
                a.source_title
                    .to_lowercase()
                    .cmp(&b.source_title.to_lowercase())
            });
        }

        let mut map = self.map.write().expect("backlinks write lock");
        *map = new_map;
    }

    /// Look up backlinks for the given canonical note path. Empty if none.
    pub fn for_path(&self, target_path: &str) -> Vec<BacklinkInfo> {
        let map = self.map.read().expect("backlinks read lock");
        map.get(target_path).cloned().unwrap_or_default()
    }
}

/// Find every `[[link]]` in `body` whose target resolves to an existing
/// note, returning ordered (link_text, canonical_path) pairs. Duplicates
/// are NOT removed — preserves order for callers that want to dedup themselves
/// or stream output in source order. Used by the export pipeline.
pub fn resolved_targets_in(body: &str) -> Vec<(String, String)> {
    let notes = crate::notes::list_notes();
    let mut by_title: HashMap<String, String> = HashMap::new();
    let mut by_slug: HashMap<String, String> = HashMap::new();
    for n in &notes {
        by_title.insert(n.title.to_lowercase(), n.path.clone());
        by_slug.insert(slugify(&n.title), n.path.clone());
    }
    let mut out = Vec::new();
    for (link_text, _, _) in scan_wikilinks(body) {
        if let Some(path) = resolve(&link_text, &by_title, &by_slug) {
            out.push((link_text, path));
        }
    }
    out
}

/// Paths of "orphan" notes — adrift from the link graph. A note is an
/// orphan when it has **no** outgoing wikilink that resolves to an
/// existing note **and** is the target of no backlinks from any other
/// note. These are the notes most in need of being woven into your web
/// (or pruned). Result is in the same order as `list_notes` (modified
/// descending).
///
/// Encrypted notes are skipped: their bodies are ciphertext, so their
/// outgoing links are unknowable and we'd wrongly flag them as orphans.
/// A side effect is that links *from* an encrypted note can't credit
/// their targets — an acceptable blind spot, since encryption hides the
/// graph by design.
pub fn orphan_paths() -> Vec<String> {
    let notes = crate::notes::list_notes();

    let mut by_title: HashMap<String, String> = HashMap::new();
    let mut by_slug: HashMap<String, String> = HashMap::new();
    for n in &notes {
        by_title.insert(n.title.to_lowercase(), n.path.clone());
        by_slug.insert(slugify(&n.title), n.path.clone());
    }

    // One pass over readable notes: who links out, and who gets linked to.
    let mut has_outgoing: HashSet<String> = HashSet::new();
    let mut is_target: HashSet<String> = HashSet::new();
    for note in &notes {
        if note.is_encrypted {
            continue;
        }
        let content = std::fs::read_to_string(&note.path).unwrap_or_default();
        let (_fm, body) = crate::frontmatter::split(&content);
        for (link_text, _, _) in scan_wikilinks(body) {
            if let Some(target) = resolve(&link_text, &by_title, &by_slug) {
                // A note linking to itself doesn't count as being woven in.
                if target != note.path {
                    has_outgoing.insert(note.path.clone());
                    is_target.insert(target);
                }
            }
        }
    }

    notes
        .iter()
        .filter(|n| !n.is_encrypted)
        .filter(|n| !has_outgoing.contains(&n.path) && !is_target.contains(&n.path))
        .map(|n| n.path.clone())
        .collect()
}

/// Rewrite every `[[link]]` in `body` whose target normalizes to `old_title`
/// (case-insensitive exact OR slug-equal) so the inner text becomes
/// `new_title` verbatim. Returns the new body and the number of links
/// replaced. Used by the rename command to keep backlinks intact.
pub fn rewrite_wikilinks_in_body(body: &str, old_title: &str, new_title: &str) -> (String, usize) {
    let old_lower = old_title.to_lowercase();
    let old_slug = slugify(old_title);
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    let mut count = 0usize;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // Find matching `]]` on the same line, no nested `[`.
            let mut j = i + 2;
            let mut found_close = false;
            while j + 1 < bytes.len() {
                if bytes[j] == b'\n' || bytes[j] == b'[' {
                    break;
                }
                if bytes[j] == b']' && bytes[j + 1] == b']' {
                    found_close = true;
                    break;
                }
                j += 1;
            }
            if found_close {
                let inner = &body[i + 2..j];
                let trimmed = inner.trim();
                if !trimmed.is_empty() {
                    let inner_lower = trimmed.to_lowercase();
                    let inner_slug = slugify(trimmed);
                    if inner_lower == old_lower || inner_slug == old_slug {
                        out.push_str("[[");
                        out.push_str(new_title);
                        out.push_str("]]");
                        count += 1;
                        i = j + 2;
                        continue;
                    }
                }
                out.push_str(&body[i..j + 2]);
                i = j + 2;
                continue;
            }
        }
        // Pass through a single char (handle multi-byte).
        let rest = &body[i..];
        if let Some(c) = rest.chars().next() {
            out.push(c);
            i += c.len_utf8();
        } else {
            break;
        }
    }
    (out, count)
}

/// Rewrite every `[[old_title]]` → `[[new_title]]` across all `.md` files
/// in `dir`, skipping `skip_path` (the renamed file itself) and encrypted
/// notes (ciphertext has no parseable wikilinks). Returns the number of
/// files changed. Shared by the in-app rename and the external-rename
/// cascade so both keep links intact by exactly the same rules.
pub fn cascade_wikilink_rename(
    dir: &Path,
    old_title: &str,
    new_title: &str,
    skip_path: &Path,
) -> usize {
    let mut changed = 0usize;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() || p == skip_path {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.to_lowercase().ends_with(".md")
            || name.starts_with(".~lock")
            || name.starts_with("~$")
        {
            continue;
        }
        let content = match std::fs::read_to_string(&p) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if crate::encryption::is_encrypted(&content) {
            continue;
        }
        let (fm, body) = crate::frontmatter::split(&content);
        let (new_body, count) = rewrite_wikilinks_in_body(body, old_title, new_title);
        if count > 0 {
            let full = crate::frontmatter::merge(&fm, &new_body);
            if std::fs::write(&p, full).is_ok() {
                changed += 1;
            }
        }
    }
    changed
}

fn slugify(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn resolve(
    target: &str,
    by_title: &HashMap<String, String>,
    by_slug: &HashMap<String, String>,
) -> Option<String> {
    let t = target.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    if let Some(p) = by_title.get(&t) {
        return Some(p.clone());
    }
    let slug = slugify(target);
    by_slug.get(&slug).cloned()
}

/// Scan `body` for `[[name]]` patterns. Returns (link_text, match_start_byte,
/// match_end_byte) tuples. Stops at unmatched brackets / newlines.
fn scan_wikilinks(body: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // Look for closing ]] on the same line, no nested [
            let start = i;
            let mut j = i + 2;
            let mut found_close = false;
            while j + 1 < bytes.len() {
                if bytes[j] == b'\n' {
                    break;
                }
                if bytes[j] == b'[' {
                    break;
                }
                if bytes[j] == b']' && bytes[j + 1] == b']' {
                    found_close = true;
                    break;
                }
                j += 1;
            }
            if found_close {
                let end = j + 2;
                // Extract the inner text safely (might span multi-byte chars).
                let inner = &body[i + 2..j];
                let link_text = inner.trim().to_string();
                if !link_text.is_empty() {
                    out.push((link_text, start, end));
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn snippet_around(body: &str, match_start: usize, match_end: usize) -> String {
    let window = 80usize;
    let start = match_start.saturating_sub(window / 2);
    let end = (match_end + window / 2).min(body.len());
    let start = floor_char_boundary(body, start);
    let end = ceil_char_boundary(body, end);
    let raw = &body[start..end];
    let compacted: String = raw
        .replace('\r', " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < body.len() { "…" } else { "" };
    format!("{prefix}{compacted}{suffix}")
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
