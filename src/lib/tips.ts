// Tips system.
//
// A small bank of user-story-style tips, surfaced one at a time on the
// boot splash (and from Settings → Tips → "browse tips…") so people
// discover malt's full keyboard surface without sitting through a
// tutorial.
//
// Each tip has three parts:
//   - category  → maps to a Settings tab; drives the small tag color
//   - headline  → 8-word max user story (italic amber, above the body)
//   - story     → the actual instruction / narration (1-2 short sentences)
//
// Persistence (all in localStorage, no backend round-trip):
//   malt.tips.seen     — JSON array of tip ids the user has already seen
//   malt.tips.last     — id of the most-recently-shown tip (used for
//                        "previous" navigation across sessions)
//   malt.tips.skip     — "1" if the user opted out of startup tips
//
// Key combos in tip text use the canonical macOS symbols (⌘, ⇧). The
// `renderKeysForOS()` helper rewrites them to "Ctrl+" / "Shift+" on
// non-Mac platforms so a Windows user doesn't see ⌘ characters.

export type TipCategory =
  | "general"
  | "shortcuts"
  | "searches"
  | "tags"
  | "ai"
  | "security"
  | "about";

export type Tip = {
  id: string;
  category: TipCategory;
  /** One-line user story, max 8 words. Renders italic + amber above
   * the body. Phrase as "I want to ..." or "[verb] [noun]" not as
   * "you can ..." — the headline is the user speaking. */
  headline: string;
  /** The instruction body. Short — under 200 chars so a narrow splash
   * still fits without scrolling. Can reference ⌘ and ⇧; renderer
   * substitutes per-OS. */
  story: string;
};

// The tip bank. Add freely — ids must be unique and stable (they
// persist into localStorage so renaming a tip resets its "seen" state,
// which is fine but worth knowing).
export const TIPS: Tip[] = [
  // ── general / discovery ──────────────────────────────────────────
  {
    id: "g-search-to-create",
    category: "general",
    headline: "Spin up today's note in one keystroke",
    story:
      "Type a name into the search bar and press Enter — if no note matches, malt creates one with that title. Daily note? Done.",
  },
  {
    id: "g-cmd-comma",
    category: "general",
    headline: "Open and close settings without leaving home row",
    story:
      "⌘, toggles Settings open and shut. Same trick: ⌘F toggles find in note, ⌘L focuses the search bar.",
  },
  {
    id: "g-unlinked-mentions",
    category: "general",
    headline: "Discover links I never made",
    story:
      "The linkbacks panel shows \"unlinked mentions\" — notes that name the current one in plain prose without a [[link]]. Hit \"link\" to wire one up. Latent structure, surfaced.",
  },
  {
    id: "g-split-pane",
    category: "general",
    headline: "Pull two notes side by side",
    story:
      "⌘-click any note row or wikilink to open it in a second pane next to the current one. Great for diffing or cross-referencing.",
  },
  {
    id: "g-last-open",
    category: "general",
    headline: "Pick up exactly where I left off",
    story:
      "Quit malt with a note open and the next launch lands you right back on it. Nothing about your session lives in RAM only.",
  },
  {
    id: "g-daily-note",
    category: "general",
    headline: "Jump to today's journal in one keystroke",
    story:
      "⌘D opens (or creates) a note titled with today's date. A fresh one is seeded with #journal so your days collect into a searchable journal — toggle that off in Settings → general.",
  },
  {
    id: "g-rename-backlinks",
    category: "general",
    headline: "Rename a note without breaking links",
    story:
      "Double-click a note row to open the actions menu; pick Rename. All [[wikilinks]] pointing at the old name get rewritten atomically.",
  },

  // ── shortcuts ───────────────────────────────────────────────────
  {
    id: "s-back-forward",
    category: "shortcuts",
    headline: "Browser-style back and forward through notes",
    story:
      "⌘[ and ⌘] walk backward and forward through the notes you've opened in the current pane. Per-pane history.",
  },
  {
    id: "s-arrows-from-anywhere",
    category: "shortcuts",
    headline: "Move between notes without touching the mouse",
    story:
      "⌘↑ / ⌘↓ (or ⌘; / ⌘K) move between notes from anywhere — editor, sidebar, search bar. Hands stay on the keyboard.",
  },
  {
    id: "s-cmd-w-close-secondary",
    category: "shortcuts",
    headline: "Close the split pane and reclaim the width",
    story:
      "When you're done with the second pane, ⌘W from either editor closes it and gives you back full-width editing.",
  },
  {
    id: "s-cmd-i-everywhere",
    category: "shortcuts",
    headline: "Have the AI finish my thought",
    story:
      "⌘; in the editor continues from your cursor. Select text first and ⌘; rewrites that selection. Re-press to re-roll.",
  },
  {
    id: "ai-steer",
    category: "ai",
    headline: "Aim the AI before it writes",
    story:
      "⌘⇧; opens a one-line steer box — \"make it darker\", \"pivot to the counterargument\", \"shorter\" — then generates with that nudge. Works for both continue and rewrite.",
  },
  {
    id: "s-esc-clear",
    category: "shortcuts",
    headline: "Get back to a clean search field",
    story:
      "Esc almost always means \"clear the query and put me back in the search bar.\" Exception: in the editor it declines a ghost completion.",
  },
  {
    id: "s-random-note",
    category: "shortcuts",
    headline: "Surprise me with a forgotten note",
    story:
      "⌘⇧R jumps to a random note in the vault — a serendipity engine for rediscovering what you've buried. (Plain ⌘R renames; add Shift for roulette.)",
  },

  // ── saved searches ──────────────────────────────────────────────
  {
    id: "ss-quick-bar",
    category: "searches",
    headline: "Bind a search to one keystroke",
    story:
      "Type any query, press ⌘S, give it a name — now you have a chip on the saved-search bar and a one-keystroke recall via ⌘1 through ⌘9.",
  },
  {
    id: "ss-drag-reorder",
    category: "searches",
    headline: "Reorder my saved searches by dragging",
    story:
      "Drag a saved-search chip onto another to drop it into that position. Other chips slide over. Slots track list order.",
  },
  {
    id: "ss-empty-builtin",
    category: "searches",
    headline: "See what I started but didn't finish",
    story:
      "The built-in \"Empty Notes\" search surfaces every stub you started but didn't write. Pair with #waiting and you have a backlog view.",
  },
  {
    id: "ss-right-click",
    category: "searches",
    headline: "Rename or unbind a saved search without losing it",
    story:
      "Right-click any saved-search chip to rename, reorder, or remove from the quick bar (without deleting). Built-ins can be unbound but never deleted.",
  },
  {
    id: "ss-semantic",
    category: "searches",
    headline: "Find what I meant, not what I typed",
    story:
      "Start a search with ~ to switch to semantic mode — \"~things that drained me\" ranks notes by meaning, not keywords. Runs on the local embedding model; fully offline, per-vault.",
  },
  {
    id: "ss-operators",
    category: "searches",
    headline: "Stack query operators for power filters",
    story:
      "Queries compose: \"tag:meeting modified:<7d\" finds meetings from the last week. \"empty:true tag:draft\" finds drafts you haven't filled in.",
  },
  {
    id: "ss-orphanage",
    category: "searches",
    headline: "Find the notes adrift from everything",
    story:
      "The Orphanage (is:orphan, or its saved-search chip) lists notes with no links in and none out — stranded thoughts waiting to be connected or pruned.",
  },
  {
    id: "ss-on-this-day",
    category: "searches",
    headline: "Re-read what I wrote a year ago today",
    story:
      "On This Day (is:onthisday) surfaces notes from this calendar day in years past — by dated title or last-edited date. A quiet window onto your own archive.",
  },
  {
    id: "ss-near-dupes",
    category: "searches",
    headline: "Catch the note I accidentally wrote twice",
    story:
      "Near-duplicates (is:duplicate) flags notes with a near-identical twin — re-typed captures, forked drafts, double-pasted clippings. Runs on the local embedding model.",
  },

  // ── tags ────────────────────────────────────────────────────────
  {
    id: "t-inline-pills",
    category: "tags",
    headline: "Tag a note without leaving the prose",
    story:
      "Type #anything inside a note. malt collects every hashtag, moves them to a hidden canonical line at the bottom, and renders them as pills.",
  },
  {
    id: "t-vocabulary",
    category: "tags",
    headline: "Make my favorite tags one keypress",
    story:
      "Settings → Tags & queries lets you seed a starter vocabulary. Those tags rank first in #-autocomplete — so #draft is one keypress, not three.",
  },
  {
    id: "t-cooccurrence",
    category: "tags",
    headline: "See which tags travel together",
    story:
      "Filter by a single tag (e.g. tag:meeting) and an \"often with\" row appears — the tags that most share notes with it. Click one to narrow to both. A map of your own structure.",
  },
  {
    id: "t-click-pill",
    category: "tags",
    headline: "Filter the sidebar by clicking a tag",
    story:
      "Click a tag pill in the editor to filter the sidebar to every note carrying it. Right-click a pill to add or remove from your starter vocabulary.",
  },

  // ── ai ──────────────────────────────────────────────────────────
  {
    id: "ai-ghost",
    category: "ai",
    headline: "Accept a ghost suggestion three different ways",
    story:
      "After ⌘;, accept the ghost with Tab, Enter, or an arrow key. Esc declines. The cursor lands at the end of the inserted text.",
  },
  {
    id: "ai-model-toggle",
    category: "ai",
    headline: "Pick fast, smart, or most-literary Claude",
    story:
      "Settings → AI switches between Haiku (fast/cheap), Sonnet (better at long context), and Opus (most literary attention). Same ⌘; trigger.",
  },
  {
    id: "ai-suggest-wikilinks",
    category: "ai",
    headline: "Have Claude propose links for this note",
    story:
      "⌘⇧L in the editor opens a modal of suggested [[wikilinks]] — title matches plus AI-proposed entity links. Tick the box to materialize stubs.",
  },
  {
    id: "ai-auto-tag",
    category: "ai",
    headline: "Let malt tag my notes in the background",
    story:
      "Settings → AI → \"auto-tag\" quietly proposes inline #hashtags for each note. Off by default. Skips encrypted notes for obvious reasons.",
  },
  {
    id: "ai-brew",
    category: "ai",
    headline: "Brew a note into next-step questions",
    story:
      "⌘⇧B in the editor opens the secondary pane and streams a brainstorm: threads to pull, connections to make, sharper framings. Click \"append\" to fold it into the note.",
  },
  {
    id: "ai-prompts",
    category: "ai",
    headline: "Read and edit every prompt malt sends",
    story:
      "Settings → Prompts surfaces every system prompt Claude sees. Edit any of them; \"reset to default\" reverts. Overrides live in plain JSON.",
  },
  {
    id: "g-vaults",
    category: "general",
    headline: "Keep work and play in separate vaults",
    story:
      "⌘⇧V opens the vault picker. Each vault is its own folder of notes — fully siloed, with its own index and embeddings. Click the vault chip at the bottom of the sidebar to switch.",
  },
  {
    id: "g-brew-save",
    category: "ai",
    headline: "Save a brew session as its own note",
    story:
      "From the Brew pane, hit \"save as note\" to drop the brainstorm into a new note in the vault. The checkbox adds a [[link back]] to the source so it shows in linkbacks.",
  },

  // ── security ────────────────────────────────────────────────────
  {
    id: "sec-encrypt",
    category: "security",
    headline: "Lock a single note with a password",
    story:
      "Right-click any note → Encrypt… to wrap its body in AES-256-GCM. File stays a single line so Dropbox and Syncthing keep working; filename stays visible.",
  },
  {
    id: "sec-reprompt",
    category: "security",
    headline: "Re-lock encrypted notes the moment I tab away",
    story:
      "By default, every cached note password is dropped when malt loses focus. Toggle off in Settings → Security if you trust your environment.",
  },
  {
    id: "sec-no-recovery",
    category: "security",
    headline: "Treat the password like the only key",
    story:
      "Encrypted notes have no password recovery — lose the password, lose the note. Keep important passwords backed up somewhere safe.",
  },

  // ── about ───────────────────────────────────────────────────────
  {
    id: "a-plain-files",
    category: "about",
    headline: "Keep my notes when I quit the app",
    story:
      "Your notes are plain .md files in a real folder. malt's index, embeddings, and config are sidecar — delete malt tomorrow and your notes are still notes.",
  },
  {
    id: "a-sync-aware",
    category: "about",
    headline: "Spot sync conflicts at a glance",
    story:
      "Sync conflict files (Dropbox/Syncthing) get a ⚠ badge in the sidebar. Click one and the original opens beside it for side-by-side merge.",
  },
];

// ─── OS-aware key rendering ──────────────────────────────────────────

// Detect OS lazily so this module works in SSR / test environments
// where `navigator` may not exist.
function detectIsMac(): boolean {
  if (typeof navigator === "undefined") return true;
  return /Mac/i.test(navigator.platform);
}

/** Rewrite ⌘ → "Ctrl+" and ⇧ → "Shift+" on non-Mac platforms.
 *
 * The hyphenated form (e.g. "⌘-click") is preserved as "Ctrl-click"
 * rather than the awkward "Ctrl+-click". Tip authors should use ⌘ and
 * ⇧ everywhere — this helper handles the OS-specific substitution at
 * render time so the source stays compact. */
export function renderKeysForOS(text: string): string {
  if (detectIsMac()) return text;
  return text
    .replace(/⌘-/g, "Ctrl-")
    .replace(/⌘/g, "Ctrl+")
    .replace(/⇧/g, "Shift+");
}

// ─── Persistence + selection ─────────────────────────────────────────

const SEEN_KEY = "malt.tips.seen";
const LAST_KEY = "malt.tips.last";
const SKIP_KEY = "malt.tips.skip";

function readSeen(): Set<string> {
  if (typeof localStorage === "undefined") return new Set();
  try {
    const raw = localStorage.getItem(SEEN_KEY);
    if (!raw) return new Set();
    const arr = JSON.parse(raw);
    return new Set(Array.isArray(arr) ? arr : []);
  } catch {
    return new Set();
  }
}

function writeSeen(seen: Set<string>) {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(SEEN_KEY, JSON.stringify([...seen]));
  } catch {
    /* quota or disabled — silently no-op */
  }
}

export function markSeen(id: string) {
  const seen = readSeen();
  seen.add(id);
  writeSeen(seen);
  if (typeof localStorage !== "undefined") {
    try {
      localStorage.setItem(LAST_KEY, id);
    } catch {
      /* ignore */
    }
  }
}

export function getLastSeen(): Tip | null {
  if (typeof localStorage === "undefined") return null;
  const id = localStorage.getItem(LAST_KEY);
  if (!id) return null;
  return TIPS.find((t) => t.id === id) ?? null;
}

export function shouldSkipOnStartup(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(SKIP_KEY) === "1";
}

export function setSkipOnStartup(skip: boolean) {
  if (typeof localStorage === "undefined") return;
  try {
    if (skip) localStorage.setItem(SKIP_KEY, "1");
    else localStorage.removeItem(SKIP_KEY);
  } catch {
    /* ignore */
  }
}

/**
 * Pick the next tip to show, biased toward unseen ones. If every tip in
 * the bank has been seen, picks a true-random tip (excluding `current`
 * to avoid repeats). Returns null only if the bank is empty.
 */
export function pickNextTip(current: Tip | null): Tip | null {
  if (TIPS.length === 0) return null;
  const seen = readSeen();
  const unseen = TIPS.filter((t) => !seen.has(t.id));
  const pool = unseen.length > 0 ? unseen : TIPS.filter((t) => t.id !== current?.id);
  if (pool.length === 0) return current; // single-tip bank edge case
  return pool[Math.floor(Math.random() * pool.length)];
}

/** Reset the seen-tip set — exposed from Settings so users can re-shuffle
 * the deck from the top. */
export function resetSeenTips() {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.removeItem(SEEN_KEY);
    localStorage.removeItem(LAST_KEY);
  } catch {
    /* ignore */
  }
}

/** Total count of tips in the bank — exposed so the UI can render
 * progress (e.g. "tip 4 of 27"). */
export function tipsBankSize(): number {
  return TIPS.length;
}

export function seenCount(): number {
  return readSeen().size;
}
