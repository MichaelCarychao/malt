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
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
}

#[derive(Serialize, Clone, Debug)]
pub struct RelatedNote {
    pub path: String,
    pub title: String,
    pub snippet: String,
    pub similarity: f32,
}

impl EmbedIndex {
    pub fn new() -> Result<Arc<Self>, String> {
        // Register sqlite-vec as a SQLite auto-extension. Must happen before
        // any Connection::open call. Done once at process start.
        register_sqlite_vec_extension();

        let db_path = db_path();
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        // WAL + NORMAL synchronous: better write throughput, durability still
        // fine for derived data we can re-embed at any time.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embed_meta (
                path TEXT PRIMARY KEY,
                content_hash TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                vec_rowid INTEGER NOT NULL UNIQUE
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS vec_notes USING vec0(embedding float[384]);",
        )
        .map_err(|e| e.to_string())?;

        Ok(Arc::new(Self {
            queue: Mutex::new(HashSet::new()),
            db: Mutex::new(conn),
            model: Mutex::new(None),
        }))
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

        // KNN: ask for k + 1 so we can drop the self-match cleanly.
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

    fn load_model(&self, app_handle: &AppHandle) -> bool {
        let mut m = self.model.lock().expect("model lock");
        if m.is_some() {
            return true;
        }
        // Frontend listens for this — drives the "preparing semantic index…"
        // banner. Emit before the (potentially slow) first-call download.
        let _ = app_handle.emit("embedding_status", "loading");
        match TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false),
        ) {
            Ok(model) => {
                *m = Some(model);
                let _ = app_handle.emit("embedding_status", "ready");
                true
            }
            Err(e) => {
                eprintln!("embeddings: model load failed: {e:?}");
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

fn db_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    p.push("embeddings.db");
    p
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
