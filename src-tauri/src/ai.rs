use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5";

const TAG_SYSTEM: &str = "You are a tagger for a personal note-taking system. Read the note and propose 1 to 5 tags that capture what the note is *about* — its topics and entities, not its style, length, or format. Use lowercase. Use kebab-case for multi-word tags. Output JSON only, no prose, no markdown fences, no commentary: {\"tags\":[\"tag-one\",\"tag-two\"]}";

const ENTITIES_SYSTEM: &str = "You are extracting linkable entities from a personal note. The user maintains a Zettelkasten-style knowledge base where any named entity that's meaningful enough to revisit gets its own note.\n\nRead the note and identify distinct entities that warrant their own page: specific people, named places, specific projects, books or articles by title, organizations, named events, products, and discrete concepts the user is clearly working with (not generic abstractions).\n\nRULES:\n- Skip generic nouns and abstract categories (\"the project\", \"my book\", \"people\", \"work\", \"breakfast\"). Only proper nouns or clearly-discrete concepts.\n- Skip passing references with no narrative weight (\"I had coffee\" — coffee isn't an entity here).\n- Skip pronouns and unspecified references (\"my friend\", \"the author\", \"someone\").\n- Skip entities that look like part of the user's own taxonomy/structure (date headers, status words, generic verbs).\n- Use the entity's own preferred form / title casing if obvious (e.g., \"Atomic Habits\" not \"atomic habits\").\n- Max 10 entities. Bias toward precision over recall — if you're unsure whether something rates a note, leave it out.\n\nOutput JSON only, no prose, no markdown fences: {\"entities\":[\"Entity One\",\"Entity Two\"]}";

const COMPLETION_SYSTEM: &str = "You are a writing assistant inside a personal note-taking app. The user sends their note with their cursor position marked literally as {INSERT HERE}.\n\nYour job: write text at the marker so that BEFORE flows into AFTER as if both were written in one pass.\n\nCRITICAL FIRST STEP — before writing a single word of the bridge, do this internally:\n\nScan the AFTER text and identify every concrete element that does NOT appear in the BEFORE text. Objects, characters, gestures, sensory details (a sound, a smell, a color, a temperature), emotional shifts, places, weather, light. Each one is an ORPHAN — it shows up later with no prior setup. Each orphan is a Chekhov's gun: if the after-text contains it, the bridge MUST plant it, or the reader will hit it as a non-sequitur.\n\nExample: the BEFORE shows a woman boarding a train. The AFTER says \"the red balloon floated away.\" The balloon is an orphan. Your bridge has to put a balloon somewhere — a child clutching one across the aisle, a balloon vendor on the platform, a stray balloon she sees through the window. Pick the option that fits the voice. Without the balloon in your bridge, the after-text reads as random.\n\nNow write the bridge. It must do BOTH:\n1. Plant every orphan you identified. (This is the part most assistants miss. Don't.)\n2. Connect the voice, tense, register, rhythm, and topic of the two halves so the seam disappears.\n\nMatch surrounding voice, tense, register, vocabulary, punctuation density, and sentence length. Aim for 1 to 3 sentences, or a short paragraph if the rhythm calls for it.\n\nOutput ONLY the bridge text. No preamble, no commentary, no markdown fences, no quotation marks, no narration about what you're doing. Do not repeat words or phrases from the immediate edges — the user joins your output verbatim, so duplication reads as a stutter.\n\nSpecial cases: if the marker is at the very end of the document (AFTER is empty), produce a natural continuation of the BEFORE. If the marker is inside a word, mid-bracket, inside a code fence, or anywhere insertion would corrupt structure, output nothing. If BEFORE and AFTER already connect cleanly with no orphans and no setup needed, output nothing.";

const REWRITE_SYSTEM: &str = "You are a writing assistant inside a personal note-taking app. The user will send their full note in the user message. Inside the note, a portion is wrapped in <rewrite>...</rewrite> tags — that is the text the user wants you to rewrite. Use the rest of the note as context for voice, topic, and style. Rewrite the marked text by unpacking details and avoiding generalities and clichés. Make it more specific, more concrete, more grounded in particulars and observable detail. Match the surrounding voice, tense, register, and formatting. Length should be roughly comparable to the original — don't pad and don't compress unless the original was bloated. Output ONLY the rewritten text. No preamble, no commentary, no markdown fences, no quotation marks, and no <rewrite> tags. If the marked text is a single word or a generic connector that can't be meaningfully improved, output it unchanged.";

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

/// Stream an infill completion from Claude. The model sees the full document
/// with `{INSERT HERE}` at the cursor position and outputs only the text to
/// insert. `on_text` is invoked for each delta as it streams in.
pub async fn stream_completion<F>(
    api_key: &str,
    model: &str,
    before: &str,
    after: &str,
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let user_msg = format!("{before}{{INSERT HERE}}{after}");
    let req = MessagesRequest {
        model,
        max_tokens: 400,
        system: Some(COMPLETION_SYSTEM),
        stream: Some(true),
        messages: vec![Message {
            role: "user",
            content: &user_msg,
        }],
    };

    let mut resp = client()
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
    mut on_text: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    let user_msg = format!("{before}<rewrite>{selected}</rewrite>{after}");
    let req = MessagesRequest {
        model,
        max_tokens: 800,
        system: Some(REWRITE_SYSTEM),
        stream: Some(true),
        messages: vec![Message {
            role: "user",
            content: &user_msg,
        }],
    };

    let mut resp = client()
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

pub async fn propose_completion(
    api_key: &str,
    model: &str,
    before: &str,
    after: &str,
) -> Result<String, String> {
    let user_msg = format!("{before}{{INSERT HERE}}{after}");
    let req = MessagesRequest {
        model,
        max_tokens: 400,
        system: Some(COMPLETION_SYSTEM),
        stream: None,
        messages: vec![Message {
            role: "user",
            content: &user_msg,
        }],
    };
    send(req, api_key).await
}

pub async fn propose_entities(api_key: &str, body: &str) -> Result<Vec<String>, String> {
    let req = MessagesRequest {
        model: DEFAULT_MODEL,
        max_tokens: 512,
        system: Some(ENTITIES_SYSTEM),
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
    let req = MessagesRequest {
        model: DEFAULT_MODEL,
        max_tokens: 256,
        system: Some(TAG_SYSTEM),
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
