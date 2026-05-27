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

export async function flushAllEditors(): Promise<void> {
  if (flushers.size === 0) return;
  await Promise.all([...flushers].map((fn) => fn().catch(() => {})));
}
