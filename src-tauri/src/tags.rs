// Hashtag extraction + the canonical "tag line" convention.
//
// Storage model:
//   - The canonical tag line is the last non-empty line of the body when it
//     matches `^\s*(#tag(\s+#tag)*)\s*$`. The editor moves any inline #tags
//     into this line on autosave so the body stays clean of tag-clutter.
//   - We still recognize inline #tags anywhere (so a freshly typed tag is
//     found before relocation runs). YAML frontmatter `tags:` is read for
//     backward compat with Obsidian / Bear / nvalt files but never written.
//
// Tag grammar:
//   - Must follow whitespace, start-of-line, or one of [({\[
//   - Starts with `#` then an ASCII letter, then [a-z0-9_/-]*
//   - Bear-style nesting via `/`: #projects/malt — query `tag:projects` ALSO
//     matches `#projects/malt` (we index every slash-prefix).
//   - URLs and code blocks/spans are skipped.

use crate::frontmatter;

/// Extract a deduplicated, lowercase, sorted list of tags from a note body.
/// Includes both inline #tags and the canonical tag line; YAML frontmatter
/// `tags:` is folded in by the caller (which has access to the parsed
/// frontmatter) — this fn deals with the body only.
pub fn extract_tags_from_body(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let cleaned = strip_inline_code(line);
        for tag in scan_line(&cleaned) {
            out.push(tag);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Like `extract_tags_from_body`, but also folds in YAML frontmatter tags.
/// Use this when you want the full set of tags for indexing / display.
pub fn extract_tags_full(content: &str) -> Vec<String> {
    let (fm, body) = frontmatter::split(content);
    let mut tags = extract_tags_from_body(body);
    if let Some(yaml_tags) = fm.tags.as_ref() {
        for t in yaml_tags {
            let canon = canonicalize(t);
            if !canon.is_empty() && !tags.contains(&canon) {
                tags.push(canon);
            }
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

/// Expand a tag list with every slash-prefix so `#projects/malt` matches
/// queries for `tag:projects`. Used at index time only.
pub fn expand_with_slash_parents(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        out.push(t.clone());
        let mut prefix = String::new();
        for part in t.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if prefix != *t {
                out.push(prefix.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Normalize a tag string (strip leading #, lowercase, trim invalid chars
/// off the ends). Returns "" if the result isn't a valid tag.
pub fn canonicalize(raw: &str) -> String {
    let s = raw.trim().trim_start_matches('#').to_lowercase();
    if s.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if !chars[0].is_ascii_alphabetic() {
        return String::new();
    }
    let cleaned: String = chars
        .into_iter()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/'))
        .collect();
    cleaned
}

/// Strip `[[X]]` brackets, preserving inner text. The AI sees prose, not
/// markup. Same-line regex equivalent (multi-line wikilinks aren't a thing).
pub fn strip_wikilink_brackets(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // Look for matching ]] on same line, no nested [.
            let mut j = i + 2;
            let mut found = false;
            while j + 1 < bytes.len() {
                if bytes[j] == b'\n' || bytes[j] == b'[' {
                    break;
                }
                if bytes[j] == b']' && bytes[j + 1] == b']' {
                    found = true;
                    break;
                }
                j += 1;
            }
            if found {
                // Emit inner [i+2..j].
                out.push_str(&text[i + 2..j]);
                i = j + 2;
                continue;
            }
        }
        // Pass through one char (handle multi-byte).
        let rest = &text[i..];
        if let Some(c) = rest.chars().next() {
            out.push(c);
            i += c.len_utf8();
        } else {
            break;
        }
    }
    out
}

/// Strip the canonical tag line + every inline `#hashtag` + every `[[...]]`
/// bracket from `text`. Used by the AI call paths so our private markup
/// doesn't pollute the LLM context. Mirrors the TS `stripTagsForAI`.
pub fn strip_tags_for_ai(text: &str) -> String {
    // Strip wikilink brackets first so subsequent passes operate on prose.
    let text = strip_wikilink_brackets(text);
    let mut lines: Vec<String> = text.split('\n').map(String::from).collect();

    // Drop the canonical tag line if it ends `text`.
    let mut last_nonempty: Option<usize> = None;
    for (i, l) in lines.iter().enumerate().rev() {
        if !l.trim().is_empty() {
            last_nonempty = Some(i);
            break;
        }
    }
    if let Some(idx) = last_nonempty {
        if is_canonical_tag_line(&lines[idx]) {
            lines.remove(idx);
            if idx > 0 && lines[idx - 1].trim().is_empty() {
                lines.remove(idx - 1);
            }
        }
    }
    let joined = lines.join("\n");

    // Remove every inline hashtag, collapsing adjacent whitespace.
    let inline = inline_tag_positions(&joined);
    let mut out = String::with_capacity(joined.len());
    let mut cursor = 0usize;
    for (from, to) in inline {
        out.push_str(&joined[cursor..from]);
        let after = joined.as_bytes().get(to).copied();
        if matches!(after, Some(b' ') | Some(b'\t')) {
            cursor = to + 1;
        } else if out.ends_with(' ') || out.ends_with('\t') {
            out.pop();
            cursor = to;
        } else {
            cursor = to;
        }
    }
    out.push_str(&joined[cursor..]);

    // Trim trailing whitespace per line, collapse blank-line runs.
    let mut result = out
        .split('\n')
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }
    result
}

/// Return (from, to) byte offsets of every inline #tag in `text`. Mirrors
/// scan_line but returns positions, not strings.
pub fn inline_tag_positions(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut line_start = 0usize;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i <= bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            let line = &text[line_start..i];
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
            } else if !in_fence {
                scan_line_positions(line, line_start, &mut out);
            }
            line_start = i + 1;
        }
        i += 1;
    }
    out
}

fn scan_line_positions(line: &str, line_offset: usize, out: &mut Vec<(usize, usize)>) {
    let masked = strip_inline_code(line);
    let bytes = masked.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let prev = if i == 0 { None } else { Some(bytes[i - 1]) };
            let valid_lead = match prev {
                None => true,
                Some(b) => (b as char).is_whitespace() || matches!(b, b'(' | b'[' | b'{' | b'"' | b'\''),
            };
            if valid_lead {
                let start = i + 1;
                if start < bytes.len() && (bytes[start] as char).is_ascii_alphabetic() {
                    let mut j = start + 1;
                    while j < bytes.len()
                        && ((bytes[j] as char).is_ascii_alphanumeric()
                            || matches!(bytes[j], b'-' | b'_' | b'/'))
                    {
                        j += 1;
                    }
                    let mut end = j;
                    while end > start && bytes[end - 1] == b'/' {
                        end -= 1;
                    }
                    if end > start {
                        out.push((line_offset + i, line_offset + end));
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
}

/// True if `line` is *only* hashtags (and whitespace). Used to detect the
/// canonical tag line at the bottom of the body. Currently called from
/// tests + frontend (via IPC); silence dead-code while we wire things up.
#[allow(dead_code)]
pub fn is_canonical_tag_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut saw_tag = false;
    for tok in trimmed.split_whitespace() {
        if !tok.starts_with('#') {
            return false;
        }
        let body = &tok[1..];
        if body.is_empty() {
            return false;
        }
        let mut chars = body.chars();
        if !chars.next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
            return false;
        }
        if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/')) {
            return false;
        }
        saw_tag = true;
    }
    saw_tag
}

// ---------- internal helpers ----------

fn strip_inline_code(line: &str) -> String {
    // Replace runs between matched backticks with spaces so positions don't
    // shift relative to the original line. We only care about exclusion, not
    // re-rendering, so this is fine.
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for c in line.chars() {
        if c == '`' {
            in_code = !in_code;
            out.push(' ');
        } else if in_code {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

fn scan_line(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#' && is_valid_lead(&chars, i) {
            let start = i + 1;
            if start < chars.len() && chars[start].is_ascii_alphabetic() {
                let mut j = start + 1;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '_' | '/'))
                {
                    j += 1;
                }
                // Strip a trailing `/` (e.g. `#a/`) — partial nesting, not a tag.
                let mut end = j;
                while end > start && chars[end - 1] == '/' {
                    end -= 1;
                }
                if end > start {
                    let tag: String = chars[start..end].iter().collect::<String>().to_lowercase();
                    out.push(tag);
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn is_valid_lead(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = chars[i - 1];
    if prev.is_whitespace() {
        return true;
    }
    matches!(prev, '(' | '[' | '{' | '"' | '\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_simple_inline() {
        let tags = extract_tags_from_body("Hello #world today");
        assert_eq!(tags, vec!["world"]);
    }

    #[test]
    fn ignores_digit_start() {
        let tags = extract_tags_from_body("Issue #1 reported");
        assert!(tags.is_empty());
    }

    #[test]
    fn ignores_markdown_heading() {
        let tags = extract_tags_from_body("# heading\nbody");
        assert!(tags.is_empty());
    }

    #[test]
    fn handles_nesting() {
        let tags = extract_tags_from_body("#projects/malt is going well");
        assert_eq!(tags, vec!["projects/malt"]);
    }

    #[test]
    fn skips_code_fence() {
        let body = "before #foo\n```\n#bar in code\n```\n#baz after";
        let tags = extract_tags_from_body(body);
        assert_eq!(tags, vec!["baz", "foo"]);
    }

    #[test]
    fn skips_inline_code() {
        let tags = extract_tags_from_body("Use `#tag` like this #yes");
        assert_eq!(tags, vec!["yes"]);
    }

    #[test]
    fn skips_url_fragments() {
        let tags = extract_tags_from_body("See https://example.com/page#section for details");
        assert!(tags.is_empty());
    }

    #[test]
    fn canonical_tag_line() {
        assert!(is_canonical_tag_line("#foo #bar"));
        assert!(is_canonical_tag_line("  #foo/bar  "));
        assert!(!is_canonical_tag_line("#foo and bar"));
        assert!(!is_canonical_tag_line("just text"));
        assert!(!is_canonical_tag_line(""));
    }

    #[test]
    fn slash_parents() {
        let expanded = expand_with_slash_parents(&["projects/malt/v0".to_string()]);
        assert_eq!(
            expanded,
            vec![
                "projects".to_string(),
                "projects/malt".to_string(),
                "projects/malt/v0".to_string(),
            ]
        );
    }
}
