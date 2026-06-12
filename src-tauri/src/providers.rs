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
}

pub const ALL: &[Provider] = &[
    Provider::Anthropic,
    Provider::Openai,
    Provider::Gemini,
    Provider::Deepseek,
    Provider::Grok,
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
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic (Claude)",
            Provider::Openai => "OpenAI (GPT)",
            Provider::Deepseek => "DeepSeek",
            Provider::Grok => "xAI (Grok)",
            Provider::Gemini => "Google (Gemini)",
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
        }
    }

    /// Base URL for the OpenAI-compat client. Anthropic returns None
    /// (callers should special-case to the `ai.rs` Anthropic client).
    pub fn openai_base_url(self) -> Option<&'static str> {
        match self {
            Provider::Anthropic => None,
            Provider::Openai => Some("https://api.openai.com/v1"),
            Provider::Deepseek => Some("https://api.deepseek.com/v1"),
            Provider::Grok => Some("https://api.x.ai/v1"),
            Provider::Gemini => Some("https://generativelanguage.googleapis.com/v1beta/openai"),
        }
    }

    /// True for providers that go through `openai_compat::stream`.
    /// Currently informational (callers branch on `== Anthropic`), kept
    /// as the clearer predicate for future dispatch sites.
    #[allow(dead_code)]
    pub fn is_openai_compat(self) -> bool {
        self.openai_base_url().is_some()
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
        }
    }
}
