// OpenAI-compatible Chat Completions client.
//
// Used for OpenAI, DeepSeek, xAI Grok, and Google Gemini's OpenAI-compat
// endpoint. All four expose the same wire format — `POST <base>/chat/completions`
// with an `Authorization: Bearer <key>` header, returning standard SSE
// `data: {json}\n\n` lines terminated by `data: [DONE]`. The only
// differences are base URL, model name, and minor capability deltas
// (which we don't exercise here).
//
// Anthropic stays in `ai.rs` — its event stream uses different field
// names and its messages API takes a top-level `system` parameter
// instead of a "system" role message.

use serde::{Deserialize, Serialize};

const ONESHOT_TIMEOUT_SECS: u64 = 30;

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    /// Legacy token cap — what DeepSeek/Grok/Gemini-compat expect.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// OpenAI's replacement: gpt-5-era and o-series models REJECT
    /// `max_tokens` with a 400, so OpenAI calls must use this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Which token-cap field a provider's endpoint expects. Exactly one is
/// ever sent.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TokenParam {
    MaxTokens,
    MaxCompletionTokens,
}

fn split_limit(limit: Option<u32>, param: TokenParam) -> (Option<u32>, Option<u32>) {
    match param {
        TokenParam::MaxTokens => (limit, None),
        TokenParam::MaxCompletionTokens => (None, limit),
    }
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

// Non-streaming response shape — only need the content.
#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<NonStreamChoice>,
}
#[derive(Deserialize)]
struct NonStreamChoice {
    message: NonStreamMessage,
}
#[derive(Deserialize)]
struct NonStreamMessage {
    #[serde(default)]
    content: Option<String>,
}

// Streaming chunk shape — deltas land in `choices[].delta.content`.
#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}
#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: Option<StreamDelta>,
}
#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

// Error envelope. Both OpenAI and the compat-mode forks use this shape.
#[derive(Deserialize)]
struct ApiError {
    error: ApiErrorBody,
}
#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
}

/// Bounded client for one-shot calls (test, tags, entities). A total
/// request deadline is correct here — these should finish fast.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(ONESHOT_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Client for streaming. No total deadline — `.timeout()` would abort
/// the whole request mid-stream and truncate a long brew. Connect
/// timeout only, so a dead endpoint still fails fast.
fn streaming_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// One-shot, non-streaming chat completion. Used for the connectivity
/// test that fires when the user clicks "test" in Settings → AI.
pub async fn send(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: Option<&str>,
    user: &str,
    limit: Option<u32>,
    token_param: TokenParam,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut messages = Vec::with_capacity(2);
    if let Some(s) = system {
        messages.push(ChatMessage { role: "system", content: s });
    }
    messages.push(ChatMessage { role: "user", content: user });
    let (max_tokens, max_completion_tokens) = split_limit(limit, token_param);
    let req = ChatCompletionRequest {
        model,
        messages,
        max_tokens,
        max_completion_tokens,
        stream: None,
    };
    let resp = client()
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("network error: {e}"))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("read error: {e}"))?;
    if !status.is_success() {
        if let Ok(api_err) = serde_json::from_str::<ApiError>(&body) {
            return Err(format!("{} ({})", api_err.error.message, status));
        }
        return Err(format!("HTTP {}: {}", status, body));
    }
    let parsed: ChatCompletionResponse =
        serde_json::from_str(&body).map_err(|e| format!("parse error: {e}: {body}"))?;
    Ok(parsed
        .choices
        .into_iter()
        .filter_map(|c| c.message.content)
        .collect::<Vec<_>>()
        .join(""))
}

/// Streaming chat completion. Invokes `on_text` with each delta as it
/// arrives. Returns when the stream completes or errors.
pub async fn stream<F>(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: Option<&str>,
    user: &str,
    limit: Option<u32>,
    token_param: TokenParam,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut messages = Vec::with_capacity(2);
    if let Some(s) = system {
        messages.push(ChatMessage { role: "system", content: s });
    }
    messages.push(ChatMessage { role: "user", content: user });
    let (max_tokens, max_completion_tokens) = split_limit(limit, token_param);
    let req = ChatCompletionRequest {
        model,
        messages,
        max_tokens,
        max_completion_tokens,
        stream: Some(true),
    };

    let mut resp = streaming_client()
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
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
        // Normalize CRLF → LF up front. Some gateways (and certain
        // proxies in front of these APIs) frame SSE events with
        // `\r\n\r\n`, which `find("\n\n")` would never match — the
        // buffer would grow forever and emit nothing. SSE is line-
        // oriented so dropping the carriage returns is safe.
        buffer.push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n"));
        // SSE events separated by blank lines.
        while let Some(end) = buffer.find("\n\n") {
            let event = buffer[..end].to_string();
            buffer.drain(..end + 2);
            for line in event.lines() {
                // Accept both "data: {...}" (OpenAI) and "data:{...}"
                // (some compat forks omit the space).
                let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    return Ok(());
                }
                let Ok(parsed) = serde_json::from_str::<StreamChunk>(data) else {
                    continue;
                };
                for choice in parsed.choices {
                    if let Some(delta) = choice.delta {
                        if let Some(text) = delta.content {
                            on_text(&text);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
