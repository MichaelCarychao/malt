// Tiny registry of "flush this editor's pending save" callbacks. Used by
// the parent before issuing a rename (or any operation that mutates a file
// path on disk) so we don't write stale content to a path that no longer
// exists. Editors register on mount and unregister on destroy.

type Flusher = () => Promise<void>;
const flushers = new Set<Flusher>();

export function registerEditorFlusher(fn: Flusher): () => void {
  flushers.add(fn);
  return () => {
    flushers.delete(fn);
  };
}

// Flush every registered editor's pending save. Returns the number of
// flushes that FAILED (0 = all clean). Callers that are about to perform a
// destructive/irreversible action (rename, move-to-vault, vault switch)
// must check the return value and ABORT if it's non-zero — otherwise a
// failed save would be silently lost when the underlying file path or
// vault changes out from under the editor. Best-effort callers can ignore
// the result. We never reject: each flusher's error is caught and counted
// so one failure can't prevent the others from running.
export async function flushAllEditors(): Promise<number> {
  if (flushers.size === 0) return 0;
  const results = await Promise.all(
    [...flushers].map((fn) =>
      fn().then(
        () => true,
        (err) => {
          console.error("editor flush failed", err);
          return false;
        },
      ),
    ),
  );
  return results.filter((ok) => !ok).length;
}
