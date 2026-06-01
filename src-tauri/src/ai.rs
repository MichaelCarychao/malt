use crate::prompts::{self, PromptKey};
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5";

// All system prompts now live in `prompts.rs` so the user can review
// and override what malt sends. We pull them at call time via
// `prompts::get(...)` — no more compile-time constants in this file.

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
/// (fail fast on a dead endpoint) but no overall cap; the stream ends
/// when the model is done or the connection drops.
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

/// Stream an infill completion from Claude. The model sees the full document
/// with `{INSERT HERE}` at the cursor position and outputs only the text to
/// insert. `on_text` is invoked for each delta as it streams in. `direction`
/// is an optional steering note (empty = none).
pub async fn stream_completion<F>(
    api_key: &str,
    model: &str,
    before: &str,
    after: &str,
    direction: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    // Pick the mode tag the system prompt expects. The model is much more
    // reliable when the mode is explicit in the wrapper than when it has
    // to infer "before is empty, must be a fresh start" from the content.
    let mode = if before.trim().is_empty() && !after.trim().is_empty() {
        "begin"
    } else if !before.trim().is_empty() && after.trim().is_empty() {
        "continue"
    } else {
        "bridge"
    };
    let user_msg = with_direction(
        format!("<{mode}>{before}{{INSERT HERE}}{after}</{mode}>"),
        direction,
    );
    let system_prompt = prompts::get(PromptKey::Completion);
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

    let mut resp = streaming_client()
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

    let mut buffer = String::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("chunk error: {e}"))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // SSE events are separated by blank lines (\n\n).
        while let Some(end) = buffer.find("\n\n") {
            let event = buffer[..end].to_string();
            buffer.drain(..end + 2);

            for line in event.lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<StreamEvent>(data) else {
                    continue;
                };
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

/// Stream a rewrite from Claude. The model sees the full document with the
/// selected portion wrapped in `<rewrite>...</rewrite>` tags and outputs only
/// the replacement text. `on_text` is invoked for each delta.
pub async fn stream_rewrite<F>(
    api_key: &str,
    model: &str,
    before: &str,
    selected: &str,
    after: &str,
    direction: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let user_msg = with_direction(
        format!("{before}<rewrite>{selected}</rewrite>{after}"),
        direction,
    );
    let system_prompt = prompts::get(PromptKey::Rewrite);
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

    let mut resp = streaming_client()
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

    let mut buffer = String::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("chunk error: {e}"))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let event = buffer[..end].to_string();
            buffer.drain(..end + 2);
            for line in event.lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<StreamEvent>(data) else {
                    continue;
                };
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

pub async fn propose_entities(api_key: &str, body: &str) -> Result<Vec<String>, String> {
    let system_prompt = prompts::get(PromptKey::Entities);
    let req = MessagesRequest {
        model: DEFAULT_MODEL,
        max_tokens: 512,
        system: Some(&system_prompt),
        stream: None,
        messages: vec![Message {
            role: "user",
            content: body,
        }],
    };
    let reply = send(req, api_key).await?;

    let json_start = reply.find('{').ok_or("no JSON in reply")?;
    let json_end = reply.rfind('}').ok_or("no JSON in reply")? + 1;
    let json_str = &reply[json_start..json_end];

    let blob: EntitiesBlob =
        serde_json::from_str(json_str).map_err(|e| format!("entities parse error: {e}: {reply}"))?;

    Ok(blob
        .entities
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .take(10)
        .collect())
}

pub async fn propose_tags(api_key: &str, body: &str) -> Result<Vec<String>, String> {
    let system_prompt = prompts::get(PromptKey::Tag);
    let req = MessagesRequest {
        model: DEFAULT_MODEL,
        max_tokens: 256,
        system: Some(&system_prompt),
        stream: None,
        messages: vec![Message {
            role: "user",
            content: body,
        }],
    };
    let reply = send(req, api_key).await?;

    let json_start = reply.find('{').ok_or("no JSON in reply")?;
    let json_end = reply.rfind('}').ok_or("no JSON in reply")? + 1;
    let json_str = &reply[json_start..json_end];

    let blob: TagsBlob =
        serde_json::from_str(json_str).map_err(|e| format!("tag parse error: {e}: {reply}"))?;

    Ok(blob
        .tags
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
        .collect())
}


/// Stream a brainstorm ("brew") from Claude. Sends the full note body
/// with the brew system prompt; the model returns a structured
/// markdown response (threads / connections / framings). `on_text` is
/// invoked for each delta as it streams in.
pub async fn stream_brew<F>(
    api_key: &str,
    model: &str,
    body: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let system_prompt = prompts::get(PromptKey::Brew);
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

    let mut resp = streaming_client()
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
        let body_text = resp.text().await.unwrap_or_default();
        if let Ok(api_err) = serde_json::from_str::<ApiError>(&body_text) {
            return Err(format!("{} ({})", api_err.error.message, status));
        }
        return Err(format!("HTTP {}: {}", status, body_text));
    }

    let mut buffer = String::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("chunk error: {e}"))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let event = buffer[..end].to_string();
            buffer.drain(..end + 2);
            for line in event.lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<StreamEvent>(data) else {
                    continue;
                };
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

/// Raw streaming: send `prompt` as the sole user message with NO system
/// prompt and NO scaffolding. Powers two-pane prompting, where the user's
/// own notes ARE the prompt. Generous max_tokens since the prompt is a
/// full instruction, not a short continuation.
pub async fn stream_raw<F>(
    api_key: &str,
    model: &str,
    prompt: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
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

    let mut resp = streaming_client()
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
        let body_text = resp.text().await.unwrap_or_default();
        if let Ok(api_err) = serde_json::from_str::<ApiError>(&body_text) {
            return Err(format!("{} ({})", api_err.error.message, status));
        }
        return Err(format!("HTTP {}: {}", status, body_text));
    }

    let mut buffer = String::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("chunk error: {e}"))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let event = buffer[..end].to_string();
            buffer.drain(..end + 2);
            for line in event.lines() {
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<StreamEvent>(data) else {
                    continue;
                };
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

// ─── Multi-provider dispatch ────────────────────────────────────────
//
// All the functions above were originally hardwired to Anthropic.
// v0.3.2 adds OpenAI, DeepSeek, Grok, and Gemini via the shared
// OpenAI-compat client in `openai_compat.rs`. These dispatcher
// functions pick the right backend based on the provider passed in
// (which lib.rs reads from config). For Anthropic, we keep calling
// the existing functions (so all the Anthropic-specific behavior
// stays intact); for everything else, we synthesize a single user
// message that bundles the mode-tagged wrapper and send via the
// OpenAI-compat client.

use crate::openai_compat;
use crate::providers::Provider;

pub async fn dispatch_stream_completion<F>(
    provider: Provider,
    api_key: &str,
    model: &str,
    before: &str,
    after: &str,
    direction: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    if provider == Provider::Anthropic {
        return stream_completion(api_key, model, before, after, direction, on_text).await;
    }
    let mode = if before.trim().is_empty() && !after.trim().is_empty() {
        "begin"
    } else if !before.trim().is_empty() && after.trim().is_empty() {
        "continue"
    } else {
        "bridge"
    };
    let user_msg = with_direction(
        format!("<{mode}>{before}{{INSERT HERE}}{after}</{mode}>"),
        direction,
    );
    let system_prompt = prompts::get(PromptKey::Completion);
    let base_url = provider.openai_base_url().ok_or("provider lacks base URL")?;
    openai_compat::stream(
        base_url,
        api_key,
        model,
        Some(&system_prompt),
        &user_msg,
        Some(400),
        |t| on_text(t),
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
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    if provider == Provider::Anthropic {
        return stream_rewrite(api_key, model, before, selected, after, direction, on_text).await;
    }
    let user_msg = with_direction(
        format!("{before}<rewrite>{selected}</rewrite>{after}"),
        direction,
    );
    let system_prompt = prompts::get(PromptKey::Rewrite);
    let base_url = provider.openai_base_url().ok_or("provider lacks base URL")?;
    openai_compat::stream(
        base_url,
        api_key,
        model,
        Some(&system_prompt),
        &user_msg,
        Some(800),
        |t| on_text(t),
    )
    .await
}

pub async fn dispatch_stream_brew<F>(
    provider: Provider,
    api_key: &str,
    model: &str,
    body: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    if provider == Provider::Anthropic {
        return stream_brew(api_key, model, body, on_text).await;
    }
    let system_prompt = prompts::get(PromptKey::Brew);
    let base_url = provider.openai_base_url().ok_or("provider lacks base URL")?;
    openai_compat::stream(
        base_url,
        api_key,
        model,
        Some(&system_prompt),
        body,
        Some(1024),
        |t| on_text(t),
    )
    .await
}

/// Multi-provider raw prompt streaming (no system / no scaffolding).
pub async fn dispatch_stream_raw<F>(
    provider: Provider,
    api_key: &str,
    model: &str,
    prompt: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    if provider == Provider::Anthropic {
        return stream_raw(api_key, model, prompt, on_text).await;
    }
    let base_url = provider.openai_base_url().ok_or("provider lacks base URL")?;
    openai_compat::stream(
        base_url,
        api_key,
        model,
        None, // no system prompt — the notes are the whole prompt
        prompt,
        Some(2048),
        |t| on_text(t),
    )
    .await
}

pub async fn dispatch_propose_tags(
    provider: Provider,
    api_key: &str,
    body: &str,
) -> Result<Vec<String>, String> {
    if provider == Provider::Anthropic {
        return propose_tags(api_key, body).await;
    }
    let system_prompt = prompts::get(PromptKey::Tag);
    let base_url = provider.openai_base_url().ok_or("provider lacks base URL")?;
    let model = provider.default_model();
    let reply = openai_compat::send(
        base_url,
        api_key,
        model,
        Some(&system_prompt),
        body,
        Some(256),
    )
    .await?;
    let json_start = reply.find('{').ok_or("no JSON in reply")?;
    let json_end = reply.rfind('}').ok_or("no JSON in reply")? + 1;
    let blob: TagsBlob = serde_json::from_str(&reply[json_start..json_end])
        .map_err(|e| format!("tag parse error: {e}: {reply}"))?;
    Ok(blob
        .tags
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
        .collect())
}

pub async fn dispatch_propose_entities(
    provider: Provider,
    api_key: &str,
    body: &str,
) -> Result<Vec<String>, String> {
    if provider == Provider::Anthropic {
        return propose_entities(api_key, body).await;
    }
    let system_prompt = prompts::get(PromptKey::Entities);
    let base_url = provider.openai_base_url().ok_or("provider lacks base URL")?;
    let model = provider.default_model();
    let reply = openai_compat::send(
        base_url,
        api_key,
        model,
        Some(&system_prompt),
        body,
        Some(512),
    )
    .await?;
    let json_start = reply.find('{').ok_or("no JSON in reply")?;
    let json_end = reply.rfind('}').ok_or("no JSON in reply")? + 1;
    let blob: EntitiesBlob = serde_json::from_str(&reply[json_start..json_end])
        .map_err(|e| format!("entities parse error: {e}: {reply}"))?;
    Ok(blob
        .entities
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .take(10)
        .collect())
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
    let base_url = provider.openai_base_url().ok_or("provider lacks base URL")?;
    openai_compat::send(
        base_url,
        api_key,
        model,
        None,
        "Reply with the single word: malt",
        Some(32),
    )
    .await
}
