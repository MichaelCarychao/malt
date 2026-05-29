mod ai;
mod backlinks;
mod config;
mod embeddings;
mod encryption;
mod export;
mod frontmatter;
mod index;
mod link_suggestions;
mod mentions;
mod notes;
mod openai_compat;
mod prompts;
mod providers;
mod saved_searches;
mod secrets;
mod tagger;
mod tags;
mod vaults;

use std::collections::HashMap;
use std::sync::Arc;
use tauri::Manager;

struct AppState {
    index: Arc<index::NoteIndex>,
    backlinks: Arc<backlinks::BacklinkIndex>,
    embeddings: Arc<embeddings::EmbedIndex>,
    /// File-watcher handle. Holding the Arc keeps the watcher alive
    /// for app lifetime; `notes::repoint_watcher` mutates it during
    /// vault switches to follow the new path.
    watcher: notes::WatcherHandle,
}

#[tauri::command]
fn list_notes() -> Vec<notes::NoteSummary> {
    notes::list_notes()
}

#[tauri::command]
fn get_notes_dir() -> String {
    notes::notes_dir().to_string_lossy().to_string()
}

#[tauri::command]
fn search_notes(
    query: String,
    state: tauri::State<AppState>,
) -> Result<Vec<notes::NoteSummary>, String> {
    if query.trim().is_empty() {
        return Ok(notes::list_notes());
    }
    let paths = state
        .index
        .search(&query, 500)
        .map_err(|e| e.to_string())?;
    let by_path: HashMap<String, notes::NoteSummary> = notes::list_notes()
        .into_iter()
        .map(|n| (n.path.clone(), n))
        .collect();

    // Highlight only bare text terms — operator tokens (tag:foo, modified:<7d)
    // shouldn't try to match against note bodies as substrings.
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.trim_start_matches('-'))
        .filter(|t| !t.contains(':'))
        .map(|t| t.to_lowercase())
        .collect();

    Ok(paths
        .into_iter()
        .filter_map(|p| {
            let mut base = by_path.get(&p).cloned()?;
            // Title matches use the existing title text.
            base.title_matches = notes::find_matches(&base.title, &terms);
            // Re-read body to build a match-context snippet.
            let content = std::fs::read_to_string(&p).unwrap_or_default();
            let (_fm, body) = frontmatter::split(&content);
            let (snippet, snippet_matches) = notes::snippet_around_match(body, &terms, 100);
            // If no match found in body, keep the original first-line snippet.
            if !snippet_matches.is_empty() {
                base.snippet = snippet;
                base.snippet_matches = snippet_matches;
            }
            Some(base)
        })
        .collect())
}

#[tauri::command]
fn read_note(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_note(
    path: String,
    content: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())?;
    // Re-embed the changed file. Hash check inside the worker skips no-ops.
    state
        .embeddings
        .enqueue_path(std::path::PathBuf::from(&path));
    Ok(())
}

// ──────────────────────────── encryption ────────────────────────────
//
// Encrypted-note IPCs. The frontend keeps a per-path password cache in
// memory and re-prompts on focus loss when the security toggle is on;
// the backend is stateless here. Each save round-trips through
// `save_encrypted_note`, which reuses the note's existing salt + a
// process-wide derived-key cache so re-saves are a fast AES-GCM pass,
// not a fresh (slow) Argon2 derivation.

#[tauri::command]
fn is_note_encrypted(path: String) -> bool {
    std::fs::read_to_string(&path)
        .map(|c| encryption::is_encrypted(&c))
        .unwrap_or(false)
}

/// Decrypt + return plaintext for an already-encrypted note.
#[tauri::command]
fn read_encrypted_note(path: String, password: String) -> Result<String, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    encryption::decrypt(&content, &password)
}

/// Encrypt `content` and write the envelope to `path`. Used for both
/// the initial encrypt-this-note action and subsequent saves while the
/// note remains encrypted.
#[tauri::command]
fn save_encrypted_note(
    path: String,
    content: String,
    password: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    // Reuse the salt from the note's current on-disk envelope so the
    // derived-key cache hits — turns each autosave into a cheap AES-GCM
    // pass instead of a fresh Argon2id run. Fresh nonce every time.
    let prior = std::fs::read_to_string(&path).unwrap_or_default();
    let envelope = encryption::encrypt_reusing_salt(&content, &password, &prior)?;
    std::fs::write(&path, envelope).map_err(|e| e.to_string())?;
    // Re-enqueue for embedding so the index drops the prior body
    // representation (embedding worker will see encrypted content + skip
    // it via the per-path is-encrypted check it inherits from the
    // updated list_notes result).
    state
        .embeddings
        .enqueue_path(std::path::PathBuf::from(&path));
    Ok(())
}

/// Permanently remove encryption from a note. Requires the current
/// password; on success the file is rewritten as plaintext.
#[tauri::command]
fn decrypt_existing_note(
    path: String,
    password: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let plaintext = encryption::decrypt(&content, &password)?;
    std::fs::write(&path, plaintext).map_err(|e| e.to_string())?;
    state
        .embeddings
        .enqueue_path(std::path::PathBuf::from(&path));
    Ok(())
}

/// Re-encrypt a note with a new password (or set a password on a
/// previously-plain note when `old_password` is empty).
#[tauri::command]
fn change_note_password(
    path: String,
    old_password: String,
    new_password: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    if new_password.is_empty() {
        return Err("new password is empty".into());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let plaintext = if encryption::is_encrypted(&content) {
        encryption::decrypt(&content, &old_password)?
    } else {
        content
    };
    let envelope = encryption::encrypt(&plaintext, &new_password)?;
    std::fs::write(&path, envelope).map_err(|e| e.to_string())?;
    state
        .embeddings
        .enqueue_path(std::path::PathBuf::from(&path));
    Ok(())
}

#[tauri::command]
fn find_backlinks(
    path: String,
    state: tauri::State<AppState>,
) -> Vec<backlinks::BacklinkInfo> {
    state.backlinks.for_path(&path)
}

/// Notes that mention this note's title in prose without a [[wikilink]].
#[tauri::command]
fn find_unlinked_mentions(path: String) -> Vec<mentions::UnlinkedMention> {
    mentions::find(&path)
}

/// Turn the first unlinked mention of `target_title` inside
/// `source_path` into a wikilink. The watcher will refresh backlinks +
/// the editor if it's open.
#[tauri::command]
fn link_unlinked_mention(
    source_path: String,
    target_title: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    mentions::link_first(&source_path, &target_title)?;
    // Refresh backlinks immediately so the linkbacks panel updates
    // without waiting for the debounced watcher event.
    state.backlinks.rebuild();
    Ok(())
}

#[tauri::command]
fn export_as_string(
    path: String,
    format: String,
    append_links: bool,
) -> Result<String, String> {
    let md = export::build_composite_markdown(&path, append_links)?;
    let title = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    match format.as_str() {
        "md" => Ok(md),
        "txt" => Ok(export::markdown_to_plaintext(&md)),
        "html" => Ok(export::markdown_to_html_document(&md, &title)),
        "html_body" => Ok(export::markdown_to_html_body(&md)),
        _ => Err(format!("unsupported string format: {format}")),
    }
}

#[tauri::command]
fn export_to_file(
    path: String,
    format: String,
    append_links: bool,
    dest_path: String,
) -> Result<(), String> {
    let md = export::build_composite_markdown(&path, append_links)?;
    let title = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let bytes: Vec<u8> = match format.as_str() {
        "md" => md.into_bytes(),
        "txt" => export::markdown_to_plaintext(&md).into_bytes(),
        "html" => export::markdown_to_html_document(&md, &title).into_bytes(),
        "epub" => export::markdown_to_epub(&md, &title)?,
        _ => return Err(format!("unsupported file format: {format}")),
    };
    std::fs::write(&dest_path, &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn count_wikilink_targets(path: String) -> Result<usize, String> {
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (_fm, body) = frontmatter::split(&raw);
    let targets = backlinks::resolved_targets_in(body);
    let unique: std::collections::HashSet<&str> =
        targets.iter().map(|(_, p)| p.as_str()).collect();
    Ok(unique.len())
}

#[tauri::command]
fn suggest_wikilinks(path: String) -> Result<Vec<link_suggestions::LinkSuggestion>, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(link_suggestions::suggest_for_note(&content, &path))
}

#[tauri::command]
async fn suggest_wikilinks_ai(
    path: String,
) -> Result<Vec<link_suggestions::LinkSuggestion>, String> {
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    // Strip our markup before sending to the model — same hygiene as the
    // completion paths. We don't want the AI fixating on existing tags or
    // wikilinks instead of finding fresh entities.
    let clean = tags::strip_tags_for_ai(&content);
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = secrets::get_api_key_for(provider.id())
        .map_err(|e| format!("no API key for {}: {e:?}", provider.label()))?;
    let entities = ai::dispatch_propose_entities(provider, &key, &clean).await?;
    Ok(link_suggestions::build_entity_suggestions(&content, &entities))
}

#[tauri::command]
fn find_related(
    path: String,
    state: tauri::State<AppState>,
) -> Vec<embeddings::RelatedNote> {
    // Top 5 above 0.55 cosine similarity. Threshold trades recall for
    // noise: too low and you get unrelated notes, too high and "related"
    // is empty for most notes.
    state.embeddings.find_related(&path, 5, 0.55)
}

/// Semantic search over the active vault — powers the `~concept` search
/// mode. Embeds the query and returns the nearest notes as NoteSummary
/// (so the sidebar renders them like any other result set). Falls back
/// to an empty list (handled as "no matches") on any error so a missing
/// API/model never breaks the search bar.
#[tauri::command]
fn semantic_search(
    query: String,
    state: tauri::State<AppState>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<notes::NoteSummary>, String> {
    let hits = state
        .embeddings
        .search_text(&query, 50, &app_handle)?;
    let by_path: HashMap<String, notes::NoteSummary> = notes::list_notes()
        .into_iter()
        .map(|n| (n.path.clone(), n))
        .collect();
    // Preserve KNN rank order; drop hits whose file vanished since
    // indexing (handles a just-deleted note still lingering in the DB).
    Ok(hits
        .into_iter()
        .filter_map(|h| by_path.get(&h.path).cloned())
        .collect())
}

#[tauri::command]
fn delete_note(path: String, state: tauri::State<AppState>) -> Result<(), String> {
    let dir = notes::notes_dir();
    let p = std::path::PathBuf::from(&path);
    // Defense in depth: only allow deletes within the notes directory.
    if !p.starts_with(&dir) {
        return Err(format!(
            "refusing to delete {} (outside notes dir)",
            p.display()
        ));
    }
    std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    state.embeddings.forget_path(&path);
    Ok(())
}

#[tauri::command]
fn rename_note(
    app_handle: tauri::AppHandle,
    state: tauri::State<AppState>,
    path: String,
    new_title: String,
) -> Result<String, String> {
    use tauri::Emitter;

    let dir = notes::notes_dir();
    let old_path = std::path::PathBuf::from(&path);
    if !old_path.starts_with(&dir) {
        return Err("refusing to rename outside notes dir".into());
    }
    if !old_path.is_file() {
        return Err(format!("file not found: {}", old_path.display()));
    }

    let old_title = old_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "unable to extract old title".to_string())?
        .to_string();

    let trimmed = new_title.trim();
    if trimmed.is_empty() {
        return Err("title is empty".into());
    }
    if trimmed == old_title {
        return Ok(path);
    }

    let sanitized = sanitize_filename(trimmed);
    if sanitized.is_empty() {
        return Err("new title produces empty filename".into());
    }

    let new_path = dir.join(format!("{}.md", sanitized));

    // Case-insensitive collision check (defensive even though Win/Mac FS is CI).
    if new_path != old_path {
        let want = format!("{}.md", sanitized).to_lowercase();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.to_lowercase() == want {
                        return Err(format!("a note named \"{sanitized}\" already exists"));
                    }
                }
            }
        }
    }

    std::fs::rename(&old_path, &new_path).map_err(|e| e.to_string())?;
    let new_path_str = new_path.to_string_lossy().to_string();

    // Rewrite backlinks in every other .md file in the directory.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.to_lowercase().ends_with(".md")
                || name.starts_with(".~lock")
                || name.starts_with("~$")
            {
                continue;
            }
            if p == new_path {
                continue;
            }
            let content = match std::fs::read_to_string(&p) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let (fm, body) = frontmatter::split(&content);
            let (new_body, count) = backlinks::rewrite_wikilinks_in_body(body, &old_title, trimmed);
            if count > 0 {
                let full = frontmatter::merge(&fm, &new_body);
                let _ = std::fs::write(&p, full);
            }
        }
    }

    // Rewire embeddings: same content, different path. Cheap point update.
    state.embeddings.rename_path(&path, &new_path_str);

    // Notify the frontend immediately so it doesn't have to wait for the
    // debounced watcher event (file watcher fires too, but coalesces).
    let _ = app_handle.emit("notes_changed", ());

    Ok(new_path_str)
}

#[tauri::command]
fn create_note(title: String) -> Result<String, String> {
    let dir = notes::notes_dir();
    let sanitized = sanitize_filename(&title);
    if sanitized.is_empty() {
        return Err("title produces an empty filename".to_string());
    }
    let mut path = dir.join(format!("{}.md", sanitized));
    let mut counter = 2;
    while path.exists() {
        path = dir.join(format!("{}-{}.md", sanitized, counter));
        counter += 1;
        if counter > 1000 {
            return Err("too many filename collisions".to_string());
        }
    }
    std::fs::write(&path, "").map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

fn sanitize_filename(title: &str) -> String {
    let invalid: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
    title
        .chars()
        .map(|c| if invalid.contains(&c) { '-' } else { c })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

#[tauri::command]
fn set_api_key(key: String) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("empty key".to_string());
    }
    secrets::set_api_key(trimmed).map_err(|e| format!("set failed: {e:?}"))?;
    // Verify round-trip immediately so storage problems surface on Save,
    // not later on Test.
    let got = secrets::get_api_key().map_err(|e| format!("verify failed: {e:?}"))?;
    if got != trimmed {
        return Err(format!(
            "keyring round-trip mismatch (stored {} chars, got back {})",
            trimmed.len(),
            got.len()
        ));
    }
    Ok(())
}

#[tauri::command]
fn has_api_key() -> bool {
    secrets::has_api_key()
}

#[tauri::command]
fn clear_api_key() -> Result<(), String> {
    secrets::clear_api_key().map_err(|e| format!("clear failed: {e:?}"))
}

#[tauri::command]
async fn test_api_key() -> Result<String, String> {
    let key = match secrets::get_api_key() {
        Ok(k) if k.is_empty() => return Err("keyring returned empty string".to_string()),
        Ok(k) => k,
        Err(keyring::Error::NoEntry) => return Err("no API key in keychain (NoEntry)".to_string()),
        Err(e) => return Err(format!("keyring read failed: {e:?}")),
    };
    ai::test_call(&key).await
}

// ── Provider-aware key + test commands ─────────────────────────────

#[tauri::command]
fn set_api_key_for(provider: providers::Provider, key: String) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("empty key".into());
    }
    secrets::set_api_key_for(provider.id(), trimmed)
        .map_err(|e| format!("set failed: {e:?}"))?;
    let got = secrets::get_api_key_for(provider.id())
        .map_err(|e| format!("verify failed: {e:?}"))?;
    if got != trimmed {
        return Err(format!(
            "keyring round-trip mismatch for {} (stored {} chars, got back {})",
            provider.label(),
            trimmed.len(),
            got.len()
        ));
    }
    Ok(())
}

#[tauri::command]
fn has_api_key_for(provider: providers::Provider) -> bool {
    secrets::has_api_key_for(provider.id())
}

#[tauri::command]
fn clear_api_key_for(provider: providers::Provider) -> Result<(), String> {
    secrets::clear_api_key_for(provider.id())
        .map_err(|e| format!("clear failed: {e:?}"))
}

#[tauri::command]
async fn test_api_key_for(provider: providers::Provider) -> Result<String, String> {
    let key = secrets::get_api_key_for(provider.id())
        .map_err(|e| format!("no key for {}: {e:?}", provider.label()))?;
    if key.is_empty() {
        return Err(format!("no key set for {}", provider.label()));
    }
    let model = config::load().model_for(provider);
    ai::dispatch_test(provider, &key, &model).await
}

#[tauri::command]
fn set_active_provider(provider: providers::Provider) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.active_provider = provider;
    config::save(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_provider_model(provider: providers::Provider, model: String) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.provider_models.insert(provider.id().to_string(), model.clone());
    // Mirror onto completion_model for legacy code paths when the
    // active provider is Anthropic; harmless otherwise.
    if cfg.active_provider == provider && provider == providers::Provider::Anthropic {
        cfg.completion_model = model;
    }
    config::save(&cfg).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct ProviderInfo {
    id: &'static str,
    label: &'static str,
    default_model: &'static str,
    suggested_models: Vec<&'static str>,
    note: &'static str,
    has_key: bool,
    model: String,
}

#[tauri::command]
fn list_providers() -> Vec<ProviderInfo> {
    let cfg = config::load();
    providers::ALL
        .iter()
        .map(|&p| ProviderInfo {
            id: p.id(),
            label: p.label(),
            default_model: p.default_model(),
            suggested_models: p.suggested_models().to_vec(),
            note: p.note(),
            has_key: secrets::has_api_key_for(p.id()),
            model: cfg.model_for(p),
        })
        .collect()
}

#[tauri::command]
fn get_config() -> config::Config {
    config::load()
}

#[tauri::command]
fn set_tagging_enabled(enabled: bool) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.tagging_enabled = enabled;
    config::save(&cfg).map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct SecurityConfig {
    reprompt_on_blur: bool,
}

#[tauri::command]
fn get_security_config() -> SecurityConfig {
    let cfg = config::load();
    SecurityConfig {
        reprompt_on_blur: cfg.reprompt_on_blur,
    }
}

#[tauri::command]
fn set_security_reprompt_on_blur(enabled: bool) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.reprompt_on_blur = enabled;
    config::save(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_daily_note_tag(enabled: bool) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.daily_note_tag = enabled;
    config::save(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
async fn complete_text_streaming(
    before: String,
    after: String,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    if before.trim().is_empty() && after.trim().is_empty() {
        return Ok(());
    }
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = secrets::get_api_key_for(provider.id())
        .map_err(|e| format!("no API key for {}: {e:?}", provider.label()))?;
    let model = cfg.model_for(provider);
    ai::dispatch_stream_completion(provider, &key, &model, &before, &after, |text| {
        let _ = on_chunk.send(text.to_string());
    })
    .await
}

#[tauri::command]
async fn rewrite_text_streaming(
    before: String,
    selected: String,
    after: String,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    if selected.trim().is_empty() {
        return Ok(());
    }
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = secrets::get_api_key_for(provider.id())
        .map_err(|e| format!("no API key for {}: {e:?}", provider.label()))?;
    let model = cfg.model_for(provider);
    ai::dispatch_stream_rewrite(provider, &key, &model, &before, &selected, &after, |text| {
        let _ = on_chunk.send(text.to_string());
    })
    .await
}

/// Build the AI-facing version of a note: title as an H1 heading,
/// blank line, then the tag-stripped body. The title is crucial
/// context — without it the model is guessing what the note is about
/// from prose alone, which is brittle on short or fragmentary notes.
/// We use `# Title` (the canonical markdown heading form) rather than
/// `Title: X` so the model immediately recognizes the document shape.
fn ai_payload_with_title(title: &str, body: &str) -> String {
    let cleaned = crate::tags::strip_tags_for_ai(body);
    let title = title.trim();
    if title.is_empty() {
        cleaned.trim().to_string()
    } else {
        format!("# {}\n\n{}", title, cleaned.trim())
    }
}

/// Brew (brainstorm) on a note body, streaming the AI's response. The
/// note body is everything except malt-private markup — strip tags
/// and frontmatter so the model sees prose, not tag detritus. The
/// title is prepended as an H1 heading so the model knows what the
/// note is *about* even when the body is fragmentary. If the body is
/// too thin the prompt asks the model to say so explicitly.
#[tauri::command]
async fn brew_streaming(
    title: String,
    body: String,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    if body.trim().is_empty() {
        return Err("nothing to brew — the note is empty.".into());
    }
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = secrets::get_api_key_for(provider.id())
        .map_err(|e| format!("no API key for {}: {e:?}", provider.label()))?;
    let model = cfg.model_for(provider);
    let payload = ai_payload_with_title(&title, &body);
    ai::dispatch_stream_brew(provider, &key, &model, &payload, |text| {
        let _ = on_chunk.send(text.to_string());
    })
    .await
}

// ─── Prompts management ────────────────────────────────────────────

#[tauri::command]
fn list_prompts() -> Vec<prompts::PromptInfo> {
    prompts::list_all()
}

#[tauri::command]
fn set_prompt(key: prompts::PromptKey, content: String) -> Result<(), String> {
    prompts::set(key, content).map_err(|e| e.to_string())
}

#[tauri::command]
fn reset_prompt(key: prompts::PromptKey) -> Result<(), String> {
    prompts::reset(key).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_notes_dir(path: Option<String>) -> Result<String, String> {
    let mut cfg = config::load();
    match path {
        None => {
            cfg.notes_dir = None;
            config::save(&cfg).map_err(|e| e.to_string())?;
        }
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                cfg.notes_dir = None;
                config::save(&cfg).map_err(|e| e.to_string())?;
            } else {
                let p = std::path::PathBuf::from(trimmed);
                if !p.exists() {
                    std::fs::create_dir_all(&p)
                        .map_err(|e| format!("can't create {}: {}", p.display(), e))?;
                }
                if !p.is_dir() {
                    return Err(format!("{} is not a directory", p.display()));
                }
                cfg.notes_dir = Some(p.to_string_lossy().to_string());
                config::save(&cfg).map_err(|e| e.to_string())?;
            }
        }
    }
    // Return the *effective* path (resolved by notes_dir() — falls back to
    // ~/malt/ when the override is None). Frontend uses this for display.
    Ok(notes::notes_dir().to_string_lossy().to_string())
}

#[tauri::command]
fn reveal_notes_dir() -> Result<(), String> {
    let dir = notes::notes_dir();
    open_path_in_explorer(&dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_note(path: String) -> Result<(), String> {
    // On Windows, `explorer /select,<path>` highlights the file in its
    // parent folder. On macOS, `open -R <path>` does the same ("reveal in
    // Finder"). On Linux there's no portable equivalent; we fall back to
    // opening the parent dir.
    let p = std::path::PathBuf::from(&path);
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", p.display()))
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&p)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let parent = p.parent().unwrap_or(&p);
        std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ── Vaults ────────────────────────────────────────────────────────

#[tauri::command]
fn list_vaults() -> vaults::VaultsState {
    vaults::load()
}

#[tauri::command]
fn active_vault_name() -> String {
    vaults::active_name()
}

#[tauri::command]
fn add_vault(name: String, path: String) -> Result<vaults::VaultsState, String> {
    vaults::add(name, path)
}

#[tauri::command]
async fn switch_vault(
    index: u32,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<vaults::VaultsState, String> {
    // Capture the path the watcher is currently observing BEFORE we
    // flip the active vault — `notes::notes_dir()` reads through the
    // vaults registry, so it changes the instant `switch` returns.
    let old_dir = notes::notes_dir();
    let updated = vaults::switch(index as usize)?;
    let new_dir = notes::notes_dir();
    notes::repoint_watcher(&state.watcher, &old_dir, &new_dir);
    // Reindex everything for the new vault. Backlinks + search index
    // rebuild are cheap (in-memory); embeddings repoint to the new
    // vault's own DB then re-queue. The notes_changed event triggers a
    // frontend refresh.
    state.index.rebuild().map_err(|e| e.to_string())?;
    state.backlinks.rebuild();
    let _ = state.embeddings.repoint();
    state.embeddings.enqueue_dir();
    let _ = tauri::Emitter::emit(&app_handle, "notes_changed", ());
    let _ = tauri::Emitter::emit(&app_handle, "vault_changed", &updated);
    Ok(updated)
}

#[tauri::command]
fn rename_vault(index: u32, name: String) -> Result<vaults::VaultsState, String> {
    vaults::rename(index as usize, name)
}

#[tauri::command]
fn remove_vault(
    index: u32,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<vaults::VaultsState, String> {
    // Capture the removed vault's path BEFORE removal so we can reclaim
    // its embedding DB. (Removing only unlinks from malt — the user's
    // .md files on disk are untouched — but the derived embedding file
    // is malt's and safe to delete.)
    let removed_path = vaults::load()
        .vaults
        .get(index as usize)
        .map(|v| v.path.clone());
    let old_dir = notes::notes_dir();
    let updated = vaults::remove(index as usize)?;
    let new_dir = notes::notes_dir();
    if old_dir != new_dir {
        notes::repoint_watcher(&state.watcher, &old_dir, &new_dir);
    }
    // Reclaim the removed vault's embedding DB file.
    if let Some(p) = removed_path {
        embeddings::delete_db_for(&p);
    }
    // Removing a vault may have shifted the active one. Reindex + repoint
    // embeddings to whatever the new active vault is.
    state.index.rebuild().map_err(|e| e.to_string())?;
    state.backlinks.rebuild();
    let _ = state.embeddings.repoint();
    state.embeddings.enqueue_dir();
    let _ = tauri::Emitter::emit(&app_handle, "notes_changed", ());
    let _ = tauri::Emitter::emit(&app_handle, "vault_changed", &updated);
    Ok(updated)
}

#[tauri::command]
fn duplicate_note(path: String) -> Result<String, String> {
    let src = std::path::PathBuf::from(&path);
    if !src.is_file() {
        return Err(format!("file not found: {}", src.display()));
    }
    let dir = notes::notes_dir();
    if !src.starts_with(&dir) {
        return Err("refusing to duplicate outside notes dir".into());
    }
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    let base = format!("{stem} copy");
    let mut candidate = dir.join(format!("{base}.md"));
    let mut counter = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} {counter}.md"));
        counter += 1;
        if counter > 1000 {
            return Err("too many name collisions duplicating".into());
        }
    }
    std::fs::copy(&src, &candidate).map_err(|e| e.to_string())?;
    Ok(candidate.to_string_lossy().to_string())
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// On the very first launch, drop a welcome note + quick-tour note into the
/// notes folder so new users land on something useful. Guarded by a
/// `welcomed` flag file in the config dir so deleting the welcome doesn't
/// resurrect it on next launch.
fn seed_welcome_notes_if_first_run() {
    let cfg_dir = match dirs::config_dir() {
        Some(p) => p.join("malt"),
        None => return,
    };
    let _ = std::fs::create_dir_all(&cfg_dir);
    let flag = cfg_dir.join("welcomed");
    if flag.exists() {
        return;
    }
    let notes_dir = notes::notes_dir();
    // Only seed if the notes dir is currently empty of .md files.
    let has_md = std::fs::read_dir(&notes_dir)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if has_md {
        // Don't disturb a user who already has notes. Just mark welcomed.
        let _ = std::fs::write(&flag, "");
        return;
    }
    let welcome = r#"# Welcome to malt

You're looking at a plain `.md` file in your notes folder. Everything malt does is built on top of files like this — no proprietary database, no lock-in. If you ever want to walk away, your notes walk with you.

## Quick orientation

- **Type to filter.** The search bar at the top filters as you type. Hit Enter on a non-matching query to create a new note with that title.
- **Wikilinks** like [[Quick Tour]] connect notes. Click one to jump. Type `[[` for autocomplete.
- **Hashtags** like the ones at the bottom of this file are search shortcuts. The pill row above the editor shows current tags; click to filter.
- **AI help** lives on `Ctrl+I` (`Cmd+I` on Mac). Bare press continues from the cursor. With a selection, it rewrites.
- **Settings** is `Ctrl+,` — every keyboard shortcut is documented there.

Open [[Quick Tour]] next for a feature walk-through. When you're done, delete both of these notes and start writing yours.

#fleeting
"#;
    let tour = r#"# Quick Tour

A working tour of malt's main moves. Tweak this file, delete it, or leave it — your call.

## Search & navigation

- `Ctrl+L` focuses the search bar.
- `Ctrl+↑/↓` (or `Ctrl+J/K`) moves the selection up/down from anywhere.
- `Ctrl+[ / Ctrl+]` is back/forward in the pane's history.
- `Ctrl+W` closes the secondary editor pane (open one by Ctrl+clicking a wikilink or list row).

## Tagging

- Type `#anything` inline and it autoreloacates to a hidden line at the bottom of the file on save. You see pills above the editor instead.
- Hover a pill, click the `×` to remove it. Right-click for the full menu (filter, remove, promote-to-vocab).
- Edit your starter vocabulary in Settings → tags & queries.

## Saved searches

- Type a query like `tag:fleeting modified:<7d` and hit `Ctrl+S` to save it.
- Activate any saved search via `Ctrl+1` through `Ctrl+9`.

## AI moves (needs an Anthropic API key — set it in Settings → ai)

- `Ctrl+I` at end of doc → continue the writing
- `Ctrl+I` with text selected → rewrite that text, unpacking generalities
- `Ctrl+I` in the middle of a doc → bridge the gap
- `Ctrl+Shift+L` → review proposed wikilinks (existing notes + AI-suggested new ones)

## Other useful bits

- `Ctrl+R` while editing → rename the note (rewrites all backlinks atomically)
- `Ctrl+Shift+E` → export the current note (.md / .html / .epub / .txt, with optional "append linked notes" for TOC-style sharing)
- Double-click any note row → rename inline

That's most of it. Have fun.

#fleeting
"#;
    let _ = std::fs::write(notes_dir.join("Welcome to malt.md"), welcome);
    let _ = std::fs::write(notes_dir.join("Quick Tour.md"), tour);
    let _ = std::fs::write(&flag, "");
}

#[tauri::command]
fn list_saved_searches() -> Vec<saved_searches::SavedSearch> {
    saved_searches::load()
}

#[tauri::command]
fn upsert_saved_search(
    item: saved_searches::SavedSearch,
) -> Result<Vec<saved_searches::SavedSearch>, String> {
    saved_searches::upsert(item).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_saved_search(id: String) -> Result<Vec<saved_searches::SavedSearch>, String> {
    saved_searches::delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn rename_saved_search(
    id: String,
    name: String,
) -> Result<Vec<saved_searches::SavedSearch>, String> {
    saved_searches::rename(&id, name).map_err(|e| e.to_string())
}

#[tauri::command]
fn reorder_saved_search(
    id: String,
    target_position: u32,
) -> Result<Vec<saved_searches::SavedSearch>, String> {
    saved_searches::reorder(&id, target_position as usize).map_err(|e| e.to_string())
}

#[tauri::command]
fn unbind_saved_search_slot(id: String) -> Result<Vec<saved_searches::SavedSearch>, String> {
    saved_searches::unbind_slot(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn next_free_search_slot() -> Option<u8> {
    saved_searches::next_free_slot()
}

#[tauri::command]
fn get_tag_vocabulary() -> Vec<String> {
    config::load().tag_vocabulary
}

#[tauri::command]
fn set_tag_vocabulary(vocabulary: Vec<String>) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.tag_vocabulary = vocabulary
        .into_iter()
        .map(|t| tags::canonicalize(&t))
        .filter(|t| !t.is_empty())
        .collect();
    config::save(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_all_tags() -> Vec<TagCount> {
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for note in notes::list_notes() {
        for tag in &note.tags {
            *counts.entry(tag.clone()).or_insert(0) += 1;
        }
    }
    let mut out: Vec<TagCount> = counts
        .into_iter()
        .map(|(name, count)| TagCount { name, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    out
}

#[derive(serde::Serialize)]
struct TagCount {
    name: String,
    count: u32,
}

fn open_path_in_explorer(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(path).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            seed_welcome_notes_if_first_run();
            let note_index = Arc::new(index::NoteIndex::new()?);
            note_index.rebuild()?;
            let backlink_index = Arc::new(backlinks::BacklinkIndex::new());
            backlink_index.rebuild();
            let embed_index = embeddings::EmbedIndex::new()
                .map_err(|e| format!("embeddings init failed: {e}"))?;
            embed_index.enqueue_dir();
            embeddings::start(embed_index.clone(), app.handle().clone());
            let tag_worker = tagger::Tagger::new();
            tag_worker.enqueue_dir();
            tagger::start(tag_worker.clone(), app.handle().clone());
            let watcher_handle = notes::start_watcher(
                app.handle().clone(),
                note_index.clone(),
                tag_worker,
                backlink_index.clone(),
                embed_index.clone(),
            )?;
            app.manage(AppState {
                index: note_index,
                backlinks: backlink_index,
                embeddings: embed_index,
                watcher: watcher_handle,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_notes,
            get_notes_dir,
            search_notes,
            read_note,
            save_note,
            create_note,
            rename_note,
            delete_note,
            find_backlinks,
            find_related,
            semantic_search,
            find_unlinked_mentions,
            link_unlinked_mention,
            suggest_wikilinks,
            suggest_wikilinks_ai,
            export_as_string,
            export_to_file,
            count_wikilink_targets,
            set_api_key,
            has_api_key,
            clear_api_key,
            test_api_key,
            set_api_key_for,
            has_api_key_for,
            clear_api_key_for,
            test_api_key_for,
            set_active_provider,
            set_provider_model,
            list_providers,
            get_config,
            set_tagging_enabled,
            set_daily_note_tag,
            set_notes_dir,
            reveal_notes_dir,
            reveal_note,
            duplicate_note,
            list_vaults,
            active_vault_name,
            add_vault,
            switch_vault,
            rename_vault,
            remove_vault,
            app_version,
            list_saved_searches,
            upsert_saved_search,
            delete_saved_search,
            rename_saved_search,
            reorder_saved_search,
            unbind_saved_search_slot,
            next_free_search_slot,
            get_tag_vocabulary,
            set_tag_vocabulary,
            list_all_tags,
            complete_text_streaming,
            rewrite_text_streaming,
            brew_streaming,
            list_prompts,
            set_prompt,
            reset_prompt,
            is_note_encrypted,
            read_encrypted_note,
            save_encrypted_note,
            decrypt_existing_note,
            change_note_password,
            get_security_config,
            set_security_reprompt_on_blur,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
