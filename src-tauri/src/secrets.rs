use keyring::Entry;

// Each provider has its own keyring entry. The original `anthropic-api-key`
// user string is preserved so the legacy single-provider install
// migrates without forcing users to re-enter their Claude key.

const SERVICE: &str = "malt";

/// User identifier for the keyring entry, per provider.
fn user_for(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "anthropic-api-key",
        "openai" => "openai-api-key",
        "deepseek" => "deepseek-api-key",
        "grok" => "grok-api-key",
        "gemini" => "gemini-api-key",
        // Optional for LM Studio (local server) — only used if the user
        // fronts their endpoint with an auth proxy.
        "lmstudio" => "lmstudio-api-key",
        // Unknown providers fall back to a namespaced slot rather than
        // silently aliasing onto Anthropic's key.
        _ => "unknown-api-key",
    }
}

fn entry_for(provider: &str) -> keyring::Result<Entry> {
    Entry::new(SERVICE, user_for(provider))
}

pub fn set_api_key_for(provider: &str, key: &str) -> keyring::Result<()> {
    entry_for(provider)?.set_password(key)
}

pub fn get_api_key_for(provider: &str) -> keyring::Result<String> {
    entry_for(provider)?.get_password()
}

pub fn clear_api_key_for(provider: &str) -> keyring::Result<()> {
    let e = entry_for(provider)?;
    match e.delete_password() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn has_api_key_for(provider: &str) -> bool {
    matches!(get_api_key_for(provider), Ok(s) if !s.is_empty())
}

// ── Backwards-compatible single-key shims ──────────────────────────
//
// Existing callers (tagger.rs background worker, embeddings, etc.)
// were written when malt only spoke Anthropic. They call get_api_key()
// expecting a Claude key. We keep that shape; the multi-provider
// path goes through `get_api_key_for(provider)`.

pub fn set_api_key(key: &str) -> keyring::Result<()> {
    set_api_key_for("anthropic", key)
}

pub fn get_api_key() -> keyring::Result<String> {
    get_api_key_for("anthropic")
}

pub fn clear_api_key() -> keyring::Result<()> {
    clear_api_key_for("anthropic")
}

pub fn has_api_key() -> bool {
    has_api_key_for("anthropic")
}
