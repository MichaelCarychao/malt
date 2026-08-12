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
    /// Per-provider endpoint overrides for the OpenAI-compat client,
    /// keyed by `provider.id()`. Exists for LM Studio, whose server
    /// lives wherever the user runs it (localhost, LAN, Tailscale) —
    /// but any compat provider can be repointed (e.g. through a proxy).
    /// Empty/absent means "use `Provider::openai_base_url()`".
    #[serde(default)]
    pub provider_base_urls: HashMap<String, String>,
    /// When true (default), a freshly-created daily note (Cmd/Ctrl+D)
    /// is seeded with a #journal tag. Turn off if you'd rather start
    /// the daily note blank.
    #[serde(default = "default_true")]
    pub daily_note_tag: bool,
    /// Per-tag visual flair applied to note cards in the sidebar (an
    /// icon glyph + an accent color). Purely presentational — lets a
    /// flat folder that mixes content types (e.g. #element / #pitch /
    /// #story) read at a glance. The editor is deliberately left alone.
    #[serde(default)]
    pub tag_styles: Vec<TagStyle>,
    /// Absolute paths of pinned notes. Pinned notes sort to the top of
    /// the list and stay visible during text searches. Paths are unique
    /// per vault, so a single global list is naturally vault-scoped (only
    /// the active vault's paths match its notes).
    #[serde(default)]
    pub pinned_paths: Vec<String>,
}

/// A single tag → flair mapping. `tag` is the canonical tag name (no
/// leading `#`). `icon` is an optional short glyph/emoji shown before the
/// title on matching cards. `color` is an optional CSS color (hex) used
/// as the card's accent. Empty `icon`/`color` means "don't apply that
/// part," so a style can be icon-only, color-only, or both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagStyle {
    pub tag: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub color: String,
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
            provider_base_urls: HashMap::new(),
            daily_note_tag: true,
            tag_styles: Vec::new(),
            pinned_paths: Vec::new(),
        }
    }
}

/// Toggle a note's pinned state; returns the new pinned list. Uses
/// `load_for_update` so a transiently unreadable config.json aborts the
/// toggle instead of saving a default config over the user's settings.
pub fn toggle_pin(path: &str) -> Result<Vec<String>, String> {
    let mut cfg = load_for_update()?;
    if let Some(i) = cfg.pinned_paths.iter().position(|p| p == path) {
        cfg.pinned_paths.remove(i);
    } else {
        cfg.pinned_paths.push(path.to_string());
    }
    save(&cfg).map_err(|e| e.to_string())?;
    Ok(cfg.pinned_paths)
}

/// Repoint a pin after a rename (keeps the pin on the renamed file).
/// Best-effort: skipped (not defaulted!) when the config can't be loaded.
pub fn repoint_pin(old: &str, new: &str) {
    let Ok(mut cfg) = load_for_update() else { return };
    let mut changed = false;
    for p in cfg.pinned_paths.iter_mut() {
        if p == old {
            *p = new.to_string();
            changed = true;
        }
    }
    if changed {
        let _ = save(&cfg);
    }
}

/// Drop a pin (after delete or move-out-of-vault). Best-effort: skipped
/// (not defaulted!) when the config can't be loaded.
pub fn remove_pin(path: &str) {
    let Ok(mut cfg) = load_for_update() else { return };
    let before = cfg.pinned_paths.len();
    cfg.pinned_paths.retain(|p| p != path);
    if cfg.pinned_paths.len() != before {
        let _ = save(&cfg);
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

    /// Resolve the compat-client base URL for a provider: user override
    /// first, compile-time default second. None for Anthropic (its
    /// dedicated client hardcodes the /v1/messages URL), even if an
    /// override is somehow present.
    pub fn base_url_for(&self, provider: Provider) -> Option<String> {
        if provider.openai_base_url().is_none() {
            return None;
        }
        if let Some(u) = self.provider_base_urls.get(provider.id()) {
            let u = u.trim().trim_end_matches('/');
            if !u.is_empty() {
                return Some(u.to_string());
            }
        }
        provider.openai_base_url().map(str::to_string)
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

/// How a config-style JSON file read went. The distinction matters because
/// the failure modes need OPPOSITE handling: a missing file is a legitimate
/// first run (seed defaults), a corrupt file must be preserved before we
/// touch anything (quarantine + start fresh), and a transiently unreadable
/// file (AV lock, disk hiccup) must never be "repaired" — acting on defaults
/// and then saving would permanently overwrite the user's real data with
/// them. That last case is exactly how a one-off IO error used to wipe
/// vaults.json / pins / saved searches.
pub(crate) enum JsonRead<T> {
    Parsed(T),
    /// File doesn't exist — safe to seed + persist defaults.
    Missing,
    /// File existed but didn't parse. The original has been renamed to a
    /// `.corrupt-<ts>.bak` sibling, so seeding + persisting defaults is safe
    /// and the user's bytes are recoverable.
    Quarantined,
    /// File exists but couldn't be read (lock/permissions/IO). Treat as
    /// read-only fallback: use in-memory defaults, NEVER persist them.
    Unreadable,
}

/// Read + parse a JSON file with the failure isolation described on
/// [`JsonRead`]. Shared by config / vaults / saved-searches / prompts.
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> JsonRead<T> {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<T>(&s) {
            Ok(v) => JsonRead::Parsed(v),
            Err(e) => {
                eprintln!(
                    "malt: {} is corrupt ({e}); quarantining and starting fresh",
                    path.display()
                );
                quarantine_corrupt(path);
                JsonRead::Quarantined
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => JsonRead::Missing,
        Err(e) => {
            eprintln!(
                "malt: {} unreadable ({e}); using defaults WITHOUT persisting",
                path.display()
            );
            JsonRead::Unreadable
        }
    }
}

/// Move an unparseable file aside (`<name>.corrupt-<unix-secs>.bak`) so a
/// fresh default can be written without destroying the user's original.
/// Best-effort: if even the rename fails we leave the file alone — the
/// caller must then treat it as Unreadable (no persist).
fn quarantine_corrupt(path: &PathBuf) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut bak = path.as_os_str().to_os_string();
    bak.push(format!(".corrupt-{ts}.bak"));
    let _ = std::fs::rename(path, PathBuf::from(bak));
}

pub fn load() -> Config {
    match read_json::<Config>(&config_path()) {
        JsonRead::Parsed(c) => c,
        _ => Config::default(),
    }
}

/// Load for a read-modify-write cycle. Errors when the file exists but
/// can't be read — proceeding would make the subsequent `save()` overwrite
/// the user's real config with defaults. Missing/quarantined files are fine
/// to start from defaults (nothing valid would be lost by saving).
pub(crate) fn load_for_update() -> Result<Config, String> {
    match read_json::<Config>(&config_path()) {
        JsonRead::Parsed(c) => Ok(c),
        JsonRead::Missing | JsonRead::Quarantined => Ok(Config::default()),
        JsonRead::Unreadable => Err(
            "config.json exists but can't be read right now — change not saved \
             (retry in a moment)"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_override_and_fallback() {
        let mut cfg = Config::default();
        // Defaults: compile-time seed for compat providers, None for Anthropic.
        assert_eq!(
            cfg.base_url_for(Provider::LmStudio).as_deref(),
            Some("http://localhost:1234/v1")
        );
        assert_eq!(cfg.base_url_for(Provider::Anthropic), None);
        // Override wins; trailing slash stripped (the client appends its own).
        cfg.provider_base_urls.insert(
            "lmstudio".into(),
            "http://sync.tailnet.ts.net:1234/v1/".into(),
        );
        assert_eq!(
            cfg.base_url_for(Provider::LmStudio).as_deref(),
            Some("http://sync.tailnet.ts.net:1234/v1")
        );
        // Blank override behaves like no override.
        cfg.provider_base_urls.insert("lmstudio".into(), "   ".into());
        assert_eq!(
            cfg.base_url_for(Provider::LmStudio).as_deref(),
            Some("http://localhost:1234/v1")
        );
        // Anthropic never resolves a compat URL, override or not.
        cfg.provider_base_urls
            .insert("anthropic".into(), "http://example.invalid".into());
        assert_eq!(cfg.base_url_for(Provider::Anthropic), None);
    }
}

pub fn save(cfg: &Config) -> std::io::Result<()> {
    // Never let a serialization failure truncate a valid config: propagate
    // the error instead of writing an empty string over existing data.
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Atomic temp-file + rename so a crash mid-write can't leave a
    // half-written / empty config.json behind.
    crate::notes::write_atomic(config_path(), &json)
}
