// Tips system.
//
// A small bank of user-story-style tips, surfaced one at a time on the
// boot splash (and from Settings → "Launch tips") so people discover
// malt's full keyboard surface without sitting through a tutorial.
//
// Persistence (all in localStorage, no backend round-trip):
//   malt.tips.seen     — JSON array of tip ids the user has already seen
//   malt.tips.last     — id of the most-recently-shown tip (used for
//                        "previous" navigation across sessions)
//   malt.tips.skip     — "1" if the user opted out of startup tips
//
// Selection algorithm:
//   - "next" tries to pick a random tip from the unseen pool. If every
//     tip has been seen, it picks a truly random tip from the full bank
//     (excluding the one currently displayed so the same tip never
//     shows twice in a row).
//   - "previous" walks back through an in-memory history stack so the
//     user can re-read whatever they just clicked past.

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
  /** Phrased as a user story / "you can …" sentence. Short — under 200
   * chars so a phone-style narrow splash still fits without scrolling. */
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
    story: "Type a name into the search bar and press Enter — if no note matches, malt creates one with that title. Your daily note becomes one keystroke.",
  },
  {
    id: "g-cmd-comma",
    category: "general",
    story: "Hit ⌘, (or Ctrl+, on Windows) anywhere to open Settings. Press it again to close. Same trick works for ⌘F (find in note) and ⌘L (focus search).",
  },
  {
    id: "g-split-pane",
    category: "general",
    story: "⌘-click any note row or wikilink to open it in a second pane next to the current one. Great for diffing, transcluding by eye, or pulling notes side-by-side.",
  },
  {
    id: "g-last-open",
    category: "general",
    story: "Quit malt with a note open and the next launch lands you right back on it. Restart-tolerant by design — nothing about your session lives in RAM only.",
  },
  {
    id: "g-rename-backlinks",
    category: "general",
    story: "Double-click a note row to open the actions menu; pick Rename. All [[wikilinks]] pointing at the old name get rewritten atomically. No broken links.",
  },

  // ── shortcuts ───────────────────────────────────────────────────
  {
    id: "s-back-forward",
    category: "shortcuts",
    story: "⌘[ and ⌘] navigate back and forward through the notes you've opened in the current pane — same model as a browser. Per-pane history.",
  },
  {
    id: "s-arrows-from-anywhere",
    category: "shortcuts",
    story: "From anywhere — editor, sidebar, search bar — ⌘↑/⌘↓ (or ⌘J/⌘K) move between notes. Hands stay on the keyboard.",
  },
  {
    id: "s-cmd-w-close-secondary",
    category: "shortcuts",
    story: "When you're done with the split pane, ⌘W from either editor closes the secondary and gives you back the full editor width.",
  },
  {
    id: "s-cmd-i-everywhere",
    category: "shortcuts",
    story: "⌘I in the editor asks Claude to continue from your cursor. Select text first and ⌘I rewrites that selection instead. Re-press to re-roll.",
  },
  {
    id: "s-esc-clear",
    category: "shortcuts",
    story: "Esc almost always means \"clear the query and put me back in the search bar\". The one exception is inside the editor, where it declines a ghost completion.",
  },

  // ── saved searches ──────────────────────────────────────────────
  {
    id: "ss-quick-bar",
    category: "searches",
    story: "Type any query, press ⌘S, give it a name — now you have a chip on the saved-search bar and a one-keystroke recall via ⌘1 through ⌘9.",
  },
  {
    id: "ss-drag-reorder",
    category: "searches",
    story: "Drag a saved-search chip onto another to move it into that position. Other chips slide over to accommodate. Slots are tied to list order.",
  },
  {
    id: "ss-empty-builtin",
    category: "searches",
    story: "The built-in \"Empty Notes\" saved search (⌘1 by default) surfaces every stub you started but didn't write. Tag it with #waiting and you've got a backlog view.",
  },
  {
    id: "ss-right-click",
    category: "searches",
    story: "Right-click any saved-search chip to rename, reorder, or remove it from the quick bar without deleting. Built-ins can be unbound but never deleted.",
  },
  {
    id: "ss-operators",
    category: "searches",
    story: "Queries compose: \"tag:meeting modified:<7d\" finds meetings from the last week. \"empty:true tag:draft\" finds drafts you haven't filled in yet.",
  },

  // ── tags ────────────────────────────────────────────────────────
  {
    id: "t-inline-pills",
    category: "tags",
    story: "Type #anything inside a note. malt collects every hashtag, moves them to a hidden canonical line at the bottom, and renders them as clickable pills.",
  },
  {
    id: "t-vocabulary",
    category: "tags",
    story: "Settings → Tags & queries lets you seed a starter tag vocabulary. Those tags rank first in #-autocomplete — so #draft is one keypress, not three.",
  },
  {
    id: "t-click-pill",
    category: "tags",
    story: "Click a tag pill in the editor to filter the sidebar to every note carrying that tag. Right-click a pill to add or remove it from your starter vocabulary.",
  },

  // ── ai ──────────────────────────────────────────────────────────
  {
    id: "ai-ghost",
    category: "ai",
    story: "After ⌘I, accept the ghost suggestion with Tab, Enter, or an arrow key. Esc declines. The cursor lands at the end of the inserted text, ready to keep typing.",
  },
  {
    id: "ai-model-toggle",
    category: "ai",
    story: "Settings → AI lets you switch between Haiku (fast/cheap), Sonnet (better at long context), and Opus (most literary attention). Same ⌘I trigger, different feel.",
  },
  {
    id: "ai-suggest-wikilinks",
    category: "ai",
    story: "⌘⇧L in the editor opens a modal of suggested [[wikilinks]] — both deterministic title matches and AI-proposed entity links. Accept the ones you want; tick the box to create stubs for any new names.",
  },
  {
    id: "ai-auto-tag",
    category: "ai",
    story: "Settings → AI → \"auto-tag\" runs a background tagger that quietly proposes inline #hashtags for your notes. Off by default. Skips encrypted notes.",
  },

  // ── security ────────────────────────────────────────────────────
  {
    id: "sec-encrypt",
    category: "security",
    story: "Right-click any note → Encrypt… to wrap its body in AES-256-GCM. The file stays a single line of text so Dropbox and Syncthing keep working. Filename remains visible.",
  },
  {
    id: "sec-reprompt",
    category: "security",
    story: "By default, every cached note password is dropped the moment malt loses focus. Toggle that off under Settings → Security if you trust your environment.",
  },
  {
    id: "sec-no-recovery",
    category: "security",
    story: "Encrypted notes have no password recovery. Lose the password, lose the note. Keep a backup of important passwords somewhere safe.",
  },

  // ── about ───────────────────────────────────────────────────────
  {
    id: "a-plain-files",
    category: "about",
    story: "Your notes are plain .md files in a real folder you chose. malt's index, embeddings, and config are sidecar — delete malt tomorrow and your notes are still notes.",
  },
  {
    id: "a-sync-aware",
    category: "about",
    story: "Sync conflict files (Dropbox/Syncthing) get a ⚠ badge in the sidebar. Click one and the original opens beside it for side-by-side merge.",
  },
];

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
