mod ai;
mod backlinks;
mod config;
mod embeddings;
mod export;
mod frontmatter;
mod index;
mod link_suggestions;
mod notes;
mod saved_searches;
mod secrets;
mod tagger;
mod tags;

use std::collections::HashMap;
use std::sync::Arc;
use tauri::Manager;

struct AppState {
    index: Arc<index::NoteIndex>,
    backlinks: Arc<backlinks::BacklinkIndex>,
    embeddings: Arc<embeddings::EmbedIndex>,
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

#[tauri::command]
fn find_backlinks(
    path: String,
    state: tauri::State<AppState>,
) -> Vec<backlinks::BacklinkInfo> {
    state.backlinks.for_path(&path)
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
    let key = secrets::get_api_key().map_err(|e| format!("no API key: {e:?}"))?;
    let entities = ai::propose_entities(&key, &clean).await?;
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

#[tauri::command]
async fn complete_text(before: String, after: String) -> Result<String, String> {
    if before.trim().is_empty() && after.trim().is_empty() {
        return Ok(String::new());
    }
    let key = secrets::get_api_key().map_err(|e| format!("no API key: {e:?}"))?;
    let model = config::load().completion_model;
    ai::propose_completion(&key, &model, &before, &after).await
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
    let key = secrets::get_api_key().map_err(|e| format!("no API key: {e:?}"))?;
    let model = config::load().completion_model;
    ai::stream_completion(&key, &model, &before, &after, |text| {
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
    let key = secrets::get_api_key().map_err(|e| format!("no API key: {e:?}"))?;
    let model = config::load().completion_model;
    ai::stream_rewrite(&key, &model, &before, &selected, &after, |text| {
        let _ = on_chunk.send(text.to_string());
    })
    .await
}

#[tauri::command]
fn set_completion_model(model: String) -> Result<(), String> {
    let mut cfg = config::load();
    cfg.completion_model = model;
    config::save(&cfg).map_err(|e| e.to_string())
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
        .setup(|app| {
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
            app.manage(AppState {
                index: note_index.clone(),
                backlinks: backlink_index.clone(),
                embeddings: embed_index.clone(),
            });
            notes::start_watcher(
                app.handle().clone(),
                note_index,
                tag_worker,
                backlink_index,
                embed_index,
            )?;
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
            suggest_wikilinks,
            suggest_wikilinks_ai,
            export_as_string,
            export_to_file,
            count_wikilink_targets,
            set_api_key,
            has_api_key,
            clear_api_key,
            test_api_key,
            get_config,
            set_tagging_enabled,
            set_completion_model,
            set_notes_dir,
            reveal_notes_dir,
            list_saved_searches,
            upsert_saved_search,
            delete_saved_search,
            next_free_search_slot,
            get_tag_vocabulary,
            set_tag_vocabulary,
            list_all_tags,
            complete_text,
            complete_text_streaming,
            rewrite_text_streaming,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
