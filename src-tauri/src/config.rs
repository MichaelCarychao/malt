use crate::providers::Provider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub tagging_enabled: bool,
    /// Default model for the active provider — kept for back-compat
    /// with installs that predate multi-provider. New provider-aware
    /// code reads `provider_models[active_provider]` first, falling
    /// back to this field, falling back to `Provider::default_model()`.
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
    /// When true (default), drop every cached encrypted-note password on
    /// window blur so encrypted notes re-lock as soon as the user
    /// switches away. Turn off if you trust your local environment and
    /// don't want to re-enter passwords constantly.
    #[serde(default = "default_true")]
    pub reprompt_on_blur: bool,
    /// Which provider AI calls dispatch to. Defaults to Anthropic so
    /// existing installs don't have to reconfigure. The keyring slot
    /// for each provider is independent (see `secrets.rs`), so the
    /// user can store one key per provider and swap freely.
    #[serde(default = "default_provider")]
    pub active_provider: Provider,
    /// Per-provider model preferences. Lets you keep "claude-opus-4-7"
    /// selected for Anthropic AND "gpt-5" for OpenAI without one
    /// clobbering the other when you switch providers.
    #[serde(default)]
    pub provider_models: HashMap<String, String>,
    /// When true (default), a freshly-created daily note (Cmd/Ctrl+D)
    /// is seeded with a #journal tag. Turn off if you'd rather start
    /// the daily note blank.
    #[serde(default = "default_true")]
    pub daily_note_tag: bool,
}

fn default_true() -> bool {
    true
}

fn default_provider() -> Provider {
    Provider::Anthropic
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tagging_enabled: false,
            completion_model: default_completion_model(),
            notes_dir: None,
            tag_vocabulary: default_tag_vocabulary(),
            reprompt_on_blur: true,
            active_provider: default_provider(),
            provider_models: HashMap::new(),
            daily_note_tag: true,
        }
    }
}

impl Config {
    /// Resolve the model name for a given provider: prefer the
    /// per-provider override, fall back to the legacy
    /// `completion_model` for Anthropic only, fall back to the
    /// provider's compile-time default.
    pub fn model_for(&self, provider: Provider) -> String {
        if let Some(m) = self.provider_models.get(provider.id()) {
            if !m.trim().is_empty() {
                return m.clone();
            }
        }
        if provider == Provider::Anthropic && !self.completion_model.trim().is_empty() {
            return self.completion_model.clone();
        }
        provider.default_model().to_string()
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
