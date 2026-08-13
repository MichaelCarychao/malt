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

/// Abort a stream if no bytes arrive for this long (mirrors ai.rs).
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Incremental SSE byte assembler shared by the Anthropic and compat
/// pumps. Three concerns in one place:
///   - UTF-8: a multibyte char can split across TCP chunks. We buffer raw
///     bytes and decode only complete sequences — the old per-chunk
///     `from_utf8_lossy` emitted U+FFFD into ghost text whenever a token
///     straddled a chunk boundary. A genuinely invalid byte (corrupt
///     stream) is replaced and skipped so the assembler can't wedge.
///   - CRLF: some gateways frame events as `\r\n\r\n`, which a naive
///     find("\n\n") never matches; normalized after each append.
///   - Framing: yields complete events (blank-line separated), keeping a
///     partial trailing event buffered for the next chunk.
pub(crate) struct SseAssembler {
    pending: Vec<u8>,
    buffer: String,
}

impl SseAssembler {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::new(),
            buffer: String::new(),
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    self.buffer.push_str(s);
                    self.pending.clear();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if let Ok(s) = std::str::from_utf8(&self.pending[..valid]) {
                        self.buffer.push_str(s);
                    }
                    match e.error_len() {
                        // Truly invalid bytes: substitute + skip, keep going.
                        Some(bad) => {
                            self.buffer.push('\u{FFFD}');
                            self.pending.drain(..valid + bad);
                        }
                        // Incomplete trailing sequence: keep it for the
                        // next chunk.
                        None => {
                            self.pending.drain(..valid);
                            break;
                        }
                    }
                }
            }
        }
        if self.buffer.contains('\r') {
            self.buffer = self.buffer.replace("\r\n", "\n");
        }
        let mut events = Vec::new();
        while let Some(end) = self.buffer.find("\n\n") {
            events.push(self.buffer[..end].to_string());
            self.buffer.drain(..end + 2);
        }
        events
    }
}

/// Strips a leading `<think>…</think>` reasoning block from a response.
///
/// Qwen-style models emit their reasoning inline at the start of the
/// completion. LM Studio normally parses it out server-side into a
/// separate `reasoning_content` field, but only when the model's
/// "reasoning section parsing" setting is on — this is the client-side
/// fallback so hidden reasoning never lands in a note. Streaming-safe:
/// the tags themselves can arrive split across deltas.
pub(crate) struct ThinkFilter {
    state: ThinkState,
    buf: String,
}

enum ThinkState {
    /// Deciding whether the stream opens with `<think>` (leading
    /// whitespace tolerated). Buffers until the prefix match resolves.
    Start,
    /// Inside the block, watching for `</think>` — keeps only a small
    /// tail so a tag split across deltas still matches.
    Suppress,
    /// Block closed; swallowing the whitespace the model emits between
    /// `</think>` and the real content.
    TrimAfterClose,
    /// Clean passthrough.
    Pass,
}

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

impl ThinkFilter {
    pub(crate) fn new() -> Self {
        Self {
            state: ThinkState::Start,
            buf: String::new(),
        }
    }

    /// Feed one delta; returns the text that should become visible.
    pub(crate) fn push(&mut self, text: &str) -> String {
        match self.state {
            ThinkState::Pass => text.to_string(),
            ThinkState::TrimAfterClose => {
                let trimmed = text.trim_start();
                if trimmed.is_empty() {
                    return String::new();
                }
                self.state = ThinkState::Pass;
                trimmed.to_string()
            }
            ThinkState::Start => {
                self.buf.push_str(text);
                let trimmed = self.buf.trim_start();
                if trimmed.is_empty() {
                    return String::new();
                }
                if let Some(rest) = trimmed.strip_prefix(THINK_OPEN) {
                    let rest = rest.to_string();
                    self.buf.clear();
                    self.state = ThinkState::Suppress;
                    return self.push(&rest);
                }
                if trimmed.len() < THINK_OPEN.len() && THINK_OPEN.starts_with(trimmed) {
                    // Could still become the opening tag — keep buffering.
                    return String::new();
                }
                // Not a reasoning block: everything buffered is content.
                self.state = ThinkState::Pass;
                std::mem::take(&mut self.buf)
            }
            ThinkState::Suppress => {
                self.buf.push_str(text);
                if let Some(idx) = self.buf.find(THINK_CLOSE) {
                    let after = self.buf[idx + THINK_CLOSE.len()..].to_string();
                    self.buf.clear();
                    self.state = ThinkState::TrimAfterClose;
                    return self.push(&after);
                }
                // Keep just enough tail to catch a closing tag that
                // straddles the next delta; drop the rest (it's reasoning).
                let keep = THINK_CLOSE.len() - 1;
                if self.buf.len() > keep {
                    let mut cut = self.buf.len() - keep;
                    while !self.buf.is_char_boundary(cut) {
                        cut += 1;
                    }
                    self.buf.drain(..cut);
                }
                String::new()
            }
        }
    }

    /// Flush at end of stream. Text still buffered in `Start` was a
    /// false-positive prefix (e.g. a reply of just "<t") — it's content.
    /// Anything buffered in `Suppress` is unterminated reasoning (the
    /// token cap ran out mid-think) and stays hidden.
    pub(crate) fn finish(&mut self) -> String {
        match self.state {
            ThinkState::Start => std::mem::take(&mut self.buf),
            _ => String::new(),
        }
    }
}

/// One-shot variant for non-streaming responses.
fn strip_think(text: &str) -> String {
    let mut f = ThinkFilter::new();
    let mut out = f.push(text);
    out.push_str(&f.finish());
    out
}

#[cfg(test)]
mod think_tests {
    use super::*;

    /// Run deltas through the filter the way the stream pump does.
    fn run(deltas: &[&str]) -> String {
        let mut f = ThinkFilter::new();
        let mut out = String::new();
        for d in deltas {
            out.push_str(&f.push(d));
        }
        out.push_str(&f.finish());
        out
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(run(&["Hello", " world"]), "Hello world");
    }

    #[test]
    fn strips_think_block_in_one_chunk() {
        assert_eq!(
            run(&["<think>hidden reasoning</think>\n\nvisible"]),
            "visible"
        );
    }

    #[test]
    fn strips_think_block_split_across_deltas() {
        // Both tags straddle delta boundaries, as SSE tokens do.
        assert_eq!(
            run(&["<th", "ink>rea", "soning</th", "ink>\n", "\nanswer"]),
            "answer"
        );
    }

    #[test]
    fn leading_whitespace_before_tag_tolerated() {
        assert_eq!(run(&["\n <think>x</think> ok"]), "ok");
    }

    #[test]
    fn false_positive_prefix_flushes_at_finish() {
        // A reply that IS just "<t" — never resolves to the tag.
        assert_eq!(run(&["<t"]), "<t");
    }

    #[test]
    fn angle_bracket_content_not_swallowed() {
        assert_eq!(run(&["<3 you"]), "<3 you");
    }

    #[test]
    fn unterminated_reasoning_stays_hidden() {
        // Token cap ran out mid-think: nothing visible is correct —
        // the caller surfaces the empty reply as an error.
        assert_eq!(run(&["<think>still thi", "nking"]), "");
    }

    #[test]
    fn multibyte_reasoning_does_not_panic() {
        assert_eq!(run(&["<think>思考中🤔", "</think>done"]), "done");
    }

    #[test]
    fn later_think_tags_are_content() {
        // Only a LEADING block is reasoning; tags mid-text pass through.
        assert_eq!(
            run(&["The tag <think> is used by Qwen"]),
            "The tag <think> is used by Qwen"
        );
    }

    #[test]
    fn chat_template_kwargs_only_serialized_when_set() {
        let req = ChatCompletionRequest {
            model: "m",
            messages: vec![],
            max_tokens: Some(5),
            max_completion_tokens: None,
            stream: None,
            chat_template_kwargs: Some(TemplateKwargs { enable_thinking: false }),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""chat_template_kwargs":{"enable_thinking":false}"#));

        let req = ChatCompletionRequest {
            model: "m",
            messages: vec![],
            max_tokens: Some(5),
            max_completion_tokens: None,
            stream: None,
            chat_template_kwargs: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("chat_template_kwargs"));
    }

    #[test]
    fn one_shot_strip_think() {
        assert_eq!(strip_think("<think>x</think>\nanswer"), "answer");
        assert_eq!(strip_think("plain"), "plain");
        assert_eq!(strip_think("<think>never closed"), "");
    }
}

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
    /// Server-side "answer directly" switch for Qwen-style hybrids:
    /// LM Studio forwards this into the model's chat template, where
    /// `enable_thinking = false` renders the no-think prompt form.
    /// Only sent when the user turns on "skip thinking" for LM Studio;
    /// servers that don't know the field ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_template_kwargs: Option<TemplateKwargs>,
}

#[derive(Serialize)]
struct TemplateKwargs {
    enable_thinking: bool,
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
    disable_thinking: bool,
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
        chat_template_kwargs: disable_thinking.then_some(TemplateKwargs {
            enable_thinking: false,
        }),
    };
    let mut builder = client()
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&req);
    // Keyless providers (LM Studio) get no Authorization header at all —
    // a "Bearer " with nothing after it trips strict proxies.
    if !api_key.is_empty() {
        builder = builder.header("Authorization", format!("Bearer {api_key}"));
    }
    let resp = builder
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
    Ok(strip_think(
        &parsed
            .choices
            .into_iter()
            .filter_map(|c| c.message.content)
            .collect::<Vec<_>>()
            .join(""),
    ))
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
    disable_thinking: bool,
    stream_id: Option<u64>,
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
        chat_template_kwargs: disable_thinking.then_some(TemplateKwargs {
            enable_thinking: false,
        }),
    };

    let mut builder = streaming_client()
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&req);
    if !api_key.is_empty() {
        builder = builder.header("Authorization", format!("Bearer {api_key}"));
    }
    let mut resp = builder
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

    // UTF-8 reassembly, CRLF normalization, and event framing live in
    // SseAssembler (shared with the Anthropic pump). Idle timeout and
    // cooperative cancellation mirror ai.rs.
    let mut asm = SseAssembler::new();
    let mut think = ThinkFilter::new();
    // A stream that ends cleanly having produced zero visible text is a
    // failure the user would otherwise never see (dots, then nothing):
    // typically a reasoning model that spent the whole token budget
    // thinking. Cancelled streams are exempt — ending early is the point.
    let mut emitted = false;
    loop {
        if crate::ai::is_cancelled(stream_id) {
            return Ok(());
        }
        let chunk = match tokio::time::timeout(IDLE_TIMEOUT, resp.chunk()).await {
            Ok(Ok(Some(c))) => c,
            // Stream ended without [DONE] (some compat servers just close):
            // flush any content still buffered in the think filter.
            Ok(Ok(None)) => {
                let tail = think.finish();
                if !tail.is_empty() {
                    emitted = true;
                    on_text(&tail);
                }
                break;
            }
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
                // Accept both "data: {...}" (OpenAI) and "data:{...}"
                // (some compat forks omit the space).
                let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                else {
                    continue;
                };
                if data.trim() == "[DONE]" {
                    let tail = think.finish();
                    if !tail.is_empty() {
                        emitted = true;
                        on_text(&tail);
                    }
                    if !emitted {
                        return Err(EMPTY_STREAM_MSG.to_string());
                    }
                    return Ok(());
                }
                // Mid-stream error envelope: surface it rather than ending
                // the stream as a deceptively clean short completion.
                if let Ok(err) = serde_json::from_str::<ApiError>(data) {
                    return Err(err.error.message);
                }
                let Ok(parsed) = serde_json::from_str::<StreamChunk>(data) else {
                    continue;
                };
                for choice in parsed.choices {
                    if let Some(delta) = choice.delta {
                        if let Some(text) = delta.content {
                            let visible = think.push(&text);
                            if !visible.is_empty() {
                                emitted = true;
                                on_text(&visible);
                            }
                        }
                    }
                }
            }
        }
    }

    if !emitted {
        return Err(EMPTY_STREAM_MSG.to_string());
    }
    Ok(())
}

const EMPTY_STREAM_MSG: &str = "the model finished without producing any visible text — \
     a reasoning model likely spent its whole token budget thinking. \
     Try 'skip thinking' in Settings → AI, or raise the model's context \
     length in LM Studio.";
