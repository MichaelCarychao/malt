// LLM provider registry.
//
// malt now speaks five providers, four of which share an OpenAI-style
// Chat Completions wire format (OpenAI itself, DeepSeek, xAI Grok,
// and Google Gemini's "openai" compat endpoint). Anthropic stays on
// its own `/v1/messages` shape with the dedicated client in `ai.rs`.
//
// Each provider has a canonical id (used for the keyring slot and
// the config field), a display label, a default model, and — for
// OpenAI-compat providers — a base URL. The active provider lives in
// `Config::active_provider`; per-provider model preferences live in
// `Config::provider_models` so changing provider doesn't blow away
// your favorite Claude model.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    Openai,
    Deepseek,
    Grok,
    Gemini,
    /// LM Studio's local OpenAI-compatible server. Unlike the hosted
    /// providers it has a user-configurable endpoint (the default
    /// localhost URL, or a LAN/Tailscale hostname — see
    /// `Config::base_url_for`) and requires no API key.
    // The canonical id is "lmstudio" (keyring slot, config keys, frontend);
    // snake_case would otherwise split this two-word variant into "lm_studio".
    #[serde(rename = "lmstudio", alias = "lm_studio")]
    LmStudio,
}

pub const ALL: &[Provider] = &[
    Provider::Anthropic,
    Provider::Openai,
    Provider::Gemini,
    Provider::Deepseek,
    Provider::Grok,
    Provider::LmStudio,
];

impl Provider {
    /// Stable lowercase id used for keyring slot + config serialization.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::Openai => "openai",
            Provider::Deepseek => "deepseek",
            Provider::Grok => "grok",
            Provider::Gemini => "gemini",
            Provider::LmStudio => "lmstudio",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::Openai => "OpenAI (GPT)",
            Provider::Deepseek => "DeepSeek",
            Provider::Grok => "xAI (Grok)",
            Provider::Gemini => "Google (Gemini)",
            Provider::LmStudio => "LM Studio (local)",
        }
    }

    /// Sensible default model per provider as of May 2026. Editable per
    /// install via Settings → AI; the value here is just the seed.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Anthropic => "claude-haiku-4-5",
            Provider::Openai => "gpt-5",
            Provider::Deepseek => "deepseek-v4-flash",
            Provider::Grok => "grok-4.3",
            Provider::Gemini => "gemini-2.5-flash",
            // Whatever the user has loaded; this seed just matches a
            // commonly-run local model. The model field must match an ID
            // LM Studio's server lists (its /v1/models endpoint).
            Provider::LmStudio => "openai/gpt-oss-20b",
        }
    }

    /// A few common model names the Settings picker surfaces as
    /// quick-pick chips. The user can type any model name into the
    /// text field — these are just suggestions.
    pub fn suggested_models(self) -> &'static [&'static str] {
        match self {
            Provider::Anthropic => &["claude-haiku-4-5", "claude-sonnet-4-6", "claude-opus-4-7"],
            Provider::Openai => &["gpt-5", "gpt-5-mini"],
            Provider::Deepseek => &["deepseek-v4-flash", "deepseek-v4-pro"],
            Provider::Grok => &["grok-4.3", "grok-4.20-multi-agent"],
            Provider::Gemini => &["gemini-2.5-flash", "gemini-2.5-pro", "gemini-2.5-flash-lite"],
            Provider::LmStudio => &["openai/gpt-oss-20b", "qwen/qwen3-8b", "meta-llama-3.1-8b-instruct"],
        }
    }

    /// Default base URL for the OpenAI-compat client. Anthropic returns
    /// None (callers should special-case to the `ai.rs` Anthropic
    /// client). This is the compile-time seed — the user can override
    /// it per provider via `Config::provider_base_urls`, which is how
    /// LM Studio gets pointed at a non-localhost (e.g. Tailscale) host.
    /// Dispatch resolves through `Config::base_url_for`, not this.
    pub fn openai_base_url(self) -> Option<&'static str> {
        match self {
            Provider::Anthropic => None,
            Provider::Openai => Some("https://api.openai.com/v1"),
            Provider::Deepseek => Some("https://api.deepseek.com/v1"),
            Provider::Grok => Some("https://api.x.ai/v1"),
            Provider::Gemini => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
            Provider::LmStudio => Some("http://localhost:1234/v1"),
        }
    }

    /// False for providers that work without an API key (local servers).
    /// Key-fetch sites fall back to an empty key instead of erroring,
    /// and the tagger's has-key gate is skipped.
    pub fn requires_key(self) -> bool {
        !matches!(self, Provider::LmStudio)
    }

    /// True for providers that go through `openai_compat::stream`.
    /// Currently informational (callers branch on `== Anthropic`), kept
    /// as the clearer predicate for future dispatch sites.
    #[allow(dead_code)]
    pub fn is_openai_compat(self) -> bool {
        self.openai_base_url().is_some()
    }

    /// Effective token cap for a call that wants `visible` tokens of
    /// output. Reasoning models served by LM Studio (gpt-oss, qwen3)
    /// spend the same budget on hidden reasoning BEFORE any visible
    /// text, so a tight cap ends the response while it's still
    /// thinking — the request succeeds with empty content. Local
    /// tokens are free, so give the local provider generous headroom;
    /// hosted providers keep the exact cap (there the cap is a cost
    /// control and their reasoning models budget separately).
    pub fn token_limit(self, visible: u32) -> u32 {
        match self {
            // 8192: Qwen-style thinkers can exceed 4096 reasoning tokens
            // on ordinary prompts, and running out surfaces as an empty
            // response. The idle timeout still bounds runaway generation.
            Provider::LmStudio => visible + 8192,
            _ => visible,
        }
    }

    /// Which token-cap field this provider's chat endpoint expects.
    /// OpenAI's current models (gpt-5 era) REJECT `max_tokens` outright;
    /// the compat forks (DeepSeek, Grok, Gemini-compat) still take it.
    pub fn token_param(self) -> crate::openai_compat::TokenParam {
        match self {
            Provider::Openai => crate::openai_compat::TokenParam::MaxCompletionTokens,
            _ => crate::openai_compat::TokenParam::MaxTokens,
        }
    }

    /// One-line hint shown beside the model picker — what kind of
    /// gotcha to expect with this provider.
    pub fn note(self) -> &'static str {
        match self {
            Provider::Anthropic => "Streaming via Anthropic's /v1/messages.",
            Provider::Openai => "Standard /v1/chat/completions. Default gpt-5; gpt-5-mini for cost.",
            Provider::Deepseek => "OpenAI-compatible. 1M context. Off-peak pricing.",
            Provider::Grok => "OpenAI-compatible. Older grok-* aliases redirect to grok-4.3.",
            Provider::Gemini => "OpenAI-compat subset — safety filters can null-out responses.",
            Provider::LmStudio => "Local server, no key needed. Endpoint takes a LAN/Tailscale host; model must match an ID loaded in LM Studio.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serde wire name must match `Provider::id()` for every variant —
    /// the frontend, keyring slots, and config keys all use `id()`, so any
    /// drift breaks command deserialization (the "unknown variant lmstudio,
    /// expected ... lm_studio" bug).
    #[test]
    fn serde_name_matches_id() {
        for &p in ALL {
            let wire = serde_json::to_string(&p).unwrap();
            assert_eq!(wire, format!("\"{}\"", p.id()));
            let back: Provider = serde_json::from_str(&format!("\"{}\"", p.id())).unwrap();
            assert_eq!(back, p);
        }
    }

    #[test]
    fn lm_studio_legacy_spelling_still_accepted() {
        let p: Provider = serde_json::from_str("\"lm_studio\"").unwrap();
        assert_eq!(p, Provider::LmStudio);
    }
}
