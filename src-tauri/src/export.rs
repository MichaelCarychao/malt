// Per-note export pipeline. Strips malt-private markup (frontmatter,
// canonical tag line, inline hashtags, wikilink brackets) so exported
// files are universally readable. Optional `append_links` walks the
// source's wikilinks (depth 1) and appends each linked note as its own
// section — useful for TOC-style notes that link out to multi-part work.

use crate::{backlinks, frontmatter, tags};

/// Build the cleaned, possibly composite markdown source for an export.
/// If `append_links` is true, every resolvable wikilink target's content
/// gets appended (cleaned the same way) after the source note, separated
/// by `\n\n---\n\n# <title>\n\n`. Targets are de-duplicated by path; first
/// occurrence wins for ordering. Recursion is intentionally NOT done —
/// depth 1 only — to keep exports bounded.
pub fn build_composite_markdown(source_path: &str, append_links: bool) -> Result<String, String> {
    let raw = std::fs::read_to_string(source_path).map_err(|e| e.to_string())?;
    let (_fm, body) = frontmatter::split(&raw);

    // Capture link targets BEFORE stripping wikilink brackets.
    let targets = if append_links {
        backlinks::resolved_targets_in(body)
    } else {
        Vec::new()
    };

    let mut out = tags::strip_tags_for_ai(body);

    if append_links {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_, path) in targets {
            if path == source_path {
                continue;
            }
            if !seen.insert(path.clone()) {
                continue;
            }
            let linked_raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let (_fm, linked_body) = frontmatter::split(&linked_raw);
            let cleaned = tags::strip_tags_for_ai(linked_body);
            if cleaned.trim().is_empty() {
                continue;
            }
            let title = std::path::Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("untitled")
                .to_string();
            // Trim trailing newlines from the running output so the
            // separator block has predictable spacing.
            while out.ends_with('\n') {
                out.pop();
            }
            out.push_str("\n\n---\n\n# ");
            out.push_str(&title);
            out.push_str("\n\n");
            out.push_str(&cleaned);
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Render markdown → HTML body fragment (no <html>/<head> wrapper).
pub fn markdown_to_html_body(md: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(md, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// Render markdown → standalone HTML document with minimal print-ready CSS.
pub fn markdown_to_html_document(md: &str, title: &str) -> String {
    let body = markdown_to_html_body(md);
    let title_escaped = escape_html(title);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title_escaped}</title>
<style>
  :root {{ color-scheme: light dark; }}
  html, body {{ background: #fefefe; color: #1a1a1a; }}
  @media (prefers-color-scheme: dark) {{
    html, body {{ background: #1a1a1a; color: #e6e6e6; }}
    a {{ color: #6cb6ff; }}
    hr {{ border-color: #333; }}
    code, pre {{ background: #222; }}
  }}
  body {{
    font-family: ui-serif, Georgia, Cambria, "Times New Roman", serif;
    font-size: 17px;
    line-height: 1.6;
    max-width: 38em;
    margin: 2.5em auto;
    padding: 0 1.25em;
  }}
  h1, h2, h3, h4, h5, h6 {{
    font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
    line-height: 1.25;
    margin: 1.6em 0 0.5em;
  }}
  h1 {{ font-size: 1.8em; }}
  h2 {{ font-size: 1.4em; }}
  h3 {{ font-size: 1.15em; }}
  p, ul, ol, blockquote, pre {{ margin: 0 0 1em; }}
  ul, ol {{ padding-left: 1.5em; }}
  blockquote {{
    border-left: 3px solid #ccc;
    padding-left: 1em;
    color: #555;
    margin-left: 0;
  }}
  hr {{ border: 0; border-top: 1px solid #ddd; margin: 2.5em 0; }}
  code, pre {{
    font-family: "SF Mono", "Cascadia Mono", Menlo, Consolas, monospace;
    font-size: 0.9em;
    background: #f4f4f4;
  }}
  code {{ padding: 1px 4px; border-radius: 3px; }}
  pre {{ padding: 0.8em 1em; overflow-x: auto; border-radius: 4px; }}
  pre code {{ background: transparent; padding: 0; }}
  img {{ max-width: 100%; height: auto; }}
  table {{ border-collapse: collapse; }}
  table td, table th {{ border: 1px solid #ddd; padding: 0.3em 0.6em; }}
</style>
</head>
<body>
{body}
</body>
</html>
"#
    )
}

/// Render markdown → plain text by walking pulldown-cmark events and
/// keeping only text content + paragraph breaks. Headings and list items
/// each become bare paragraphs.
pub fn markdown_to_plaintext(md: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};
    let parser = Parser::new(md);
    let mut out = String::new();
    for event in parser {
        match event {
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => {
                out.push_str(&t);
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push('\n'),
            Event::End(TagEnd::Heading(_))
            | Event::End(TagEnd::Paragraph)
            | Event::End(TagEnd::Item)
            | Event::End(TagEnd::CodeBlock)
            | Event::End(TagEnd::BlockQuote(_)) => {
                out.push_str("\n\n");
            }
            Event::Start(Tag::Item) => {
                // Bullet/number — keep it light so plain-text still reads as list.
                out.push_str("• ");
            }
            Event::Rule => {
                out.push_str("\n---\n\n");
            }
            _ => {}
        }
    }
    // Collapse 3+ newlines to 2.
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.trim().to_string()
}

/// Build an EPUB 3 file from cleaned markdown. Single-chapter for now —
/// the composite-doc convention puts all linked-note appended sections in
/// the same XHTML doc, separated by horizontal rules.
pub fn markdown_to_epub(md: &str, title: &str) -> Result<Vec<u8>, String> {
    use epub_builder::{EpubBuilder, EpubContent, ReferenceType, ZipLibrary};
    let body = markdown_to_html_body(md);
    let title_escaped = escape_html(title);
    let xhtml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" lang="en" xml:lang="en">
<head><title>{title_escaped}</title>
<style type="text/css">
body {{ font-family: serif; line-height: 1.5; }}
h1, h2, h3 {{ font-family: sans-serif; }}
code, pre {{ font-family: monospace; }}
pre {{ background: #f4f4f4; padding: 0.5em; overflow: auto; }}
blockquote {{ border-left: 3px solid #ccc; padding-left: 0.8em; color: #555; }}
hr {{ border: 0; border-top: 1px solid #ccc; margin: 2em 0; }}
</style>
</head>
<body>
{body}
</body>
</html>"#
    );

    let mut output: Vec<u8> = Vec::new();
    let zip = ZipLibrary::new().map_err(|e| format!("epub zip: {e}"))?;
    // Can't chain to .generate() because it takes `self` while the other
    // builder methods return `&mut Self`. Bind it explicitly.
    let mut builder = EpubBuilder::new(zip).map_err(|e| format!("epub init: {e}"))?;
    builder
        .metadata("title", title)
        .map_err(|e| format!("epub title: {e}"))?;
    builder
        .metadata("author", "malt")
        .map_err(|e| format!("epub author: {e}"))?;
    builder
        .add_content(
            EpubContent::new("note.xhtml", xhtml.as_bytes())
                .title(title)
                .reftype(ReferenceType::Text),
        )
        .map_err(|e| format!("epub content: {e}"))?;
    builder
        .generate(&mut output)
        .map_err(|e| format!("epub generate: {e}"))?;
    Ok(output)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
