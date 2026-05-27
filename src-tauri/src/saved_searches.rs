// Persistent registry of named queries, lifted directly from nvUltra's
// saved-searches model: each saved search has a name, the raw query string,
// and an optional slot (1-9) bound to ⌘N for keyboard-first access.
//
// Storage: ~/.config/malt/saved_searches.json — trivial JSON file, easy to
// hand-edit. We don't bother with a real DB for this.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub query: String,
    /// Slot 1-9 bound to ⌘N; None = no shortcut binding.
    pub slot: Option<u8>,
}

fn path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    let _ = std::fs::create_dir_all(&p);
    p.push("saved_searches.json");
    p
}

pub fn load() -> Vec<SavedSearch> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(items: &[SavedSearch]) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(items).unwrap_or_default();
    std::fs::write(path(), json)
}

/// Insert or replace a saved search by id. If `slot` is set, any other item
/// holding that slot is unbound so slots stay unique.
pub fn upsert(item: SavedSearch) -> std::io::Result<Vec<SavedSearch>> {
    let mut items = load();
    if let Some(s) = item.slot {
        for other in items.iter_mut() {
            if other.id != item.id && other.slot == Some(s) {
                other.slot = None;
            }
        }
    }
    if let Some(existing) = items.iter_mut().find(|x| x.id == item.id) {
        *existing = item;
    } else {
        items.push(item);
    }
    save(&items)?;
    Ok(items)
}

pub fn delete(id: &str) -> std::io::Result<Vec<SavedSearch>> {
    let mut items = load();
    items.retain(|x| x.id != id);
    save(&items)?;
    Ok(items)
}

/// Find the lowest-numbered slot (1..=9) not currently bound.
pub fn next_free_slot() -> Option<u8> {
    let items = load();
    for s in 1u8..=9 {
        if !items.iter().any(|x| x.slot == Some(s)) {
            return Some(s);
        }
    }
    None
}
