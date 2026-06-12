use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Frontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Every other YAML key, preserved verbatim. malt only *interprets*
    /// `tags`, but external systems (and humans) may store arbitrary
    /// metadata here — `id`, `status`, image refs, pipeline edges. Before
    /// this existed, `merge()` rebuilt files from the typed struct alone
    /// and silently dropped unknown keys on every rename / link-mention /
    /// auto-tag round-trip. Flattening keeps them intact so malt is a safe
    /// editor over a folder some other tool also owns.
    #[serde(flatten)]
    pub extra: serde_yaml::Mapping,
}

/// Split a `.md` file's content into its YAML frontmatter (parsed) and the
/// body following it. If no frontmatter is present, returns a default
/// Frontmatter and the original content.
pub fn split(content: &str) -> (Frontmatter, &str) {
    let after_open = if let Some(s) = content.strip_prefix("---\n") {
        s
    } else if let Some(s) = content.strip_prefix("---\r\n") {
        s
    } else {
        return (Frontmatter::default(), content);
    };

    let (yaml_str, body) = if let Some(pos) = after_open.find("\n---\n") {
        (&after_open[..pos], &after_open[pos + 5..])
    } else if let Some(pos) = after_open.find("\n---\r\n") {
        (&after_open[..pos], &after_open[pos + 6..])
    } else {
        return (Frontmatter::default(), content);
    };

    // Unparseable YAML between valid markers: treat the WHOLE file as body
    // (no frontmatter) rather than defaulting. Returning a default here used
    // to make every rewrite path (rename cascade, auto-tag, link-mention)
    // emit merge(default, body) — silently deleting the user's hand-written
    // (if malformed) frontmatter block from the file. As opaque body text it
    // survives the round-trip byte-for-byte; the cost is merely that tags/
    // metadata inside it aren't interpreted until the YAML is fixed.
    match serde_yaml::from_str::<Frontmatter>(yaml_str) {
        Ok(fm) => (fm, body),
        Err(_) => (Frontmatter::default(), content),
    }
}

/// Build a file string from a frontmatter + body. Skips writing the YAML
/// markers entirely if the frontmatter has nothing to serialize — but
/// "nothing" now means no tags AND no preserved extra keys, so we never
/// drop metadata some other tool put there.
pub fn merge(fm: &Frontmatter, body: &str) -> String {
    let has_tags = fm.tags.as_ref().is_some_and(|t| !t.is_empty());
    let has_extra = !fm.extra.is_empty();
    if !has_tags && !has_extra {
        return body.trim_start_matches('\n').to_string();
    }
    let yaml = serde_yaml::to_string(fm).unwrap_or_default();
    format!("---\n{}---\n{}", yaml, body.trim_start_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_captures_unknown_keys() {
        let content = "---\nid: abc-123\nstatus: approved\ntags:\n  - element\n---\nProse body.\n";
        let (fm, body) = split(content);
        assert_eq!(fm.tags.as_deref(), Some(&["element".to_string()][..]));
        assert_eq!(body, "Prose body.\n");
        assert_eq!(
            fm.extra.get("id").and_then(|v| v.as_str()),
            Some("abc-123")
        );
        assert_eq!(
            fm.extra.get("status").and_then(|v| v.as_str()),
            Some("approved")
        );
    }

    #[test]
    fn merge_preserves_unknown_keys_when_tags_wiped() {
        // Simulates the tag-relocation / rename path: tags get cleared
        // (moved to the inline canonical line) but external metadata must
        // survive on disk.
        let content = "---\nid: keep-me\nstatus: pending\ntags:\n  - story\n---\nBody.\n";
        let (mut fm, body) = split(content);
        fm.tags = None;
        let out = merge(&fm, body);
        assert!(out.contains("id: keep-me"), "id dropped: {out}");
        assert!(out.contains("status: pending"), "status dropped: {out}");
        assert!(!out.contains("story"), "tag list should be wiped: {out}");
        assert!(out.contains("Body."));
    }

    #[test]
    fn merge_emits_no_frontmatter_when_truly_empty() {
        let fm = Frontmatter::default();
        assert_eq!(merge(&fm, "Just a body.\n"), "Just a body.\n");
    }

    // Regression: malformed YAML between valid markers must survive a
    // split → merge round-trip as opaque body text, not get dropped.
    #[test]
    fn malformed_yaml_is_preserved_as_body() {
        let content = "---\nid: ok\n\tbad: tabs aren't valid yaml indentation\n---\nBody.\n";
        let (fm, body) = split(content);
        assert!(fm.tags.is_none());
        assert!(fm.extra.is_empty());
        assert_eq!(body, content, "unparseable frontmatter must stay in the body");
        let out = merge(&fm, body);
        assert_eq!(out, content, "round-trip must be lossless");
    }
}
