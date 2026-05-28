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

/// Stable id for the built-in "Empty Notes" saved search. We seed this on
/// every load() so the user always has at least one example present;
/// they can rename it, reorder it, or unbind its slot, but they can't
/// delete it.
const EMPTY_NOTES_ID: &str = "_builtin_empty_notes";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub query: String,
    /// Slot 1-9 bound to ⌘N; None = no shortcut binding. Derived from
    /// position in the list, not user-controlled directly.
    pub slot: Option<u8>,
    /// True for built-in searches that the app seeds and protects from
    /// deletion. Renaming, reordering, and slot-unbinding still work.
    /// Defaults to false for backwards compatibility with older files.
    #[serde(default)]
    pub is_builtin: bool,
}

fn builtin_empty_notes() -> SavedSearch {
    SavedSearch {
        id: EMPTY_NOTES_ID.to_string(),
        name: "Empty Notes".to_string(),
        query: "empty:true".to_string(),
        slot: None, // filled in by assign_slots
        is_builtin: true,
    }
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
/// order takes over). Seeds the built-in Empty Notes search at the
/// front on first load.
pub fn load() -> Vec<SavedSearch> {
    let mut items: Vec<SavedSearch> = std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Stable sort: slotted items by slot ascending, then unbound items in
    // their existing array order. Stable sort means ties (e.g. two unbound)
    // preserve insertion order.
    items.sort_by_key(|x| x.slot.unwrap_or(u8::MAX));
    // Seed the Empty Notes builtin if missing. New users land with it at
    // position 1 (slot ⌘1). Existing users get it appended; they can
    // reorder to taste.
    if !items.iter().any(|x| x.id == EMPTY_NOTES_ID) {
        if items.is_empty() {
            items.push(builtin_empty_notes());
        } else {
            // Insert at the front but only for first-time seeding. Use
            // append-style placement so we don't bump someone's hard-won
            // slot 1 binding on upgrade.
            items.push(builtin_empty_notes());
        }
    }
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
    // Reject deletion of built-in searches (they get re-seeded on next
    // load anyway, but the user shouldn't see the operation succeed and
    // then have the item reappear).
    if items.iter().any(|x| x.id == id && x.is_builtin) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Built-in saved searches can't be deleted. Use 'Remove from quick bar' instead.",
        ));
    }
    items.retain(|x| x.id != id);
    assign_slots(&mut items);
    save(&items)?;
    Ok(items)
}

/// Move a saved search past the slot range so it loses its keyboard
/// binding and disappears from the chip bar — but stays in the Settings
/// list. Equivalent to reorder(id, MAX_SLOTS + 1) but clearer at call
/// sites and more discoverable.
pub fn unbind_slot(id: &str) -> std::io::Result<Vec<SavedSearch>> {
    reorder(id, MAX_SLOTS + 1)
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
