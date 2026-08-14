// Word-level diff for the brew implement review UI.
//
// Hand-rolled Myers O(ND) over word tokens — no dependency, because the
// project is deliberately dependency-minimal and the diff here is
// DISPLAY-ONLY: accept/cancel dispatch the pristine revised/original
// strings, never text reconstructed from these segments. A diff bug can
// make the preview ugly; it cannot corrupt a note.
//
// Invariants (hold by construction, relied on by the review renderer):
//   concat(segments where type !== "add") === oldText
//   concat(segments where type !== "del") === newText
//
// Tokens are words AND whitespace runs (/\S+|\s+/g), so concatenating
// tokens reproduces the input byte-for-byte — no lossy re-joining.

export type DiffSegment = { type: "same" | "del" | "add"; text: string };

/** Above this edit distance the Myers walk is abandoned and the middle
 * renders as one struck block + one added block. Keeps worst-case work
 * bounded at ~D² trace memory for total rewrites, where a fine-grained
 * word diff would be visual noise anyway. */
const MAX_EDIT_DISTANCE = 2000;

function tokenize(text: string): string[] {
  return text.match(/\S+|\s+/g) ?? [];
}

export function diffWords(oldText: string, newText: string): DiffSegment[] {
  if (oldText === newText) {
    return oldText ? [{ type: "same", text: oldText }] : [];
  }
  const a = tokenize(oldText);
  const b = tokenize(newText);

  // Trim common prefix/suffix — the typical implement result is a few
  // localized edits, so the Myers walk only sees a small middle.
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }

  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  const prefix = a.slice(0, start).join("");
  const suffix = a.slice(endA).join("");

  const segments: DiffSegment[] = [];
  if (prefix) segments.push({ type: "same", text: prefix });
  segments.push(...diffMiddle(midA, midB));
  if (suffix) segments.push({ type: "same", text: suffix });
  return coalesce(segments);
}

/** Myers greedy O(ND) on the trimmed middle. Returns del/add/same
 * segments (uncoalesced). Falls back to [del-all, add-all] when the
 * edit distance exceeds MAX_EDIT_DISTANCE. */
function diffMiddle(a: string[], b: string[]): DiffSegment[] {
  if (a.length === 0 && b.length === 0) return [];
  if (a.length === 0) return [{ type: "add", text: b.join("") }];
  if (b.length === 0) return [{ type: "del", text: a.join("") }];

  const max = Math.min(a.length + b.length, MAX_EDIT_DISTANCE);
  const offset = max;
  // v[k+offset] = furthest x on diagonal k; trace keeps a copy per D for
  // backtracking. Memory is O(D²) — bounded by the cap.
  let v = new Int32Array(2 * max + 2);
  const trace: Int32Array[] = [];
  let foundD = -1;

  outer: for (let d = 0; d <= max; d++) {
    trace.push(v.slice());
    const next = v.slice();
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && v[k - 1 + offset] < v[k + 1 + offset])) {
        x = v[k + 1 + offset]; // down: insertion from b
      } else {
        x = v[k - 1 + offset] + 1; // right: deletion from a
      }
      let y = x - k;
      while (x < a.length && y < b.length && a[x] === b[y]) {
        x++;
        y++;
      }
      next[k + offset] = x;
      if (x >= a.length && y >= b.length) {
        foundD = d;
        trace.push(next);
        break outer;
      }
    }
    v = next;
  }

  if (foundD < 0) {
    // Edit distance exceeds the cap — degrade to block form.
    return [
      { type: "del", text: a.join("") },
      { type: "add", text: b.join("") },
    ];
  }

  // Backtrack from (a.length, b.length) through the trace, emitting ops
  // in reverse, then flip.
  const rev: DiffSegment[] = [];
  let x = a.length;
  let y = b.length;
  for (let d = foundD; d > 0; d--) {
    const vPrev = trace[d]; // v state entering round d
    const k = x - y;
    let prevK: number;
    if (k === -d || (k !== d && vPrev[k - 1 + offset] < vPrev[k + 1 + offset])) {
      prevK = k + 1; // came from an insertion
    } else {
      prevK = k - 1; // came from a deletion
    }
    const prevX = vPrev[prevK + offset];
    const prevY = prevX - prevK;
    // Snake (matched run) after the edit that entered this diagonal.
    const snakeStartX = prevK === k + 1 ? prevX : prevX + 1;
    const snakeStartY = prevK === k + 1 ? prevY + 1 : prevY;
    if (x > snakeStartX) {
      rev.push({ type: "same", text: a.slice(snakeStartX, x).join("") });
    }
    if (prevK === k + 1) {
      rev.push({ type: "add", text: b[prevY] });
    } else {
      rev.push({ type: "del", text: a[prevX] });
    }
    x = prevX;
    y = prevY;
  }
  if (x > 0) {
    rev.push({ type: "same", text: a.slice(0, x).join("") });
  }
  rev.reverse();
  return rev;
}

/** Merge adjacent same-type segments, and fold whitespace-only "same"
 * runs that sit between a del and an add into the del side — otherwise
 * a reworded sentence renders as word-space-word-space confetti. */
function coalesce(segments: DiffSegment[]): DiffSegment[] {
  // Pass 1: whitespace-only "same" between two changed segments joins
  // the preceding changed segment.
  const folded: DiffSegment[] = [];
  for (let i = 0; i < segments.length; i++) {
    const seg = segments[i];
    const prev = folded[folded.length - 1];
    const next = segments[i + 1];
    if (
      seg.type === "same" &&
      seg.text.trim() === "" &&
      prev &&
      prev.type !== "same" &&
      next &&
      next.type !== "same"
    ) {
      // Duplicate the whitespace into both sides so the concat
      // invariants survive: it leaves with the del, arrives with the add.
      folded.push({ type: "del", text: seg.text });
      folded.push({ type: "add", text: seg.text });
      continue;
    }
    folded.push(seg);
  }
  // Pass 2: merge adjacent same-type segments; deletions sort before
  // additions inside one change region for stable render order.
  const out: DiffSegment[] = [];
  for (const seg of folded) {
    if (seg.text === "") continue;
    const prev = out[out.length - 1];
    if (prev && prev.type === seg.type) {
      prev.text += seg.text;
    } else if (
      prev &&
      prev.type === "add" &&
      seg.type === "del" &&
      out.length >= 1
    ) {
      // Reorder del before add within a contiguous change region.
      const beforePrev = out[out.length - 2];
      if (beforePrev && beforePrev.type === "del") {
        beforePrev.text += seg.text;
      } else {
        out.splice(out.length - 1, 0, { ...seg });
      }
    } else {
      out.push({ ...seg });
    }
  }
  return out;
}
