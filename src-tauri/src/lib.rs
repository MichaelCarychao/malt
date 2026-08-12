mod ai;
mod backlinks;
mod config;
mod embeddings;
mod encryption;
mod export;
mod fnv;
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
async fn list_notes() -> Result<Vec<notes::NoteSummary>, String> {
    // Cache hit is microseconds, but a cold/cross-vault miss re-scans the
    // whole vault — keep that off the IPC thread.
    tauri::async_runtime::spawn_blocking(notes::list_notes)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_notes_dir() -> String {
    notes::notes_dir().to_string_lossy().to_string()
}

#[tauri::command]
async fn search_notes(
    query: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<notes::NoteSummary>, String> {
    if query.trim().is_empty() {
        // Still a full-vault disk read; keep it off the IPC thread too.
        return tauri::async_runtime::spawn_blocking(notes::list_notes)
            .await
            .map_err(|e| e.to_string());
    }
    // The query reads the whole vault from disk twice (list_notes + per-hit
    // body re-read) plus runs the tantivy search. That's all blocking I/O —
    // move it onto a blocking worker so the IPC thread never stalls. State
    // can't cross into the closure, so clone the Arc the work needs first.
    let index = state.index.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let paths = index.search(&query, 500).map_err(|e| e.to_string())?;
        let by_path: HashMap<String, notes::NoteSummary> = notes::list_notes()
            .into_iter()
            .map(|n| (n.path.clone(), n))
            .collect();

        // Highlight only bare text terms — operator tokens (tag:foo,
        // modified:<7d) shouldn't try to match against note bodies as
        // substrings.
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
                // Title matches highlight the *displayed* name (H1 or filename).
                base.title_matches = notes::find_matches(&base.display_title, &terms);
                // Body for the match-context snippet comes from the note
                // cache — this used to be a second per-hit disk read on
                // every keystroke.
                let content = notes::cached_content(&p)
                    .unwrap_or_else(|| std::sync::Arc::from(""));
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
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reject paths that escape the active vault before any read/write. Note
/// commands receive an absolute path the frontend got from `list_notes`,
/// but defense-in-depth: a crafted path must not let an IPC caller touch
/// files outside the vault.
///
/// Canonicalize both sides and compare canonical-to-canonical — on Windows
/// `canonicalize` adds a `\\?\` prefix, so a raw-vs-canonical `starts_with`
/// would never match. For a path that doesn't exist yet (e.g. a note about
/// to be created) we canonicalize its parent instead. If canonicalization
/// fails entirely, fall back to the prior lexical `starts_with` check rather
/// than hard-failing a legitimate flow.
fn ensure_in_vault(path: &str) -> Result<(), String> {
    let dir = notes::notes_dir();
    let target = std::path::Path::new(path);

    let canon_dir = std::fs::canonicalize(&dir);
    // Canonicalize the target itself if it exists, otherwise its parent
    // (the file is about to be created inside that directory).
    let canon_target = if target.exists() {
        std::fs::canonicalize(target)
    } else if let Some(parent) = target.parent() {
        std::fs::canonicalize(parent)
    } else {
        std::fs::canonicalize(target)
    };

    match (canon_dir, canon_target) {
        (Ok(d), Ok(t)) if t.starts_with(&d) => Ok(()),
        (Ok(_), Ok(_)) => Err(format!(
            "refusing to access {} (outside vault)",
            target.display()
        )),
        // Canonicalization failed on one side (permissions, race, etc.) —
        // fall back to the prior lexical check so we don't break valid use.
        _ => {
            if target.starts_with(&dir) {
                Ok(())
            } else {
                Err(format!(
                    "refusing to access {} (outside vault)",
                    target.display()
                ))
            }
        }
    }
}

#[tauri::command]
fn read_note(path: String) -> Result<String, String> {
    ensure_in_vault(&path)?;
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
async fn save_note(
    path: String,
    content: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_in_vault(&path)?;
    let index = state.index.clone();
    let embeddings = state.embeddings.clone();
    // write_atomic fsyncs and the index upsert commits — blocking work that
    // used to serialize the IPC lane on every 300ms-debounced autosave.
    tauri::async_runtime::spawn_blocking(move || {
    // Defense in depth (belt-and-suspenders behind the frontend's
    // never-write-plaintext-over-ciphertext rule): if the file on disk is
    // currently an encrypted envelope and the incoming content is NOT one,
    // refuse the write. Otherwise a stray plaintext save would silently
    // destroy the encryption and leak the note's contents. Reads of a
    // not-yet-existing file return Err → nothing to clobber, proceed.
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if encryption::is_encrypted(&existing) && !encryption::is_encrypted(&content) {
                return Err(
                    "refusing to overwrite an encrypted note with plaintext".to_string(),
                );
            }
        }
        notes::write_atomic(&path, &content).map_err(|e| e.to_string())?;
        // Update the search index (and through it, the note cache) so the
        // edit is searchable immediately and doesn't depend on the
        // debounced watcher (which can drop or coalesce events). Log on
        // failure rather than failing the save — the note is already
        // safely on disk, and the watcher rebuild is a backstop.
        if let Err(e) = index.upsert(&path) {
            eprintln!("save_note: index upsert failed for {path}: {e}");
        }
        // Re-embed the changed file. Hash check inside the worker skips no-ops.
        embeddings.enqueue_path(std::path::PathBuf::from(&path));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
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
    // Out-of-vault paths are reported as not-encrypted; the read/save guards
    // reject any real access regardless.
    if ensure_in_vault(&path).is_err() {
        return false;
    }
    std::fs::read_to_string(&path)
        .map(|c| encryption::is_encrypted(&c))
        .unwrap_or(false)
}

/// Decrypt + return plaintext for an already-encrypted note. Async: a
/// cold-cache decrypt runs Argon2id (~100ms+), which must not stall the
/// IPC lane.
#[tauri::command]
async fn read_encrypted_note(path: String, password: String) -> Result<String, String> {
    ensure_in_vault(&path)?;
    tauri::async_runtime::spawn_blocking(move || {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        encryption::decrypt(&content, &password)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Encrypt `content` and write the envelope to `path`. Used for both
/// the initial encrypt-this-note action and subsequent saves while the
/// note remains encrypted.
#[tauri::command]
async fn save_encrypted_note(
    path: String,
    content: String,
    password: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_in_vault(&path)?;
    let index = state.index.clone();
    let embeddings = state.embeddings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Reuse the salt from the note's current on-disk envelope so the
        // derived-key cache hits — turns each autosave into a cheap AES-GCM
        // pass instead of a fresh Argon2id run. Fresh nonce every time.
        let prior = std::fs::read_to_string(&path).unwrap_or_default();
        let envelope = encryption::encrypt_reusing_salt(&content, &password, &prior)?;
        notes::write_atomic(&path, &envelope).map_err(|e| e.to_string())?;
        // Reindex now: upsert re-reads the file, sees the MALT-ENC envelope,
        // and reindexes title-only — dropping the prior plaintext body from
        // search immediately rather than leaving it exposed until the
        // watcher fires.
        if let Err(e) = index.upsert(&path) {
            eprintln!("save_encrypted_note: index upsert failed for {path}: {e}");
        }
        // Re-enqueue for embedding so the index drops the prior body
        // representation (embedding worker will see encrypted content + skip
        // it via the per-path is-encrypted check it inherits from the
        // updated list_notes result).
        embeddings.enqueue_path(std::path::PathBuf::from(&path));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Permanently remove encryption from a note. Requires the current
/// password; on success the file is rewritten as plaintext.
#[tauri::command]
async fn decrypt_existing_note(
    path: String,
    password: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_in_vault(&path)?;
    let index = state.index.clone();
    let embeddings = state.embeddings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let plaintext = encryption::decrypt(&content, &password)?;
        notes::write_atomic(&path, &plaintext).map_err(|e| e.to_string())?;
        // Now plaintext on disk — reindex so the body becomes searchable again.
        if let Err(e) = index.upsert(&path) {
            eprintln!("decrypt_existing_note: index upsert failed for {path}: {e}");
        }
        embeddings.enqueue_path(std::path::PathBuf::from(&path));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Re-encrypt a note with a new password (or set a password on a
/// previously-plain note when `old_password` is empty).
#[tauri::command]
async fn change_note_password(
    path: String,
    old_password: String,
    new_password: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    ensure_in_vault(&path)?;
    if new_password.is_empty() {
        return Err("new password is empty".into());
    }
    let index = state.index.clone();
    let embeddings = state.embeddings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let plaintext = if encryption::is_encrypted(&content) {
            encryption::decrypt(&content, &old_password)?
        } else {
            content
        };
        let envelope = encryption::encrypt(&plaintext, &new_password)?;
        notes::write_atomic(&path, &envelope).map_err(|e| e.to_string())?;
        // Result is an encrypted envelope; reindex so the search index
        // reflects encrypted (title-only) state — important when setting a
        // password on a previously-plain note, so its body stops showing up
        // in results.
        if let Err(e) = index.upsert(&path) {
            eprintln!("change_note_password: index upsert failed for {path}: {e}");
        }
        embeddings.enqueue_path(std::path::PathBuf::from(&path));
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn find_backlinks(
    path: String,
    state: tauri::State<AppState>,
) -> Vec<backlinks::BacklinkInfo> {
    state.backlinks.for_path(&path)
}

/// Notes that mention this note's title in prose without a [[wikilink]].
/// Index-backed + off-thread so it stays snappy on large vaults: the
/// tantivy index narrows the corpus to candidate notes containing the
/// title, and the precise word-boundary scan runs on a blocking worker
/// so the UI thread never stalls.
#[tauri::command]
async fn find_unlinked_mentions(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<mentions::UnlinkedMention>, String> {
    let Some(title) = mentions::title_for(&path) else {
        return Ok(Vec::new());
    };
    let candidates = state.index.notes_containing(&title).unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || mentions::find(&path, &title, &candidates))
        .await
        .map_err(|e| e.to_string())?;
    Ok(result)
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
    // Rewrites source_path on disk — vault-only, like every other write.
    ensure_in_vault(&source_path)?;
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
    // The export pipeline reads the source file (and, with append_links,
    // its link targets) — same vault-only rule as read_note. Without this,
    // export_as_string(path, "md") was an arbitrary-file-read primitive.
    ensure_in_vault(&path)?;
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
    ensure_in_vault(&path)?;
    // The destination is user-chosen via the native save dialog, so any
    // directory is fine — but the extension must match the format. This
    // keeps a compromised webview from using export as a write-anything
    // primitive (e.g. dropping a .bat into the Startup folder).
    let dest_ext = std::path::Path::new(&dest_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let ext_ok = match format.as_str() {
        "md" => dest_ext == "md",
        "txt" => dest_ext == "txt",
        "html" => dest_ext == "html" || dest_ext == "htm",
        "epub" => dest_ext == "epub",
        _ => false,
    };
    if !ext_ok {
        return Err(format!(
            "destination extension .{dest_ext} doesn't match format {format}"
        ));
    }
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
    notes::write_atomic_bytes(&dest_path, &bytes).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn count_wikilink_targets(path: String) -> Result<usize, String> {
    ensure_in_vault(&path)?;
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (_fm, body) = frontmatter::split(&raw);
    let targets = backlinks::resolved_targets_in(body);
    let unique: std::collections::HashSet<&str> =
        targets.iter().map(|(_, p)| p.as_str()).collect();
    Ok(unique.len())
}

#[tauri::command]
fn suggest_wikilinks(path: String) -> Result<Vec<link_suggestions::LinkSuggestion>, String> {
    ensure_in_vault(&path)?;
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(link_suggestions::suggest_for_note(&content, &path))
}

#[tauri::command]
async fn suggest_wikilinks_ai(
    path: String,
) -> Result<Vec<link_suggestions::LinkSuggestion>, String> {
    // Reads the file AND ships its contents to the configured LLM — the
    // strictest possible reason to keep it vault-scoped.
    ensure_in_vault(&path)?;
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    // AI is disabled for encrypted notes: the file is ciphertext, and the user
    // must decrypt it first (M2). Refuse rather than ship the envelope to a model.
    if crate::encryption::is_encrypted(&content) {
        return Err("AI is disabled for encrypted notes; decrypt the note first".into());
    }
    // Strip our markup before sending to the model — same hygiene as the
    // completion paths. We don't want the AI fixating on existing tags or
    // wikilinks instead of finding fresh entities.
    let clean = tags::strip_tags_for_ai(&content);
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = api_key_for_call(provider)?;
    let model = cfg.model_for(provider);
    let entities = ai::dispatch_propose_entities(provider, &key, &model, &clean).await?;
    Ok(link_suggestions::build_entity_suggestions(&content, &entities))
}

#[tauri::command]
async fn find_related(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<embeddings::RelatedNote>, String> {
    // Top 5 above 0.55 cosine similarity. Threshold trades recall for
    // noise: too low and you get unrelated notes, too high and "related"
    // is empty for most notes.
    //
    // The KNN probe hits the per-vault sqlite DB (blocking) — run it on a
    // worker so the IPC thread stays free. State can't move into the
    // closure; clone the Arc first. Frontend still receives the same
    // RelatedNote list (Result is transparent: Ok unwraps to the array).
    let embeddings = state.embeddings.clone();
    tauri::async_runtime::spawn_blocking(move || embeddings.find_related(&path, 5, 0.55))
        .await
        .map_err(|e| e.to_string())
}

/// Semantic search over the active vault — powers the `~concept` search
/// mode. Embeds the query and returns the nearest notes as NoteSummary
/// (so the sidebar renders them like any other result set). Falls back
/// to an empty list (handled as "no matches") on any error so a missing
/// API/model never breaks the search bar.
#[tauri::command]
async fn semantic_search(
    query: String,
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<notes::NoteSummary>, String> {
    // search_text embeds the query (model inference) + runs a sqlite KNN
    // probe, then we re-read the vault to build summaries — all blocking.
    // Move it onto a worker. State can't cross the closure boundary; clone
    // the embeddings Arc, and AppHandle is Clone + Send so it moves too.
    let embeddings = state.embeddings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let hits = embeddings.search_text(&query, 50, &app_handle)?;
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
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "The Orphanage" — notes adrift from the link graph: no resolving
/// outgoing wikilinks and no backlinks. Powers the `is:orphan` report.
/// Returned in modified-descending order (same as the default listing).
#[tauri::command]
async fn list_orphans() -> Result<Vec<notes::NoteSummary>, String> {
    // Whole-vault link scan — cache-backed now, but still O(corpus); keep
    // it off the IPC thread.
    tauri::async_runtime::spawn_blocking(|| {
        let orphans: std::collections::HashSet<String> =
            backlinks::orphan_paths().into_iter().collect();
        notes::list_notes()
            .into_iter()
            .filter(|n| orphans.contains(&n.path))
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// "Near-duplicates" — notes that have at least one other note within
/// ~0.9 cosine similarity. Powers the `is:duplicate` report. The KNN
/// probe runs on a blocking worker so a large vault never stalls the UI.
/// Returned tightest-duplicate first (not modified order).
#[tauri::command]
async fn list_near_duplicates(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<notes::NoteSummary>, String> {
    let embeddings = state.embeddings.clone();
    let dupe_paths = tauri::async_runtime::spawn_blocking(move || {
        // 0.9 cosine ≈ "basically the same note". High enough to avoid
        // flagging merely-related notes (those live in the Linkbacks
        // "related" panel at a lower floor).
        embeddings.near_duplicate_paths(0.9)
    })
    .await
    .map_err(|e| e.to_string())?;

    // Map paths → summaries, preserving the similarity ranking.
    let order: std::collections::HashMap<String, usize> = dupe_paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.clone(), i))
        .collect();
    let mut out: Vec<notes::NoteSummary> = notes::list_notes()
        .into_iter()
        .filter(|n| order.contains_key(&n.path))
        .collect();
    out.sort_by_key(|n| *order.get(&n.path).unwrap_or(&usize::MAX));
    Ok(out)
}

#[tauri::command]
fn delete_note(path: String, state: tauri::State<AppState>) -> Result<(), String> {
    // Canonicalizing vault guard — same rule as read/save (the lexical
    // starts_with it replaces both missed symlink escapes and falsely
    // rejected case-variant / 8.3-short-name paths on Windows).
    ensure_in_vault(&path)?;
    let p = std::path::PathBuf::from(&path);
    std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    // Drop it from the search index now so a deleted note can't surface in
    // results before the watcher catches up. Log on failure (file is gone
    // regardless; watcher rebuild is the backstop).
    if let Err(e) = state.index.remove(&path) {
        eprintln!("delete_note: index remove failed for {path}: {e}");
    }
    state.embeddings.forget_path(&path);
    config::remove_pin(&path);
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
    ensure_in_vault(&path)?;
    let old_path = std::path::PathBuf::from(&path);
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

    // Case-insensitive collision check (defensive even though Win/Mac FS is
    // CI). Skip the file being renamed itself — otherwise a case-only
    // rename ("Note" -> "note") always collided with its own old name and
    // was impossible. fs::rename handles in-place case changes fine.
    if new_path != old_path {
        let want = format!("{}.md", sanitized).to_lowercase();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path() == old_path {
                    continue;
                }
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

    // Rewrite backlinks in every other .md file in the directory. Shared
    // with the external-rename cascade in the watcher.
    backlinks::cascade_wikilink_rename(&dir, &old_title, trimmed, &new_path);

    // Rewire embeddings: same content, different path. Cheap point update.
    state.embeddings.rename_path(&path, &new_path_str);
    // Move the note in the search index: drop the old path's document and
    // index it under the new path. (Other notes whose [[links]] the cascade
    // rewrote are refreshed by the watcher rebuild.) Log on failure — the
    // rename already succeeded on disk.
    if let Err(e) = state.index.remove(&path) {
        eprintln!("rename_note: index remove failed for {path}: {e}");
    }
    if let Err(e) = state.index.upsert(&new_path_str) {
        eprintln!("rename_note: index upsert failed for {new_path_str}: {e}");
    }
    // Keep a pin attached to the renamed file.
    config::repoint_pin(&path, &new_path_str);

    // Notify the frontend immediately so it doesn't have to wait for the
    // debounced watcher event (file watcher fires too, but coalesces).
    let _ = app_handle.emit("notes_changed", ());

    Ok(new_path_str)
}

#[tauri::command]
fn create_note(title: String, state: tauri::State<AppState>) -> Result<String, String> {
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
    notes::write_atomic(&path, "").map_err(|e| e.to_string())?;
    let path_str = path.to_string_lossy().to_string();
    // Index the new (empty) note immediately so it's searchable — and so
    // `empty:true` reports surface it — without waiting for the watcher.
    // Log on failure; the file exists regardless and the watcher backstops.
    if let Err(e) = state.index.upsert(&path_str) {
        eprintln!("create_note: index upsert failed for {path_str}: {e}");
    }
    Ok(path_str)
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

/// Fetch the API key for an AI call. Providers that don't require one
/// (LM Studio — a local server) fall back to an empty key instead of
/// failing, so a keyless endpoint just works; a key the user DID store
/// (auth proxy in front of the endpoint) is still sent.
pub(crate) fn api_key_for_call(provider: providers::Provider) -> Result<String, String> {
    match secrets::get_api_key_for(provider.id()) {
        Ok(k) if !k.is_empty() => Ok(k),
        _ if !provider.requires_key() => Ok(String::new()),
        Ok(_) => Err(format!("empty API key for {}", provider.label())),
        Err(e) => Err(format!("no API key for {}: {e:?}", provider.label())),
    }
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
    // Keyless providers (LM Studio) get an empty key — this doubles as
    // the endpoint connectivity test.
    let key = api_key_for_call(provider)?;
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

/// Set (or clear, with an empty string) the endpoint override for an
/// OpenAI-compat provider. How LM Studio gets pointed at a LAN or
/// Tailscale host instead of localhost.
#[tauri::command]
fn set_provider_base_url(provider: providers::Provider, base_url: String) -> Result<(), String> {
    let trimmed = base_url.trim().trim_end_matches('/').to_string();
    if !trimmed.is_empty()
        && !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
    {
        return Err("endpoint must start with http:// or https://".into());
    }
    let mut cfg = config::load_for_update()?;
    if trimmed.is_empty() {
        cfg.provider_base_urls.remove(provider.id());
    } else {
        cfg.provider_base_urls
            .insert(provider.id().to_string(), trimmed);
    }
    config::save(&cfg).map_err(|e| e.to_string())
}

/// Toggle "skip thinking" for LM Studio calls (appends Qwen's /no_think
/// soft switch to prompts — see `ai::no_think_suffix`).
#[tauri::command]
fn set_lmstudio_no_think(enabled: bool) -> Result<(), String> {
    let mut cfg = config::load_for_update()?;
    cfg.lmstudio_no_think = enabled;
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
    /// False for local servers (LM Studio) — the key row becomes optional.
    requires_key: bool,
    /// Effective compat endpoint (override or default); None for Anthropic.
    base_url: Option<String>,
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
            requires_key: p.requires_key(),
            base_url: cfg.base_url_for(p),
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

/// Flag an in-flight AI stream as cancelled. The pumps poll between
/// chunks and close the connection — the user dismissed the ghost or
/// superseded the request, so the provider should stop generating
/// (and billing) immediately.
#[tauri::command]
fn cancel_ai_stream(stream_id: u64) {
    ai::cancel_stream(stream_id);
}

#[tauri::command]
async fn complete_text_streaming(
    before: String,
    after: String,
    direction: Option<String>,
    stream_id: Option<u64>,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    if before.trim().is_empty() && after.trim().is_empty() {
        return Ok(());
    }
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = api_key_for_call(provider)?;
    let model = cfg.model_for(provider);
    let dir = direction.unwrap_or_default();
    let result =
        ai::dispatch_stream_completion(provider, &key, &model, &before, &after, &dir, stream_id, |text| {
            let _ = on_chunk.send(text.to_string());
        })
        .await;
    ai::clear_cancel(stream_id);
    result
}

#[tauri::command]
async fn rewrite_text_streaming(
    before: String,
    selected: String,
    after: String,
    direction: Option<String>,
    stream_id: Option<u64>,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    if selected.trim().is_empty() {
        return Ok(());
    }
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = api_key_for_call(provider)?;
    let model = cfg.model_for(provider);
    let dir = direction.unwrap_or_default();
    let result = ai::dispatch_stream_rewrite(
        provider, &key, &model, &before, &selected, &after, &dir, stream_id,
        |text| {
            let _ = on_chunk.send(text.to_string());
        },
    )
    .await;
    ai::clear_cancel(stream_id);
    result
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
    stream_id: Option<u64>,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    if body.trim().is_empty() {
        return Err("nothing to brew — the note is empty.".into());
    }
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = api_key_for_call(provider)?;
    let model = cfg.model_for(provider);
    let payload = ai_payload_with_title(&title, &body);
    let result = ai::dispatch_stream_brew(provider, &key, &model, &payload, stream_id, |text| {
        let _ = on_chunk.send(text.to_string());
    })
    .await;
    ai::clear_cancel(stream_id);
    result
}

/// Two-pane prompting: stream a completion for a RAW prompt — the
/// caller-concatenated editor contents — with no system prompt and no
/// scaffolding. The frontend builds `prompt` as [other pane] + [focused
/// pane], so notes act as reusable pre-prompts.
#[tauri::command]
async fn prompt_streaming(
    prompt: String,
    stream_id: Option<u64>,
    on_chunk: tauri::ipc::Channel<String>,
) -> Result<(), String> {
    if prompt.trim().is_empty() {
        return Ok(());
    }
    let cfg = config::load();
    let provider = cfg.active_provider;
    let key = api_key_for_call(provider)?;
    let model = cfg.model_for(provider);
    let result = ai::dispatch_stream_raw(provider, &key, &model, &prompt, stream_id, |text| {
        let _ = on_chunk.send(text.to_string());
    })
    .await;
    ai::clear_cancel(stream_id);
    result
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
async fn set_notes_dir(
    path: Option<String>,
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    // Since vaults landed (v0.3.1), the registry — not config.notes_dir —
    // decides where notes live, so this command used to be dead UI: it
    // wrote a field nothing read and the Settings picker silently snapped
    // back. It now repoints the ACTIVE vault at the chosen folder (None =
    // reset to ~/malt) and runs the same reindex sequence as a vault
    // switch. The legacy config field is still written so a downgraded
    // malt finds the notes too.
    let updated = vaults::set_active_path(path.clone())?;
    if let Ok(mut cfg) = config::load_for_update() {
        cfg.notes_dir = path.and_then(|p| {
            let t = p.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        });
        let _ = config::save(&cfg);
    }
    let new_dir = notes::notes_dir();
    notes::repoint_watcher(&state.watcher, &new_dir);
    if let Err(e) = state.index.rebuild() {
        eprintln!("set_notes_dir: index rebuild failed: {e}");
    }
    state.backlinks.rebuild();
    if let Err(e) = state.embeddings.repoint() {
        eprintln!("set_notes_dir: embeddings repoint failed: {e}");
    }
    state.embeddings.enqueue_dir();
    let _ = tauri::Emitter::emit(&app_handle, "notes_changed", ());
    let _ = tauri::Emitter::emit(&app_handle, "vault_changed", &updated);
    Ok(new_dir.to_string_lossy().to_string())
}

#[tauri::command]
fn reveal_notes_dir(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    // Open the active vault's folder in the OS file manager (showing the .md
    // files inside). Routed through the opener plugin, which uses the native
    // shell APIs rather than spawning `explorer.exe` / `open` from inside the
    // app process — the latter was unreliable (the folder often never
    // appeared, especially for vaults outside the default location).
    let dir = notes::notes_dir();
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_note(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    ensure_in_vault(&path)?;
    // Reveal the note in its containing folder, selected. The opener plugin
    // uses each platform's native reveal-and-select (Windows Shell COM
    // SHOpenFolderAndSelectItems, macOS Finder reveal, Linux file-manager
    // D-Bus) instead of a flaky `explorer /select,` / `open -R` spawn.
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
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
    let updated = vaults::switch(index as usize)?;
    let new_dir = notes::notes_dir();
    notes::sweep_tmp_files(&new_dir);
    // The watcher tracks the dir it actually observes, so repointing
    // doesn't depend on re-deriving "old" from the (already-flipped)
    // registry.
    notes::repoint_watcher(&state.watcher, &new_dir);
    // Reindex everything for the new vault. The note cache detects the
    // dir change and rescans; backlinks + search index rebuild from it;
    // embeddings repoint to the new vault's own DB then re-queue. Errors
    // here are LOGGED, not returned — the switch already happened, every
    // subsystem self-heals via the watcher, and bailing midway used to
    // leave embeddings pointed at the old vault's DB.
    if let Err(e) = state.index.rebuild() {
        eprintln!("switch_vault: index rebuild failed: {e}");
    }
    state.backlinks.rebuild();
    if let Err(e) = state.embeddings.repoint() {
        eprintln!("switch_vault: embeddings repoint failed: {e}");
    }
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
    let updated = vaults::remove(index as usize)?;
    let new_dir = notes::notes_dir();
    notes::repoint_watcher(&state.watcher, &new_dir);
    // Removing a vault may have shifted the active one. Repoint the
    // embeddings connection FIRST — if the removed vault was active, its
    // DB is still held open, and deleting an open sqlite file fails on
    // Windows (orphaning it). Then reclaim the file.
    if let Err(e) = state.embeddings.repoint() {
        eprintln!("remove_vault: embeddings repoint failed: {e}");
    }
    if let Some(p) = removed_path {
        embeddings::delete_db_for(&p);
    }
    if let Err(e) = state.index.rebuild() {
        eprintln!("remove_vault: index rebuild failed: {e}");
    }
    state.backlinks.rebuild();
    state.embeddings.enqueue_dir();
    let _ = tauri::Emitter::emit(&app_handle, "notes_changed", ());
    let _ = tauri::Emitter::emit(&app_handle, "vault_changed", &updated);
    Ok(updated)
}

#[tauri::command]
fn duplicate_note(path: String) -> Result<String, String> {
    ensure_in_vault(&path)?;
    let src = std::path::PathBuf::from(&path);
    if !src.is_file() {
        return Err(format!("file not found: {}", src.display()));
    }
    let dir = notes::notes_dir();
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
- **AI help** lives on `Ctrl+;` (`Cmd+;` on Mac). Bare press continues from the cursor. With a selection, it rewrites.
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

- Type `#anything` inline and it automatically relocates to a hidden line at the bottom of the file on save. You see pills above the editor instead.
- Hover a pill, click the `×` to remove it. Right-click for the full menu (filter, remove, promote-to-vocab).
- Edit your starter vocabulary in Settings → tags & queries.

## Saved searches

- Type a query like `tag:fleeting modified:<7d` and hit `Ctrl+S` to save it.
- Activate any saved search via `Ctrl+1` through `Ctrl+9`.

## AI moves (bring your own key — Anthropic, OpenAI, DeepSeek, Grok, or Gemini — Settings → ai)

- `Ctrl+;` at end of doc → continue the writing
- `Ctrl+;` with text selected → rewrite that text, unpacking generalities
- `Ctrl+;` in the middle of a doc → bridge the gap
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
fn get_tag_styles() -> Vec<config::TagStyle> {
    config::load().tag_styles
}

/// Persist per-tag flair. Tag names are canonicalized (so "#Element",
/// "Element", and "element" collapse to one style) and entries with an
/// empty tag — or no icon AND no color — are dropped as no-ops.
#[tauri::command]
fn set_tag_styles(styles: Vec<config::TagStyle>) -> Result<Vec<config::TagStyle>, String> {
    let mut cleaned: Vec<config::TagStyle> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in styles {
        let tag = tags::canonicalize(&s.tag);
        if tag.is_empty() {
            continue;
        }
        if s.icon.trim().is_empty() && s.color.trim().is_empty() {
            continue;
        }
        // First style wins for a given tag (preserves user ordering as
        // priority — the frontend applies the first matching style's
        // color as the card accent).
        if !seen.insert(tag.clone()) {
            continue;
        }
        cleaned.push(config::TagStyle {
            tag,
            icon: s.icon.trim().to_string(),
            color: s.color.trim().to_string(),
        });
    }
    let mut cfg = config::load();
    cfg.tag_styles = cleaned.clone();
    config::save(&cfg).map_err(|e| e.to_string())?;
    Ok(cleaned)
}

#[tauri::command]
fn get_pinned() -> Vec<String> {
    config::load().pinned_paths
}

#[tauri::command]
fn toggle_pin(path: String) -> Result<Vec<String>, String> {
    config::toggle_pin(&path).map_err(|e| e.to_string())
}

/// Move a note's `.md` file into another vault's folder. Cross-vault
/// moves are inherently lossy for links (vaults are siloed by design), so
/// this just relocates the file, drops the note's embedding from this
/// vault, and clears any pin. Returns the new absolute path.
#[tauri::command]
fn move_note_to_vault(
    path: String,
    target_index: u32,
    state: tauri::State<AppState>,
) -> Result<String, String> {
    ensure_in_vault(&path)?;
    let src = std::path::PathBuf::from(&path);
    if !src.is_file() {
        return Err(format!("file not found: {}", src.display()));
    }
    let cur_dir = notes::notes_dir();
    let vaults = vaults::load();
    let target = vaults
        .vaults
        .get(target_index as usize)
        .ok_or("target vault not found")?;
    let target_dir = std::path::PathBuf::from(&target.path);
    if target_dir == cur_dir {
        return Err("that's the current vault".into());
    }
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    let file_name = src.file_name().ok_or("bad filename")?;
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();
    // Collision-safe destination in the target vault.
    let mut dest = target_dir.join(file_name);
    let mut counter = 2;
    while dest.exists() {
        dest = target_dir.join(format!("{}-{}.md", stem, counter));
        counter += 1;
        if counter > 1000 {
            return Err("too many name collisions in target vault".into());
        }
    }
    // Move: atomic rename on the same volume, else copy + remove.
    if std::fs::rename(&src, &dest).is_err() {
        std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
        std::fs::remove_file(&src).map_err(|e| e.to_string())?;
    }
    // The note left the active vault — drop it from the search index +
    // note cache (it lingered in both until the watcher caught up), its
    // embedding, and any pin.
    if let Err(e) = state.index.remove(&path) {
        eprintln!("move_note_to_vault: index remove failed for {path}: {e}");
    }
    state.embeddings.forget_path(&path);
    config::remove_pin(&path);
    Ok(dest.to_string_lossy().to_string())
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
async fn list_all_tags() -> Result<Vec<TagCount>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
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
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct TagCount {
    name: String,
    count: u32,
}

/// Tags that co-occur with `tag` across the corpus, ranked by how often
/// they share a note with it. Powers the "often with" chips shown when
/// filtering by a single tag. `tag` is canonicalized first so "Draft",
/// "#draft", and "draft" all resolve the same.
#[tauri::command]
async fn tag_cooccurrence(tag: String) -> Result<Vec<TagCount>, String> {
    let target = tags::canonicalize(&tag);
    if target.is_empty() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let mut counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        for note in notes::list_notes() {
            if !note.tags.iter().any(|t| t == &target) {
                continue;
            }
            for t in &note.tags {
                if t != &target {
                    *counts.entry(t.clone()).or_insert(0) += 1;
                }
            }
        }
        let mut out: Vec<TagCount> = counts
            .into_iter()
            .map(|(name, count)| TagCount { name, count })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        out.truncate(8);
        out
    })
    .await
    .map_err(|e| e.to_string())
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
            // Reclaim staging files leaked by a hard crash mid-save.
            notes::sweep_tmp_files(&notes::notes_dir());
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
            list_orphans,
            list_near_duplicates,
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
            set_provider_base_url,
            set_lmstudio_no_think,
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
            get_tag_styles,
            set_tag_styles,
            get_pinned,
            toggle_pin,
            move_note_to_vault,
            list_all_tags,
            tag_cooccurrence,
            complete_text_streaming,
            rewrite_text_streaming,
            brew_streaming,
            prompt_streaming,
            cancel_ai_stream,
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
