// Unlinked mentions: notes that name another note in plain prose
// without a [[wikilink]]. Obsidian's killer discovery feature —
// surfaces latent connections you never explicitly linked.
//
// For a target note, we scan every other note's body for case-
// insensitive, word-boundary occurrences of the target's title that
// are NOT already inside a [[...]] span. The "link it" action rewrites
// the first such occurrence into a wikilink.
//
// Matching notes:
//   - Skip the target itself and any encrypted note (can't read body).
//   - Word-boundary: the char before/after the match must be a non-
//     alphanumeric (so "Cat" doesn't match inside "Category").
//   - Case-insensitive via a lowercased haystack; byte offsets are used
//     directly (correct for ASCII titles) and every destructive use
//     re-verifies the slice, so a Unicode-folding length drift can
//     never corrupt a file — it just skips that occurrence.

use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct UnlinkedMention {
    pub path: String,
    pub title: String,
    pub snippet: String,
    pub count: usize,
}

/// Byte ranges of `[[...]]` spans in `body`, so we can exclude matches
/// that are already linked.
fn wikilink_spans(body: &str) -> Vec<(usize, usize)> {
    let bytes = body.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(rel) = body[i + 2..].find("]]") {
                let end = i + 2 + rel + 2;
                spans.push((i, end));
                i = end;
                continue;
            }
        }
        i += 1;
    }
    spans
}

fn in_any_span(pos: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(s, e)| pos >= s && pos < e)
}

/// Is the byte at `idx` (or the absence of one, at string edge) a
/// word character? Used for boundary checks.
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Find unlinked, word-boundary, not-already-linked occurrences of
/// `title` in `body`. Returns the byte start offsets in `body`.
fn unlinked_offsets(body: &str, title_lower: &str, title_len: usize) -> Vec<usize> {
    if title_lower.is_empty() {
        return Vec::new();
    }
    let body_lower = body.to_lowercase();
    // Byte offsets from the lowercased haystack are used against the
    // ORIGINAL body. For ASCII (the overwhelming common case) they
    // align exactly. We still verify each slice before trusting it.
    let aligned = body_lower.len() == body.len();
    let spans = wikilink_spans(body);
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    for (idx, _) in body_lower.match_indices(title_lower) {
        if !aligned {
            // Unicode case-folding changed lengths somewhere; bail on
            // precise offsets rather than risk a mis-slice. (Rare.)
            // Still count it as a mention by recording idx 0 once.
            out.push(usize::MAX);
            continue;
        }
        let end = idx + title_len;
        if end > bytes.len() {
            continue;
        }
        // Word boundaries.
        let before_ok = idx == 0 || !is_word_byte(bytes[idx - 1]);
        let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
        if !before_ok || !after_ok {
            continue;
        }
        // Not already inside a wikilink.
        if in_any_span(idx, &spans) {
            continue;
        }
        out.push(idx);
    }
    out
}

/// Build a one-line snippet around the first occurrence offset.
fn snippet_around(body: &str, offset: usize, title_len: usize) -> String {
    if offset == usize::MAX {
        // Unaligned fallback: just take the first line that contains
        // the title case-insensitively.
        return body.lines().next().unwrap_or("").chars().take(120).collect();
    }
    let start = body[..offset]
        .char_indices()
        .rev()
        .nth(40)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end_target = offset + title_len + 40;
    let end = body
        .char_indices()
        .find(|&(i, _)| i >= end_target)
        .map(|(i, _)| i)
        .unwrap_or(body.len());
    let raw = &body[start..end];
    let compact: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < body.len() { "…" } else { "" };
    format!("{prefix}{compact}{suffix}")
}

/// The title (file stem) malt would hunt mentions of, or None if it's
/// too short to be worth scanning (1–2 chars produce noise). Exposed so
/// the caller can feed it to the index for candidate lookup.
pub fn title_for(target_path: &str) -> Option<String> {
    let title = std::path::Path::new(target_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if title.chars().count() < 3 {
        None
    } else {
        Some(title)
    }
}

/// Find unlinked mentions of `title` among `candidate_paths` — the notes
/// the index says contain the title text. Scoping to candidates instead
/// of the whole corpus keeps this O(matches), not O(vault), so it stays
/// fast as vaults grow into the tens of thousands of notes.
///
/// The candidate set is a superset (the index matches tokenized phrases;
/// we still do the precise word-boundary + not-already-linked check per
/// candidate here), so passing a slightly-too-broad list is fine.
pub fn find(target_path: &str, title: &str, candidate_paths: &[String]) -> Vec<UnlinkedMention> {
    let title_lower = title.to_lowercase();
    let title_len = title.len();

    let mut out = Vec::new();
    for path in candidate_paths {
        if path == target_path {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if crate::encryption::is_encrypted(&content) {
            continue;
        }
        let (_fm, body) = crate::frontmatter::split(&content);
        let offsets = unlinked_offsets(body, &title_lower, title_len);
        if offsets.is_empty() {
            continue;
        }
        let display_title = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string();
        let snippet = snippet_around(body, offsets[0], title_len);
        out.push(UnlinkedMention {
            path: path.clone(),
            title: display_title,
            snippet,
            count: offsets.len(),
        });
    }
    // Most-mentions first — those are the strongest latent links.
    out.sort_by(|a, b| b.count.cmp(&a.count));
    out
}

/// Rewrite the FIRST unlinked occurrence of `target_title` in
/// `source_path`'s body into a `[[target_title]]` wikilink, preserving
/// the original casing of the matched text. Returns Ok(()) even if no
/// occurrence is found (idempotent no-op) so the UI can refresh
/// uniformly. Refuses to touch encrypted notes.
pub fn link_first(source_path: &str, target_title: &str) -> Result<(), String> {
    let content = std::fs::read_to_string(source_path).map_err(|e| e.to_string())?;
    if crate::encryption::is_encrypted(&content) {
        return Err("can't edit an encrypted note".into());
    }
    let (fm, body) = crate::frontmatter::split(&content);
    let title_lower = target_title.to_lowercase();
    let title_len = target_title.len();
    let offsets = unlinked_offsets(body, &title_lower, title_len);
    let Some(&idx) = offsets.first() else {
        return Ok(()); // nothing to link
    };
    if idx == usize::MAX {
        return Ok(()); // unaligned fallback — don't risk a mis-slice
    }
    let end = idx + title_len;
    // Verify the slice really is the title (case-insensitive) before
    // mutating — belt-and-suspenders against offset drift.
    if body.get(idx..end).map(|s| s.to_lowercase()) != Some(title_lower) {
        return Ok(());
    }
    let matched = &body[idx..end]; // preserve original casing
    let new_body = format!("{}[[{}]]{}", &body[..idx], matched, &body[end..]);
    let new_content = crate::frontmatter::merge(&fm, &new_body);
    crate::notes::write_atomic(source_path, &new_content).map_err(|e| e.to_string())?;
    Ok(())
}
