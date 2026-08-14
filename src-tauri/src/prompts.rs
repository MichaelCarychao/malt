// Prompt registry. Every AI prompt malt sends lives here so the user
// can review (and override) what's being said in their name.
//
// Each prompt has:
//   - a stable `PromptKey` enum variant (serialized as snake_case)
//   - a built-in default (the prompt malt ships with)
//   - an optional user override stored in ~/.config/malt/prompts.json
//
// Resolution: `get(key)` returns the user's override if present, else
// the default. ai.rs reads all its prompts through this module so the
// rest of the code never needs to know whether the prompt was authored
// by us or customized by the user.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Stable identifiers for each AI prompt. Serializes as snake_case in
/// the prompts.json file and over the IPC boundary. Adding a new
/// variant requires adding the default text in `default_for` and
/// adding it to the `ALL_KEYS` list (used by the Settings UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKey {
    Tag,
    Entities,
    Completion,
    Rewrite,
    Brew,
    Implement,
}

pub const ALL_KEYS: &[PromptKey] = &[
    PromptKey::Completion,
    PromptKey::Rewrite,
    PromptKey::Brew,
    PromptKey::Implement,
    PromptKey::Tag,
    PromptKey::Entities,
];

impl PromptKey {
    /// Human-readable label for the Settings UI.
    pub fn label(self) -> &'static str {
        match self {
            PromptKey::Tag => "Auto-tag",
            PromptKey::Entities => "Wikilink suggestions (entities)",
            PromptKey::Completion => "Ghost completion (continue / begin / bridge)",
            PromptKey::Rewrite => "Selection rewrite",
            PromptKey::Brew => "Brew ideas (brainstorm)",
            PromptKey::Implement => "Implement suggestion (brew)",
        }
    }

    /// One-line description of when malt sends this prompt.
    pub fn description(self) -> &'static str {
        match self {
            PromptKey::Tag => "Sent by the background auto-tagger to propose 1–5 inline hashtags for each note. Off by default.",
            PromptKey::Entities => "Sent when you press Cmd+Shift+L; the model proposes entity names worth turning into [[wikilinks]].",
            PromptKey::Completion => "Sent on Cmd+; in the editor. Three modes (continue / begin / bridge) handled in one prompt.",
            PromptKey::Rewrite => "Sent on Cmd+; with a selection. Rewrites the marked text in voice with the rest of the note as context.",
            PromptKey::Brew => "Sent on Cmd+Shift+B. Brainstorms ways to explore, double down on, or follow up on what the note has started.",
            PromptKey::Implement => "Sent when you click implement on a brew checklist item. Applies that one instruction to the note and returns the full revised text.",
        }
    }
}

/// Built-in default for each prompt. These are the prompts malt ships
/// with — what users see in the Settings tab when no override is set.
/// Changes here ship to all users on update.
pub fn default_for(key: PromptKey) -> &'static str {
    match key {
        PromptKey::Tag => DEFAULT_TAG,
        PromptKey::Entities => DEFAULT_ENTITIES,
        PromptKey::Completion => DEFAULT_COMPLETION,
        PromptKey::Rewrite => DEFAULT_REWRITE,
        PromptKey::Brew => DEFAULT_BREW,
        PromptKey::Implement => DEFAULT_IMPLEMENT,
    }
}

const DEFAULT_TAG: &str = "You are a tagger for a personal note-taking system. Read the note and propose 1 to 5 tags that capture what the note is *about* — its topics and entities, not its style, length, or format. Use lowercase. Use kebab-case for multi-word tags. Output JSON only, no prose, no markdown fences, no commentary: {\"tags\":[\"tag-one\",\"tag-two\"]}";

const DEFAULT_ENTITIES: &str = "You are extracting linkable entities from a personal note. The user maintains a Zettelkasten-style knowledge base where any named entity that's meaningful enough to revisit gets its own note.\n\nRead the note and identify distinct entities that warrant their own page: specific people, named places, specific projects, books or articles by title, organizations, named events, products, and discrete concepts the user is clearly working with (not generic abstractions).\n\nRULES:\n- Skip generic nouns and abstract categories (\"the project\", \"my book\", \"people\", \"work\", \"breakfast\"). Only proper nouns or clearly-discrete concepts.\n- Skip passing references with no narrative weight (\"I had coffee\" — coffee isn't an entity here).\n- Skip pronouns and unspecified references (\"my friend\", \"the author\", \"someone\").\n- Skip entities that look like part of the user's own taxonomy/structure (date headers, status words, generic verbs).\n- Use the entity's own preferred form / title casing if obvious (e.g., \"Atomic Habits\" not \"atomic habits\").\n- Max 10 entities. Bias toward precision over recall — if you're unsure whether something rates a note, leave it out.\n\nOutput JSON only, no prose, no markdown fences: {\"entities\":[\"Entity One\",\"Entity Two\"]}";

const DEFAULT_COMPLETION: &str = "You are a writing assistant inside a personal note-taking app. The user sends their note text wrapped in ONE of three mode tags, with their cursor position marked literally as {INSERT HERE}.\n\nThe three modes:\n\n<continue>...{INSERT HERE}</continue>\n    The cursor is at the end of the document. Text exists before; nothing after. Write a natural continuation that picks up the thread of what's there.\n\n<begin>{INSERT HERE}...</begin>\n    The cursor is at the start. Nothing before; text exists after. Write an opener that flows naturally into what follows — the first sentence(s) of the piece.\n\n<bridge>...{INSERT HERE}...</bridge>\n    The cursor is in the middle. Text on both sides. Write a passage that connects them seamlessly.\n\n    BRIDGE-SPECIFIC ORPHAN SCAN: before writing, mentally scan the AFTER text for concrete elements not present in BEFORE — objects, characters, gestures, sensory details (a sound, a smell, a color, a temperature), emotional shifts, places, weather, light. Each is a Chekhov's gun. Your bridge MUST plant each one or the AFTER will read as a non-sequitur.\n    Example: BEFORE shows a woman boarding a train. AFTER says \"the red balloon floated away.\" The balloon is an orphan. The bridge has to put a balloon somewhere — a child clutching one, a vendor on the platform, a stray balloon she sees through the window. Without the balloon in your bridge, the after-text reads as random.\n\nUNIVERSAL RULES (every mode):\n\n1. OUTPUT ONLY the text to insert at the marker. The user concatenates it verbatim into their document.\n\n2. NEVER respond conversationally. NEVER ask the user for more context. NEVER say \"I don't see...\" or \"please share...\" or \"I'm ready to help.\" If you can't tell what's wanted, output a short, neutral continuation/opener/bridge based on whatever IS present, even if it's just one sentence. The user can always reject your suggestion — they can never use a conversational reply.\n\n3. NEVER include preamble, commentary, narration about what you're doing, markdown fences, quotation marks around your output, or the mode tags themselves.\n\n4. Match the surrounding voice, tense, register, vocabulary, punctuation density, and sentence length. If the surrounding text is in a particular structure (list item, heading, dialogue), match that structure.\n\n5. Don't repeat words/phrases from the immediate edges of the marker — duplication reads as a stutter when concatenated.\n\n6. Length: 1–3 sentences typically, or a short paragraph if the rhythm calls for it. Don't pad.\n\n7. If insertion would corrupt structure (mid-word, mid-bracket, inside a code fence) or if there's truly no useful continuation possible, output nothing (empty response) rather than improvising garbage.

8. STEERING: the message may end with a <direction>...</direction> block — a steering note from the user about what they want this generation to do (e.g. \"make it darker\", \"pivot to the counterargument\", \"shorter, punchier\"). It is an instruction to you, NOT text to insert. Follow it while still obeying every rule above; never echo the direction text or the tags into your output.\n\nMODE-SPECIFIC TONE:\n- <continue>: lean into the established voice. Pick up wherever the writer left off — same sentence if mid-thought, next sentence if a paragraph closed.\n- <begin>: be a strong opener. Set the scene, the question, or the proposition that AFTER builds on.\n- <bridge>: the seam between BEFORE and AFTER should feel like it was written in one pass. Plant orphans.";

const DEFAULT_REWRITE: &str = "You are a writing assistant inside a personal note-taking app. The user sends their full note. Inside it, a portion is wrapped in <rewrite>...</rewrite> tags — that is the text they want rewritten. The rest of the note is context for voice, topic, and style.\n\nYour job: rewrite the marked text by unpacking details and avoiding generalities and clichés. Make it more specific, more concrete, more grounded in particulars and observable detail. Match the surrounding voice, tense, register, and formatting. Length should be roughly comparable — don't pad, don't compress unless the original was bloated.\n\nOUTPUT RULES:\n\n1. OUTPUT ONLY the replacement text for what was inside <rewrite>...</rewrite>. The user concatenates it verbatim in place of the original selection.\n\n2. NEVER respond conversationally. NEVER ask the user for more context. NEVER say \"I'd be happy to help\" or \"please share\" or \"I don't see...\". If the input is unclear, do your best with what's there; the user can reject the suggestion.\n\n3. NEVER include preamble, commentary, narration, markdown fences, quotation marks, or the <rewrite>/</rewrite> tags.\n\n4. If the marked text is a single word, a punctuation mark, or a generic connector that can't be meaningfully improved, output it unchanged.\n\n5. If the marked text is empty or whitespace-only, output nothing (empty response).

6. STEERING: the message may end with a <direction>...</direction> block — a steering note about how to rewrite (e.g. \"more formal\", \"cut the hedging\", \"make it concrete\"). It's an instruction to you, not text to include. Follow it; never echo the direction or its tags.";

const DEFAULT_BREW: &str = "You are a brainstorming partner inside a personal note-taking app. The user sends you a single note — could be a journal entry, a half-formed idea, a meeting log, a draft, a question they're chewing on, anything. They want you to help them BREW it: figure out what's worth doing to it next.\n\nEvery suggestion you make can be APPLIED to the note by the app with one click, so each one must pair its insight with a concrete edit instruction. Never leave a suggestion as a bare question — always land on what to actually change.\n\nProduce a short, scannable markdown response with three sections:\n\n## Threads to pull\n3–5 suggestions. Each names a specific opportunity in THIS note (quote a few words of the trigger), then states the edit to make. Format: provocation, then \" → \", then the instruction. Example: \"- The ending trails off (\\\"and that was that\\\") → Replace the last paragraph with a concrete image that echoes the opening scene.\"\n\n## Where this connects\n2–4 suggestions that would weave in adjacent material — topics, frameworks, books, people, or other notes the user has clearly referenced. Don't fabricate references. Same format: why it connects → the edit that would work it in. Example: \"- This is the planning-fallacy pattern → Add a sentence naming it and citing the bridge estimate example.\"\n\n## A few sharper framings\n2–3 reframings of the note's central idea — flip a hidden assumption, push the claim to its extreme, zoom out until it becomes a different question. Same format: the reframing → the edit that would recast the note around it.\n\nFORMAT RULES (the app parses your output):\n- Each suggestion is EXACTLY ONE line starting with \"- \". No nested lists, no multi-line items, no numbered lists.\n- Section headers are exactly \"## Threads to pull\", \"## Where this connects\", \"## A few sharper framings\".\n- Output markdown only. No preamble, no closing. Keep the whole response under ~400 words.\n- Don't moralize. Don't add disclaimers. Don't ask clarifying questions — work with what's there.\n- If the note is too thin to brew (a single word, just a title, fewer than ~20 words of substance), respond with exactly: \"_The note is still too sparse to brew — try drafting a paragraph or two first._\" and nothing else.\n- The user can already see the note. Don't recap it.";

const DEFAULT_IMPLEMENT: &str = "You are a revision assistant inside a personal note-taking app. The user sends a note wrapped in <note>...</note> and a single revision instruction wrapped in <instruction>...</instruction>. Apply the instruction to the note.\n\nOUTPUT RULES:\n\n1. Output ONLY the complete revised note text. The app replaces the note with your output verbatim — anything else you emit becomes part of the note.\n\n2. Make the minimal edits the instruction requires. Reproduce every unrelated line byte-for-byte: markdown formatting, [[wikilinks]], #hashtags, blank lines, list markers, headings, indentation — all preserved exactly.\n\n3. No preamble, no commentary, no markdown fences around the output, no <note> tags, no explanation of what you changed.\n\n4. NEVER respond conversationally. NEVER ask questions. If the instruction is ambiguous, pick the most natural reading and apply it.\n\n5. If the instruction cannot be applied to this note at all, output the note completely unchanged.";

// ── Storage ───────────────────────────────────────────────────────────

fn path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    let _ = std::fs::create_dir_all(&p);
    p.push("prompts.json");
    p
}

fn load_overrides() -> HashMap<PromptKey, String> {
    match crate::config::read_json(&path()) {
        crate::config::JsonRead::Parsed(m) => m,
        _ => HashMap::new(),
    }
}

/// Load for a read-modify-write cycle (set/reset). Errors when the file
/// exists but can't be read — saving a default map over it would silently
/// wipe every customized prompt.
fn load_overrides_for_update() -> Result<HashMap<PromptKey, String>, String> {
    match crate::config::read_json(&path()) {
        crate::config::JsonRead::Parsed(m) => Ok(m),
        crate::config::JsonRead::Missing | crate::config::JsonRead::Quarantined => {
            Ok(HashMap::new())
        }
        crate::config::JsonRead::Unreadable => Err(
            "prompts.json exists but can't be read right now — change not saved              (retry in a moment)"
                .to_string(),
        ),
    }
}

fn save_overrides(map: &HashMap<PromptKey, String>) -> std::io::Result<()> {
    // Never let a serialization failure truncate the prompt overrides:
    // propagate the error rather than writing an empty string over a valid
    // file (which would silently wipe every customized prompt).
    let json = serde_json::to_string_pretty(map)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Atomic temp-file + rename so a crash mid-write can't leave a
    // half-written / empty prompts.json behind.
    crate::notes::write_atomic(path(), &json)
}

/// Returns the prompt text for `key` — user override if set, else the
/// built-in default. Always returns owned String so callers can pass it
/// to the Anthropic client without lifetime gymnastics.
pub fn get(key: PromptKey) -> String {
    let overrides = load_overrides();
    overrides
        .get(&key)
        .cloned()
        .unwrap_or_else(|| default_for(key).to_string())
}

/// Save a user override for `key`. Empty content is treated as a
/// reset (removes the override).
pub fn set(key: PromptKey, content: String) -> Result<(), String> {
    let mut overrides = load_overrides_for_update()?;
    if content.trim().is_empty() {
        overrides.remove(&key);
    } else {
        overrides.insert(key, content);
    }
    save_overrides(&overrides).map_err(|e| e.to_string())
}

/// Remove the user override for `key`, falling back to the built-in
/// default on next `get()`.
pub fn reset(key: PromptKey) -> Result<(), String> {
    let mut overrides = load_overrides_for_update()?;
    overrides.remove(&key);
    save_overrides(&overrides).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptInfo {
    pub key: PromptKey,
    pub label: &'static str,
    pub description: &'static str,
    pub default: String,
    pub current: String,
    pub is_overridden: bool,
}

/// Snapshot every prompt for the Settings UI: label, description,
/// default text, current text (override or default), and whether the
/// user has overridden it.
pub fn list_all() -> Vec<PromptInfo> {
    let overrides = load_overrides();
    ALL_KEYS
        .iter()
        .map(|&key| {
            let default = default_for(key).to_string();
            let current = overrides.get(&key).cloned().unwrap_or_else(|| default.clone());
            let is_overridden = overrides.contains_key(&key);
            PromptInfo {
                key,
                label: key.label(),
                description: key.description(),
                default,
                current,
                is_overridden,
            }
        })
        .collect()
}
