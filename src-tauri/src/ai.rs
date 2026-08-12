use crate::prompts::{self, PromptKey};
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5";

/// Abort a stream if no bytes arrive for this long. Without it, a
/// stalled-but-open connection (wifi drop mid-stream, hung proxy) left the
/// command pending forever — the ghost spinner never resolved.
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

// All system prompts live in `prompts.rs` so the user can review and
// override what malt sends. We pull them at call time via `prompts::get`.

// ─── Cooperative cancellation ───────────────────────────────────────
//
// The frontend hands each streaming command an optional `stream_id` and
// calls `cancel_ai_stream(id)` when the user dismisses the ghost /
// supersedes the request. The pumps poll `is_cancelled` between chunks
// and return early — dropping the response closes the connection, so the
// provider stops generating (and billing) within a token or two.

fn cancel_registry() -> &'static std::sync::Mutex<std::collections::HashSet<u64>> {
    static REG: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u64>>> =
        std::sync::OnceLock::new();
    REG.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

pub fn cancel_stream(id: u64) {
    cancel_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id);
}

pub(crate) fn is_cancelled(id: Option<u64>) -> bool {
    match id {
        None => false,
        Some(id) => cancel_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id),
    }
}

/// Forget a stream id once its command finishes — the registry must not
/// grow for the lifetime of the process.
pub(crate) fn clear_cancel(id: Option<u64>) {
    if let Some(id) = id {
        cancel_registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }
}

// ─── Wire types ─────────────────────────────────────────────────────

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    messages: Vec<Message<'a>>,
}

#[derive(Deserialize)]
struct StreamEvent {
    #[serde(default)]
    delta: Option<StreamDelta>,
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    error: ApiErrorBody,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
}

#[derive(Deserialize)]
struct TagsBlob {
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct EntitiesBlob {
    entities: Vec<String>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Client for STREAMING calls. `reqwest`'s `.timeout()` is a *total*
/// request deadline — applying it to a stream kills the whole request
/// at the deadline even while tokens are still flowing, truncating long
/// brews / completions mid-sentence. Streaming gets a connect timeout
/// (fail fast on a dead endpoint); per-chunk liveness is enforced by
/// IDLE_TIMEOUT in the pump instead.
fn streaming_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

async fn send(req: MessagesRequest<'_>, api_key: &str) -> Result<String, String> {
    let resp = client()
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read error: {e}"))?;

    if !status.is_success() {
        if let Ok(api_err) = serde_json::from_str::<ApiError>(&text) {
            return Err(format!("{} ({})", api_err.error.message, status));
        }
        return Err(format!("HTTP {}: {}", status, text));
    }

    let parsed: MessagesResponse =
        serde_json::from_str(&text).map_err(|e| format!("parse error: {e}"))?;
    Ok(parsed
        .content
        .into_iter()
        .filter_map(|b| b.text)
        .collect::<Vec<_>>()
        .join(""))
}

use crate::openai_compat::SseAssembler;

/// Read an Anthropic streaming response to completion, invoking `on_text`
/// per delta. One pump for all four generation paths (completion, rewrite,
/// brew, raw) — they used to be four near-identical copies, each with the
/// same three latent bugs: per-chunk lossy UTF-8 decoding that turned a
/// multibyte token split across TCP chunks into U+FFFD; no idle timeout
/// (a stalled connection hung forever); and `error` SSE events silently
/// swallowed, so an overloaded upstream looked like a clean, short
/// completion.
async fn pump_anthropic_sse<F>(
    mut resp: reqwest::Response,
    stream_id: Option<u64>,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let mut asm = SseAssembler::new();
    loop {
        if is_cancelled(stream_id) {
            // Dropping `resp` closes the connection — generation stops
            // upstream. The user moved on; this is a clean exit.
            return Ok(());
        }
        let chunk = match tokio::time::timeout(IDLE_TIMEOUT, resp.chunk()).await {
            Ok(Ok(Some(c))) => c,
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(format!("chunk error: {e}")),
            Err(_) => {
                return Err(format!(
                    "stream stalled (no data for {}s)",
                    IDLE_TIMEOUT.as_secs()
                ))
            }
        };
        for event in asm.push(&chunk) {
            for line in event.lines() {
                let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<StreamEvent>(data) else {
                    continue;
                };
                // Mid-stream error event (overloaded_error etc.): surface it
                // instead of letting the stream end "successfully" short.
                if let Some(err) = parsed.error {
                    return Err(err.message);
                }
                if let Some(delta) = parsed.delta {
                    if let Some(text) = delta.text {
                        on_text(&text);
                    }
                }
            }
        }
    }
    Ok(())
}

/// POST a streaming Anthropic request and pump it. Shared error handling
/// for the non-2xx case.
async fn stream_anthropic<F>(
    api_key: &str,
    req: MessagesRequest<'_>,
    stream_id: Option<u64>,
    on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let resp = streaming_client()
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if let Ok(api_err) = serde_json::from_str::<ApiError>(&body) {
            return Err(format!("{} ({})", api_err.error.message, status));
        }
        return Err(format!("HTTP {}: {}", status, body));
    }
    pump_anthropic_sse(resp, stream_id, on_text).await
}

pub async fn test_call(api_key: &str) -> Result<String, String> {
    let req = MessagesRequest {
        model: DEFAULT_MODEL,
        max_tokens: 32,
        system: None,
        stream: None,
        messages: vec![Message {
            role: "user",
            content: "Reply with the single word: malt",
        }],
    };
    send(req, api_key).await
}

/// Append an optional steering note to a user message as a
/// `<direction>...</direction>` block. No-op when empty. Shared by every
/// generation path so steering works identically across providers.
fn with_direction(user_msg: String, direction: &str) -> String {
    let d = direction.trim();
    if d.is_empty() {
        user_msg
    } else {
        format!("{user_msg}\n\n<direction>{d}</direction>")
    }
}

/// Pick the mode tag the completion system prompt expects. The model is
/// much more reliable when the mode is explicit in the wrapper than when
/// it has to infer "before is empty, must be a fresh start".
fn completion_mode(before: &str, after: &str) -> &'static str {
    if before.trim().is_empty() && !after.trim().is_empty() {
        "begin"
    } else if !before.trim().is_empty() && after.trim().is_empty() {
        "continue"
    } else {
        "bridge"
    }
}

// ─── Multi-provider dispatch ────────────────────────────────────────
//
// Anthropic keeps its dedicated /v1/messages client (above); OpenAI,
// DeepSeek, Grok, and Gemini go through the shared OpenAI-compat client
// in `openai_compat.rs`. The dispatchers build the identical mode-tagged
// wrapper for both paths.

use crate::openai_compat;
use crate::providers::Provider;

/// Resolve the compat-client base URL: the user's config override wins
/// over the provider's compile-time default. This is how LM Studio gets
/// pointed at a non-localhost (e.g. Tailscale) endpoint. A per-call
/// config read, like `prompts::get` above — both are tiny JSON files.
fn compat_base_url(provider: Provider) -> Result<String, String> {
    crate::config::load()
        .base_url_for(provider)
        .ok_or_else(|| "provider lacks base URL".to_string())
}

/// Qwen's "answer directly" soft switch, appended to LM Studio prompts
/// when "skip thinking" is on in Settings → AI. Best-effort: hybrid
/// reasoning models honor `/no_think`; models without the convention
/// (e.g. gpt-oss, whose effort is set in LM Studio itself) see a few
/// harmless extra characters. Empty for every other provider, so hosted
/// prompts are untouched. Called only on compat paths (after the
/// Anthropic early-return), keeping the config read off Anthropic calls.
fn no_think_suffix(provider: Provider) -> &'static str {
    if provider == Provider::LmStudio && crate::config::load().lmstudio_no_think {
        "\n\n/no_think"
    } else {
        ""
    }
}

pub async fn dispatch_stream_completion<F>(
    provider: Provider,
    api_key: &str,
    model: &str,
    before: &str,
    after: &str,
    direction: &str,
    stream_id: Option<u64>,
    on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let mode = completion_mode(before, after);
    let user_msg = with_direction(
        format!("<{mode}>{before}{{INSERT HERE}}{after}</{mode}>"),
        direction,
    );
    let system_prompt = prompts::get(PromptKey::Completion);
    if provider == Provider::Anthropic {
        let req = MessagesRequest {
            model,
            max_tokens: 400,
            system: Some(&system_prompt),
            stream: Some(true),
            messages: vec![Message {
                role: "user",
                content: &user_msg,
            }],
        };
        return stream_anthropic(api_key, req, stream_id, on_text).await;
    }
    let base_url = compat_base_url(provider)?;
    let system_prompt = format!("{system_prompt}{}", no_think_suffix(provider));
    openai_compat::stream(
        &base_url,
        api_key,
        model,
        Some(&system_prompt),
        &user_msg,
        Some(provider.token_limit(400)),
        provider.token_param(),
        stream_id,
        on_text,
    )
    .await
}

pub async fn dispatch_stream_rewrite<F>(
    provider: Provider,
    api_key: &str,
    model: &str,
    before: &str,
    selected: &str,
    after: &str,
    direction: &str,
    stream_id: Option<u64>,
    on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let user_msg = with_direction(
        format!("{before}<rewrite>{selected}</rewrite>{after}"),
        direction,
    );
    let system_prompt = prompts::get(PromptKey::Rewrite);
    if provider == Provider::Anthropic {
        let req = MessagesRequest {
            model,
            max_tokens: 800,
            system: Some(&system_prompt),
            stream: Some(true),
            messages: vec![Message {
                role: "user",
                content: &user_msg,
            }],
        };
        return stream_anthropic(api_key, req, stream_id, on_text).await;
    }
    let base_url = compat_base_url(provider)?;
    let system_prompt = format!("{system_prompt}{}", no_think_suffix(provider));
    openai_compat::stream(
        &base_url,
        api_key,
        model,
        Some(&system_prompt),
        &user_msg,
        Some(provider.token_limit(800)),
        provider.token_param(),
        stream_id,
        on_text,
    )
    .await
}

pub async fn dispatch_stream_brew<F>(
    provider: Provider,
    api_key: &str,
    model: &str,
    body: &str,
    stream_id: Option<u64>,
    on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let system_prompt = prompts::get(PromptKey::Brew);
    if provider == Provider::Anthropic {
        let req = MessagesRequest {
            model,
            max_tokens: 1024,
            system: Some(&system_prompt),
            stream: Some(true),
            messages: vec![Message {
                role: "user",
                content: body,
            }],
        };
        return stream_anthropic(api_key, req, stream_id, on_text).await;
    }
    let base_url = compat_base_url(provider)?;
    let system_prompt = format!("{system_prompt}{}", no_think_suffix(provider));
    openai_compat::stream(
        &base_url,
        api_key,
        model,
        Some(&system_prompt),
        body,
        Some(provider.token_limit(1024)),
        provider.token_param(),
        stream_id,
        on_text,
    )
    .await
}

/// Raw streaming: send `prompt` as the sole user message with NO system
/// prompt and NO scaffolding. Powers two-pane prompting, where the user's
/// own notes ARE the prompt. Generous max_tokens since the prompt is a
/// full instruction, not a short continuation.
pub async fn dispatch_stream_raw<F>(
    provider: Provider,
    api_key: &str,
    model: &str,
    prompt: &str,
    stream_id: Option<u64>,
    on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    if provider == Provider::Anthropic {
        let req = MessagesRequest {
            model,
            max_tokens: 2048,
            system: None,
            stream: Some(true),
            messages: vec![Message {
                role: "user",
                content: prompt,
            }],
        };
        return stream_anthropic(api_key, req, stream_id, on_text).await;
    }
    let base_url = compat_base_url(provider)?;
    let prompt = format!("{prompt}{}", no_think_suffix(provider));
    openai_compat::stream(
        &base_url,
        api_key,
        model,
        None, // no system prompt — the notes are the whole prompt
        &prompt,
        Some(provider.token_limit(2048)),
        provider.token_param(),
        stream_id,
        on_text,
    )
    .await
}

/// Extract the first {...} JSON object from a model reply (some models add
/// prose or fences around the JSON despite instructions).
fn json_slice(reply: &str) -> Result<&str, String> {
    let start = reply.find('{').ok_or("no JSON in reply")?;
    let end = reply.rfind('}').ok_or("no JSON in reply")? + 1;
    Ok(&reply[start..end])
}

fn sanitize_tags(blob: TagsBlob) -> Vec<String> {
    blob.tags
        .into_iter()
        .map(|t| {
            t.trim()
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .take(5)
        .collect()
}

fn sanitize_entities(blob: EntitiesBlob) -> Vec<String> {
    blob.entities
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .take(10)
        .collect()
}

/// Propose tags via the given provider + model. `model` comes from the
/// caller's config (per-provider override) — these one-shot paths used to
/// hardcode each provider's default model, silently ignoring the model the
/// user picked in Settings.
pub async fn dispatch_propose_tags(
    provider: Provider,
    api_key: &str,
    model: &str,
    body: &str,
) -> Result<Vec<String>, String> {
    let system_prompt = prompts::get(PromptKey::Tag);
    let reply = if provider == Provider::Anthropic {
        let req = MessagesRequest {
            model,
            max_tokens: 256,
            system: Some(&system_prompt),
            stream: None,
            messages: vec![Message {
                role: "user",
                content: body,
            }],
        };
        send(req, api_key).await?
    } else {
        let base_url = compat_base_url(provider)?;
        let system_prompt = format!("{system_prompt}{}", no_think_suffix(provider));
        openai_compat::send(
            &base_url,
            api_key,
            model,
            Some(&system_prompt),
            body,
            Some(provider.token_limit(256)),
            provider.token_param(),
        )
        .await?
    };
    let blob: TagsBlob = serde_json::from_str(json_slice(&reply)?)
        .map_err(|e| format!("tag parse error: {e}: {reply}"))?;
    Ok(sanitize_tags(blob))
}

pub async fn dispatch_propose_entities(
    provider: Provider,
    api_key: &str,
    model: &str,
    body: &str,
) -> Result<Vec<String>, String> {
    let system_prompt = prompts::get(PromptKey::Entities);
    let reply = if provider == Provider::Anthropic {
        let req = MessagesRequest {
            model,
            max_tokens: 512,
            system: Some(&system_prompt),
            stream: None,
            messages: vec![Message {
                role: "user",
                content: body,
            }],
        };
        send(req, api_key).await?
    } else {
        let base_url = compat_base_url(provider)?;
        let system_prompt = format!("{system_prompt}{}", no_think_suffix(provider));
        openai_compat::send(
            &base_url,
            api_key,
            model,
            Some(&system_prompt),
            body,
            Some(provider.token_limit(512)),
            provider.token_param(),
        )
        .await?
    };
    let blob: EntitiesBlob = serde_json::from_str(json_slice(&reply)?)
        .map_err(|e| format!("entities parse error: {e}: {reply}"))?;
    Ok(sanitize_entities(blob))
}

/// Connectivity test for any provider — used by the "test" button in
/// Settings → AI. One-shot, returns "ok" payload trimmed.
pub async fn dispatch_test(
    provider: Provider,
    api_key: &str,
    model: &str,
) -> Result<String, String> {
    if provider == Provider::Anthropic {
        return test_call(api_key).await;
    }
    let base_url = compat_base_url(provider)?;
    let user_msg = format!(
        "Reply with the single word: malt{}",
        no_think_suffix(provider)
    );
    openai_compat::send(
        &base_url,
        api_key,
        model,
        None,
        &user_msg,
        Some(provider.token_limit(32)),
        provider.token_param(),
    )
    .await
    .and_then(|reply| {
        // A 200 with empty content means the token cap ran out before any
        // visible text (reasoning models) or the server returned nothing —
        // either way "ok" would be a lie the user pays for later.
        if reply.trim().is_empty() {
            Err("connected, but the model returned empty content — \
                 if this is a reasoning model, it may need a larger token budget"
                .to_string())
        } else {
            Ok(reply)
        }
    })
}
