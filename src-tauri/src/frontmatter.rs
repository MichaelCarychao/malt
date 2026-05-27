use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct Frontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
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

    let fm: Frontmatter = serde_yaml::from_str(yaml_str).unwrap_or_default();
    (fm, body)
}

/// Build a file string from a frontmatter + body. Skips writing the YAML
/// markers entirely if the frontmatter has nothing to serialize.
pub fn merge(fm: &Frontmatter, body: &str) -> String {
    let has_tags = fm.tags.as_ref().is_some_and(|t| !t.is_empty());
    if !has_tags {
        return body.trim_start_matches('\n').to_string();
    }
    let yaml = serde_yaml::to_string(fm).unwrap_or_default();
    format!("---\n{}---\n{}", yaml, body.trim_start_matches('\n'))
}
