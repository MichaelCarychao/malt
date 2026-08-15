// Per-note brew sessions, persisted across app restarts.
//
// A brew is wed to the note it was brewed from: re-opening brew on a
// note shows its cached session (suggestions, in-place edits, done
// checkmarks) instead of re-running, and navigating the primary pane
// swaps the pane to the new note's session. Sessions live in
// localStorage so they survive restarts and app updates — keyed by
// absolute note path (globally unique across vaults, so vault switches
// don't need to clear anything). Recency-pruned to a fixed cap. The
// user's personal checklist items are NOT here (they persist per vault
// under their own key, owned by BrewPane).
//
// Plain Map, not reactive state: BrewPane mirrors the displayed session
// into its own $state and writes back through these objects; +page only
// does existence checks at event time. Mutators here persist; BrewPane
// calls persistBrewSessions() after direct writes to session objects.

export type BrewItemState = {
  /** Checked off after an accepted implement (toggleable). */
  done?: boolean;
  /** In-place edit override for an AI suggestion's text. */
  text?: string;
};

export type BrewSession = {
  /** Raw streamed brew markdown, as received (possibly partial). */
  output: string;
  /** Per-item overlay keyed by parse-stable item id. */
  itemState: Record<string, BrewItemState>;
  error: string | null;
  /** False until the first explicit brew of this note. */
  hasRun: boolean;
  /** The stream was cancelled by navigating away mid-brew. */
  interrupted: boolean;
  /** Last touch, for recency pruning. */
  touchedAt: number;
};

const STORAGE_KEY = "malt.brewSessions";
/** Recency cap: oldest sessions are pruned past this. Sessions are a
 * few KB each, so the cap keeps the store well under localStorage
 * quotas even for heavy brewers. */
const MAX_SESSIONS = 40;

function loadStored(): [string, BrewSession][] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const obj = JSON.parse(raw) as Record<string, BrewSession>;
    return Object.entries(obj).filter(
      ([, s]) => s && typeof s.output === "string",
    );
  } catch {
    return [];
  }
}

export const brewSessions = new Map<string, BrewSession>(loadStored());

let saveTimer: number | null = null;
/** Debounced write-through — call after any session mutation. Cheap to
 * call often; the actual serialization runs at most twice a second. */
export function persistBrewSessions() {
  if (typeof localStorage === "undefined") return;
  if (saveTimer !== null) return;
  saveTimer = window.setTimeout(() => {
    saveTimer = null;
    try {
      // Prune least-recently-touched sessions past the cap.
      if (brewSessions.size > MAX_SESSIONS) {
        const byAge = [...brewSessions.entries()].sort(
          (a, b) => (a[1].touchedAt ?? 0) - (b[1].touchedAt ?? 0),
        );
        for (const [path] of byAge.slice(0, brewSessions.size - MAX_SESSIONS)) {
          brewSessions.delete(path);
        }
      }
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify(Object.fromEntries(brewSessions)),
      );
    } catch {
      /* quota/disabled — sessions degrade to in-memory */
    }
  }, 500) as unknown as number;
}

export function sessionFor(path: string): BrewSession {
  let s = brewSessions.get(path);
  if (!s) {
    s = {
      output: "",
      itemState: {},
      error: null,
      hasRun: false,
      interrupted: false,
      touchedAt: Date.now(),
    };
    brewSessions.set(path, s);
  } else {
    s.touchedAt = Date.now();
  }
  return s;
}

/** Rename-safe: carry a session to the note's new path. */
export function remapSession(oldPath: string, newPath: string) {
  const s = brewSessions.get(oldPath);
  if (s) {
    brewSessions.delete(oldPath);
    brewSessions.set(newPath, s);
    persistBrewSessions();
  }
}

export function dropSession(path: string) {
  if (brewSessions.delete(path)) persistBrewSessions();
}
