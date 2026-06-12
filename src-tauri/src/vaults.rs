// Vault registry. A vault is just a named notes folder.
//
// malt started life with a single notes_dir (the `notes_dir` field
// on Config); v0.3.1 generalizes that to a list of named vaults, of
// which exactly one is active at a time. Switching vaults reroutes
// every file-reading path (notes::notes_dir → vaults::active_path)
// and triggers a full reindex; the watcher stays pointed at the new
// path because notes::notes_dir is checked on every event.
//
// Storage: ~/.config/malt/vaults.json — independent of the legacy
// config.json so users who downgrade still see their notes via
// notes_dir fallback.
//
// Migration: on first load, if vaults.json doesn't exist, we
// auto-create a single "default" vault pointing at whatever config's
// notes_dir resolves to (or ~/malt). The legacy notes_dir setting is
// left alone for backwards compatibility but is no longer read.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vault {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultsState {
    pub vaults: Vec<Vault>,
    pub active_index: usize,
}

impl Default for VaultsState {
    fn default() -> Self {
        Self {
            vaults: Vec::new(),
            active_index: 0,
        }
    }
}

fn registry_path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    let _ = std::fs::create_dir_all(&p);
    p.push("vaults.json");
    p
}

fn fallback_default_path() -> String {
    // Mirror the resolution logic in notes::default_dir: ~/malt, with
    // a sensible fallback if home_dir() is unavailable. Kept in sync
    // by hand — these two helpers should always agree.
    if let Some(home) = dirs::home_dir() {
        let mut p = home;
        p.push("malt");
        return p.to_string_lossy().to_string();
    }
    "./malt".to_string()
}

/// Seed a fresh single-vault state: the legacy config notes_dir if set
/// (so existing users don't see their notes "disappear" on upgrade),
/// else the OS default.
fn seeded_default() -> VaultsState {
    let legacy = crate::config::load().notes_dir;
    let path = legacy.unwrap_or_else(fallback_default_path);
    VaultsState {
        vaults: vec![Vault {
            name: "Default".to_string(),
            path,
        }],
        active_index: 0,
    }
}

/// Load the vault registry. Failure handling is deliberately asymmetric
/// (see config::JsonRead): a MISSING file is a first run — seed + persist
/// a default vault; a CORRUPT file is quarantined to a .bak first, then
/// seeded (the user's original bytes survive for manual recovery); an
/// UNREADABLE file (AV lock, transient IO error) returns an in-memory
/// default WITHOUT persisting — this used to `save()` unconditionally,
/// which let one transient read failure permanently overwrite the user's
/// entire vault registry with a single "Default" entry.
pub fn load() -> VaultsState {
    let mut state = match crate::config::read_json::<VaultsState>(&registry_path()) {
        crate::config::JsonRead::Parsed(s) => s,
        crate::config::JsonRead::Missing | crate::config::JsonRead::Quarantined => {
            let seeded = seeded_default();
            let _ = save(&seeded);
            seeded
        }
        crate::config::JsonRead::Unreadable => return seeded_default(),
    };
    if state.vaults.is_empty() {
        // Parsed fine but empty — a legitimate (if odd) on-disk state;
        // re-seed so the app always has somewhere to point.
        state = seeded_default();
        let _ = save(&state);
    }
    // Clamp active_index defensively — if the user hand-edited the
    // JSON and put a bad value, fall back to 0 rather than panicking.
    if state.active_index >= state.vaults.len() {
        state.active_index = 0;
    }
    state
}

/// Load for a read-modify-write cycle (add/switch/rename/remove). Errors
/// when the registry exists but can't be read right now — mutating a
/// default state and saving it would wipe the user's real vault list.
fn load_for_update() -> Result<VaultsState, String> {
    match crate::config::read_json::<VaultsState>(&registry_path()) {
        crate::config::JsonRead::Unreadable => Err(
            "vaults.json exists but can't be read right now — change not saved \
             (retry in a moment)"
                .to_string(),
        ),
        _ => Ok(load()),
    }
}

fn save(state: &VaultsState) -> std::io::Result<()> {
    // Never let a serialization failure truncate the vault registry:
    // propagate the error rather than writing an empty string over the
    // existing list of vaults. (load()'s self-seed write swallows this
    // via `let _ = save(..)`, so a failed seed is skipped, not panicked.)
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Atomic temp-file + rename so a crash mid-write can't leave a
    // half-written / empty vaults.json behind.
    crate::notes::write_atomic(registry_path(), &json)
}

/// Path of the currently-active vault. notes::notes_dir() calls this.
pub fn active_path() -> PathBuf {
    let state = load();
    // Defense-in-depth: load() already clamps active_index, but this is
    // the hottest path (every command + every watcher event), so degrade
    // to the OS default rather than panicking on a malformed registry.
    match state.vaults.get(state.active_index) {
        Some(v) => PathBuf::from(&v.path),
        None => PathBuf::from(fallback_default_path()),
    }
}

/// Name of the active vault — surfaced in the sidebar header next to
/// the note count.
pub fn active_name() -> String {
    let state = load();
    // Same defense-in-depth as active_path(): fall back to the basename
    // of the default path (and a "notes" literal) instead of panicking.
    match state.vaults.get(state.active_index) {
        Some(v) => v.name.clone(),
        None => PathBuf::from(fallback_default_path())
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("notes")
            .to_string(),
    }
}

/// Add a new vault. Returns the new VaultsState (with active_index
/// updated to point at the new vault, since users almost always want
/// to jump into a vault they just added). Errors if `path` doesn't
/// exist or isn't readable.
pub fn add(name: String, path: String) -> Result<VaultsState, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        // Try to create it — picking a fresh folder for a fresh vault
        // is a reasonable flow ("New vault from empty dir").
        std::fs::create_dir_all(&p).map_err(|e| format!("create vault dir: {e}"))?;
    }
    if !p.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    let mut state = load_for_update()?;
    state.vaults.push(Vault {
        name: if name.trim().is_empty() {
            p.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("Untitled vault")
                .to_string()
        } else {
            name
        },
        path,
    });
    state.active_index = state.vaults.len() - 1;
    save(&state).map_err(|e| e.to_string())?;
    Ok(state)
}

/// Switch the active vault. Returns the new state.
pub fn switch(index: usize) -> Result<VaultsState, String> {
    let mut state = load_for_update()?;
    if index >= state.vaults.len() {
        return Err(format!("vault index {index} out of range"));
    }
    state.active_index = index;
    save(&state).map_err(|e| e.to_string())?;
    Ok(state)
}

/// Rename a vault in place.
pub fn rename(index: usize, name: String) -> Result<VaultsState, String> {
    let mut state = load_for_update()?;
    if index >= state.vaults.len() {
        return Err(format!("vault index {index} out of range"));
    }
    if name.trim().is_empty() {
        return Err("vault name can't be empty".to_string());
    }
    state.vaults[index].name = name;
    save(&state).map_err(|e| e.to_string())?;
    Ok(state)
}

/// Remove a vault from the registry. The actual notes folder on disk
/// is left alone — removing a vault from malt's awareness is purely
/// a registry operation. If the removed vault was active, active
/// shifts to index 0; if the last vault is removed, a fresh "Default"
/// is seeded.
pub fn remove(index: usize) -> Result<VaultsState, String> {
    let mut state = load_for_update()?;
    if index >= state.vaults.len() {
        return Err(format!("vault index {index} out of range"));
    }
    state.vaults.remove(index);
    if state.active_index >= state.vaults.len() && !state.vaults.is_empty() {
        state.active_index = state.vaults.len() - 1;
    } else if state.vaults.is_empty() {
        // Re-seed with a default so the app always has somewhere to
        // point at — this matches the load() invariant.
        state.vaults.push(Vault {
            name: "Default".to_string(),
            path: fallback_default_path(),
        });
        state.active_index = 0;
    } else if index < state.active_index {
        state.active_index -= 1;
    }
    save(&state).map_err(|e| e.to_string())?;
    Ok(state)
}
