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

/// Stable ids for the built-in saved searches. We seed any that are
/// missing on every load() so the user always has these "report" lenses
/// present; they can rename, reorder, or unbind a slot, but they can't
/// delete a builtin. Each maps to a special query the search bar knows
/// how to route (empty:true, is:orphan, is:onthisday, is:duplicate).
const EMPTY_NOTES_ID: &str = "_builtin_empty_notes";
const ORPHANAGE_ID: &str = "_builtin_orphanage";
const ON_THIS_DAY_ID: &str = "_builtin_on_this_day";
const NEAR_DUPES_ID: &str = "_builtin_near_dupes";

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

/// The built-in saved searches, in seed order. `load()` ensures each is
/// present (appending any that are missing) so upgrades pick up new
/// reports without disturbing a user's existing slot bindings.
fn builtins() -> Vec<SavedSearch> {
    let mk = |id: &str, name: &str, query: &str| SavedSearch {
        id: id.to_string(),
        name: name.to_string(),
        query: query.to_string(),
        slot: None, // filled in by assign_slots
        is_builtin: true,
    };
    vec![
        mk(EMPTY_NOTES_ID, "Empty Notes", "empty:true"),
        mk(ORPHANAGE_ID, "The Orphanage", "is:orphan"),
        mk(ON_THIS_DAY_ID, "On This Day", "is:onthisday"),
        mk(NEAR_DUPES_ID, "Near-duplicates", "is:duplicate"),
    ]
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
    load_inner().unwrap_or_else(|_| {
        // Unreadable (transient IO) — serve the builtins in memory but
        // never persist over the user's real file. Mutations are blocked
        // separately via load_for_update.
        let mut items = Vec::new();
        for b in builtins() {
            items.push(b);
        }
        assign_slots(&mut items);
        items
    })
}

/// Load for a read-modify-write cycle (upsert/delete/rename/reorder).
/// Errors when the file exists but can't be read — saving a default list
/// over it would silently drop every custom saved search.
fn load_for_update() -> Result<Vec<SavedSearch>, String> {
    load_inner().map_err(|_| {
        "saved_searches.json exists but can't be read right now — change not \
         saved (retry in a moment)"
            .to_string()
    })
}

fn load_inner() -> Result<Vec<SavedSearch>, ()> {
    let mut items: Vec<SavedSearch> = match crate::config::read_json(&path()) {
        crate::config::JsonRead::Parsed(v) => v,
        // Missing → first run; Quarantined → original preserved in a .bak.
        // Both are safe to rebuild from scratch (and later persist).
        crate::config::JsonRead::Missing | crate::config::JsonRead::Quarantined => Vec::new(),
        crate::config::JsonRead::Unreadable => return Err(()),
    };
    // Stable sort: slotted items by slot ascending, then unbound items in
    // their existing array order. Stable sort means ties (e.g. two unbound)
    // preserve insertion order.
    items.sort_by_key(|x| x.slot.unwrap_or(u8::MAX));
    // Seed any missing builtins. Fresh installs land with all four at
    // positions 1-4 (⌘1-⌘4) — a guided tour of malt's report lenses.
    // Existing users get newly-added builtins appended at the end, so we
    // never bump a hard-won slot binding on upgrade. They can reorder or
    // unbind from Settings → saved searches.
    for b in builtins() {
        if !items.iter().any(|x| x.id == b.id) {
            items.push(b);
        }
    }
    assign_slots(&mut items);
    Ok(items)
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
    // Never let a serialization failure truncate the saved-search list:
    // propagate the error rather than writing an empty string over a valid
    // file (which would silently drop every saved search + slot binding).
    let json = serde_json::to_string_pretty(items)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Atomic temp-file + rename so a crash mid-write can't leave a
    // half-written / empty saved_searches.json behind.
    crate::notes::write_atomic(path(), &json)
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
pub fn upsert(item: SavedSearch) -> Result<Vec<SavedSearch>, String> {
    let mut items = load_for_update()?;
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
    save(&items).map_err(|e| e.to_string())?;
    Ok(items)
}

pub fn delete(id: &str) -> Result<Vec<SavedSearch>, String> {
    let mut items = load_for_update()?;
    // Reject deletion of built-in searches (they get re-seeded on next
    // load anyway, but the user shouldn't see the operation succeed and
    // then have the item reappear).
    if items.iter().any(|x| x.id == id && x.is_builtin) {
        return Err(
            "Built-in saved searches can't be deleted. Use 'Remove from quick bar' instead."
                .to_string(),
        );
    }
    items.retain(|x| x.id != id);
    assign_slots(&mut items);
    save(&items).map_err(|e| e.to_string())?;
    Ok(items)
}

/// Move a saved search past the slot range so it loses its keyboard
/// binding and disappears from the chip bar — but stays in the Settings
/// list. Equivalent to reorder(id, MAX_SLOTS + 1) but clearer at call
/// sites and more discoverable.
pub fn unbind_slot(id: &str) -> Result<Vec<SavedSearch>, String> {
    reorder(id, MAX_SLOTS + 1)
}

/// Rename the saved search with `id`. Position/slot unchanged.
pub fn rename(id: &str, name: String) -> Result<Vec<SavedSearch>, String> {
    let mut items = load_for_update()?;
    if let Some(item) = items.iter_mut().find(|x| x.id == id) {
        item.name = name;
    }
    // `load_for_update()` already assigned slots; rename doesn't touch order.
    save(&items).map_err(|e| e.to_string())?;
    Ok(items)
}

/// Move the saved search with `id` to 1-indexed `target_position`,
/// shifting other items to accommodate. Positions past the end of the
/// list clamp to the end (i.e. append). No-op if `id` is unknown.
pub fn reorder(id: &str, target_position: usize) -> Result<Vec<SavedSearch>, String> {
    let mut items = load_for_update()?;
    if let Some(idx) = items.iter().position(|x| x.id == id) {
        let item = items.remove(idx);
        let target_idx = target_position.saturating_sub(1).min(items.len());
        items.insert(target_idx, item);
        assign_slots(&mut items);
        save(&items).map_err(|e| e.to_string())?;
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
