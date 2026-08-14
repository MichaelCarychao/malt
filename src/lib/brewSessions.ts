// Per-note brew sessions.
//
// A brew is wed to the note it was brewed from: re-opening brew on a
// note shows its cached session (suggestions, in-place edits, done
// checkmarks) instead of re-running, and navigating the primary pane
// swaps the pane to the new note's session. Sessions live in memory for
// the app session only — a fresh launch starts clean. The user's
// personal checklist items are NOT here (they persist per vault in
// localStorage, owned by BrewPane).
//
// Plain Map, not reactive state: BrewPane mirrors the displayed session
// into its own $state and writes back through these objects; +page only
// does existence checks at event time (Cmd+Shift+B, navigation).

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
};

export const brewSessions = new Map<string, BrewSession>();

export function sessionFor(path: string): BrewSession {
  let s = brewSessions.get(path);
  if (!s) {
    s = { output: "", itemState: {}, error: null, hasRun: false, interrupted: false };
    brewSessions.set(path, s);
  }
  return s;
}

/** Rename-safe: carry a session to the note's new path. */
export function remapSession(oldPath: string, newPath: string) {
  const s = brewSessions.get(oldPath);
  if (s) {
    brewSessions.delete(oldPath);
    brewSessions.set(newPath, s);
  }
}

export function dropSession(path: string) {
  brewSessions.delete(path);
}

export function clearSessions() {
  brewSessions.clear();
}
