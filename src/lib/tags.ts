// Hashtag detection + the canonical-tag-line relocation transform.
// Mirrors src-tauri/src/tags.rs — keep the two in sync.
//
// Grammar:
//   - #tag must follow whitespace, start-of-line, or one of [({"'.
//   - Starts with an ASCII letter, then [a-z0-9_/-].
//   - Lowercase canonical.
//   - URL fragments (https://...#x) and code blocks/spans are skipped.

export type TagMatch = {
  from: number; // char offset in the original string
  to: number; // exclusive
  tag: string; // canonical (lowercase)
};

const OPEN_LEAD = new Set(["(", "[", "{", '"', "'"]);

function isLetter(c: string): boolean {
  return /^[a-z]$/i.test(c);
}
function isTagChar(c: string): boolean {
  return /^[a-z0-9_/-]$/i.test(c);
}
function isValidLead(prev: string | undefined): boolean {
  if (prev === undefined) return true;
  if (/\s/.test(prev)) return true;
  return OPEN_LEAD.has(prev);
}

/**
 * Find every inline hashtag in `body`, respecting code fences + inline code.
 * Returns matches in body order (deduplicated by position, not by tag value).
 */
export function findInlineTags(body: string): TagMatch[] {
  const out: TagMatch[] = [];
  let lineStart = 0;
  let inFence = false;
  for (let i = 0; i <= body.length; i++) {
    if (i === body.length || body[i] === "\n") {
      const line = body.slice(lineStart, i);
      const trimmed = line.trimStart();
      if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
        inFence = !inFence;
      } else if (!inFence) {
        scanLineInto(line, lineStart, out);
      }
      lineStart = i + 1;
    }
  }
  return out;
}

function scanLineInto(line: string, lineOffset: number, out: TagMatch[]): void {
  // Mask inline-code regions with spaces so offsets stay aligned.
  const masked = maskInlineCode(line);
  for (let i = 0; i < masked.length; i++) {
    if (masked[i] !== "#") continue;
    if (!isValidLead(i === 0 ? undefined : masked[i - 1])) continue;
    const start = i + 1;
    if (start >= masked.length) break;
    if (!isLetter(masked[start])) continue;
    let j = start + 1;
    while (j < masked.length && isTagChar(masked[j])) j++;
    // Strip trailing slashes — partial nesting isn't a tag.
    let end = j;
    while (end > start && masked[end - 1] === "/") end--;
    if (end > start) {
      const tag = masked.slice(start, end).toLowerCase();
      out.push({ from: lineOffset + i, to: lineOffset + end, tag });
    }
    i = j - 1;
  }
}

function maskInlineCode(line: string): string {
  let inCode = false;
  let out = "";
  for (const c of line) {
    if (c === "`") {
      inCode = !inCode;
      out += " ";
    } else if (inCode) {
      out += " ";
    } else {
      out += c;
    }
  }
  return out;
}

/**
 * Returns true if `line` is entirely hashtags + whitespace.
 */
export function isCanonicalTagLine(line: string): boolean {
  const trimmed = line.trim();
  if (!trimmed) return false;
  const toks = trimmed.split(/\s+/);
  for (const tok of toks) {
    if (!tok.startsWith("#")) return false;
    const body = tok.slice(1);
    if (!body || !isLetter(body[0])) return false;
    for (const c of body.slice(1)) {
      if (!isTagChar(c)) return false;
    }
  }
  return true;
}

/**
 * Locate the canonical tag line if present. Returns the line index (0-based)
 * and the parsed tag set, or null. The "canonical" line is the last
 * non-empty line in the body that's entirely hashtags.
 */
export function findCanonicalTagLine(body: string): { lineIdx: number; tags: string[] } | null {
  const lines = body.split("\n");
  for (let i = lines.length - 1; i >= 0; i--) {
    const l = lines[i];
    if (l.trim() === "") continue;
    if (isCanonicalTagLine(l)) {
      const tags = l
        .trim()
        .split(/\s+/)
        .map((t) => t.replace(/^#/, "").toLowerCase());
      return { lineIdx: i, tags };
    }
    return null; // first non-empty line from bottom is something else
  }
  return null;
}

/**
 * Given the current body, produce a normalized version where:
 *   - All inline #tags above the canonical line are removed (with adjacent
 *     whitespace collapsed sensibly)
 *   - All discovered tags are merged with the existing canonical-line tags
 *   - The canonical tag line is rewritten as the last non-empty line
 *
 * Returns the new body + the unified tag set. If no changes are needed,
 * returns the input body unchanged so the editor can short-circuit.
 */
export function relocateTagsToBottom(body: string): { body: string; tags: string[] } {
  const canonical = findCanonicalTagLine(body);
  const canonicalIdx = canonical?.lineIdx ?? -1;
  const lines = body.split("\n");

  const aboveText = canonicalIdx === -1 ? body : lines.slice(0, canonicalIdx).join("\n");
  const inline = findInlineTags(aboveText);

  // If there are no inline tags above and the canonical line is already at
  // the bottom (no trailing blank lines past it), nothing to do.
  if (inline.length === 0) {
    if (canonicalIdx === -1) return { body, tags: [] };
    // Check that nothing nonempty comes after the canonical line.
    const tail = lines.slice(canonicalIdx + 1).join("\n").trim();
    if (tail === "") {
      return { body, tags: canonical!.tags };
    }
  }

  // Rewrite the above-text with inline tags removed.
  let newAbove = "";
  let cursor = 0;
  for (const m of inline) {
    newAbove += aboveText.slice(cursor, m.from);
    // Collapse one adjacent space — prefer the trailing space if the
    // following char is whitespace; otherwise consume one leading space.
    const after = aboveText[m.to];
    if (after === " " || after === "\t") {
      cursor = m.to + 1;
    } else if (
      newAbove.length > 0 &&
      (newAbove.endsWith(" ") || newAbove.endsWith("\t"))
    ) {
      newAbove = newAbove.slice(0, -1);
      cursor = m.to;
    } else {
      cursor = m.to;
    }
  }
  newAbove += aboveText.slice(cursor);
  // Trim trailing whitespace on every line, collapse runs of >2 blank lines.
  newAbove = newAbove
    .split("\n")
    .map((l) => l.replace(/[ \t]+$/, ""))
    .join("\n")
    .replace(/\n{3,}/g, "\n\n");

  // Merge tags: from existing canonical line + freshly extracted.
  const allTags = new Set<string>();
  for (const t of canonical?.tags ?? []) allTags.add(t);
  for (const m of inline) allTags.add(m.tag);
  const sorted = [...allTags].sort();

  // Compose the new body. If there are tags, emit a canonical tag line at
  // the very bottom, separated by a blank line if the body has content.
  let result = newAbove.replace(/\n+$/, "");
  if (sorted.length > 0) {
    if (result.length > 0) result += "\n\n";
    result += sorted.map((t) => `#${t}`).join(" ");
  }
  result += "\n";

  return { body: result, tags: sorted };
}

/**
 * Strip wikilink brackets, replacing `[[X]]` with bare `X`. The AI shouldn't
 * see our private markup; the inner text IS the prose. Multi-line wikilinks
 * aren't a thing (the resolver doesn't allow `\n` inside the brackets), so
 * a same-line regex is sufficient. Nested `[` is also disallowed.
 */
export function stripWikilinkBrackets(text: string): string {
  return text.replace(/\[\[([^\[\]\n]+)\]\]/g, "$1");
}

/**
 * Strip the canonical tag line + every inline hashtag + every `[[...]]`
 * bracket from `text`. Used before sending content to the AI so our private
 * markup doesn't pollute the model context. Whitespace around removed tags
 * is collapsed sensibly; blank-line runs are clamped to a single blank.
 */
export function stripTagsForAI(text: string): string {
  // 0) Strip wikilink brackets first so subsequent passes operate on pure
  //    prose. `[[Atomic Habits]]` → `Atomic Habits`.
  text = stripWikilinkBrackets(text);
  // 1) Drop the canonical tag line if it ends `text` (last non-empty line).
  const lines = text.split("\n");
  let lastNonempty = -1;
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].trim() !== "") {
      lastNonempty = i;
      break;
    }
  }
  if (lastNonempty >= 0 && isCanonicalTagLine(lines[lastNonempty])) {
    lines.splice(lastNonempty, 1);
    // Also drop a single preceding blank to avoid trailing blank lines.
    if (lastNonempty - 1 >= 0 && lines[lastNonempty - 1].trim() === "") {
      lines.splice(lastNonempty - 1, 1);
    }
  }
  let result = lines.join("\n");

  // 2) Remove every remaining inline hashtag, collapsing adjacent whitespace.
  const inline = findInlineTags(result);
  let out = "";
  let cursor = 0;
  for (const m of inline) {
    out += result.slice(cursor, m.from);
    const after = result[m.to];
    if (after === " " || after === "\t") {
      cursor = m.to + 1;
    } else if (out.length > 0 && (out.endsWith(" ") || out.endsWith("\t"))) {
      out = out.slice(0, -1);
      cursor = m.to;
    } else {
      cursor = m.to;
    }
  }
  out += result.slice(cursor);

  return out
    .split("\n")
    .map((l) => l.replace(/[ \t]+$/, ""))
    .join("\n")
    .replace(/\n{3,}/g, "\n\n");
}

/**
 * Find the range of the canonical tag line in the body (start and end char
 * offsets in the full document). Returns null if there is no canonical
 * tag line. Used by editor decorations to hide the line.
 */
export function canonicalTagLineRange(body: string): { from: number; to: number } | null {
  const lines = body.split("\n");
  let offset = 0;
  let lastNonempty = -1;
  let lastNonemptyOffset = 0;
  let lastNonemptyLen = 0;
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (l.trim() !== "") {
      lastNonempty = i;
      lastNonemptyOffset = offset;
      lastNonemptyLen = l.length;
    }
    offset += l.length + 1; // +1 for the newline
  }
  if (lastNonempty < 0) return null;
  if (!isCanonicalTagLine(lines[lastNonempty])) return null;
  // Refuse to hide the canonical line if there is no non-empty content
  // above it — otherwise the user sees nothing and the cursor has no
  // valid landing spot. This shows up on brand-new notes whose only
  // content so far is a hashtag, or on legacy notes that were just tags.
  let hasContentAbove = false;
  for (let i = 0; i < lastNonempty; i++) {
    if (lines[i].trim() !== "") {
      hasContentAbove = true;
      break;
    }
  }
  if (!hasContentAbove) return null;
  return { from: lastNonemptyOffset, to: lastNonemptyOffset + lastNonemptyLen };
}
