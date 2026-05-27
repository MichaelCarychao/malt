use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub tagging_enabled: bool,
    #[serde(default = "default_completion_model")]
    pub completion_model: String,
    /// Optional override for the notes directory. When None, malt falls back
    /// to `~/malt/`. Settable in the UI so notes can live in e.g. Dropbox.
    #[serde(default)]
    pub notes_dir: Option<String>,
    /// Starter vocabulary surfaced first in #-autocomplete and recommended
    /// in the editor's pill row. Object/status tags by design.
    #[serde(default = "default_tag_vocabulary")]
    pub tag_vocabulary: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tagging_enabled: false,
            completion_model: default_completion_model(),
            notes_dir: None,
            tag_vocabulary: default_tag_vocabulary(),
        }
    }
}

fn default_tag_vocabulary() -> Vec<String> {
    vec![
        "draft".to_string(),
        "fleeting".to_string(),
        "waiting".to_string(),
        "archive".to_string(),
        "meeting".to_string(),
        "recipe".to_string(),
        "journal".to_string(),
    ]
}

fn default_completion_model() -> String {
    "claude-haiku-4-5".to_string()
}

fn config_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    let _ = std::fs::create_dir_all(&p);
    p.push("config.json");
    p
}

pub fn load() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(cfg).unwrap_or_default();
    std::fs::write(config_path(), json)
}
