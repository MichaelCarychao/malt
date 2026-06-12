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
        // Summaries + content straight from the note cache — this rebuild
        // used to be one of several redundant full-vault disk passes per
        // watcher batch.
        let notes = crate::notes::list_with_content();

        // Build lookup tables for the resolver — case-insensitive title and
        // slug-normalized title — keyed to canonical paths.
        let mut by_title: HashMap<String, String> = HashMap::new();
        let mut by_slug: HashMap<String, String> = HashMap::new();
        for (n, _) in &notes {
            by_title.insert(n.title.to_lowercase(), n.path.clone());
            by_slug.insert(slugify(&n.title), n.path.clone());
        }

        let mut new_map: HashMap<String, Vec<BacklinkInfo>> = HashMap::new();
        for (note, content) in &notes {
            let (_fm, body) = crate::frontmatter::split(content);
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

        // Poison-tolerant: a panic elsewhere must not permanently kill
        // backlinks (matches the index/cache locks).
        let mut map = self.map.write().unwrap_or_else(|e| e.into_inner());
        *map = new_map;
    }

    /// Look up backlinks for the given canonical note path. Empty if none.
    pub fn for_path(&self, target_path: &str) -> Vec<BacklinkInfo> {
        let map = self.map.read().unwrap_or_else(|e| e.into_inner());
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
    let notes = crate::notes::list_with_content();

    let mut by_title: HashMap<String, String> = HashMap::new();
    let mut by_slug: HashMap<String, String> = HashMap::new();
    for (n, _) in &notes {
        by_title.insert(n.title.to_lowercase(), n.path.clone());
        by_slug.insert(slugify(&n.title), n.path.clone());
    }

    // One pass over readable notes: who links out, and who gets linked to.
    let mut has_outgoing: HashSet<String> = HashSet::new();
    let mut is_target: HashSet<String> = HashSet::new();
    for (note, content) in &notes {
        if note.is_encrypted {
            continue;
        }
        let (_fm, body) = crate::frontmatter::split(content);
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

    let mut out: Vec<String> = notes
        .iter()
        .filter(|(n, _)| !n.is_encrypted)
        .filter(|(n, _)| !has_outgoing.contains(&n.path) && !is_target.contains(&n.path))
        .map(|(n, _)| n.path.clone())
        .collect();
    // list_with_content has no inherent order (HashMap-backed); restore the
    // modified-descending contract the report relies on.
    let order: HashMap<&str, u64> = notes
        .iter()
        .map(|(n, _)| (n.path.as_str(), n.modified))
        .collect();
    out.sort_by(|a, b| order.get(b.as_str()).cmp(&order.get(a.as_str())));
    out
}

/// Split a wikilink's inner text into `(target, alias)` on the FIRST `|`.
/// `[[Target|Alias]]` links to `Target` but displays `Alias`; a bare
/// `[[Target]]` has no alias. The target is what we resolve, count as a
/// backlink, and rewrite on rename — the alias is purely presentational.
/// Both halves are returned UNTRIMMED so the rewriter can preserve the
/// caller's exact spacing around the pipe; resolution trims separately.
pub(crate) fn split_alias(inner: &str) -> (&str, Option<&str>) {
    match inner.find('|') {
        Some(p) => (&inner[..p], Some(&inner[p + 1..])),
        None => (inner, None),
    }
}

/// Rewrite every `[[link]]` in `body` that resolves to the note being
/// renamed (was titled `old_title`) so the link target becomes `new_title`
/// verbatim. Returns the new body and the number of links replaced. Used by
/// the rename command to keep backlinks intact.
///
/// Resolution is **identity-aware**, mirroring `resolve`'s priority so a
/// rename never hijacks a link that actually points at a *different*
/// existing note:
///   - An exact case-insensitive title match to `old_title` always rewrites
///     (on a flat, case-insensitive folder no two notes share a title, so
///     this is unambiguous).
///   - A mere slug match (slugify collapses case + punctuation, so "Note 1",
///     "Note-1", "note1" all collide) rewrites ONLY when the renamed note is
///     the *unambiguous* slug owner: no other current note shares that slug
///     and no other note claims the exact target title. If a slug-colliding
///     sibling exists we leave the link alone rather than risk redirecting it
///     to the wrong file.
/// `existing_titles_lower` / `existing_slugs_lower` are the lowercased title
/// and slug sets of the CURRENT notes (post-rename, so they describe
/// `new_title`, not `old_title`); the caller builds them once and shares
/// them across files.
///
/// Links inside fenced or inline code are passed through byte-for-byte: a
/// `[[...]]` in a code sample is documentation, not a link, so renaming a
/// note must never edit it.
///
/// `[[Target|Alias]]` rewrites only the `Target` half and preserves
/// `|Alias` exactly.
pub fn rewrite_wikilinks_in_body(
    body: &str,
    old_title: &str,
    new_title: &str,
    existing_titles_lower: &HashSet<String>,
    existing_slugs_lower: &HashSet<String>,
) -> (String, usize) {
    let old_lower = old_title.to_lowercase();
    let old_slug = slugify(old_title);
    // The slug fallback is safe only when the renamed note alone owned this
    // slug. If a *different* current note already carries `old_slug`, a bare
    // slug-equal link is ambiguous — don't touch it.
    let slug_fallback_safe = !existing_slugs_lower.contains(&old_slug);
    let code = crate::link_suggestions::code_mask(body);
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
                // Inside a code fence / inline-code span? Leave the whole
                // `[[..]]` untouched — it's a code sample, not a link.
                let in_code = (i..j + 2).any(|k| k < code.len() && code[k]);
                if !in_code {
                    let inner = &body[i + 2..j];
                    // Only the target half (left of the first `|`) resolves;
                    // the alias half is preserved verbatim.
                    let (target_raw, alias) = split_alias(inner);
                    let trimmed = target_raw.trim();
                    if !trimmed.is_empty() {
                        let inner_lower = trimmed.to_lowercase();
                        let exact_old = inner_lower == old_lower;
                        // Slug fallback: target slug-collides with the renamed
                        // note, the renamed note is the sole slug owner, AND no
                        // other note owns this target by exact title.
                        let slug_old = !exact_old
                            && slug_fallback_safe
                            && slugify(trimmed) == old_slug
                            && !existing_titles_lower.contains(&inner_lower);
                        if exact_old || slug_old {
                            out.push_str("[[");
                            out.push_str(new_title);
                            if let Some(alias) = alias {
                                out.push('|');
                                out.push_str(alias);
                            }
                            out.push_str("]]");
                            count += 1;
                            i = j + 2;
                            continue;
                        }
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
    // Lowercased titles + slugs of the CURRENT notes (the renamed file already
    // lives at its new path on disk, so these describe `new_title`). Shared
    // across every file so the per-link slug fallback can tell when some
    // *other* note owns a target — by exact title or by a colliding slug — and
    // must not be hijacked by this rename.
    let current = crate::notes::list_notes();
    let existing_titles_lower: HashSet<String> =
        current.iter().map(|n| n.title.to_lowercase()).collect();
    let existing_slugs_lower: HashSet<String> =
        current.iter().map(|n| slugify(&n.title)).collect();
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
        let (new_body, count) = rewrite_wikilinks_in_body(
            body,
            old_title,
            new_title,
            &existing_titles_lower,
            &existing_slugs_lower,
        );
        if count > 0 {
            let full = crate::frontmatter::merge(&fm, &new_body);
            if crate::notes::write_atomic(&p, &full).is_ok() {
                // Keep the note cache current for the file we just rewrote
                // — the index/backlink rebuilds right after the rename read
                // from the cache, not from disk.
                let _ = crate::notes::refresh_path(&p.to_string_lossy());
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
    // Defensive: if a caller passes raw inner text, only the part left of
    // the first `|` is the link target (`[[Target|Alias]]`). `scan_wikilinks`
    // already strips the alias, but resolving here too keeps `resolve`
    // correct in isolation.
    let (target, _alias) = split_alias(target);
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
/// match_end_byte) tuples where `link_text` is the resolvable TARGET (the
/// part left of a `[[Target|Alias]]` pipe, trimmed). Stops at unmatched
/// brackets / newlines.
///
/// Wikilinks inside fenced or inline code are skipped: a `[[...]]` in a code
/// sample is documentation, so it must not count as a backlink (nor get
/// rewritten on rename).
fn scan_wikilinks(body: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let code = crate::link_suggestions::code_mask(body);
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
                // Skip links living inside a code fence / inline-code span.
                let in_code = (start..end).any(|k| k < code.len() && code[k]);
                if !in_code {
                    // Extract the inner text safely (might span multi-byte
                    // chars), then drop any `|alias` — only the target half
                    // resolves and produces a backlink.
                    let inner = &body[i + 2..j];
                    let (target, _alias) = split_alias(inner);
                    let link_text = target.trim().to_string();
                    if !link_text.is_empty() {
                        out.push((link_text, start, end));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(ts: &[&str]) -> HashSet<String> {
        ts.iter().map(|t| t.to_lowercase()).collect()
    }
    fn slugs(ts: &[&str]) -> HashSet<String> {
        ts.iter().map(|t| slugify(t)).collect()
    }

    // H6: `scan_wikilinks` extracts only the TARGET half of an aliased link,
    // so resolution + backlink crediting use "Note 1", not "Note 1|alias".
    #[test]
    fn scan_strips_alias_to_target() {
        let body = "see [[Note 1|the first note]] here";
        let found = scan_wikilinks(body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "Note 1");
    }

    // H4: links inside fenced or inline code are not scanned (so they never
    // count as backlinks).
    #[test]
    fn scan_skips_code() {
        let body = "real [[Note 1]]\n```\ncode [[Note 1]]\n```\ninline `[[Note 1]]`";
        let found = scan_wikilinks(body);
        // Only the first, prose-level link is seen.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "Note 1");
        assert_eq!(found[0].1, 5); // byte offset of the prose link
    }

    // H4: rename must not touch a `[[link]]` that lives inside a code block,
    // while still rewriting the genuine prose link.
    #[test]
    fn rename_preserves_links_in_code() {
        let body = "prose [[Note 1]]\n```md\nexample: [[Note 1]]\n```\ntail `[[Note 1]]` done";
        let (out, count) = rewrite_wikilinks_in_body(
            body,
            "Note 1",
            "Renamed",
            &titles(&["renamed"]),
            &slugs(&["renamed"]),
        );
        assert_eq!(count, 1, "only the prose link is rewritten");
        assert!(out.contains("prose [[Renamed]]"));
        assert!(out.contains("example: [[Note 1]]"), "fenced link untouched");
        assert!(out.contains("`[[Note 1]]`"), "inline-code link untouched");
    }

    // H6: an aliased link is rewritten on its TARGET only; the `|alias` is
    // preserved verbatim.
    #[test]
    fn rename_rewrites_alias_target_only() {
        let body = "[[Note 1|the first note]] and [[Note 1]]";
        let (out, count) = rewrite_wikilinks_in_body(
            body,
            "Note 1",
            "Renamed",
            &titles(&["renamed"]),
            &slugs(&["renamed"]),
        );
        assert_eq!(count, 2);
        assert_eq!(out, "[[Renamed|the first note]] and [[Renamed]]");
    }

    // Baseline that must not regress: an exact case-insensitive title match is
    // always rewritten.
    #[test]
    fn rename_rewrites_exact_title_case_insensitive() {
        let body = "link to [[note 1]] here";
        let (out, count) = rewrite_wikilinks_in_body(
            body,
            "Note 1",
            "Renamed",
            &titles(&["renamed"]),
            &slugs(&["renamed"]),
        );
        assert_eq!(count, 1);
        assert_eq!(out, "link to [[Renamed]] here");
    }

    // H5: a slug-colliding DISTINCT note must not be hijacked. Renaming
    // "Note 1" while a separate note "Note-1" exists must leave [[Note-1]]
    // alone (it resolves to the other file), even though both slugify to
    // "note1".
    #[test]
    fn rename_does_not_hijack_slug_colliding_sibling() {
        // Post-rename note set: the renamed file is now "Renamed"; the distinct
        // "Note-1" still exists and shares the slug "note1" with old "Note 1".
        let existing_titles = titles(&["renamed", "note-1"]);
        let existing_slugs = slugs(&["renamed", "note-1"]); // {"renamed","note1"}
        let body = "points elsewhere [[Note-1]]";
        let (out, count) = rewrite_wikilinks_in_body(
            body,
            "Note 1",
            "Renamed",
            &existing_titles,
            &existing_slugs,
        );
        assert_eq!(count, 0, "slug-colliding sibling link must be untouched");
        assert_eq!(out, body);
    }

    // H5 corollary: with NO colliding sibling, a slug-variant link to the
    // renamed note IS still rewritten (we don't over-correct and drop genuine
    // links).
    #[test]
    fn rename_still_rewrites_slug_variant_when_unambiguous() {
        let body = "punctuation variant [[Note-1]] here";
        let (out, count) = rewrite_wikilinks_in_body(
            body,
            "Note 1",
            "Renamed",
            &titles(&["renamed"]),
            &slugs(&["renamed"]),
        );
        assert_eq!(count, 1);
        assert_eq!(out, "punctuation variant [[Renamed]] here");
    }

    // H6 + H4 together with the alias splitter as a unit.
    #[test]
    fn split_alias_splits_on_first_pipe() {
        assert_eq!(split_alias("Target|Alias"), ("Target", Some("Alias")));
        assert_eq!(split_alias("Bare"), ("Bare", None));
        // Only the FIRST pipe splits; later pipes stay in the alias.
        assert_eq!(split_alias("T|a|b"), ("T", Some("a|b")));
    }
}
