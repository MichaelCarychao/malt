// Persistent registry of named queries, lifted directly from nvUltra's
// saved-searches model. Each saved search has a name and a raw query string;
// the first 9 in list order are bound to ⌘1–⌘9 and shown on the saved-search
// chip bar. Items past position 9 are still saved and editable from
// Settings → Saved searches, just without a keyboard binding.
//
// Storage: ~/.config/malt/saved_searches.json — trivial JSON file, easy to
// hand-edit. The JSON array order IS the canonical order; `slot` is derived
// from position (`Some(index+1)` if index < 9, else `None`) and overwritten
// on every save. We persist `slot` anyway for human readability when
// inspecting the file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MAX_SLOTS: usize = 9;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub query: String,
    /// Slot 1-9 bound to ⌘N; None = no shortcut binding. Derived from
    /// position in the list, not user-controlled directly.
    pub slot: Option<u8>,
}

fn path() -> PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    p.push("malt");
    let _ = std::fs::create_dir_all(&p);
    p.push("saved_searches.json");
    p
}

/// Read items from disk, then sort + reassign slots from position so the
/// returned list is canonical. Handles legacy files where `slot` was
/// user-edited (we trust slot for ordering on first load, then array
/// order takes over).
pub fn load() -> Vec<SavedSearch> {
    let mut items: Vec<SavedSearch> = std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Stable sort: slotted items by slot ascending, then unbound items in
    // their existing array order. Stable sort means ties (e.g. two unbound)
    // preserve insertion order.
    items.sort_by_key(|x| x.slot.unwrap_or(u8::MAX));
    assign_slots(&mut items);
    items
}

/// Overwrite each item's `slot` field from its position in `items`.
fn assign_slots(items: &mut [SavedSearch]) {
    for (i, item) in items.iter_mut().enumerate() {
        item.slot = if i < MAX_SLOTS {
            Some((i + 1) as u8)
        } else {
            None
        };
    }
}

fn save(items: &[SavedSearch]) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(items).unwrap_or_default();
    std::fs::write(path(), json)
}

/// Insert or replace a saved search by id.
///
/// - If an item with the same id exists, its name/query are updated in
///   place at its current position (slot/order unchanged unless the
///   caller passes a different `slot` to request a move).
/// - If new, the item is appended (assigned the next slot if one is free,
///   else stored unbound).
/// - If `item.slot` is set and differs from the current position, the
///   item is moved into that 1-indexed position and others shift to
///   accommodate.
pub fn upsert(item: SavedSearch) -> std::io::Result<Vec<SavedSearch>> {
    let mut items = load();
    let requested = item.slot;

    if let Some(idx) = items.iter().position(|x| x.id == item.id) {
        // Update existing entry. Preserve order unless caller explicitly
        // asks for a slot change.
        items[idx] = item;
        if let Some(s) = requested {
            let target_idx = (s as usize).saturating_sub(1).min(items.len().saturating_sub(1));
            if target_idx != idx {
                let it = items.remove(idx);
                items.insert(target_idx, it);
            }
        }
    } else {
        // New entry. Honor caller-requested slot if any, else append.
        if let Some(s) = requested {
            let target_idx = (s as usize).saturating_sub(1).min(items.len());
            items.insert(target_idx, item);
        } else {
            items.push(item);
        }
    }

    assign_slots(&mut items);
    save(&items)?;
    Ok(items)
}

pub fn delete(id: &str) -> std::io::Result<Vec<SavedSearch>> {
    let mut items = load();
    items.retain(|x| x.id != id);
    assign_slots(&mut items);
    save(&items)?;
    Ok(items)
}

/// Rename the saved search with `id`. Position/slot unchanged.
pub fn rename(id: &str, name: String) -> std::io::Result<Vec<SavedSearch>> {
    let mut items = load();
    if let Some(item) = items.iter_mut().find(|x| x.id == id) {
        item.name = name;
    }
    // `load()` already assigned slots; rename doesn't touch order.
    save(&items)?;
    Ok(items)
}

/// Move the saved search with `id` to 1-indexed `target_position`,
/// shifting other items to accommodate. Positions past the end of the
/// list clamp to the end (i.e. append). No-op if `id` is unknown.
pub fn reorder(id: &str, target_position: usize) -> std::io::Result<Vec<SavedSearch>> {
    let mut items = load();
    if let Some(idx) = items.iter().position(|x| x.id == id) {
        let item = items.remove(idx);
        let target_idx = target_position.saturating_sub(1).min(items.len());
        items.insert(target_idx, item);
        assign_slots(&mut items);
        save(&items)?;
    }
    Ok(items)
}

/// Next free slot 1-9, or None if all are bound. Used by the
/// save-search modal to pre-populate the slot field.
pub fn next_free_slot() -> Option<u8> {
    let items = load();
    if items.len() < MAX_SLOTS {
        Some((items.len() + 1) as u8)
    } else {
        None
    }
}
