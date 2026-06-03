// Semantic similarity over notes. Architecture:
//   - fastembed-rs runs a local ONNX model (BGE-small-en-v1.5 by default,
//     384-dim, ~33MB downloaded once to ~/.cache/fastembed/).
//   - sqlite-vec stores embeddings in a `vec0` virtual table, with a parallel
//     `embed_meta` table keyed by note path for hash-based dedup.
//   - A worker thread polls a queue of paths to embed; the file watcher and
//     save_note hook into this so we incrementally re-embed only what changed.
//   - find_related(path, k) runs sqlite-vec's KNN MATCH operator.

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const TICK: Duration = Duration::from_millis(500);
const EMBED_DIM: usize = 384;
/// Cap per-note text fed to the embedder. BGE-small truncates at ~512
/// tokens anyway; this just avoids reading enormous files into memory.
const MAX_TEXT_CHARS: usize = 12000;

pub struct EmbedIndex {
    queue: Mutex<HashSet<PathBuf>>,
    db: Mutex<Connection>,
    model: Mutex<Option<TextEmbedding>>,
    /// When the last model-load attempt failed. Used to back off retries
    /// so a broken/offline model download doesn't re-attempt — and re-emit
    /// the "indexing…" status — on every single enqueue (which made the
    /// status pill flash on every keystroke via autosave re-embeds).
    last_load_fail: Mutex<Option<Instant>>,
}

/// Don't re-attempt a failed model load more than once per this window.
const LOAD_RETRY_BACKOFF: Duration = Duration::from_secs(120);

#[derive(Serialize, Clone, Debug)]
pub struct RelatedNote {
    pub path: String,
    pub title: String,
    pub snippet: String,
    pub similarity: f32,
}

// `distance_metric=cosine` is load-bearing: without it sqlite-vec defaults
// to L2, but every consumer here computes `sim = 1 - distance/2` and gates on
// cosine thresholds (related 0.55, near-dup 0.9). With cosine, `distance` is a
// true cosine distance in [0, 2] and that math is correct. Changing the metric
// is a schema change — see SCHEMA_VERSION / the migration in open_active_db.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS embed_meta (
        path TEXT PRIMARY KEY,
        content_hash TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        vec_rowid INTEGER NOT NULL UNIQUE
    );
    CREATE VIRTUAL TABLE IF NOT EXISTS vec_notes USING vec0(embedding float[384] distance_metric=cosine);";

/// Bump this whenever SCHEMA changes in a way that invalidates existing rows
/// (e.g. the L2→cosine distance-metric switch in v1). `open_active_db` drops
/// and recreates the tables when an existing DB's `user_version` is below
/// this, then stamps the DB with the new version. Embeddings are derived and
/// cheap to recompute, so a full drop is acceptable — `enqueue_dir` at
/// startup/vault-switch repopulates everything.
const SCHEMA_VERSION: i64 = 1;

/// Open (creating if needed) the embedding DB for the *currently active
/// vault*. Each vault gets its own file so semantic results are fully
/// siloed — no cross-vault leakage, and removing a vault can reclaim its
/// embeddings by deleting one file.
fn open_active_db() -> Result<Connection, String> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    // WAL + NORMAL synchronous: better write throughput, durability still
    // fine for derived data we can re-embed at any time.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");

    // Schema migration. `CREATE ... IF NOT EXISTS` alone would keep an old
    // L2 vec_notes table on existing installs (the L2→cosine switch in v1),
    // silently miscalibrating every cosine threshold. So if this DB predates
    // the current SCHEMA_VERSION, drop BOTH tables and recreate. embed_meta
    // must go too: it's the content-hash cache, and keeping it would make the
    // worker think notes are already embedded and skip re-populating the new
    // (cosine) vec_notes. Fresh installs report version 0 with no tables, so
    // the DROP IF EXISTS is a harmless no-op and they land at v1 cleanly too.
    let db_version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap_or(0);
    if db_version < SCHEMA_VERSION {
        conn.execute_batch(
            "DROP TABLE IF EXISTS vec_notes;
             DROP TABLE IF EXISTS embed_meta;",
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    if db_version < SCHEMA_VERSION {
        // Stamp the version only after the (cosine) tables exist, so an error
        // above leaves the DB un-stamped and the migration retried next open.
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| e.to_string())?;
    }
    Ok(conn)
}

impl EmbedIndex {
    pub fn new() -> Result<Arc<Self>, String> {
        // Register sqlite-vec as a SQLite auto-extension. Must happen before
        // any Connection::open call. Done once at process start.
        register_sqlite_vec_extension();

        let conn = open_active_db()?;
        Ok(Arc::new(Self {
            queue: Mutex::new(HashSet::new()),
            db: Mutex::new(conn),
            model: Mutex::new(None),
            last_load_fail: Mutex::new(None),
        }))
    }

    /// Repoint the connection at the active vault's DB. Called on vault
    /// switch / removal (right before enqueue_dir re-embeds the new
    /// vault). Also clears the in-flight queue so we don't process paths
    /// from the vault we just left.
    pub fn repoint(&self) -> Result<(), String> {
        let conn = open_active_db()?;
        {
            let mut q = self.queue.lock().expect("queue lock");
            q.clear();
        }
        let mut guard = self.db.lock().expect("db lock");
        *guard = conn;
        Ok(())
    }

    pub fn enqueue_path(&self, path: PathBuf) {
        if is_md_file(&path) {
            self.queue.lock().expect("queue lock").insert(path);
        }
    }

    /// Walk the current notes dir, queueing every .md file for processing.
    /// The worker's hash check ensures we only re-embed actually-changed
    /// files, so calling this at startup is cheap.
    pub fn enqueue_dir(&self) {
        let dir = crate::notes::notes_dir();
        let mut q = self.queue.lock().expect("queue lock");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_md_file(&path) {
                    q.insert(path);
                }
            }
        }
    }

    /// Drop a note's embedding (after a delete on disk). Failures are
    /// silently tolerated — stale rows just take up space.
    pub fn forget_path(&self, path: &str) {
        let conn = self.db.lock().expect("db lock");
        let row: Result<i64, _> = conn.query_row(
            "SELECT vec_rowid FROM embed_meta WHERE path = ?",
            params![path],
            |r| r.get(0),
        );
        if let Ok(rowid) = row {
            let _ = conn.execute("DELETE FROM vec_notes WHERE rowid = ?", params![rowid]);
            let _ = conn.execute("DELETE FROM embed_meta WHERE path = ?", params![path]);
        }
    }

    /// Rewire an embedding to a new path (used after rename_note). We don't
    /// need to recompute the embedding — the content didn't change, just the
    /// filename.
    pub fn rename_path(&self, old: &str, new_path: &str) {
        let conn = self.db.lock().expect("db lock");
        let _ = conn.execute(
            "UPDATE embed_meta SET path = ?1 WHERE path = ?2",
            params![new_path, old],
        );
    }

    /// Return the top-N most similar notes to `path`, excluding `path`
    /// itself and anything below the similarity floor. Returns [] if the
    /// query note has no embedding yet, or sqlite-vec isn't loaded.
    pub fn find_related(
        &self,
        path: &str,
        limit: usize,
        sim_floor: f32,
    ) -> Vec<RelatedNote> {
        let conn = self.db.lock().expect("db lock");

        // Look up the query embedding via embed_meta.vec_rowid → vec_notes.embedding.
        let query_blob: Vec<u8> = match conn.query_row(
            "SELECT v.embedding FROM vec_notes v
             JOIN embed_meta m ON v.rowid = m.vec_rowid
             WHERE m.path = ?1",
            params![path],
            |r| r.get::<_, Vec<u8>>(0),
        ) {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };

        // The DB is now per-vault (see open_active_db / repoint), so every
        // row already belongs to the active vault — no cross-vault filter
        // needed. Ask for k+1 to absorb the self-match.
        let k = limit + 1;
        let mut stmt = match conn.prepare(
            "SELECT m.path, v.distance
             FROM vec_notes v
             JOIN embed_meta m ON v.rowid = m.vec_rowid
             WHERE v.embedding MATCH ?1 AND k = ?2
             ORDER BY v.distance",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut hits: Vec<(String, f32)> = match stmt.query_map(
            params![query_blob, k as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?)),
        ) {
            Ok(it) => it.filter_map(Result::ok).collect(),
            Err(_) => return Vec::new(),
        };
        // stmt + conn drop naturally at the end of this fn's scope.

        // sqlite-vec returns cosine *distance* (0 = identical, 2 = opposite).
        // similarity = 1 - distance/2  ∈ [0, 1].
        hits.retain(|(p, _)| p != path);
        let mut out: Vec<RelatedNote> = Vec::new();
        for (p, dist) in hits.into_iter().take(limit) {
            let sim = 1.0 - (dist / 2.0);
            if sim < sim_floor {
                break;
            }
            let title = title_for(&p);
            let snippet = snippet_for(&p);
            out.push(RelatedNote {
                path: p,
                title,
                snippet,
                similarity: sim,
            });
        }
        out
    }

    /// Semantic search: embed an arbitrary query string and return the
    /// active vault's notes nearest to it. Powers the `~concept` search
    /// mode. Loads the model on first use (may emit the embedding-status
    /// "loading" banner). Returns ranked RelatedNote (reusing that shape
    /// — `similarity` doubles as the relevance score).
    pub fn search_text(
        &self,
        query: &str,
        limit: usize,
        app_handle: &AppHandle,
    ) -> Result<Vec<RelatedNote>, String> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        if !self.load_model(app_handle) {
            return Err("semantic model unavailable".into());
        }
        let embedding: Vec<f32> = {
            let mut m = self.model.lock().expect("model lock");
            let model = m.as_mut().ok_or("model gone after load")?;
            let mut out = model
                .embed(vec![q.to_string()], None)
                .map_err(|e| format!("embed failed: {e:?}"))?;
            out.pop().ok_or("embed returned empty")?
        };
        if embedding.len() != EMBED_DIM {
            return Err("embedding dim mismatch".into());
        }
        let query_blob = f32_slice_to_le_bytes(&embedding);

        let conn = self.db.lock().expect("db lock");
        let mut stmt = conn
            .prepare(
                "SELECT m.path, v.distance
                 FROM vec_notes v
                 JOIN embed_meta m ON v.rowid = m.vec_rowid
                 WHERE v.embedding MATCH ?1 AND k = ?2
                 ORDER BY v.distance",
            )
            .map_err(|e| e.to_string())?;
        let hits: Vec<(String, f32)> = stmt
            .query_map(params![query_blob, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();

        Ok(hits
            .into_iter()
            .map(|(p, dist)| {
                let sim = 1.0 - (dist / 2.0);
                let title = title_for(&p);
                let snippet = snippet_for(&p);
                RelatedNote {
                    path: p,
                    title,
                    snippet,
                    similarity: sim,
                }
            })
            .collect())
    }

    /// Paths of notes that have at least one **near-duplicate** — another
    /// note within `sim_floor` cosine similarity (≈0.9 = "basically the
    /// same note"). Surfaces accidental copies, re-typed notes, and
    /// heavily-overlapping drafts. Probes each note's nearest neighbor via
    /// sqlite-vec KNN; that's N small queries, so callers run it off-thread
    /// (spawn_blocking) and only on explicit request. Returned tightest-
    /// duplicate first.
    pub fn near_duplicate_paths(&self, sim_floor: f32) -> Vec<String> {
        let conn = self.db.lock().expect("db lock");

        // Snapshot (path, rowid) for every embedded note. Bind the
        // collected Vec to a name before the block ends so the MappedRows
        // temporary drops before `stmt` does (avoids an E0597 borrow).
        let rows: Vec<(String, i64)> = {
            let mut stmt = match conn.prepare("SELECT path, vec_rowid FROM embed_meta") {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let collected: Vec<(String, i64)> = match stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            {
                Ok(it) => it.filter_map(Result::ok).collect(),
                Err(_) => return Vec::new(),
            };
            collected
        };

        // Reused KNN probe: nearest 2 (self + best other).
        let mut probe = match conn.prepare(
            "SELECT m.path, v.distance
             FROM vec_notes v
             JOIN embed_meta m ON v.rowid = m.vec_rowid
             WHERE v.embedding MATCH ?1 AND k = 2
             ORDER BY v.distance",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let mut scored: Vec<(String, f32)> = Vec::new();
        for (path, rowid) in &rows {
            let blob: Vec<u8> = match conn.query_row(
                "SELECT embedding FROM vec_notes WHERE rowid = ?1",
                params![rowid],
                |r| r.get(0),
            ) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let hits: Vec<(String, f32)> = match probe.query_map(params![blob], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, f32>(1)?))
            }) {
                Ok(it) => it.filter_map(Result::ok).collect(),
                Err(_) => continue,
            };
            // Best neighbor that isn't the note itself.
            if let Some((_, dist)) = hits.into_iter().find(|(p, _)| p != path) {
                let sim = 1.0 - (dist / 2.0);
                if sim >= sim_floor {
                    scored.push((path.clone(), sim));
                }
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(p, _)| p).collect()
    }

    fn load_model(&self, app_handle: &AppHandle) -> bool {
        let mut m = self.model.lock().expect("model lock");
        if m.is_some() {
            return true;
        }
        // Back off after a recent failure. Without this, a broken/offline
        // model download re-attempts on *every* enqueue — and every autosave
        // enqueues — so the "indexing…" status pill flashed on each
        // keystroke. Skip silently (no emit) until the backoff elapses.
        {
            let fail = self.last_load_fail.lock().expect("load-fail lock");
            if let Some(t) = *fail {
                if t.elapsed() < LOAD_RETRY_BACKOFF {
                    return false;
                }
            }
        }
        // Frontend listens for this — drives the "preparing semantic index…"
        // banner. Emit before the (potentially slow) first-call download.
        let _ = app_handle.emit("embedding_status", "loading");
        match TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false),
        ) {
            Ok(model) => {
                *m = Some(model);
                *self.last_load_fail.lock().expect("load-fail lock") = None;
                let _ = app_handle.emit("embedding_status", "ready");
                true
            }
            Err(e) => {
                eprintln!("embeddings: model load failed: {e:?}");
                *self.last_load_fail.lock().expect("load-fail lock") = Some(Instant::now());
                let _ = app_handle.emit("embedding_status", "error");
                false
            }
        }
    }

    fn next(&self) -> Option<PathBuf> {
        let mut q = self.queue.lock().expect("queue lock");
        let next = q.iter().next().cloned();
        if let Some(p) = &next {
            q.remove(p);
        }
        next
    }

    fn process(&self, path: PathBuf, app_handle: &AppHandle) -> Result<bool, String> {
        if !path.is_file() {
            // File no longer exists — clean up if we still hold a row.
            let path_str = path.to_string_lossy().to_string();
            self.forget_path(&path_str);
            return Ok(false);
        }

        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        // Encrypted notes can't be embedded — ciphertext has no semantic
        // signal. Drop any prior row for this path so stale embeddings
        // don't leak into related-notes results, then bail.
        if crate::encryption::is_encrypted(&content) {
            let path_str = path.to_string_lossy().to_string();
            self.forget_path(&path_str);
            return Ok(false);
        }
        let (_fm, body) = crate::frontmatter::split(&content);
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let text: String = format!("{title}\n\n{body}")
            .chars()
            .take(MAX_TEXT_CHARS)
            .collect();
        let text_trimmed = text.trim();
        if text_trimmed.is_empty() {
            return Ok(false);
        }
        let hash = hash_str(text_trimmed);
        let path_str = path.to_string_lossy().to_string();

        // Hash-skip: nothing to do if we already embedded this content.
        {
            let conn = self.db.lock().expect("db lock");
            let prev_hash: Result<String, _> = conn.query_row(
                "SELECT content_hash FROM embed_meta WHERE path = ?",
                params![&path_str],
                |r| r.get(0),
            );
            if let Ok(h) = prev_hash {
                if h == hash {
                    return Ok(false);
                }
            }
        }

        if !self.load_model(app_handle) {
            return Err("model not available".into());
        }

        // Embed (synchronous; ~5-30ms per call on CPU depending on length).
        let embedding: Vec<f32> = {
            let mut m = self.model.lock().expect("model lock");
            let model = m.as_mut().ok_or("model gone after load")?;
            let mut out = model
                .embed(vec![text_trimmed.to_string()], None)
                .map_err(|e| format!("embed failed: {e:?}"))?;
            out.pop().ok_or("embed returned empty")?
        };
        if embedding.len() != EMBED_DIM {
            return Err(format!(
                "embedding dim mismatch: got {}, expected {EMBED_DIM}",
                embedding.len()
            ));
        }
        let blob = f32_slice_to_le_bytes(&embedding);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Insert or update — atomic within a transaction.
        let mut conn = self.db.lock().expect("db lock");
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let existing_rowid: Result<i64, _> = tx.query_row(
            "SELECT vec_rowid FROM embed_meta WHERE path = ?",
            params![&path_str],
            |r| r.get(0),
        );
        match existing_rowid {
            Ok(rowid) => {
                tx.execute(
                    "UPDATE vec_notes SET embedding = ?1 WHERE rowid = ?2",
                    params![blob, rowid],
                )
                .map_err(|e| e.to_string())?;
                tx.execute(
                    "UPDATE embed_meta SET content_hash = ?1, updated_at = ?2 WHERE path = ?3",
                    params![hash, now, &path_str],
                )
                .map_err(|e| e.to_string())?;
            }
            Err(_) => {
                tx.execute(
                    "INSERT INTO vec_notes(embedding) VALUES (?1)",
                    params![blob],
                )
                .map_err(|e| e.to_string())?;
                let rowid = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO embed_meta(path, content_hash, updated_at, vec_rowid)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![&path_str, hash, now, rowid],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
}

pub fn start(index: Arc<EmbedIndex>, app_handle: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(TICK);
        let Some(path) = index.next() else { continue };
        match index.process(path, &app_handle) {
            Ok(true) => {
                let _ = app_handle.emit("related_changed", ());
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("embeddings: {e}");
            }
        }
    });
}

/// Per-vault embedding DB path: `…/malt/embeddings/vault-<hash>.db`,
/// where <hash> is a stable hash of the active vault's absolute path.
/// `db_file_for(path)` is the same computation for an arbitrary vault
/// (used to delete a removed vault's DB).
fn db_path() -> PathBuf {
    db_file_for(&crate::vaults::active_path().to_string_lossy())
}

pub fn db_file_for(vault_path: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    p.push("embeddings");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    vault_path.hash(&mut h);
    p.push(format!("vault-{:016x}.db", h.finish()));
    p
}

/// Delete a vault's embedding DB (and its WAL/SHM sidecars). Best-effort
/// — used when a vault is removed from the registry so we don't leave
/// orphaned embedding files behind. Errors are ignored.
pub fn delete_db_for(vault_path: &str) {
    let base = db_file_for(vault_path);
    for suffix in ["", "-wal", "-shm"] {
        let p = if suffix.is_empty() {
            base.clone()
        } else {
            PathBuf::from(format!("{}{}", base.to_string_lossy(), suffix))
        };
        let _ = std::fs::remove_file(p);
    }
}

fn is_md_file(path: &Path) -> bool {
    path.is_file()
        && path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase())
            == Some("md".to_string())
}

fn hash_str(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn f32_slice_to_le_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for &f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

fn title_for(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string()
}

fn snippet_for(path: &str) -> String {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let (_fm, body) = crate::frontmatter::split(&content);
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .or_else(|| body.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("")
        .chars()
        .take(100)
        .collect()
}

/// Register sqlite-vec as a SQLite auto-extension exactly once per process.
fn register_sqlite_vec_extension() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // sqlite3_auto_extension takes a function pointer with a specific
        // C signature; transmute is the standard incantation for SQLite
        // extensions. Documented in sqlite-vec README and rusqlite docs.
        type Init = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        let init: Init = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(init));
    });
}
