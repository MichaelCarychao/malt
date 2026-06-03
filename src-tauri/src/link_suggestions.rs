// Deterministic post-hoc link suggestions: scan the body of a note for
// case-insensitive word-boundary occurrences of any *other* note's title.
// AI-driven entity suggestions are a separate layer (added next).
//
// Exclusions (we don't want to suggest re-linking text that's already
// special-cased): code fences, inline `code`, existing [[wikilinks]],
// inline #hashtags.

use crate::frontmatter;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct LinkSuggestion {
    /// Original-case text of the first match (used as the label in the UI).
    pub term: String,
    /// The note we'd link to.
    pub candidate_title: String,
    /// Canonical path of that note (so the frontend can resolve it).
    pub candidate_path: String,
    /// "link" for existing-note matches. (AI-proposed entity creation will
    /// use "create" once that layer lands.)
    pub kind: String,
    /// Byte offsets in the original body where the term appears.
    pub positions: Vec<(usize, usize)>,
}

/// Build `kind: "create"` LinkSuggestions for AI-proposed entity names by
/// scanning the body for word-boundary occurrences of each name. Skips
/// entities whose canonical form matches an existing note title (those
/// would already be in the deterministic pass). Uses the same exclusion
/// mask logic so we don't suggest wrapping text that's already in a
/// wikilink, code block, or hashtag.
pub fn build_entity_suggestions(content: &str, entities: &[String]) -> Vec<LinkSuggestion> {
    let (_fm, body) = frontmatter::split(content);
    let mask = build_exclusion_mask(body);
    let body_lower = body.to_lowercase();
    let existing_titles: std::collections::HashSet<String> = crate::notes::list_notes()
        .into_iter()
        .map(|n| n.title.to_lowercase())
        .collect();
    let mut out: Vec<LinkSuggestion> = Vec::new();
    let mut seen_titles: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entity in entities {
        let canon = entity.trim();
        if canon.chars().count() < 2 {
            continue;
        }
        let entity_lower = canon.to_lowercase();
        // Skip duplicates from the AI and anything that matches an existing
        // note title (the deterministic pass already handled those).
        if !seen_titles.insert(entity_lower.clone()) {
            continue;
        }
        if existing_titles.contains(&entity_lower) {
            continue;
        }
        let positions = find_word_boundary_matches(&body_lower, &entity_lower, &mask);
        if positions.is_empty() {
            // AI hallucinated a name that isn't actually in the body — skip.
            continue;
        }
        let (first_start, first_end) = positions[0];
        let term = body[first_start..first_end].to_string();
        out.push(LinkSuggestion {
            term,
            candidate_title: canon.to_string(),
            candidate_path: String::new(),
            kind: "create".to_string(),
            positions,
        });
    }
    out
}

pub fn suggest_for_note(content: &str, current_path: &str) -> Vec<LinkSuggestion> {
    let (_fm, body) = frontmatter::split(content);
    let mask = build_exclusion_mask(body);

    let body_lower = body.to_lowercase();
    let mut out: Vec<LinkSuggestion> = Vec::new();

    // Sort notes by title length descending so longer (more specific)
    // matches appear first in the UI.
    let mut notes = crate::notes::list_notes();
    notes.sort_by(|a, b| b.title.chars().count().cmp(&a.title.chars().count()));

    for note in &notes {
        if note.path == current_path {
            continue;
        }
        let title = &note.title;
        // Skip 1-char titles — too many false positives.
        if title.chars().count() < 2 {
            continue;
        }
        let title_lower = title.to_lowercase();
        // Sanity: if title is whitespace-only, skip.
        if title_lower.trim().is_empty() {
            continue;
        }

        let positions = find_word_boundary_matches(&body_lower, &title_lower, &mask);
        if positions.is_empty() {
            continue;
        }
        let (first_start, first_end) = positions[0];
        let term = body[first_start..first_end].to_string();
        out.push(LinkSuggestion {
            term,
            candidate_title: title.clone(),
            candidate_path: note.path.clone(),
            kind: "link".to_string(),
            positions,
        });
    }
    out
}

fn find_word_boundary_matches(
    haystack_lower: &str,
    needle_lower: &str,
    mask: &[bool],
) -> Vec<(usize, usize)> {
    let mut positions: Vec<(usize, usize)> = Vec::new();
    if needle_lower.is_empty() {
        return positions;
    }
    let needle_len = needle_lower.len();
    let mut start = 0usize;
    while let Some(rel_idx) = haystack_lower[start..].find(needle_lower) {
        let abs_start = start + rel_idx;
        let abs_end = abs_start + needle_len;
        if is_word_boundary(haystack_lower, abs_start, abs_end)
            && !overlaps_mask(mask, abs_start, abs_end)
        {
            positions.push((abs_start, abs_end));
        }
        // Advance by at least one to avoid infinite loop on empty needles.
        start = abs_start + needle_len.max(1);
        if start > haystack_lower.len() {
            break;
        }
    }
    positions
}

fn overlaps_mask(mask: &[bool], from: usize, to: usize) -> bool {
    for i in from..to {
        if i < mask.len() && mask[i] {
            return true;
        }
    }
    false
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let prev_ok = if start == 0 {
        true
    } else {
        let prev = bytes[start - 1];
        // Non-ASCII bytes (multi-byte UTF-8 chars) are >= 0x80, never
        // alphanumeric in the ASCII sense — that's the right answer for
        // word-boundary purposes here.
        !(prev as char).is_ascii_alphanumeric() && prev != b'_'
    };
    let next_ok = if end >= bytes.len() {
        true
    } else {
        let next = bytes[end];
        !(next as char).is_ascii_alphanumeric() && next != b'_'
    };
    prev_ok && next_ok
}

/// Build a per-byte mask of just the *code* regions of `body`: line-based
/// code fences (``` / ~~~) and single-line inline backtick spans. Shared
/// with the backlink rewriter (`backlinks::rewrite_wikilinks_in_body` /
/// `scan_wikilinks`) so that a `[[link]]` written inside a code sample is
/// never resolved, counted as a backlink, or rewritten on rename — keeping
/// code blocks byte-for-byte intact. This is deliberately narrower than
/// `build_exclusion_mask`, which *also* masks wikilinks + hashtags (right
/// for suggestion scanning, wrong for the link parser that needs to SEE
/// the wikilinks).
pub(crate) fn code_mask(body: &str) -> Vec<bool> {
    let bytes = body.as_bytes();
    let mut mask = vec![false; bytes.len()];

    // --- Code fences (line-based ``` / ~~~) ---
    let mut line_start = 0usize;
    let mut in_fence = false;
    let mut fence_start: Option<usize> = None;
    let mut i = 0usize;
    while i <= bytes.len() {
        let at_line_end = i == bytes.len() || bytes[i] == b'\n';
        if at_line_end {
            let line = &body[line_start..i];
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                if in_fence {
                    // closing
                    if let Some(start) = fence_start.take() {
                        let end = i.min(bytes.len());
                        for j in start..end {
                            mask[j] = true;
                        }
                    }
                    in_fence = false;
                } else {
                    in_fence = true;
                    fence_start = Some(line_start);
                }
            }
            line_start = i + 1;
        }
        i += 1;
    }
    if in_fence {
        if let Some(start) = fence_start {
            for j in start..bytes.len() {
                mask[j] = true;
            }
        }
    }

    // --- Inline backtick code spans (single line) ---
    let mut in_code = false;
    let mut code_start = 0usize;
    for j in 0..bytes.len() {
        if bytes[j] == b'\n' {
            in_code = false; // backticks don't span lines
            continue;
        }
        if bytes[j] == b'`' {
            if in_code {
                for k in code_start..=j {
                    if k < mask.len() {
                        mask[k] = true;
                    }
                }
                in_code = false;
            } else {
                code_start = j;
                in_code = true;
            }
        }
    }

    mask
}

/// Build a per-byte mask of regions we should NOT consider for link
/// suggestions: code fences, inline code, existing wikilinks, hashtags.
fn build_exclusion_mask(body: &str) -> Vec<bool> {
    let bytes = body.as_bytes();
    // Start from the code-only mask (fences + inline backticks), then layer
    // wikilink + hashtag exclusions on top.
    let mut mask = code_mask(body);

    // --- Existing [[wikilinks]] ---
    let mut k = 0usize;
    while k + 3 < bytes.len() {
        if bytes[k] == b'[' && bytes[k + 1] == b'[' {
            let mut m = k + 2;
            let mut found = false;
            while m + 1 < bytes.len() {
                if bytes[m] == b'\n' || bytes[m] == b'[' {
                    break;
                }
                if bytes[m] == b']' && bytes[m + 1] == b']' {
                    for n in k..=(m + 1) {
                        if n < mask.len() {
                            mask[n] = true;
                        }
                    }
                    found = true;
                    k = m + 2;
                    break;
                }
                m += 1;
            }
            if !found {
                k += 1;
            }
        } else {
            k += 1;
        }
    }

    // --- Hashtags (inline and the canonical line) ---
    for (start, end) in crate::tags::inline_tag_positions(body) {
        for j in start..end {
            if j < mask.len() {
                mask[j] = true;
            }
        }
    }

    mask
}
