<script lang="ts">
  import { onMount, onDestroy, tick } from "svelte";
  import { invoke, Channel } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import {
    EditorState,
    Compartment,
    StateField,
    StateEffect,
    RangeSetBuilder,
    Prec,
    Annotation,
  } from "@codemirror/state";
  import {
    EditorView,
    Decoration,
    WidgetType,
    ViewPlugin,
    keymap,
    lineNumbers,
    highlightActiveLine,
    highlightActiveLineGutter,
    highlightSpecialChars,
    tooltips,
  } from "@codemirror/view";
  import type { DecorationSet, ViewUpdate } from "@codemirror/view";
  import {
    syntaxHighlighting,
    defaultHighlightStyle,
    indentOnInput,
    bracketMatching,
  } from "@codemirror/language";
  import { history, defaultKeymap, historyKeymap } from "@codemirror/commands";
  import {
    closeBrackets,
    closeBracketsKeymap,
    autocompletion,
    startCompletion,
    type CompletionContext,
    type CompletionResult,
    type Completion,
  } from "@codemirror/autocomplete";
  import {
    search,
    searchKeymap,
    openSearchPanel,
    closeSearchPanel,
  } from "@codemirror/search";

  // Toggle CodeMirror's search panel: close if open, open if not.
  // Detection via DOM presence of `.cm-search` (the panel form's class)
  // — there's no exported "isPanelOpen" selector in @codemirror/search.
  function toggleSearchPanel(view: EditorView): void {
    const isOpen = !!view.dom.querySelector(".cm-search");
    if (isOpen) {
      closeSearchPanel(view);
    } else {
      view.focus();
      openSearchPanel(view);
    }
  }
  import { markdown } from "@codemirror/lang-markdown";
  import { oneDark } from "@codemirror/theme-one-dark";
  import { vim, Vim } from "@replit/codemirror-vim";
  import { registerEditorFlusher } from "./editorRegistry";
  import {
    findInlineTags,
    findCanonicalTagLine,
    canonicalTagLineRange,
    relocateTagsToBottom,
    stripTagsForAI,
  } from "./tags";

  type NoteRef = { path: string; title: string; is_empty?: boolean };

  let {
    path,
    query = "",
    allNotes = [],
    onNavigate,
    onCreate,
    onReady,
    onCount,
    onClose,
    onRename,
    tagVocabulary = [],
    allTags = [],
    onTagClick,
    onTagPromote,
    onSaved,
    onFinderReady,
    password = null,
    isEncrypted = false,
    onBrew,
    getPrePrompt,
    onEncryptedAiBlocked,
  }: {
    path: string | null;
    query?: string;
    allNotes?: NoteRef[];
    // `alt` is true when the user wanted a "split" gesture (Cmd/Ctrl+click).
    // Parent decides what that means contextually (the other pane).
    onNavigate?: (path: string, alt: boolean) => void;
    onCreate?: (title: string, alt: boolean) => Promise<string | null>;
    onReady?: (view: EditorView) => void;
    onCount?: (words: number, chars: number) => void;
    onClose?: () => void;
    onRename?: (path: string) => void;
    // Tag vocabulary ranked first in #-autocomplete (starter vocab from settings).
    tagVocabulary?: string[];
    // Every tag known to the corpus — autocomplete falls back to these.
    allTags?: string[];
    // User clicked a pill in the editor's tag row.
    onTagClick?: (tag: string) => void;
    // User toggled vocab membership for a tag (from the pill right-click menu).
    onTagPromote?: (tag: string, inVocab: boolean) => void;
    // Fires after an autosave completes successfully — parent uses this for
    // the "saved" status-bar pulse.
    onSaved?: () => void;
    // Called once per mounted view with a function that focuses + opens
    // CodeMirror's search panel. Used by the global Cmd+F forwarder so
    // pressing Cmd+F from the sidebar / search bar still lands in find.
    onFinderReady?: (openFind: () => void) => void;
    /** Plaintext password for an encrypted note. When non-null, the
     * editor reads via `read_encrypted_note` and saves via
     * `save_encrypted_note`. When null on an encrypted note, the editor
     * stays empty/locked (parent should pop a password modal before
     * navigating). */
    password?: string | null;
    /** Whether this note's file is wrapped in the malt encryption
     * envelope. Distinguishes "no password supplied because the note
     * is plaintext" from "password is required but not yet known". */
    isEncrypted?: boolean;
    /** Called when the user fires Cmd+Shift+B inside the editor. The
     * parent decides where to route the brew (typically: open the
     * secondary pane in brew mode with the note body passed back to
     * the AI stream). */
    onBrew?: (noteBody: string) => void;
    /** Two-pane prompting (Cmd/Ctrl+Shift+'). Returns the OTHER pane's
     * content to prepend as a raw pre-prompt, or null when there's no
     * second note pane open (feature no-ops). The parent supplies a
     * pane-specific implementation. */
    getPrePrompt?: () => Promise<string | null>;
    /** Fired when the user triggers an AI action (complete / rewrite / prompt /
     * brew) on an encrypted note. AI is disabled until the note is decrypted;
     * the parent offers to decrypt (M2). */
    onEncryptedAiBlocked?: (path: string) => void;
  } = $props();

  // Right-click pill menu: floating div anchored at cursor.
  let pillMenu = $state<{ tag: string; x: number; y: number } | null>(null);

  // Steer-generation modal (Cmd/Ctrl+Shift+;). The text is injected as a
  // <direction> note into the next completion/rewrite so the user can
  // nudge the model ("make it darker", "pivot to the counterargument").
  // Notes tagged #prompt appear in a house-style dropdown; the selected
  // note's body is appended to the system prompt as a standing style
  // guide. Selection persists across sessions via localStorage.
  let steerOpen = $state(false);
  let steerText = $state("");
  let steerInputEl: HTMLInputElement | null = $state(null);
  let steerStyleNotes = $state<NoteRef[]>([]);
  let steerStylePath = $state<string>(
    localStorage.getItem("malt.steerStylePath") ?? "",
  );

  // Set to the on-disk version when the file changes underneath unsaved
  // edits — a genuine conflict. While non-null we DON'T overwrite the
  // buffer and we pause autosave (see scheduleSave), so neither side
  // clobbers the other until the user picks "keep mine" / "use theirs".
  let externalConflict = $state<string | null>(null);
  function useMyVersion() {
    externalConflict = null;
    // Persist the buffer over the external change ("mine" wins).
    if (currentPath && view) void flushSave(currentPath, view.state.doc.toString());
  }
  function useTheirVersion() {
    const fresh = externalConflict;
    externalConflict = null;
    if (fresh === null || !view) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: fresh },
      annotations: internalDocRewrite.of(true),
    });
    lastSavedContent = fresh;
  }

  function openSteer() {
    steerText = "";
    steerOpen = true;
    // Refresh the house-style list each open — cheap (note cache) and
    // picks up newly-tagged #prompt notes without a restart.
    void invoke<NoteRef[]>("list_prompt_notes")
      .then((list) => {
        steerStyleNotes = list;
        // Drop a stale selection (note deleted or untagged since).
        if (steerStylePath && !list.some((n) => n.path === steerStylePath)) {
          steerStylePath = "";
        }
      })
      .catch(() => (steerStyleNotes = []));
    void tick().then(() => steerInputEl?.focus());
  }
  function cancelSteer() {
    steerOpen = false;
    // Hand focus back to the editor so the user keeps typing.
    view?.focus();
  }
  async function submitSteer() {
    const dir = steerText.trim();
    steerOpen = false;
    localStorage.setItem("malt.steerStylePath", steerStylePath);
    let style = "";
    if (steerStylePath) {
      try {
        const body = await invoke<string>("read_note", { path: steerStylePath });
        style = stripTagsForAI(body);
      } catch (e) {
        // Stale/unreadable style note — generate without it rather than
        // blocking the whole request.
        console.error("style note read failed", e);
      }
    }
    if (view) {
      view.focus();
      void fetchCompletion(view, dir, style);
    }
  }

  // Link-suggestion modal (Cmd+Shift+L → review & apply).
  type LinkSuggestion = {
    term: string;
    candidate_title: string;
    candidate_path: string;
    kind: string; // "link" for now; "create" reserved for AI layer
    positions: [number, number][];
  };
  let linkSuggestions = $state<LinkSuggestion[]>([]);
  let linkAiSuggestions = $state<LinkSuggestion[]>([]);
  let linkSuggestionsOpen = $state(false);
  let linkSuggestionsLoading = $state(false);
  /// When true, accepting an AI-proposed new-note suggestion ALSO creates
  /// the empty .md file so the wikilink resolves immediately. When false,
  /// the brackets are inserted but the link stays broken until the user
  /// clicks it to create. Persisted across modal sessions.
  let createNotesIfNeeded = $state(
    typeof localStorage !== "undefined" &&
      localStorage.getItem("malt.linkSuggestions.createNotes") === "1"
  );
  $effect(() => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(
        "malt.linkSuggestions.createNotes",
        createNotesIfNeeded ? "1" : "0"
      );
    }
  });
  // Tracked separately from the deterministic load so the user sees
  // "scanning… AI thinking…" once the deterministic results are visible.
  let linkAiLoading = $state(false);
  let linkAiError = $state<string | null>(null);
  // Key: candidate_path for kind:"link", "ai:" + candidate_title for kind:"create".
  let linkSuggestionChecked = $state<Record<string, boolean>>({});

  function suggestionKey(s: LinkSuggestion): string {
    return s.kind === "create" ? `ai:${s.candidate_title}` : s.candidate_path;
  }

  function defaultCheckedForTitle(title: string): boolean {
    // Default-on for capitalized OR multi-word titles. Lowercase single
    // words are more likely to be common-noun matches with high noise.
    const trimmed = title.trim();
    if (trimmed.includes(" ")) return true;
    return /^[A-Z]/.test(trimmed);
  }

  async function openLinkSuggestions() {
    if (!view || !currentPath) return;
    // FLUSH (not just cancel) the pending autosave first: the backend
    // scans the file ON DISK, so buffer and disk must agree or every
    // returned position is shifted by the unsaved edits. Running the tag
    // relocation now also means it can't fire mid-review and shift things
    // later. If the save fails we still proceed — the apply step verifies
    // each slice before wrapping, so a mismatch degrades to a skipped
    // suggestion, never a corrupted note.
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    relocateTags(false);
    try {
      await flushSave(currentPath, view.state.doc.toString());
    } catch {
      /* already logged in flushSave */
    }
    const path = currentPath;
    linkSuggestionsLoading = true;
    linkAiLoading = false;
    linkAiError = null;
    linkAiSuggestions = [];
    linkSuggestionsOpen = true;
    try {
      const result = await invoke<LinkSuggestion[]>("suggest_wikilinks", { path });
      // Bail if the user dismissed mid-flight.
      if (!linkSuggestionsOpen || currentPath !== path) return;
      linkSuggestions = result;
      const checks: Record<string, boolean> = { ...linkSuggestionChecked };
      for (const s of result) {
        checks[suggestionKey(s)] = defaultCheckedForTitle(s.candidate_title);
      }
      linkSuggestionChecked = checks;
    } catch (e) {
      console.error("suggest_wikilinks failed", e);
      linkSuggestions = [];
    } finally {
      linkSuggestionsLoading = false;
    }
    // Fire-and-forget the AI pass. Don't await — the deterministic results
    // are already visible and useful. AI fills in below when it returns.
    void loadAiSuggestions(path);
  }

  async function loadAiSuggestions(path: string) {
    linkAiLoading = true;
    linkAiError = null;
    try {
      const result = await invoke<LinkSuggestion[]>("suggest_wikilinks_ai", { path });
      if (!linkSuggestionsOpen || currentPath !== path) return;
      linkAiSuggestions = result;
      const checks: Record<string, boolean> = { ...linkSuggestionChecked };
      for (const s of result) {
        checks[suggestionKey(s)] = defaultCheckedForTitle(s.candidate_title);
      }
      linkSuggestionChecked = checks;
    } catch (e) {
      console.error("suggest_wikilinks_ai failed", e);
      if (!linkSuggestionsOpen || currentPath !== path) return;
      linkAiError = String(e);
    } finally {
      if (currentPath === path) linkAiLoading = false;
    }
  }

  function cancelLinkSuggestions() {
    linkSuggestionsOpen = false;
    linkSuggestions = [];
    linkAiSuggestions = [];
    linkSuggestionChecked = {};
    linkAiError = null;
  }

  async function applyLinkSuggestions() {
    if (!view) return;
    // Build a change set wrapping each selected occurrence in [[...]].
    // Casing is preserved verbatim from the doc — the wikilink resolver
    // is case/slug-insensitive for existing notes, and for new-note
    // suggestions (kind: "create"), clicking the broken link will create
    // a file with whatever title is in the brackets.
    // Backend positions are UTF-8 BYTE offsets into the file; CodeMirror
    // addresses the doc in UTF-16 code units. Translate exactly (one pass
    // over the doc) — for ASCII the two coincide, but any non-ASCII char
    // before a match shifts them apart. Offsets that don't land on a char
    // boundary (drift from an edit) simply don't map and get skipped.
    const docText = view.state.doc.toString();
    const wantedBytes = new Set<number>();
    for (const s of [...linkSuggestions, ...linkAiSuggestions]) {
      if (!linkSuggestionChecked[suggestionKey(s)]) continue;
      for (const [from, to] of s.positions) {
        wantedBytes.add(from);
        wantedBytes.add(to);
      }
    }
    const byteToU16 = mapByteOffsetsToUtf16(docText, wantedBytes);

    const changes: { from: number; to: number; insert: string }[] = [];
    const acceptedCreates: { title: string }[] = [];
    for (const s of [...linkSuggestions, ...linkAiSuggestions]) {
      if (!linkSuggestionChecked[suggestionKey(s)]) continue;
      const termLower = s.term.toLowerCase();
      for (const [bFrom, bTo] of s.positions) {
        const from = byteToU16.get(bFrom);
        const to = byteToU16.get(bTo);
        if (from === undefined || to === undefined || from >= to) continue;
        const original = docText.slice(from, to);
        // Verify the slice really is the matched term before wrapping —
        // final guard against any drift between the scanned file and the
        // buffer (an edit while the modal was open, a failed flush). A
        // mismatched occurrence is skipped, never wrapped blind.
        if (original.toLowerCase() !== termLower) continue;
        changes.push({ from, to, insert: `[[${original}]]` });
      }
      if (s.kind === "create" && createNotesIfNeeded) {
        acceptedCreates.push({ title: s.candidate_title });
      }
    }
    if (changes.length === 0) {
      cancelLinkSuggestions();
      return;
    }
    // Sort by `from` descending so earlier positions don't shift while we
    // apply (CodeMirror handles this via its ChangeSet, but being explicit
    // keeps the mental model clean).
    changes.sort((a, b) => b.from - a.from);
    view.dispatch({ changes });
    // Fire-and-forget create_note for the accepted "create" suggestions.
    // If a collision occurs (note already exists by that title), the
    // backend errors — we silently skip since the wikilink will still
    // resolve to the existing note via fuzzy/slug matching.
    for (const c of acceptedCreates) {
      try {
        await invoke<string>("create_note", { title: c.title });
      } catch {
        /* note may already exist by some name variation — fine */
      }
    }
    cancelLinkSuggestions();
  }

  /** Map a set of UTF-8 byte offsets in `text` to UTF-16 code-unit
   * offsets, in one linear pass. Offsets that fall mid-character (or past
   * the end) are simply absent from the result — callers treat a missing
   * mapping as "skip this occurrence". */
  function mapByteOffsetsToUtf16(text: string, byteOffsets: Set<number>): Map<number, number> {
    const sorted = [...byteOffsets].sort((a, b) => a - b);
    const out = new Map<number, number>();
    let wi = 0;
    let byte = 0;
    let i = 0;
    while (i < text.length && wi < sorted.length) {
      while (wi < sorted.length && sorted[wi] === byte) {
        out.set(byte, i);
        wi++;
      }
      if (wi >= sorted.length) break;
      const cp = text.codePointAt(i)!;
      i += cp > 0xffff ? 2 : 1;
      byte += cp <= 0x7f ? 1 : cp <= 0x7ff ? 2 : cp <= 0xffff ? 3 : 4;
    }
    while (wi < sorted.length && sorted[wi] === byte) {
      out.set(byte, text.length);
      wi++;
    }
    return out;
  }

  function totalSelectedOccurrences(): number {
    let n = 0;
    for (const s of [...linkSuggestions, ...linkAiSuggestions]) {
      if (linkSuggestionChecked[suggestionKey(s)]) {
        n += s.positions.length;
      }
    }
    return n;
  }

  function totalSelectedSuggestions(): number {
    let n = 0;
    for (const s of [...linkSuggestions, ...linkAiSuggestions]) {
      if (linkSuggestionChecked[suggestionKey(s)]) n += 1;
    }
    return n;
  }

  function totalCandidates(): number {
    return linkSuggestions.length + linkAiSuggestions.length;
  }

  function toggleAllSuggestions(value: boolean) {
    const next: Record<string, boolean> = {};
    for (const s of [...linkSuggestions, ...linkAiSuggestions]) {
      next[suggestionKey(s)] = value;
    }
    linkSuggestionChecked = next;
  }

  function handleLinkSuggestionsKey(e: KeyboardEvent) {
    if (!linkSuggestionsOpen) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      cancelLinkSuggestions();
    } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      e.stopPropagation();
      applyLinkSuggestions();
    }
  }

  function isInVocab(tag: string): boolean {
    return tagVocabulary.includes(tag);
  }

  // Strip every occurrence of `#tag` from the doc (inline + canonical line),
  // collapsing adjacent whitespace. We dispatch as a single transaction so
  // CodeMirror's undo treats it atomically; the relocate-on-autosave will
  // tidy the canonical line if needed.
  function removeTagFromDoc(targetTag: string) {
    if (!view) return;
    const doc = view.state.doc.toString();
    const matches = findInlineTags(doc).filter((m) => m.tag === targetTag);
    if (matches.length === 0) return;
    const changes: { from: number; to: number; insert: string }[] = [];
    for (const m of matches) {
      const after = doc[m.to];
      const prev = m.from > 0 ? doc[m.from - 1] : "";
      if (after === " " || after === "\t") {
        changes.push({ from: m.from, to: m.to + 1, insert: "" });
      } else if (prev === " " || prev === "\t") {
        changes.push({ from: m.from - 1, to: m.to, insert: "" });
      } else {
        changes.push({ from: m.from, to: m.to, insert: "" });
      }
    }
    // Bypass protectTagLine: removing a tag legitimately edits the hidden
    // canonical line (where finalized tags live). Without the annotation the
    // filter drops the change and the × button silently does nothing.
    view.dispatch({ changes, annotations: internalDocRewrite.of(true) });
    // That annotation also makes the autosave listener treat this as an
    // internal echo and skip it, so schedule the save ourselves to persist
    // the removal (the debounced pass also tidies the canonical line).
    scheduleSave();
    pillMenu = null;
  }

  function openPillMenu(e: MouseEvent, tag: string) {
    e.preventDefault();
    e.stopPropagation();
    pillMenu = { tag, x: e.clientX, y: e.clientY };
  }

  function dismissPillMenu() {
    pillMenu = null;
  }

  function handlePillMenuEsc(e: KeyboardEvent) {
    if (e.key === "Escape" && pillMenu) {
      e.preventDefault();
      e.stopPropagation();
      pillMenu = null;
    }
  }

  function handleMenuFilter(tag: string) {
    pillMenu = null;
    onTagClick?.(tag);
  }

  function handleMenuPromote(tag: string) {
    const inVocab = isInVocab(tag);
    pillMenu = null;
    onTagPromote?.(tag, !inVocab);
  }

  // Tags currently extracted from the open note's content. Updated by the
  // tag-watcher ViewPlugin whenever the doc changes.
  let currentTags = $state<string[]>([]);
  // Live refs the completion source reads at trigger time (we want fresh
  // vocab + corpus, not stale closure values from when the editor mounted).
  let currentVocab: string[] = [];
  let currentAllTags: string[] = [];
  $effect(() => {
    currentVocab = tagVocabulary;
    // Repaint inline pills so vocab/ad-hoc styling stays in sync.
    if (view) view.dispatch({ effects: tagPillRedraw.of() });
  });
  $effect(() => {
    currentAllTags = allTags;
  });

  let container: HTMLDivElement;
  let view: EditorView | null = null;
  let vimComp = new Compartment();
  let saveTimer: number | null = null;
  let currentPath: string | null = null;
  let lastSavedContent = "";
  // Encryption identity captured for the note ACTUALLY loaded into the view —
  // not the live `isEncrypted`/`password` props. Props update to the INCOMING
  // note before loadPath flushes the OUTGOING one, so saving against live props
  // can write note A with note B's codec (plaintext over ciphertext, or A
  // encrypted under B's password). flushSave reads these instead, and loadPath
  // sets them at the same point it sets currentPath/lastSavedContent.
  let currentIsEncrypted = false;
  let currentPassword: string | null = null;
  let fetchGen = 0;
  // Id of the in-flight AI stream (completion / rewrite / two-pane prompt)
  // so dismissing the ghost can cancel it server-side — otherwise the
  // provider keeps generating (and billing) a completion nobody will see.
  let activeStreamId: number | null = null;
  function newStreamId(): number {
    // Random 48-bit id — unique across panes and the brew module without
    // shared state.
    return Math.floor(Math.random() * 2 ** 48);
  }
  function cancelActiveStream() {
    if (activeStreamId !== null) {
      void invoke("cancel_ai_stream", { streamId: activeStreamId }).catch(() => {});
      activeStreamId = null;
    }
  }
  // Monotonic guard so overlapping external-change handlers can't race:
  // only the most recent disk read is allowed to act on its result.
  let externalChangeGen = 0;
  // Monotonic guard for loadPath itself: rapid note switches / a password
  // change mid-read can re-enter loadPath and mount a second EditorView into
  // the same container. Only the newest call is allowed to mount (mirrors the
  // externalChangeGen pattern).
  let loadGen = 0;
  let currentHighlightQuery = "";
  let currentAllNotes: NoteRef[] = [];

  const SAVE_DEBOUNCE_MS = 300;

  // ---------------------------------------------------------------
  // Ghost-text completion: state field, widget, decoration, keymap
  // ---------------------------------------------------------------

  type Ghost =
    | { mode: "insert"; text: string; pos: number; pending?: boolean; error?: boolean }
    | { mode: "rewrite"; text: string; from: number; to: number; pending?: boolean; error?: boolean };

  const setGhost = StateEffect.define<Ghost | null>();

  class GhostWidget extends WidgetType {
    readonly text: string;
    readonly isRewrite: boolean;
    readonly pending: boolean;
    readonly isError: boolean;
    constructor(text: string, isRewrite: boolean, pending: boolean, isError: boolean) {
      super();
      this.text = text;
      this.isRewrite = isRewrite;
      this.pending = pending;
      this.isError = isError;
    }
    eq(other: GhostWidget): boolean {
      return (
        other.text === this.text &&
        other.isRewrite === this.isRewrite &&
        other.pending === this.pending &&
        other.isError === this.isError
      );
    }
    toDOM(): HTMLElement {
      const span = document.createElement("span");
      if (this.isError) {
        // A failed generation, shown briefly where the dots were — the
        // answer to "the dots vanished and I don't know why".
        span.className = "cm-ghost cm-ghost-error";
        span.textContent = `⚠ ${this.text}`;
        return span;
      }
      if (this.pending) {
        // Waiting for the model (a thinking model can take 20-30s): three
        // pulsing accent-colored dots, deliberately louder than ghost grey
        // so the pending spot stays findable while the user writes elsewhere.
        span.className = "cm-ghost cm-ghost-pending";
        for (let i = 0; i < 3; i++) {
          const dot = document.createElement("span");
          dot.className = "dot";
          dot.textContent = "●";
          span.appendChild(dot);
        }
        return span;
      }
      span.className = this.isRewrite ? "cm-ghost cm-ghost-rewrite" : "cm-ghost";
      span.textContent = this.text;
      return span;
    }
    ignoreEvent(): boolean {
      return true;
    }
  }

  // Clickable markdown task checkbox. Replaces the `[ ]` / `[x]` marker
  // (3 chars at `pos`) with a glyph; clicking flips the inner char in the
  // doc — the raw markdown stays the source of truth. Self-contained: the
  // widget wires its own mousedown so we don't have to thread events
  // through the editor's other handlers.
  class TaskCheckWidget extends WidgetType {
    readonly checked: boolean;
    readonly pos: number;
    readonly view: EditorView;
    constructor(checked: boolean, pos: number, view: EditorView) {
      super();
      this.checked = checked;
      this.pos = pos;
      this.view = view;
    }
    eq(other: TaskCheckWidget): boolean {
      // View is stable per editor; only checked/pos affect rendering.
      return other.checked === this.checked && other.pos === this.pos;
    }
    toDOM(): HTMLElement {
      const span = document.createElement("span");
      span.className = this.checked ? "cm-task-check checked" : "cm-task-check";
      span.textContent = this.checked ? "☑" : "☐";
      span.setAttribute("aria-hidden", "true");
      span.title = "Toggle to-do";
      span.addEventListener("mousedown", (e) => {
        e.preventDefault();
        e.stopPropagation();
        const cur = this.view.state.doc.sliceString(this.pos, this.pos + 3);
        if (!/^\[[ xX]\]$/.test(cur)) return; // stale — bail rather than corrupt
        const isChecked = /\[[xX]\]/.test(cur);
        this.view.dispatch({
          changes: { from: this.pos + 1, to: this.pos + 2, insert: isChecked ? " " : "x" },
        });
      });
      return span;
    }
    ignoreEvent(): boolean {
      // We handle our own mousedown; keep CM from also acting on it.
      return true;
    }
  }

  const ghostField = StateField.define<Ghost | null>({
    create() {
      return null;
    },
    update(value, tr) {
      for (const effect of tr.effects) {
        if (effect.is(setGhost)) return effect.value;
      }
      // Edits elsewhere in the note slide the anchor along instead of
      // killing the ghost — generation can take 20-30s on a local
      // thinking model, and the user shouldn't have to freeze for it.
      // Only an edit that touches the anchor itself (or the rewrite
      // range) dismisses: there the context the model saw is stale.
      // Cursor movement never dismisses; Esc declines, Tab/arrows accept
      // only at the anchor, Mod-Enter accepts from anywhere.
      if (tr.docChanged && value) {
        let touched = false;
        if (value.mode === "insert") {
          const pos = value.pos;
          tr.changes.iterChangedRanges((fromA, toA) => {
            if (fromA <= pos && toA >= pos) touched = true;
          });
          if (touched) return null;
          return { ...value, pos: tr.changes.mapPos(value.pos) };
        }
        const { from, to } = value;
        tr.changes.iterChangedRanges((fromA, toA) => {
          if (fromA <= to && toA >= from) touched = true;
        });
        if (touched) return null;
        return {
          ...value,
          from: tr.changes.mapPos(from, 1),
          to: tr.changes.mapPos(to, -1),
        };
      }
      return value;
    },
    provide: (f) =>
      EditorView.decorations.from(f, (ghost): DecorationSet => {
        if (!ghost) return Decoration.none;
        if (ghost.mode === "rewrite") {
          return Decoration.set([
            Decoration.replace({
              widget: new GhostWidget(ghost.text, true, ghost.pending ?? false, ghost.error ?? false),
            }).range(ghost.from, ghost.to),
          ]);
        }
        return Decoration.set([
          Decoration.widget({
            widget: new GhostWidget(ghost.text, false, ghost.pending ?? false, ghost.error ?? false),
            side: 1,
          }).range(ghost.pos),
        ]);
      }),
  });

  async function fetchCompletion(v: EditorView, direction = "", style = "") {
    // AI is disabled on encrypted notes — block and let the parent offer to
    // decrypt (M2). Covers both completion and rewrite (this fn handles both).
    if (isEncrypted) {
      onEncryptedAiBlocked?.(currentPath ?? "");
      return;
    }
    cancelActiveStream();
    const myGen = ++fetchGen;
    const streamId = newStreamId();
    activeStreamId = streamId;
    const sel = v.state.selection.main;
    const docLen = v.state.doc.length;
    const hasSelection = sel.from !== sel.to;

    if (hasSelection) {
      // REWRITE mode — strip hashtags from before/after context so the AI
      // doesn't see (and potentially regurgitate) `#tag` markup. The
      // selection itself is left intact in case the user wants tags
      // preserved across the rewrite.
      const before = stripTagsForAI(v.state.doc.sliceString(0, sel.from));
      const selected = v.state.doc.sliceString(sel.from, sel.to);
      const after = stripTagsForAI(v.state.doc.sliceString(sel.to, docLen));

      v.dispatch({
        effects: setGhost.of({ mode: "rewrite", text: "…", from: sel.from, to: sel.to, pending: true }),
      });

      let accumulated = "";
      let started = false;
      const channel = new Channel<string>();
      channel.onmessage = (chunk: string) => {
        if (myGen !== fetchGen || !view) return;
        // The field is the source of truth for the anchor — it slides as
        // the user edits elsewhere. Gone means an edit touched the range:
        // stop paying for tokens that will never render.
        const g = view.state.field(ghostField, false);
        if (!g || g.mode !== "rewrite") {
          cancelActiveStream();
          return;
        }
        accumulated += chunk;
        // Trim edges: model occasionally starts with a newline or ends with
        // trailing whitespace despite the "OUTPUT ONLY" rule. Interior
        // whitespace is intentional and preserved.
        const display = accumulated.replace(/\r/g, "").replace(/^\s+|\s+$/g, "");
        if (display) {
          started = true;
          v.dispatch({
            effects: setGhost.of({ ...g, text: display, pending: false }),
          });
        }
      };

      try {
        await invoke("rewrite_text_streaming", { before, selected, after, direction, style, streamId, onChunk: channel });
        if (myGen === fetchGen && !started && view) {
          v.dispatch({ effects: setGhost.of(null) });
        }
      } catch (e) {
        showGhostError(myGen, e);
        console.error("rewrite_text_streaming failed", e);
      } finally {
        if (activeStreamId === streamId) activeStreamId = null;
      }
      return;
    }

    // INSERT / CONTINUATION mode — strip hashtags from context (same
    // reasoning as REWRITE above).
    const cursor = sel.head;
    const before = stripTagsForAI(v.state.doc.sliceString(0, cursor));
    const after = stripTagsForAI(v.state.doc.sliceString(cursor, docLen));
    if (!before.trim() && !after.trim()) return;

    v.dispatch({ effects: setGhost.of({ mode: "insert", text: "…", pos: cursor, pending: true }) });

    let accumulated = "";
    let started = false;
    const channel = new Channel<string>();
    channel.onmessage = (chunk: string) => {
      if (myGen !== fetchGen || !view) return;
      // Anchor lives in the field and slides as the user edits elsewhere.
      // Gone means an edit landed on the anchor itself — stop the stream.
      const g = view.state.field(ghostField, false);
      if (!g || g.mode !== "insert") {
        cancelActiveStream();
        return;
      }
      accumulated += chunk;
      // Trim edges: model occasionally starts with a newline or ends with
      // trailing whitespace despite the "OUTPUT ONLY" rule. Interior
      // whitespace is intentional and preserved.
      const display = accumulated.replace(/\r/g, "").replace(/^\s+|\s+$/g, "");
      if (display) {
        started = true;
        v.dispatch({
          effects: setGhost.of({ ...g, text: display, pending: false }),
        });
      }
    };

    try {
      await invoke("complete_text_streaming", { before, after, direction, style, streamId, onChunk: channel });
      if (myGen === fetchGen && !started && view) {
        v.dispatch({ effects: setGhost.of(null) });
      }
    } catch (e) {
      showGhostError(myGen, e);
      console.error("complete_text_streaming failed", e);
    } finally {
      if (activeStreamId === streamId) activeStreamId = null;
    }
  }

  // Two-pane prompting (Cmd/Ctrl+Shift+'): the OTHER pane's content is a
  // raw pre-prompt prepended to THIS pane's content; the whole concatenation
  // is sent with no scaffolding (see prompt_streaming). The response streams
  // as ghost text appended at the end of this note. No-ops when no second
  // note pane is open (getPrePrompt returns null).
  async function runTwoPanePrompt(v: EditorView) {
    if (!getPrePrompt) return;
    if (isEncrypted) {
      onEncryptedAiBlocked?.(currentPath ?? "");
      return;
    }
    let prePrompt: string | null = null;
    try {
      prePrompt = await getPrePrompt();
    } catch {
      return;
    }
    if (prePrompt == null || !view) return;
    cancelActiveStream();
    const myGen = ++fetchGen;
    const streamId = newStreamId();
    activeStreamId = streamId;
    // Strip hashtags (+ the canonical tag line and wikilink brackets) from
    // BOTH panes — same hygiene as the other AI paths — so the model never
    // sees our #tag markup and can't echo or invent tags in the reply.
    const focused = stripTagsForAI(v.state.doc.toString());
    const pre = stripTagsForAI(prePrompt);
    const prompt = `${pre}\n\n${focused}`.trim();
    if (!prompt) return;
    const pos = v.state.doc.length; // append the response at the end
    v.dispatch({ effects: setGhost.of({ mode: "insert", text: "…", pos, pending: true }) });

    let accumulated = "";
    let started = false;
    const channel = new Channel<string>();
    channel.onmessage = (chunk: string) => {
      if (myGen !== fetchGen || !view) return;
      const g = view.state.field(ghostField, false);
      if (!g || g.mode !== "insert") {
        cancelActiveStream();
        return;
      }
      accumulated += chunk;
      const display = accumulated.replace(/\r/g, "").replace(/^\s+|\s+$/g, "");
      if (display) {
        started = true;
        v.dispatch({ effects: setGhost.of({ ...g, text: display, pending: false }) });
      }
    };
    try {
      await invoke("prompt_streaming", { prompt, streamId, onChunk: channel });
      if (myGen === fetchGen && !started && view) {
        v.dispatch({ effects: setGhost.of(null) });
      }
    } catch (e) {
      showGhostError(myGen, e);
      console.error("prompt_streaming failed", e);
    } finally {
      if (activeStreamId === streamId) activeStreamId = null;
    }
  }

  // Strip wrappers some models add despite the prompt's "no fences / no
  // quotes" instruction. Runs once at accept-time (not per stream chunk),
  // so it works regardless of provider. Only unwraps when the ENTIRE
  // output is enveloped — never touches fences/quotes that are part of
  // the intended text.
  function sanitizeCompletion(text: string): string {
    let t = text;
    // Whole-output triple-backtick fence: ```[lang]\n … \n```
    const fence = t.match(/^```[^\n]*\n([\s\S]*?)\n?```$/);
    if (fence) t = fence[1];
    // Whole-output matched quotes (straight or curly). Only if both ends
    // match and there's no same-quote inside (so we don't maul prose).
    const pairs: [string, string][] = [['"', '"'], ["'", "'"], ["“", "”"]];
    for (const [open, close] of pairs) {
      if (t.length >= 2 && t.startsWith(open) && t.endsWith(close)) {
        const inner = t.slice(1, -1);
        if (!inner.includes(open) && !inner.includes(close)) {
          t = inner;
          break;
        }
      }
    }
    return t;
  }

  // Whether the cursor is at the ghost: exactly on the insert anchor, or
  // inside the rewrite range. Gates the "accept" gestures that double as
  // ordinary editing keys (Tab, arrows) now that the ghost survives the
  // cursor wandering off to write elsewhere.
  function cursorAtGhost(v: EditorView): boolean {
    const ghost = v.state.field(ghostField, false);
    if (!ghost) return false;
    const head = v.state.selection.main.head;
    return ghost.mode === "insert"
      ? head === ghost.pos
      : head >= ghost.from && head <= ghost.to;
  }

  function acceptCompletion(v: EditorView): boolean {
    const ghost = v.state.field(ghostField, false);
    if (!ghost) return false;
    // Nothing to accept: pending dots are a placeholder and an error
    // notice is not text. Let the key fall through to normal editing.
    if (ghost.pending || ghost.error) return false;
    // Accepting from a cursor parked elsewhere (Mod-Enter) inserts the
    // text but leaves the cursor where the user is writing; accepting at
    // the anchor moves the cursor past the inserted text as before.
    const atGhost = cursorAtGhost(v);
    // Accepting mid-stream keeps the text shown so far; stop paying for
    // tokens that will never render.
    cancelActiveStream();
    const clean = sanitizeCompletion(ghost.text);
    if (ghost.mode === "rewrite") {
      v.dispatch({
        changes: { from: ghost.from, to: ghost.to, insert: clean },
        ...(atGhost ? { selection: { anchor: ghost.from + clean.length } } : {}),
        effects: setGhost.of(null),
      });
    } else {
      v.dispatch({
        changes: { from: ghost.pos, to: ghost.pos, insert: clean },
        ...(atGhost ? { selection: { anchor: ghost.pos + clean.length } } : {}),
        effects: setGhost.of(null),
      });
    }
    return true;
  }

  // Replace a pending/streaming ghost with a short inline error notice at
  // the same spot, auto-dismissed after a few seconds. Always insert-style
  // (a rewrite error must not keep the user's selected text hidden behind
  // a replace decoration). No-op if the ghost is already gone.
  function showGhostError(myGen: number, e: unknown) {
    if (myGen !== fetchGen || !view) return;
    const g = view.state.field(ghostField, false);
    if (!g) return;
    const msg = String(e).replace(/\s+/g, " ").trim().slice(0, 140);
    if (!msg) return;
    const pos = g.mode === "insert" ? g.pos : g.to;
    view.dispatch({ effects: setGhost.of({ mode: "insert", text: msg, pos, error: true }) });
    setTimeout(() => {
      if (myGen !== fetchGen || !view) return;
      const cur = view.state.field(ghostField, false);
      if (cur?.error) view.dispatch({ effects: setGhost.of(null) });
    }, 6000);
  }

  function declineGhost(v: EditorView): boolean {
    const ghost = v.state.field(ghostField, false);
    if (!ghost) return false;
    cancelActiveStream();
    v.dispatch({ effects: setGhost.of(null) });
    return true;
  }

  // Arrow keys accept the ghost only when the cursor is AT it (then pass
  // through so the cursor still moves). When the user has wandered off to
  // write elsewhere, arrows are just navigation — the ghost stays pending.
  // Esc declines from anywhere; Mod-Enter accepts from anywhere.
  function acceptThenPassThrough(v: EditorView): boolean {
    if (cursorAtGhost(v)) acceptCompletion(v);
    return false; // let the key's default behavior also run
  }

  /**
   * Toggle markdown wrapping around the selection (or current word if
   * no selection). `marker` is the delimiter — "**" for bold, "_" for
   * italic. Behavior:
   *
   *   1. If there's a selection, wrap it (or un-wrap if both ends are
   *      already inside identical markers).
   *   2. If there's no selection, expand to the word at the cursor and
   *      wrap that. If the cursor isn't on a word (e.g. on whitespace),
   *      insert the markers and place the cursor between them so the
   *      next keystroke starts inside the styled span.
   *
   * The cursor lands after the (now-applied or stripped) marker on the
   * trailing edge so typing can continue uninterrupted.
   */
  function toggleMarkdownWrap(v: EditorView, marker: string): void {
    const state = v.state;
    const sel = state.selection.main;
    const doc = state.doc;
    const docStr = doc.toString();
    const len = marker.length;

    let from = sel.from;
    let to = sel.to;

    // Empty selection → expand to the word at the cursor. Word chars
    // are letters, digits, underscores, hyphens — close enough for prose.
    if (from === to) {
      const isWord = (c: string) => /[\p{L}\p{N}_\-']/u.test(c);
      let s = from;
      while (s > 0 && isWord(docStr[s - 1])) s--;
      let e = to;
      while (e < docStr.length && isWord(docStr[e])) e++;
      // If cursor was between non-word chars, fall through to "insert
      // empty marker pair and place cursor between" below.
      from = s;
      to = e;
    }

    const inner = docStr.slice(from, to);

    // Check if the range is already wrapped — un-wrap if so.
    const alreadyWrapped =
      from >= len &&
      to + len <= docStr.length &&
      docStr.slice(from - len, from) === marker &&
      docStr.slice(to, to + len) === marker;

    if (alreadyWrapped) {
      v.dispatch({
        changes: [
          { from: to, to: to + len, insert: "" },
          { from: from - len, to: from, insert: "" },
        ],
        selection: { anchor: from - len, head: to - len },
      });
      return;
    }

    // If the selection itself starts and ends with the marker, strip
    // the inner markers (covers "select **word** and toggle").
    const inlineWrapped =
      inner.length >= 2 * len &&
      inner.startsWith(marker) &&
      inner.endsWith(marker);
    if (inlineWrapped) {
      const stripped = inner.slice(len, inner.length - len);
      v.dispatch({
        changes: { from, to, insert: stripped },
        selection: { anchor: from, head: from + stripped.length },
      });
      return;
    }

    // Wrap the range. Empty inner → just place cursor between markers
    // so the user types inside the styled span.
    if (inner.length === 0) {
      v.dispatch({
        changes: { from, to, insert: marker + marker },
        selection: { anchor: from + len, head: from + len },
      });
    } else {
      v.dispatch({
        changes: { from, to, insert: marker + inner + marker },
        selection: { anchor: from + len, head: from + len + inner.length },
      });
    }
  }

  const completionKeymap = Prec.highest(
    keymap.of([
      {
        // Mod-; = AI/Insert. Previously Mod-i (collided with the
        // universal italics convention), then Mod-j (turned out to
        // be webview/OS-grabbed on Tauri — silently swallowed before
        // CodeMirror ever saw it). Mod-; is safe across browsers,
        // OSes, and Tauri's webview, and stays close to the home
        // row on QWERTY.
        key: "Mod-;",
        run: (v) => {
          void fetchCompletion(v);
          return true;
        },
      },
      {
        // Mod-Shift-; = steer: pop a modal for a one-line direction
        // note, then run the same completion/rewrite with that nudge
        // injected as a <direction> block. Bind both ";" and its
        // shifted form ":" since browsers/OS report the shifted key
        // inconsistently.
        key: "Mod-Shift-;",
        run: () => {
          openSteer();
          return true;
        },
      },
      {
        key: "Mod-Shift-:",
        run: () => {
          openSteer();
          return true;
        },
      },
      {
        // Mod-Shift-' = two-pane prompt: prepend the OTHER pane's content
        // as a raw pre-prompt and stream the response here. Bind both the
        // apostrophe and its shifted double-quote form (reported
        // inconsistently across OS/browser).
        key: "Mod-Shift-'",
        run: (v) => {
          void runTwoPanePrompt(v);
          return true;
        },
      },
      {
        key: 'Mod-Shift-"',
        run: (v) => {
          void runTwoPanePrompt(v);
          return true;
        },
      },
      // Mod-b → toggle **bold** around selection or word at cursor.
      // Mod-i → toggle _italic_ around selection or word at cursor.
      // Wrapped in completionKeymap so they get Prec.highest like AI.
      {
        key: "Mod-b",
        run: (v) => {
          toggleMarkdownWrap(v, "**");
          return true;
        },
      },
      {
        key: "Mod-i",
        run: (v) => {
          toggleMarkdownWrap(v, "_");
          return true;
        },
      },
      {
        // Mod-Shift-b = brew. Fires onBrew callback; parent opens the
        // brew pane and streams the model's response into it.
        key: "Mod-Shift-b",
        run: (v) => {
          if (!onBrew) return false;
          onBrew(v.state.doc.toString());
          return true;
        },
      },
      { key: "Mod-Enter", run: (v) => acceptCompletion(v) },
      { key: "Escape", run: (v) => declineGhost(v) },
      // Tab accepts only at the anchor — elsewhere it stays an ordinary
      // Tab so writing in another paragraph isn't hijacked by a pending
      // completion.
      { key: "Tab", run: (v) => (cursorAtGhost(v) ? acceptCompletion(v) : false) },
      { key: "ArrowLeft", run: acceptThenPassThrough },
      { key: "ArrowRight", run: acceptThenPassThrough },
      { key: "ArrowUp", run: acceptThenPassThrough },
      { key: "ArrowDown", run: acceptThenPassThrough },
      {
        key: "Mod-w",
        run: () => {
          if (onClose) {
            onClose();
            return true;
          }
          return false;
        },
      },
      {
        key: "Mod-r",
        run: () => {
          if (onRename && currentPath) {
            onRename(currentPath);
            return true;
          }
          return false;
        },
      },
      {
        key: "Mod-Shift-l",
        run: () => {
          void openLinkSuggestions();
          return true;
        },
      },
    ])
  );

  // Combined mousedown: prioritize wikilink navigation over ghost-accept.
  // Use DOM-target detection (not posAtCoords): we only navigate when the
  // click LANDS ON the rendered link span. Clicks past end-of-line, in
  // gutter, or in margins fall through to default cursor positioning even
  // when the nearest doc position happens to coincide with a wikilink edge.
  const completionMouseHandlers = EditorView.domEventHandlers({
    mousedown(event, view) {
      const evTarget = event.target as HTMLElement | null;
      const linkEl = evTarget?.closest?.(".cm-wikilink, .cm-wikilink-broken");
      if (linkEl) {
        // Prefer the data-target the decoration attached (the resolvable
        // half of an aliased link). Fall back to parsing the visible text:
        // strip brackets, drop any |alias — clicking an aliased link used
        // to create a junk note literally titled "Target-Alias".
        const attr = linkEl.getAttribute("data-target");
        const raw = attr ?? linkEl.textContent ?? "";
        const target = raw
          .replace(/^\[\[/, "")
          .replace(/\]\]$/, "")
          .split("|")[0]
          .trim();
        if (target) {
          const alt = event.metaKey || event.ctrlKey;
          declineGhost(view);
          event.preventDefault();
          void handleWikilinkClick(target, alt);
          return true;
        }
      }
      const ghost = view.state.field(ghostField, false);
      if (ghost) acceptCompletion(view);
      return false;
    },
  });

  // ---------------------------------------------------------------
  // Query-match highlighting (mirrors the list-row highlighter)
  // ---------------------------------------------------------------

  const isWordChar = (ch: string) => /[\p{L}\p{N}_]/u.test(ch);

  function withinEditDistanceOne(a: string, b: string): boolean {
    const ac = [...a];
    const bc = [...b];
    const [short, long] = ac.length <= bc.length ? [ac, bc] : [bc, ac];
    if (long.length - short.length > 1) return false;
    if (long.length === short.length) {
      let m = 0;
      for (let i = 0; i < short.length; i++) {
        if (short[i] !== long[i]) {
          m++;
          if (m > 1) return false;
        }
      }
      return true;
    }
    let i = 0,
      j = 0,
      skipped = false;
    while (i < short.length && j < long.length) {
      if (short[i] === long[j]) {
        i++;
        j++;
      } else if (!skipped) {
        skipped = true;
        j++;
      } else {
        return false;
      }
    }
    return true;
  }

  function wordMatchesTerm(wordLower: string, term: string): boolean {
    if (wordLower.includes(term)) return true;
    if ([...term].length >= 4 && withinEditDistanceOne(wordLower, term)) return true;
    return false;
  }

  function findMatchRanges(text: string, query: string): [number, number][] {
    const q = query.trim();
    if (!q) return [];
    const terms = q
      .split(/\s+/)
      .map((t) => t.toLowerCase())
      .filter((t) => t.length > 0);
    if (!terms.length) return [];
    const ranges: [number, number][] = [];
    const n = text.length;
    let i = 0;
    while (i < n) {
      if (!isWordChar(text[i])) {
        i++;
        continue;
      }
      const start = i;
      while (i < n && isWordChar(text[i])) i++;
      const end = i;
      const wordLower = text.slice(start, end).toLowerCase();
      for (const term of terms) {
        if (wordMatchesTerm(wordLower, term)) {
          ranges.push([start, end]);
          break;
        }
      }
    }
    return ranges;
  }

  function buildHighlightDecorations(text: string, query: string): DecorationSet {
    const ranges = findMatchRanges(text, query);
    if (!ranges.length) return Decoration.none;
    return Decoration.set(
      ranges.map(([from, to]) =>
        Decoration.mark({ class: "cm-search-match" }).range(from, to)
      )
    );
  }

  // ---------------------------------------------------------------
  // Wikilinks: [[Name]] decoration + resolver + autocomplete + click
  // ---------------------------------------------------------------

  const WIKILINK_RE = /\[\[([^\[\]\n]+)\]\]/g;

  function slugify(s: string): string {
    return s.toLowerCase().replace(/[\s_\-]+/g, "");
  }

  function resolveWikilink(target: string): NoteRef | null {
    // [[Target|Alias]]: only the part left of the first pipe resolves.
    const pipe = target.indexOf("|");
    if (pipe >= 0) target = target.slice(0, pipe);
    const t = target.trim().toLowerCase();
    if (!t) return null;
    const pool = allNotes.length ? allNotes : currentAllNotes;
    let m = pool.find((n) => n.title.toLowerCase() === t);
    if (m) return m;
    const slug = slugify(target);
    m = pool.find((n) => slugify(n.title) === slug);
    return m ?? null;
  }

  const wikilinkRedraw = StateEffect.define<void>();

  const wikilinkPlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = this.compute(view);
      }
      update(u: ViewUpdate) {
        if (
          u.docChanged ||
          u.viewportChanged ||
          u.selectionSet || // recompute on cursor move so brackets re-hide
          u.transactions.some((tr) =>
            tr.effects.some((e) => e.is(wikilinkRedraw))
          )
        ) {
          this.decorations = this.compute(u.view);
        }
      }
      compute(view: EditorView): DecorationSet {
        // Collect first, sort, then add — RangeSetBuilder requires ascending
        // `from` order, and we add mark + (optional) two replaces per link.
        type Add = { from: number; to: number; dec: Decoration };
        const adds: Add[] = [];
        const sel = view.state.selection.main;
        for (const { from, to } of view.visibleRanges) {
          const text = view.state.doc.sliceString(from, to);
          WIKILINK_RE.lastIndex = 0;
          let m;
          while ((m = WIKILINK_RE.exec(text)) !== null) {
            const start = from + m.index;
            const end = start + m[0].length;
            const inner = m[1];
            const pipeIdx = inner.indexOf("|");
            const target = (pipeIdx >= 0 ? inner.slice(0, pipeIdx) : inner).trim();
            const resolved = resolveWikilink(target);
            // Three states for wikilinks:
            //   - broken: no matching note → cm-wikilink-broken (dashed amber)
            //   - empty:  matching note but the body is empty → cm-wikilink-empty (muted)
            //   - filled: matching note with content → cm-wikilink (live blue)
            // Three states for wikilinks; CSS handles the actual paint.
            // The cascade fight with oneDark's tok-link is resolved by
            // hoisting the entire wikilinkPlugin to Prec.highest so OUR
            // span becomes the inner one (see extension list below).
            const cls = !resolved
              ? "cm-wikilink-broken"
              : resolved.is_empty
                ? "cm-wikilink cm-wikilink-empty"
                : "cm-wikilink";
            adds.push({
              from: start,
              to: end,
              // data-target carries the RESOLVABLE half so the click
              // handler doesn't have to re-derive it from the (possibly
              // alias-only) visible text.
              dec: Decoration.mark({ class: cls, attributes: { "data-target": target } }),
            });
            // Hide `[[` and `]]` unless the cursor / selection is anywhere
            // inside [start, end] (inclusive at both ends — so the cursor
            // arriving at the link's edge reveals the brackets too). For
            // aliased links the hidden opening run extends through
            // "Target|" so only the alias shows — standard wiki behavior.
            const cursorInLink = sel.from <= end && sel.to >= start;
            if (!cursorInLink) {
              const openHideTo = pipeIdx >= 0 ? start + 2 + pipeIdx + 1 : start + 2;
              adds.push({ from: start, to: openHideTo, dec: Decoration.replace({}) });
              adds.push({ from: end - 2, to: end, dec: Decoration.replace({}) });
            }
          }
        }
        adds.sort((a, b) => a.from - b.from || a.to - b.to);
        const builder = new RangeSetBuilder<Decoration>();
        for (const a of adds) builder.add(a.from, a.to, a.dec);
        return builder.finish();
      }
    },
    {
      decorations: (p) => p.decorations,
    }
  );

  // ─── Inline markdown (bold / italic) ────────────────────────────────
  //
  // Visual rendering for **bold** and _italic_ markers. The actual
  // styling is applied as a mark decoration on the inner text. When
  // the user's cursor / selection isn't touching the span, the markers
  // themselves get hidden via Decoration.replace, so the editor reads
  // WYSIWYG. The malt.alwaysShowMarkdown setting disables the hide so
  // markers are always visible (useful for plain-markdown purists).
  //
  // We deliberately only match **bold** and _italic_ — `*italic*` is
  // skipped because it collides with bullet list markers, and `__bold__`
  // gets the same shape as **bold** but is rarely typed by hand.

  // **bold** OR __bold__. Capture group 1 = the marker pair so we can
  // backreference for the closing token. The inner text must start
  // and end with a non-whitespace/non-marker char (rules out `** **`
  // and avoids gobbling list-item asterisks).
  const BOLD_RE = /(\*\*|__)(?=\S)([^\n]*?\S)\1/g;
  // *italic* OR _italic_. Must NOT be a bold marker (the negative
  // lookahead `(?!\1)` after the opening marker prevents matching the
  // first `*` of `**`). Surrounding chars (look-behind / look-ahead)
  // must not be word chars or the same marker — so `snake_case` and
  // `**bold**` aren't mis-matched, but `_italic_` standing alone is.
  const ITALIC_RE = /(?<![*_\w])(\*|_)(?!\1)(?=\S)([^\n]*?\S)\1(?![*_\w])/g;

  function getAlwaysShowMarkdown(): boolean {
    return (
      typeof localStorage !== "undefined" &&
      localStorage.getItem("malt.alwaysShowMarkdown") === "1"
    );
  }

  const mdInlineRedraw = StateEffect.define<void>();

  const mdInlinePlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = this.compute(view);
      }
      update(u: ViewUpdate) {
        if (
          u.docChanged ||
          u.viewportChanged ||
          u.selectionSet ||
          u.transactions.some((tr) =>
            tr.effects.some((e) => e.is(mdInlineRedraw))
          )
        ) {
          this.decorations = this.compute(u.view);
        }
      }
      compute(view: EditorView): DecorationSet {
        type Add = { from: number; to: number; dec: Decoration };
        const adds: Add[] = [];
        const sel = view.state.selection.main;
        const alwaysShow = getAlwaysShowMarkdown();
        for (const { from, to } of view.visibleRanges) {
          const text = view.state.doc.sliceString(from, to);
          // Bold first: **X** OR __X__ — marker is 2 chars either way.
          this.scan(adds, text, from, sel, BOLD_RE, "cm-md-bold", alwaysShow);
          // Italic second: *X* OR _X_ — marker is 1 char either way.
          // Runs AFTER bold so the bold ranges already exist; italic
          // patterns inside bold (e.g. **a *b* c**) match independently
          // and produce a nested span via CodeMirror's RangeSet.
          this.scan(adds, text, from, sel, ITALIC_RE, "cm-md-italic", alwaysShow);
        }
        adds.sort((a, b) => a.from - b.from || a.to - b.to);
        const builder = new RangeSetBuilder<Decoration>();
        for (const a of adds) builder.add(a.from, a.to, a.dec);
        return builder.finish();
      }
      scan(
        adds: { from: number; to: number; dec: Decoration }[],
        text: string,
        offset: number,
        sel: { from: number; to: number },
        re: RegExp,
        cls: string,
        alwaysShow: boolean,
      ): void {
        re.lastIndex = 0;
        let m;
        while ((m = re.exec(text)) !== null) {
          const start = offset + m.index;
          const end = start + m[0].length;
          // Marker length is captured in group 1 (either `**`/`__` for
          // bold or `*`/`_` for italic), so we derive it instead of
          // hard-coding.
          const markerLen = m[1].length;
          // Style the inner text. We mark the WHOLE span (markers
          // included) so the visual bold/italic also colors the
          // markers when they're visible — and so we can hide markers
          // independently below without losing styling.
          adds.push({ from: start, to: end, dec: Decoration.mark({ class: cls }) });
          const cursorInside = sel.from <= end && sel.to >= start;
          if (!cursorInside && !alwaysShow) {
            adds.push({
              from: start,
              to: start + markerLen,
              dec: Decoration.replace({}),
            });
            adds.push({
              from: end - markerLen,
              to: end,
              dec: Decoration.replace({}),
            });
          }
        }
      }
    },
    {
      decorations: (p) => p.decorations,
    }
  );

  // Listen for the "always-show markdown" toggle changing in Settings —
  // when it flips we need to recompute every editor's decorations.
  function handleMdToggle() {
    if (view) view.dispatch({ effects: mdInlineRedraw.of() });
  }

  // ----- Task checkboxes ------------------------------------------------
  // Render markdown task markers (`- [ ]` / `- [x]`, also `*`/`+`/`1.`)
  // as clickable checkboxes. The cursor-inside check keeps the raw `[ ]`
  // editable when you're typing on that line.
  const TASK_LINE_RE = /^(\s*(?:[-*+]|\d+\.)\s+)\[([ xX])\]/;
  const taskCheckboxPlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = this.compute(view);
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.viewportChanged || u.selectionSet) {
          this.decorations = this.compute(u.view);
        }
      }
      compute(view: EditorView): DecorationSet {
        const adds: { from: number; to: number; dec: Decoration }[] = [];
        const sel = view.state.selection.main;
        for (const { from, to } of view.visibleRanges) {
          let pos = from;
          while (pos <= to) {
            const line = view.state.doc.lineAt(pos);
            const m = TASK_LINE_RE.exec(line.text);
            if (m) {
              const bracketStart = line.from + m[1].length;
              const bracketEnd = bracketStart + 3;
              const checked = m[2].toLowerCase() === "x";
              // Leave the raw marker visible/editable while the cursor is
              // in it; otherwise swap in the checkbox widget.
              const cursorInside = sel.from <= bracketEnd && sel.to >= bracketStart;
              if (!cursorInside) {
                adds.push({
                  from: bracketStart,
                  to: bracketEnd,
                  dec: Decoration.replace({
                    widget: new TaskCheckWidget(checked, bracketStart, view),
                  }),
                });
              }
            }
            pos = line.to + 1;
          }
        }
        const builder = new RangeSetBuilder<Decoration>();
        for (const a of adds) builder.add(a.from, a.to, a.dec);
        return builder.finish();
      }
    },
    {
      decorations: (p) => p.decorations,
    },
  );

  // ----- Hashtag plumbing ----------------------------------------------
  // Three ViewPlugins:
  //   - tagWatcher: extract current tags whenever the doc changes
  //   - tagPillPlugin: style inline #tags as pill chips
  //   - tagLineHider: hide the canonical tag line at the bottom (replace
  //     decoration so the cursor can't enter it either)

  // CodeMirror Text instances are immutable, so whole-doc derivations can
  // be cached per doc version. Before this, tagWatcher + tagPillPlugin +
  // tagLineHider + the protectTagLine transaction filter EACH stringified
  // and rescanned the entire document on every keystroke AND cursor move.
  const inlineTagsCache = new WeakMap<object, ReturnType<typeof findInlineTags>>();
  function cachedInlineTags(doc: { toString(): string }): ReturnType<typeof findInlineTags> {
    let v = inlineTagsCache.get(doc);
    if (!v) {
      v = findInlineTags(doc.toString());
      inlineTagsCache.set(doc, v);
    }
    return v;
  }
  const canonicalRangeCache = new WeakMap<object, { from: number; to: number } | null>();
  function cachedCanonicalRange(doc: { toString(): string }): { from: number; to: number } | null {
    if (!canonicalRangeCache.has(doc)) {
      canonicalRangeCache.set(doc, canonicalTagLineRange(doc.toString()));
    }
    return canonicalRangeCache.get(doc) ?? null;
  }

  // A hashtag is "in progress" while the caret sits inside it or at its
  // trailing edge — the user is still typing or editing it. An in-progress
  // tag is NOT finalized (no pill in the row, no relocation to the canonical
  // line) until a boundary follows it or the caret moves off it. `m` offsets
  // and `caret` are both doc-absolute.
  function isCaretInTag(m: { from: number; to: number }, caret: number): boolean {
    return caret > m.from && caret <= m.to;
  }

  // The canonical tag line, but only once it's FINALIZED. While the caret is on
  // that line the user is still typing/editing the tag, so treat it as ordinary
  // editable text — not hidden, not protected, not pilled, not in the tag row —
  // until the caret leaves the line. Without this, a freshly-typed `#j` on a line
  // below body text is instantly classified as the (hidden, protected) canonical
  // line, which hid it mid-word AND made protectTagLine drop the next keystroke.
  function liveCanonicalRange(state: EditorState): { from: number; to: number } | null {
    const range = cachedCanonicalRange(state.doc);
    if (!range) return null;
    const head = state.selection.main.head;
    const line = state.doc.lineAt(range.from);
    if (head >= line.from && head <= line.to) return null;
    return range;
  }

  const tagWatcher = ViewPlugin.fromClass(
    class {
      constructor(view: EditorView) {
        this.refresh(view);
      }
      update(u: ViewUpdate) {
        // Refresh on selection moves too: leaving a half-typed tag should make
        // its pill appear, and entering one should hide it again.
        if (u.docChanged || u.selectionSet) this.refresh(u.view);
      }
      refresh(view: EditorView) {
        const doc = view.state.doc.toString();
        const caret = view.state.selection.main.head;
        let canonical = findCanonicalTagLine(doc);
        // (doc string built once here; the heavy per-move rescans below use
        // the per-Text caches.)
        // The line the caret is on isn't a finalized tag line yet — treat it as
        // ordinary text so its tags don't pop into the row while being typed.
        if (canonical) {
          const cl = view.state.doc.line(canonical.lineIdx + 1);
          if (caret >= cl.from && caret <= cl.to) canonical = null;
        }
        const aboveText = canonical
          ? doc.split("\n").slice(0, canonical.lineIdx).join("\n")
          : doc;
        // Skip the tag the caret is still inside — no pill for a half-typed
        // hashtag; it appears once a boundary follows or the caret moves away.
        const inline = cachedInlineTags(view.state.doc)
          .filter((m) => (canonical ? m.to <= (aboveText.length) : true))
          .filter((m) => !isCaretInTag(m, caret))
          .map((m) => m.tag);
        const set = new Set<string>([...(canonical?.tags ?? []), ...inline]);
        const next = [...set].sort();
        // Only assign if changed so we don't trigger unnecessary reactivity.
        if (
          next.length !== currentTags.length ||
          next.some((t, i) => t !== currentTags[i])
        ) {
          currentTags = next;
        }
      }
    }
  );

  // StateEffect to trigger a redraw of inline pills when the vocab changes
  // (so a freshly promoted tag re-styles from ad-hoc to vocab without a doc
  // edit).
  const tagPillRedraw = StateEffect.define<void>();

  const tagPillPlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = this.compute(view);
      }
      update(u: ViewUpdate) {
        if (
          u.docChanged ||
          u.viewportChanged ||
          // Recompute on caret moves too: a half-typed tag must pill the moment
          // the caret leaves it (cursor move / click) and un-pill when re-entered.
          u.selectionSet ||
          u.transactions.some((tr) => tr.effects.some((e) => e.is(tagPillRedraw)))
        ) {
          this.decorations = this.compute(u.view);
        }
      }
      compute(view: EditorView): DecorationSet {
        const builder = new RangeSetBuilder<Decoration>();
        const caret = view.state.selection.main.head;
        const canonical = liveCanonicalRange(view.state);
        const matches = cachedInlineTags(view.state.doc);
        for (const m of matches) {
          // Skip matches inside the canonical tag line — that whole line is
          // hidden by tagLineHider, so styling individual pills there is wasted.
          if (canonical && m.from >= canonical.from && m.to <= canonical.to) continue;
          // Don't pill the tag the caret is still inside: a half-typed #tag
          // stays plain editable text and only becomes a pill once the caret
          // leaves it (space / return / cursor move / blur). Mirrors tagWatcher,
          // so the inline pill and the tag-row pill finalize at the same moment.
          if (isCaretInTag(m, caret)) continue;
          const cls = currentVocab.includes(m.tag)
            ? "cm-hashtag-inline"
            : "cm-hashtag-inline cm-hashtag-adhoc";
          builder.add(m.from, m.to, Decoration.mark({ class: cls }));
        }
        return builder.finish();
      }
    },
    { decorations: (p) => p.decorations }
  );

  // CodeMirror requires replace-decorations that span line breaks to come
  // from a StateField (line-structure model needs to see them as state, not
  // as a view-plugin afterthought). Our hider eats the preceding newline so
  // it crosses a line boundary, so this lives in a StateField even though
  // tag-pill marks (which never span newlines) stay in a ViewPlugin.
  function computeTagLineHide(state: EditorState): DecorationSet {
    const range = liveCanonicalRange(state);
    if (!range) return Decoration.none;
    const line = state.doc.lineAt(range.from);
    const from = line.from > 0 ? line.from - 1 : line.from;
    return Decoration.set([Decoration.replace({}).range(from, line.to)]);
  }

  const tagLineHider = StateField.define<DecorationSet>({
    create(state) {
      return computeTagLineHide(state);
    },
    update(value, tr) {
      // Recompute on selection changes too: the hide now depends on where the
      // caret is (the line it sits on is shown as editable, not hidden), so it
      // must update when the caret moves on/off the tag line, not just on edits.
      if (!tr.docChanged && !tr.selection) return value;
      return computeTagLineHide(tr.state);
    },
    provide: (f) => EditorView.decorations.from(f),
  });

  // Marks a transaction as an internal, programmatic rewrite (the
  // relocate-tags-to-bottom transform, external-change reloads, full-doc
  // resets) so the protect filter below lets it through. User keystrokes
  // carry no annotation and are subject to protection.
  const internalDocRewrite = Annotation.define<boolean>();

  // Protect the hidden canonical tag line from BOTH user edits and the cursor.
  //
  // The old approach (EditorState.changeFilter) only blocked *deletions* that
  // touched the region — CodeMirror still allows "insertions adjacent to it",
  // so typing at the very end of a note (cursor at/after the hidden tags)
  // either merged the character into the last tag or briefly un-hid the whole
  // line (the "flash") until the debounced relocate fired. A transactionFilter
  // is authoritative: it (1) drops any user change that reaches past the end of
  // the visible body, and (2) clamps the selection so the caret can never sit
  // on or after the hidden tags. Net effect: Ctrl+End / Cmd+Down / a click
  // below the tags all land at the end of the visible body, and typing there
  // appends to the body — tags never appear in the editor and never absorb a
  // keystroke. Internal rewrites (relocate-to-bottom, external reloads) carry
  // the internalDocRewrite annotation and pass through untouched.
  const protectTagLine = EditorState.transactionFilter.of((tr) => {
    if (tr.annotation(internalDocRewrite)) return tr;
    // Only protect a FINALIZED tag line. If the caret is on it, the user is
    // typing/editing the tag — let every keystroke through (this is what was
    // dropping characters when you typed a #tag on a line below body text).
    const range = liveCanonicalRange(tr.startState);
    if (!range) return tr;
    const line = tr.startState.doc.lineAt(range.from);
    // `guard` = last editable position: just before the hidden separator
    // newline that precedes the tag line. Everything > guard is off-limits.
    const guard = line.from - 1;

    // (1) Block any user change that reaches into the protected region. This
    // catches boundary insertions (the char-into-tag merge) as well as
    // deletions/replacements. Drop the edit and park the caret at the body end
    // so the keystroke the user *meant* for the body still has somewhere to go.
    if (tr.docChanged) {
      let reachesTags = false;
      tr.changes.iterChangedRanges((_fromA, toA) => {
        if (toA > guard) reachesTags = true;
      });
      if (reachesTags) {
        return { selection: { anchor: guard } };
      }
    }

    // (2) Keep the caret out of the hidden region on plain navigation
    // (Ctrl+End, Cmd+Down, clicking below the tags).
    if (tr.selection && !tr.docChanged) {
      const { anchor, head } = tr.selection.main;
      if (anchor > guard || head > guard) {
        return [
          tr,
          { selection: { anchor: Math.min(anchor, guard), head: Math.min(head, guard) } },
        ];
      }
    }
    return tr;
  });

  // Make the hidden tag region atomic so arrow keys / word-motion skip
  // over it cleanly instead of stepping into invisible characters.
  const tagLineAtomic = EditorView.atomicRanges.of((view) => {
    return view.state.field(tagLineHider, false) ?? Decoration.none;
  });

  // Completion source for hashtag autocomplete. Fires when the user has
  // typed `#letter...` preceded by whitespace / start-of-line. Vocabulary
  // tags rank first, then any corpus tag, then anything the user has
  // already started typing (passes through unchanged).
  function hashtagCompletions(context: CompletionContext): CompletionResult | null {
    const line = context.state.doc.lineAt(context.pos);
    const before = line.text.slice(0, context.pos - line.from);
    const m = before.match(/(?:^|[\s(\[{"'])#([a-zA-Z][a-zA-Z0-9_/-]*)?$/);
    if (!m) return null;
    const typed = (m[1] ?? "").toLowerCase();
    const startOfPartial = context.pos - typed.length;
    const seen = new Set<string>();
    const opts: { label: string; type: string; boost?: number; detail?: string }[] = [];
    for (const t of currentVocab) {
      if (seen.has(t)) continue;
      if (!t.startsWith(typed)) continue;
      seen.add(t);
      opts.push({ label: t, type: "tag", boost: 10, detail: "vocab" });
    }
    for (const t of currentAllTags) {
      if (seen.has(t)) continue;
      if (!t.startsWith(typed)) continue;
      seen.add(t);
      opts.push({ label: t, type: "tag" });
    }
    if (opts.length === 0 && typed.length === 0) return null;
    return {
      from: startOfPartial,
      to: context.pos,
      options: opts,
      validFor: /^[a-zA-Z0-9_/-]*$/,
    };
  }

  // Synchronous lookup of a wikilink at a given doc position. Used by the
  // click handler and the autocomplete; scans only the line containing pos.
  function wikilinkAtPos(text: string, line: { from: number; text: string }, pos: number): string | null {
    const posInLine = pos - line.from;
    WIKILINK_RE.lastIndex = 0;
    let m;
    while ((m = WIKILINK_RE.exec(line.text)) !== null) {
      const start = m.index;
      const end = m.index + m[0].length;
      if (posInLine >= start && posInLine <= end) {
        return m[1].trim();
      }
    }
    return null;
  }

  async function handleWikilinkClick(target: string, alt: boolean) {
    const resolved = resolveWikilink(target);
    if (resolved) {
      onNavigate?.(resolved.path, alt);
    } else if (onCreate) {
      await onCreate(target, alt);
    }
  }

  function wikilinkCompletions(context: CompletionContext): CompletionResult | null {
    const line = context.state.doc.lineAt(context.pos);
    const posInLine = context.pos - line.from;
    const before = line.text.slice(0, posInLine);
    const lastOpen = before.lastIndexOf("[[");
    if (lastOpen < 0) return null;
    // No closing ]] (or stray ]) between [[ and cursor — abort.
    if (before.slice(lastOpen + 2).includes("]")) return null;

    const partial = before.slice(lastOpen + 2);
    const q = partial.toLowerCase();

    // Read live: allNotes is a reactive prop. Fall back to the snapshot if
    // the prop hasn't propagated yet during very first render.
    const pool = allNotes.length ? allNotes : currentAllNotes;
    const scored = pool
      .map((n) => {
        const t = n.title.toLowerCase();
        let score = -1;
        if (!q) score = 1;
        else if (t.startsWith(q)) score = 100 - (t.length - q.length);
        else if (t.includes(q)) score = 50 - (t.length - q.length);
        return { n, score };
      })
      .filter((x) => x.score >= 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, 10);

    if (scored.length === 0) return null;

    const options: Completion[] = scored.map(({ n }) => {
      const title = n.title;
      return {
        label: title,
        type: "text",
        // Smart apply: look ahead from `to` for an existing ...]] tail
        // (no stray [ or newline before it) and consume it; otherwise add
        // a fresh ]]. Either way the result is a clean [[Title]].
        apply: (view: EditorView, _completion: Completion, from: number, to: number) => {
          const doc = view.state.doc;
          const lookahead = doc.sliceString(to, Math.min(to + 200, doc.length));
          let tailLen = 0;
          for (let i = 0; i < lookahead.length; i++) {
            const ch = lookahead[i];
            if (ch === "\n" || ch === "[") break;
            if (ch === "]" && lookahead[i + 1] === "]") {
              tailLen = i + 2;
              break;
            }
          }
          const insert = title + "]]";
          view.dispatch({
            changes: { from, to: to + tailLen, insert },
            selection: { anchor: from + insert.length },
            userEvent: "input.complete",
          });
        },
      };
    });

    return {
      from: line.from + lastOpen + 2,
      to: context.pos,
      options,
      validFor: /^[^\[\]\n]*$/,
    };
  }

  const setHighlightQuery = StateEffect.define<string>();

  const highlightField = StateField.define<DecorationSet>({
    create(state) {
      return buildHighlightDecorations(state.doc.toString(), currentHighlightQuery);
    },
    update(value, tr) {
      let queryChanged = false;
      for (const effect of tr.effects) {
        if (effect.is(setHighlightQuery)) {
          currentHighlightQuery = effect.value;
          queryChanged = true;
        }
      }
      if (queryChanged || tr.docChanged) {
        return buildHighlightDecorations(tr.state.doc.toString(), currentHighlightQuery);
      }
      return value.map(tr.changes);
    },
    provide: (f) => EditorView.decorations.from(f),
  });

  // ---------------------------------------------------------------
  // Standard editor wiring
  // ---------------------------------------------------------------

  function getVimEnabled(): boolean {
    return typeof localStorage !== "undefined" && localStorage.getItem("malt.vim") === "1";
  }

  function emitCount(text: string) {
    if (!onCount) return;
    const trimmed = text.trim();
    const words = trimmed ? trimmed.split(/\s+/).length : 0;
    const chars = text.length;
    onCount(words, chars);
  }

  async function flushSave(p: string, content: string) {
    if (content === lastSavedContent) return;
    // Bind the codec to the BUFFER being saved (captured for the loaded note),
    // never to live props — see currentIsEncrypted/currentPassword above.
    try {
      if (currentIsEncrypted) {
        // Encrypted note but no captured password (the note is locked, or its
        // password was cleared on blur). Falling back to save_note here would
        // write PLAINTEXT over the on-disk ciphertext and silently destroy the
        // encryption. Dropping one autosave is strictly safer than corrupting
        // the file — skip and warn rather than save.
        if (!currentPassword) {
          console.warn(
            "malt: skipping autosave of encrypted note with no password (locked?):",
            p,
          );
          return;
        }
        await invoke("save_encrypted_note", {
          path: p,
          content,
          password: currentPassword,
        });
      } else {
        await invoke("save_note", { path: p, content });
      }
      lastSavedContent = content;
      onSaved?.();
    } catch (e) {
      // Genuine save failure (IPC/disk error). Log, then re-throw so an
      // explicit flushAllEditors() before a destructive op can detect it and
      // abort rather than lose content. The fire-and-forget autosave/blur/
      // destroy callers .catch() this so they don't raise unhandled rejections.
      console.error("save_note failed", e);
      throw e;
    }
  }

  // Relocate inline #tags to the hidden canonical line at the bottom, then
  // clamp the caret to the end of the visible body. When `force` is false a
  // tag the caret is still inside is left in place (the user is mid-type — see
  // v0.4.16); `force` finalizes every tag regardless, used on focus loss where
  // the user is clearly done with it.
  function relocateTags(force: boolean) {
    if (!view) return;
    const oldContent = view.state.doc.toString();
    const caret = view.state.selection.main.head;
    const typingTag =
      !force && findInlineTags(oldContent).some((m) => isCaretInTag(m, caret));
    const { body: newContent, cuts } = typingTag
      ? { body: oldContent, cuts: [] as [number, number][] }
      : relocateTagsToBottom(oldContent);
    if (newContent === oldContent) return;
    // Map the caret through the removed spans so it stays at the same
    // LOGICAL position. Keeping the raw offset (the old behavior) made the
    // cursor jump rightward — usually to the end of the body via the clamp
    // — whenever a tag ABOVE the caret relocated on autosave.
    let mapped = caret;
    for (const [f, t] of cuts) {
      if (t <= caret) {
        mapped -= t - f;
      } else if (f < caret) {
        mapped -= caret - f;
      }
    }
    // Clamp to the end of the VISIBLE body — never into or past the
    // now-hidden canonical tag line, or the next keystroke could merge into
    // a tag (#foo + #bar -> #foo#bar) and re-expose it. (The blank-line
    // collapse can still drift the mapped position slightly left; the clamp
    // plus drift-toward-start keeps it out of the hidden region.)
    const range = canonicalTagLineRange(newContent);
    const maxCaret = range
      ? newContent.slice(0, range.from).replace(/\s+$/, "").length
      : newContent.length;
    const cursor = Math.max(0, Math.min(mapped, maxCaret));
    view.dispatch({
      changes: { from: 0, to: oldContent.length, insert: newContent },
      selection: { anchor: cursor },
      // Legitimately rebuilds the canonical tag line, so bypass protectTagLine.
      annotations: internalDocRewrite.of(true),
    });
  }

  // The parent deleted a note. If it's the one in this editor, mark the
  // buffer clean and cancel any pending autosave — otherwise the upcoming
  // pane switch would flush the old content back to the deleted path and
  // save_note would recreate the file ("deleted" notes resurrecting).
  function handleNoteDeleted(e: Event) {
    const detail = (e as CustomEvent<{ path: string }>).detail;
    if (!detail || detail.path !== currentPath || !view) return;
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    lastSavedContent = view.state.doc.toString();
  }

  // Finalize any in-progress tag when the editor loses focus — whether to
  // another part of the app or another window entirely. Deferred a microtask
  // so we never dispatch into a view that navigation is tearing down (loadPath
  // nulls `view` synchronously before this would run).
  function finalizeTagsOnBlur() {
    queueMicrotask(() => {
      if (!view || !currentPath || externalConflict !== null) return;
      // Blur normally ends composition, but if the IME is somehow still
      // composing, don't dispatch a doc-replacing relocate into it (would risk
      // dropping characters). Persist the current text as-is and let the
      // compositionend handler finalize tags once the IME commits.
      if (view.composing) {
        flushSave(currentPath, view.state.doc.toString()).catch(() => {});
        return;
      }
      relocateTags(true);
      flushSave(currentPath, view.state.doc.toString()).catch(() => {});
    });
  }

  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer);
    if (!currentPath || !view) return;
    const p = currentPath;
    saveTimer = window.setTimeout(() => {
      if (!view) return;
      // Mid-IME composition (choosing a CJK/accented candidate): relocateTags
      // dispatches a full-doc replace, which cancels the in-flight composition
      // and can drop or duplicate the partially-composed characters. Don't
      // touch the doc now — re-arm the debounce so the save lands just after
      // composition ends (also covered by the compositionend handler below).
      if (view.composing) {
        saveTimer = null;
        scheduleSave();
        return;
      }
      // A conflict is pending resolution — don't write either side until
      // the user chooses, or we'd silently clobber the on-disk version.
      if (externalConflict !== null) {
        saveTimer = null;
        return;
      }
      // Migrate inline hashtags to the hidden canonical line, then save.
      relocateTags(false);
      flushSave(p, view.state.doc.toString()).catch(() => {});
      saveTimer = null;
    }, SAVE_DEBOUNCE_MS) as unknown as number;
  }

  async function loadPath(p: string | null) {
    // Claim this load. Any await below yields to the event loop, so a newer
    // loadPath (rapid switch / password change) can start; only the newest
    // myGen is allowed to mount a view (see loadGen).
    const myGen = ++loadGen;
    // Capture the codec for the note we're ABOUT to load, before any prop can
    // shift underneath us. These are the values that read `p`, so they're the
    // correct values to later write `p` back with (committed to
    // currentIsEncrypted/currentPassword only once this load wins, below).
    const encForThisLoad = isEncrypted;
    const pwForThisLoad = password;
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    if (view && currentPath) {
      // Flush the OUTGOING buffer with ITS still-current captured codec
      // (currentIsEncrypted/currentPassword aren't reassigned until this load
      // wins below, so they still describe the note being left here). This
      // runs on SAME-path reloads too — a password change (lock on blur,
      // unlock) re-runs loadPath with currentPath === p, and skipping the
      // flush there discarded up to 300ms of typing. flushSave no-ops when
      // the buffer is clean. A failed flush must not abort the load
      // (flushSave re-throws); log + continue.
      try {
        await flushSave(currentPath, view.state.doc.toString());
      } catch {
        /* already logged in flushSave */
      }
    }
    if (view) {
      view.destroy();
      view = null;
    }
    currentPath = p;
    // A pending conflict belongs to the note we're leaving — drop it.
    externalConflict = null;
    if (!p) return;

    let body = "";
    try {
      if (encForThisLoad) {
        if (!pwForThisLoad) {
          // Locked: render an empty editor so the user sees nothing
          // sensitive. Parent will pop the password modal.
          body = "";
        } else {
          body = await invoke<string>("read_encrypted_note", { path: p, password: pwForThisLoad });
        }
      } else {
        body = await invoke<string>("read_note", { path: p });
      }
    } catch (e) {
      console.error("read_note failed", e);
      body = "";
    }
    // Superseded mid-read by a newer loadPath — bail BEFORE touching the
    // save-identity globals or mounting, so the stale read can't mount a
    // duplicate view or leave the wrong codec captured for the live buffer.
    if (myGen !== loadGen) return;
    lastSavedContent = body;
    // Commit the captured codec for the now-loaded note. flushSave reads these
    // (never the live props) so the buffer is always written with its own codec.
    currentIsEncrypted = encForThisLoad;
    currentPassword = pwForThisLoad;

    // Custom setup — basicSetup includes drawSelection() which conflicts
    // with WebView2's native contenteditable selection. Native selection
    // works reliably; we just need the rest of the editor amenities.
    // Ensure the field's initial create() sees the current prop value.
    currentHighlightQuery = query;
    currentAllNotes = allNotes;

    // Initial count for the loaded note.
    emitCount(body);
    const extensions = [
      completionKeymap,
      completionMouseHandlers,
      // Editor lost focus (to another app pane, a pill, a modal, or another
      // window) → finalize any tag the user was mid-typing.
      EditorView.domEventHandlers({
        blur: () => {
          finalizeTagsOnBlur();
          return false;
        },
        // IME composition just finished — any relocate/save we deferred while
        // view.composing was true (see scheduleSave) can run now. Re-arm the
        // debounce so the canonical tag line is rebuilt for the committed text.
        compositionend: () => {
          scheduleSave();
          return false;
        },
      }),
      lineNumbers(),
      highlightActiveLineGutter(),
      highlightSpecialChars(),
      history(),
      indentOnInput(),
      syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
      bracketMatching(),
      closeBrackets(),
      autocompletion({
        override: [wikilinkCompletions, hashtagCompletions],
        activateOnTyping: true,
      }),
      // Render tooltips (including the autocomplete popup) in a fixed-position
      // layer so they escape the overflow:hidden on the editor pane wrappers
      // (added for the linkbacks split). Without this, the popup is clipped
      // and invisible even though it exists in CM's internal state.
      tooltips({ position: "fixed" }),
      highlightActiveLine(),
      // search() adds the find/replace panel; searchKeymap binds Cmd+F /
      // Cmd+G / Cmd+Shift+G / Cmd+Alt+F (replace) etc. inside the editor.
      search({ top: true }),
      // Override Mod-f with a TOGGLE before searchKeymap's open-only binding.
      // Higher Prec + returning true short-circuits the searchKeymap version
      // so a second press closes the panel instead of just refocusing it.
      Prec.high(
        keymap.of([
          {
            key: "Mod-f",
            run: (v) => {
              toggleSearchPanel(v);
              return true;
            },
          },
        ])
      ),
      keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...historyKeymap, ...searchKeymap]),
      markdown(),
      oneDark,
      EditorView.lineWrapping,
      vimComp.of(getVimEnabled() ? vim() : []),
      ghostField,
      highlightField,
      // Prec.highest is the actual fix for wikilink coloring. CodeMirror
      // nests overlapping mark decorations: a HIGHER-precedence decoration
      // becomes the INNER span. The markdown link-token decoration from
      // syntaxHighlighting(oneDark) was outpriotizing our default-precedence
      // wikilink decoration, making oneDark's `tok-link` span inner and
      // winning the color cascade. Hoisting wikilinkPlugin to Prec.highest
      // inverts that: our wikilink span becomes inner, so its color (set
      // via class + inline style) wins. !important and inline styles on
      // the outer were both ineffective for the same reason.
      Prec.highest(wikilinkPlugin),
      // Same Prec.highest trick as wikilinks: oneDark + lang-markdown
      // already styles strong/em ranges, but we want our font-weight/style
      // and marker-hiding to win, so we hoist precedence.
      Prec.highest(mdInlinePlugin),
      taskCheckboxPlugin,
      tagWatcher,
      tagPillPlugin,
      tagLineHider,
      protectTagLine,
      tagLineAtomic,
      EditorView.updateListener.of((u) => {
        if (u.docChanged) {
          // Our own internal doc rewrites — external-change reloads and the
          // tag-relocation pass — must NOT schedule a save. Otherwise merely
          // *viewing* a note another tool just edited would relocate its
          // tags and write malt's version back over that tool's file (an
          // echo write). Real user edits carry no such annotation and
          // autosave normally.
          const internal = u.transactions.some(
            (tr) => !!tr.annotation(internalDocRewrite),
          );
          if (!internal) scheduleSave();
          emitCount(u.state.doc.toString());
        }
        // Finalize a freshly-typed tag on a plain cursor move too — not only on
        // edit (debounce) and blur. When the caret LEAVES an inline tag, relocate
        // it to the canonical line right away, so its inline copy doesn't linger
        // beside the tag-row pill until the next keystroke/blur. Deferred via a
        // microtask (can't dispatch inside an update); the relocate's own rewrite
        // carries internalDocRewrite, so it can't re-trigger this.
        if (u.selectionSet && !u.docChanged && externalConflict === null) {
          const internalSel = u.transactions.some(
            (tr) => !!tr.annotation(internalDocRewrite),
          );
          const prevHead = u.startState.selection.main.head;
          const newHead = u.state.selection.main.head;
          const leftATag =
            !internalSel &&
            cachedInlineTags(u.startState.doc).some(
              (m) => isCaretInTag(m, prevHead) && !isCaretInTag(m, newHead),
            );
          if (leftATag) {
            queueMicrotask(() => {
              if (!view || !currentPath || externalConflict !== null) return;
              relocateTags(false);
              void flushSave(currentPath, view.state.doc.toString()).catch(() => {});
            });
          }
        }
        // Open wikilink completion whenever the cursor sits inside an open
        // [[...  context. CodeMirror's auto-trigger only fires on word
        // chars, so brackets alone wouldn't open it. Idempotent: if a popup
        // is already showing, this just keeps it.
        if (u.docChanged || u.selectionSet) {
          const cursor = u.state.selection.main.head;
          const line = u.state.doc.lineAt(cursor);
          const before = line.text.slice(0, cursor - line.from);
          const lastOpen = before.lastIndexOf("[[");
          if (lastOpen >= 0 && !before.slice(lastOpen + 2).includes("]")) {
            startCompletion(u.view);
          }
        }
      }),
    ];
    const state = EditorState.create({ doc: body, extensions });
    // Final re-entrancy guard right before we touch the DOM: if a newer
    // loadPath superseded us, do NOT append a second EditorView to the
    // container (the orphaned one would leak listeners and fire saves against
    // the wrong note). `view` is already null here, so bailing is clean.
    if (myGen !== loadGen) return;
    view = new EditorView({ state, parent: container });

    const active = document.activeElement;
    const inputFocused =
      active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement;
    // A locked note renders an EMPTY hidden view behind the unlock form —
    // focusing it would swallow keystrokes (Enter typed newlines into the
    // void instead of reaching the unlock flow). Leave focus where it is;
    // unlocking remounts the view and focuses it then.
    const locked = encForThisLoad && !pwForThisLoad;
    if (!inputFocused && !locked) {
      view.focus();
    }

    if (getVimEnabled() && body.length === 0) {
      // @replit/codemirror-vim types handleKey's first arg as its legacy
      // `CodeMirror` adapter, but the CM6 build accepts the EditorView at
      // runtime (that's the documented CM6 entry point). Cast through
      // unknown to satisfy the checker without a behavior change.
      Vim.handleKey(view as unknown as Parameters<typeof Vim.handleKey>[0], "i", "macro");
    }

    onReady?.(view);
    // Hand the parent a function that TOGGLES this view's search panel.
    // Used by the global Cmd+F forwarder; closing on a second press
    // keeps behavior consistent with the in-editor binding.
    if (onFinderReady) {
      const v = view;
      onFinderReady(() => toggleSearchPanel(v));
    }
  }

  function handleVimChange(e: Event) {
    const detail = (e as CustomEvent<boolean>).detail;
    if (view) {
      view.dispatch({
        effects: vimComp.reconfigure(detail ? vim() : []),
      });
    }
  }

  let unlistenNotes: UnlistenFn | null = null;

  async function handleExternalChange() {
    if (!currentPath || !view) return;
    // Burst events (sync storms) fire this repeatedly; tag each run so a
    // slower earlier read can't apply its result after a newer one.
    const myGen = ++externalChangeGen;
    // Read with the CAPTURED codec for the loaded buffer — the same
    // discipline flushSave uses. The live isEncrypted/password props update
    // to the INCOMING note during a switch, so reading currentPath with
    // them could fetch note A's raw envelope as if it were plaintext and
    // surface ciphertext in the conflict bar.
    const pathAtStart = currentPath;
    const encAtStart = currentIsEncrypted;
    const pwAtStart = currentPassword;
    let fresh = "";
    try {
      if (encAtStart && pwAtStart) {
        fresh = await invoke<string>("read_encrypted_note", {
          path: pathAtStart,
          password: pwAtStart,
        });
      } else if (encAtStart) {
        // Locked: don't try to read. Save would clobber anyway.
        return;
      } else {
        fresh = await invoke<string>("read_note", { path: pathAtStart });
      }
    } catch {
      return;
    }
    // A newer external-change handler started while we awaited the read —
    // let it win; acting on our now-stale `fresh` could clobber. Same if
    // the editor moved to a different note (or tore down) mid-read.
    if (myGen !== externalChangeGen) return;
    if (!view || currentPath !== pathAtStart) return;
    // Disk already matches what we last wrote — nothing external happened
    // (this is also how our own autosave echoes back via the watcher).
    if (fresh === lastSavedContent) return;
    const current = view.state.doc.toString();
    // Buffer already equals disk — just resync our marker + clear any
    // stale conflict.
    if (current === fresh) {
      lastSavedContent = fresh;
      externalConflict = null;
      return;
    }
    // The file changed on disk to something we didn't write. If the buffer
    // also has unsaved edits, both sides diverged from our last save — a
    // genuine conflict. Don't clobber the user's work: surface a choice and
    // let scheduleSave pause autosave until they resolve it.
    if (current !== lastSavedContent) {
      externalConflict = fresh;
      return;
    }
    // Clean buffer (nothing unsaved) → safe to fast-forward to the new
    // on-disk version live. Bypass the tag-line protect filter.
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: fresh },
      annotations: internalDocRewrite.of(true),
    });
    lastSavedContent = fresh;
  }

  $effect(() => {
    // Re-load whenever either the path OR the password changes. The
    // password change case happens after the user unlocks an encrypted
    // note: parent flips `password` from null to the actual string,
    // and we need to re-decrypt and rebuild the view.
    const _ = password;
    void loadPath(path);
  });

  // Push query changes into the open editor so highlights update live.
  $effect(() => {
    const q = query;
    if (view && q !== currentHighlightQuery) {
      view.dispatch({ effects: setHighlightQuery.of(q) });
    }
  });

  // Push allNotes changes so the wikilink decoration ViewPlugin recomputes
  // and so the snapshot fallback in completions stays current.
  $effect(() => {
    const notes = allNotes;
    currentAllNotes = notes;
    if (view) {
      view.dispatch({ effects: wikilinkRedraw.of() });
    }
  });

  let unregisterFlusher: (() => void) | null = null;

  onMount(async () => {
    window.addEventListener("malt:vim-changed", handleVimChange);
    window.addEventListener("malt:markdown-toggle-changed", handleMdToggle);
    // Use capture so the pill menu's Esc handling beats the parent's global
    // Esc-clears-search behavior in +page.svelte.
    window.addEventListener("keydown", handlePillMenuEsc, true);
    window.addEventListener("keydown", handleLinkSuggestionsKey, true);
    // App window lost focus (alt-tab to another application) → finalize any
    // in-progress tag, same as an in-app blur.
    window.addEventListener("blur", finalizeTagsOnBlur);
    window.addEventListener("malt:note-deleted", handleNoteDeleted);
    unlistenNotes = await listen("notes_changed", handleExternalChange);
    // Allow the parent to await all pending saves before a rename / other
    // path-mutating op so we don't write stale content to a renamed file.
    unregisterFlusher = registerEditorFlusher(async () => {
      if (currentPath && view) {
        if (saveTimer) {
          clearTimeout(saveTimer);
          saveTimer = null;
        }
        // Flush unconditionally (flushSave no-ops when the buffer is already
        // saved); a genuine failure propagates to flushAllEditors so a pending
        // destructive op (rename/move/vault-switch) aborts instead of losing edits.
        await flushSave(currentPath, view.state.doc.toString());
      }
    });
  });

  onDestroy(() => {
    window.removeEventListener("malt:vim-changed", handleVimChange);
    window.removeEventListener("malt:markdown-toggle-changed", handleMdToggle);
    window.removeEventListener("keydown", handlePillMenuEsc, true);
    window.removeEventListener("keydown", handleLinkSuggestionsKey, true);
    window.removeEventListener("blur", finalizeTagsOnBlur);
    window.removeEventListener("malt:note-deleted", handleNoteDeleted);
    unlistenNotes?.();
    unregisterFlusher?.();
    if (saveTimer && currentPath && view) {
      flushSave(currentPath, view.state.doc.toString()).catch(() => {});
    }
    view?.destroy();
  });
</script>

{#if currentTags.length > 0}
  <div class="tag-row">
    {#each currentTags as t (t)}
      <span
        class="tag-pill-wrap"
        class:adhoc={!isInVocab(t)}
      >
        <button
          class="tag-pill"
          onclick={() => onTagClick?.(t)}
          oncontextmenu={(e) => openPillMenu(e, t)}
          title={isInVocab(t)
            ? `Filter by tag:${t} · right-click for more`
            : `Ad-hoc tag (not in vocabulary) · right-click for more`}
          tabindex="-1"
        >#{t}</button>
        <button
          class="tag-pill-remove"
          onclick={(e) => { e.stopPropagation(); removeTagFromDoc(t); }}
          title={`Remove #${t} from this note`}
          tabindex="-1"
          aria-label={`Remove tag ${t}`}
        >×</button>
      </span>
    {/each}
  </div>
{/if}
{#if externalConflict !== null}
  <div class="conflict-bar" role="alert">
    <span class="conflict-msg">⚠ changed on disk while you had unsaved edits</span>
    <button
      class="conflict-btn"
      onclick={useMyVersion}
      title="Keep what's in the editor and overwrite the file on disk"
      tabindex="-1"
    >keep mine</button>
    <button
      class="conflict-btn theirs"
      onclick={useTheirVersion}
      title="Discard your unsaved edits and load the version from disk"
      tabindex="-1"
    >use theirs</button>
  </div>
{/if}
<div bind:this={container} class="editor"></div>

{#if steerOpen}
  <div class="steer-backdrop" role="presentation" onclick={cancelSteer}>
    <div
      class="steer-panel"
      role="dialog"
      aria-modal="true"
      aria-label="Steer the generation"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="steer-label">steer the generation</div>
      <input
        class="steer-input"
        type="text"
        placeholder="e.g. make it darker · pivot to the counterargument · shorter, punchier"
        bind:this={steerInputEl}
        bind:value={steerText}
        onkeydown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); void submitSteer(); }
          else if (e.key === "Escape") { e.preventDefault(); cancelSteer(); }
        }}
      />
      {#if steerStyleNotes.length > 0}
        <label class="steer-style-row">
          <span class="steer-style-label">house style</span>
          <select
            class="steer-style-select"
            bind:value={steerStylePath}
            onkeydown={(e) => {
              if (e.key === "Enter") { e.preventDefault(); void submitSteer(); }
              else if (e.key === "Escape") { e.preventDefault(); cancelSteer(); }
            }}
          >
            <option value="">none</option>
            {#each steerStyleNotes as n (n.path)}
              <option value={n.path}>{n.title}</option>
            {/each}
          </select>
        </label>
      {/if}
      <div class="steer-hint">
        A one-time nudge for the AI generation at your cursor (or your selection).
        {#if steerStyleNotes.length > 0}The house style (any note tagged #prompt) rides along as a standing instruction.{/if}
        Enter to generate · Esc to cancel.
      </div>
    </div>
  </div>
{/if}

{#if linkSuggestionsOpen}
  <div
    class="link-modal-backdrop"
    role="presentation"
    onclick={cancelLinkSuggestions}
  >
    <div
      class="link-modal"
      role="dialog"
      aria-modal="true"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="link-modal-header">
        <span class="link-modal-title">wikilink suggestions</span>
        <span class="link-modal-hint">
          {#if linkSuggestionsLoading && linkSuggestions.length === 0}
            scanning…
          {:else if totalCandidates() === 0 && !linkAiLoading}
            no matches found
          {:else}
            {totalCandidates()} candidate{totalCandidates() === 1 ? "" : "s"}
            · {totalSelectedSuggestions()} selected
            {#if totalSelectedOccurrences() !== totalSelectedSuggestions()}
              ({totalSelectedOccurrences()} occurrence{totalSelectedOccurrences() === 1 ? "" : "s"})
            {/if}
          {/if}
        </span>
      </div>
      {#if !linkSuggestionsLoading && linkSuggestions.length > 0}
        <div class="link-modal-tools">
          <button class="link-tool-btn" onclick={() => toggleAllSuggestions(true)}>select all</button>
          <button class="link-tool-btn" onclick={() => toggleAllSuggestions(false)}>select none</button>
          <label class="link-create-toggle">
            <input type="checkbox" bind:checked={createNotesIfNeeded} />
            Create new-note files immediately
          </label>
        </div>
      {/if}
      <div class="link-modal-body">
        {#if linkSuggestionsLoading && linkSuggestions.length === 0}
          <div class="link-empty">scanning your notes for matches…</div>
        {:else}
          {#if linkSuggestions.length > 0}
            <div class="link-section-label">Link to existing notes</div>
            <ul class="link-list">
              {#each linkSuggestions as s (s.candidate_path)}
                <li class="link-row">
                  <label class="link-label">
                    <input
                      type="checkbox"
                      checked={linkSuggestionChecked[suggestionKey(s)] ?? false}
                      onchange={(e) => {
                        linkSuggestionChecked = {
                          ...linkSuggestionChecked,
                          [suggestionKey(s)]: (e.target as HTMLInputElement).checked,
                        };
                      }}
                    />
                    <span class="link-term">{s.term}</span>
                    {#if s.term.toLowerCase() !== s.candidate_title.toLowerCase()}
                      <span class="link-arrow">→</span>
                      <span class="link-title">{s.candidate_title}</span>
                    {/if}
                    <span class="link-count">×{s.positions.length}</span>
                  </label>
                </li>
              {/each}
            </ul>
          {/if}

          <div class="link-section-label ai">
            Could be new notes
            {#if linkAiLoading}
              <span class="link-ai-status">thinking…</span>
            {:else if linkAiError}
              <span class="link-ai-status err">{linkAiError}</span>
            {:else if linkAiSuggestions.length === 0 && linkSuggestions.length > 0}
              <span class="link-ai-status">no new entities found</span>
            {/if}
          </div>
          {#if linkAiSuggestions.length > 0}
            <ul class="link-list">
              {#each linkAiSuggestions as s (suggestionKey(s))}
                <li class="link-row ai">
                  <label class="link-label">
                    <input
                      type="checkbox"
                      checked={linkSuggestionChecked[suggestionKey(s)] ?? false}
                      onchange={(e) => {
                        linkSuggestionChecked = {
                          ...linkSuggestionChecked,
                          [suggestionKey(s)]: (e.target as HTMLInputElement).checked,
                        };
                      }}
                    />
                    <span class="link-term">{s.term}</span>
                    {#if s.term.toLowerCase() !== s.candidate_title.toLowerCase()}
                      <span class="link-arrow">→</span>
                      <span class="link-title">{s.candidate_title}</span>
                    {/if}
                    <span class="link-count">×{s.positions.length}</span>
                    <span class="link-create-hint">new</span>
                  </label>
                </li>
              {/each}
            </ul>
          {/if}

          {#if linkSuggestions.length === 0 && linkAiSuggestions.length === 0 && !linkAiLoading}
            <div class="link-empty">
              No matches found. Either there's nothing linkable here, or the AI couldn't surface any candidates worth a separate note.
            </div>
          {/if}
        {/if}
      </div>
      <div class="link-modal-actions">
        <button class="link-btn cancel" onclick={cancelLinkSuggestions}>cancel</button>
        <button
          class="link-btn confirm"
          onclick={applyLinkSuggestions}
          disabled={linkSuggestionsLoading || totalSelectedSuggestions() === 0}
        >Apply {totalSelectedSuggestions()} wikilink{totalSelectedSuggestions() === 1 ? "" : "s"}{#if totalSelectedOccurrences() !== totalSelectedSuggestions()} ({totalSelectedOccurrences()} wraps){/if}</button>
      </div>
    </div>
  </div>
{/if}

{#if pillMenu}
  <div
    class="pill-menu-backdrop"
    role="presentation"
    onclick={dismissPillMenu}
    oncontextmenu={(e) => { e.preventDefault(); dismissPillMenu(); }}
  ></div>
  <div
    class="pill-menu"
    style:left={`${pillMenu.x}px`}
    style:top={`${pillMenu.y}px`}
    role="menu"
  >
    <button class="pill-menu-item" onclick={() => handleMenuFilter(pillMenu!.tag)}>
      Filter by <span class="op">#{pillMenu.tag}</span>
    </button>
    <button class="pill-menu-item" onclick={() => removeTagFromDoc(pillMenu!.tag)}>
      Remove from note
    </button>
    <button class="pill-menu-item" onclick={() => handleMenuPromote(pillMenu!.tag)}>
      {isInVocab(pillMenu.tag) ? "Demote from vocab" : "Promote to vocab"}
    </button>
  </div>
{/if}

<style>
  .tag-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 4px 10px;
    background: #161616;
    border-bottom: 1px solid #2a2a2a;
    flex-shrink: 0;
  }
  .tag-pill-wrap {
    display: inline-flex;
    align-items: stretch;
    border: 1px solid rgba(108, 182, 255, 0.25);
    border-radius: 9px;
    overflow: hidden;
    background: rgba(108, 182, 255, 0.08);
    line-height: 1;
  }
  .tag-pill-wrap.adhoc {
    border-color: rgba(220, 180, 100, 0.28);
    background: rgba(220, 180, 100, 0.07);
  }
  .tag-pill {
    background: transparent;
    border: 0;
    color: #97b8d8;
    font: inherit;
    font-size: 10px;
    padding: 1px 7px 2px;
    cursor: pointer;
    line-height: 1.5;
  }
  .tag-pill-wrap.adhoc .tag-pill {
    color: #d6b06a;
    font-style: italic;
  }
  .tag-pill-wrap:hover .tag-pill {
    background: rgba(108, 182, 255, 0.18);
    color: #cce1f5;
  }
  .tag-pill-wrap.adhoc:hover .tag-pill {
    background: rgba(220, 180, 100, 0.18);
    color: #f1d394;
  }
  /* × button is hidden until the wrap is hovered (keyboard focus also reveals it). */
  .tag-pill-remove {
    background: transparent;
    border: 0;
    border-left: 1px solid rgba(108, 182, 255, 0.18);
    color: #66798c;
    font: inherit;
    font-size: 11px;
    padding: 0 5px;
    cursor: pointer;
    line-height: 1;
    width: 0;
    overflow: hidden;
    transition: width 100ms ease, padding 100ms ease;
    padding-inline: 0;
  }
  .tag-pill-wrap:hover .tag-pill-remove,
  .tag-pill-remove:focus-visible {
    width: auto;
    padding: 0 5px;
  }
  .tag-pill-remove:hover {
    background: rgba(220, 80, 80, 0.15);
    color: #f4a5a5;
  }
  .tag-pill-wrap.adhoc .tag-pill-remove {
    border-left-color: rgba(220, 180, 100, 0.22);
  }
  :global(.cm-hashtag-inline) {
    background: rgba(108, 182, 255, 0.08);
    border: 1px solid rgba(108, 182, 255, 0.22);
    border-radius: 6px;
    color: #97b8d8 !important;
    padding: 0 3px;
    font-size: 92%;
  }
  /* Clickable task checkbox (replaces the `[ ]` / `[x]` marker). */
  :global(.cm-task-check) {
    cursor: pointer;
    color: #7a8694;
    user-select: none;
  }
  :global(.cm-task-check:hover) {
    color: #aab4c0;
  }
  :global(.cm-task-check.checked) {
    color: #7ed29b;
  }
  :global(.cm-hashtag-inline.cm-hashtag-adhoc) {
    background: rgba(220, 180, 100, 0.07);
    border-color: rgba(220, 180, 100, 0.24);
    color: #d6b06a !important;
    font-style: italic;
  }
  .pill-menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 199;
  }
  .pill-menu {
    position: fixed;
    z-index: 200;
    background: #1c1c1c;
    border: 1px solid #333;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.5);
    padding: 4px;
    min-width: 180px;
    display: flex;
    flex-direction: column;
  }
  .pill-menu-item {
    background: transparent;
    border: 0;
    color: #ccc;
    font: inherit;
    font-size: 12px;
    text-align: left;
    padding: 5px 10px;
    cursor: pointer;
    white-space: nowrap;
  }
  .pill-menu-item:hover {
    background: #2a2a2a;
    color: #fff;
  }
  .pill-menu-item .op {
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    color: #97b8d8;
  }
  .steer-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    z-index: 250;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 22vh;
  }
  .steer-panel {
    background: #1a1a1a;
    border: 1px solid #333;
    border-top: 2px solid #d6b06a;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
    width: min(560px, 90%);
    padding: 14px 16px;
  }
  .steer-label {
    color: #d6b06a;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    margin-bottom: 8px;
  }
  .steer-input {
    width: 100%;
    box-sizing: border-box;
    background: #0f0f0f;
    border: 1px solid #333;
    color: #e0e0e0;
    font: inherit;
    font-size: 14px;
    padding: 8px 10px;
    outline: 0;
  }
  .steer-input:focus {
    border-color: #d6b06a;
  }
  .steer-hint {
    color: #666;
    font-size: 11px;
    margin-top: 8px;
    line-height: 1.4;
  }
  .steer-style-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
  }
  .steer-style-label {
    color: #888;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    white-space: nowrap;
  }
  .steer-style-select {
    flex: 1;
    background: #161616;
    border: 1px solid #2e2e2e;
    color: #e0e0e0;
    font: inherit;
    font-size: 12px;
    padding: 4px 6px;
    border-radius: 2px;
  }
  .steer-style-select:focus {
    outline: none;
    border-color: #6cb6ff;
  }
  .link-modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    z-index: 250;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 32px;
  }
  .link-modal {
    background: #1a1a1a;
    border: 1px solid #333;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
    width: min(620px, 100%);
    max-height: 80vh;
    display: flex;
    flex-direction: column;
  }
  .link-modal-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 10px 16px;
    border-bottom: 1px solid #2a2a2a;
  }
  .link-modal-title {
    color: #888;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .link-modal-hint {
    color: #666;
    font-size: 11px;
  }
  .link-modal-tools {
    display: flex;
    gap: 6px;
    padding: 6px 16px;
    border-bottom: 1px solid #232323;
    background: #161616;
  }
  .link-tool-btn {
    background: transparent;
    border: 1px solid #2e2e2e;
    color: #888;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 2px 8px;
    cursor: pointer;
    border-radius: 2px;
  }
  .link-tool-btn:hover {
    border-color: #555;
    color: #e0e0e0;
  }
  .link-create-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: #aaa;
    font-size: 11px;
    cursor: pointer;
    margin-left: auto;
  }
  .link-create-toggle input {
    margin: 0;
  }
  .link-modal-body {
    flex: 1 1 auto;
    overflow-y: auto;
    min-height: 0;
  }
  .link-empty {
    padding: 24px 20px;
    color: #777;
    font-size: 12px;
    line-height: 1.5;
    text-align: center;
  }
  .link-section-label {
    padding: 6px 16px 4px;
    color: #888;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    background: #161616;
    border-bottom: 1px solid #232323;
    border-top: 1px solid #232323;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }
  .link-section-label.ai {
    color: #d6b06a;
  }
  .link-ai-status {
    color: #777;
    font-size: 9px;
    text-transform: none;
    font-style: italic;
    letter-spacing: 0;
  }
  .link-ai-status.err {
    color: #c66;
  }
  .link-list {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .link-row.ai .link-term {
    color: #f1d394;
  }
  .link-row.ai .link-title {
    color: #d6b06a;
  }
  .link-create-hint {
    color: #d6b06a;
    background: rgba(220, 180, 100, 0.12);
    border: 1px solid rgba(220, 180, 100, 0.25);
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 0 5px;
    border-radius: 8px;
    margin-left: 4px;
  }
  .link-row {
    border-bottom: 1px solid #232323;
  }
  .link-row:last-child {
    border-bottom: 0;
  }
  .link-label {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    cursor: pointer;
    font-size: 13px;
    color: #ccc;
  }
  .link-label:hover {
    background: #1f1f1f;
  }
  .link-label input[type="checkbox"] {
    margin: 0;
    flex-shrink: 0;
  }
  .link-term {
    color: #e0e0e0;
    font-weight: 500;
  }
  .link-arrow {
    color: #555;
  }
  .link-title {
    color: #97b8d8;
    font-style: italic;
  }
  .link-count {
    margin-left: auto;
    color: #666;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .link-modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 10px 16px;
    border-top: 1px solid #2a2a2a;
    background: #161616;
  }
  .link-btn {
    background: transparent;
    border: 1px solid #333;
    color: #aaa;
    font: inherit;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 6px 14px;
    cursor: pointer;
  }
  .link-btn:hover {
    border-color: #555;
    color: #e0e0e0;
  }
  .link-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .link-btn.confirm:hover:not(:disabled) {
    border-color: #6cb6ff;
    color: #cce6ff;
    background: rgba(108, 182, 255, 0.08);
  }
  .editor {
    flex: 1 1 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  /* Non-destructive conflict affordance: shown when the file changed on
     disk while the buffer had unsaved edits. Sits above the editor; the
     user keeps editing freely and resolves whenever. */
  .conflict-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px;
    background: #3a2e16;
    border-bottom: 1px solid #5c4a1f;
    color: #e6c875;
    font-size: 11px;
    flex: 0 0 auto;
  }
  .conflict-msg {
    flex: 1 1 auto;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .conflict-btn {
    flex: 0 0 auto;
    background: #2a2a2a;
    border: 1px solid #5c4a1f;
    color: #e6c875;
    font: inherit;
    font-size: 11px;
    padding: 2px 8px;
    cursor: pointer;
    border-radius: 3px;
  }
  .conflict-btn:hover {
    background: #3a3320;
    color: #fff;
  }
  .conflict-btn.theirs {
    border-color: #555;
    color: #bbb;
  }
  .conflict-btn.theirs:hover {
    background: #333;
    color: #fff;
  }
  :global(.editor .cm-editor) {
    flex: 1 1 auto;
    min-height: 0;
    background: #1a1a1a;
  }
  :global(.editor .cm-editor.cm-focused) {
    outline: none;
  }
  :global(.editor .cm-scroller) {
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 14px;
    line-height: 1.55;
    /* Center the [gutter | text] group when the text column is narrower
       than the pane (see --editor-text-width below). No-op at full width:
       the content flex-grows to fill, leaving no free space to distribute. */
    justify-content: center;
  }
  :global(.editor .cm-content) {
    padding: 10px 0;
    /* Cap the editable column to the user's chosen width. The variable is
       set on <main> (inherited down here) by the bottom-bar width slider;
       `none` (its default / max setting) means full-width as before. */
    max-width: var(--editor-text-width, none);
  }
  :global(.editor .cm-editor),
  :global(.editor .cm-scroller),
  :global(.editor .cm-content),
  :global(.editor .cm-line),
  :global(.editor .cm-line *) {
    user-select: text !important;
    -webkit-user-select: text !important;
  }
  :global(.editor .cm-content) {
    cursor: text;
  }
  :global(.editor .cm-gutters) {
    background: #1a1a1a;
    border-right: 1px solid #2a2a2a;
    color: #444;
  }
  :global(.editor .cm-activeLineGutter),
  :global(.editor .cm-activeLine) {
    background: #232323;
  }
  :global(.editor .cm-ghost) {
    color: #666;
    font-style: italic;
    white-space: pre-wrap;
    pointer-events: none;
  }
  :global(.editor .cm-ghost-pending) {
    color: #6cb6ff;
    font-style: normal;
    letter-spacing: 0.35em;
    padding-left: 0.15em;
  }
  :global(.editor .cm-ghost-pending .dot) {
    display: inline-block;
    font-size: 0.7em;
    animation: ghost-pulse 1.2s ease-in-out infinite;
  }
  :global(.editor .cm-ghost-pending .dot:nth-child(2)) {
    animation-delay: 0.2s;
  }
  :global(.editor .cm-ghost-pending .dot:nth-child(3)) {
    animation-delay: 0.4s;
  }
  @keyframes -global-ghost-pulse {
    0%,
    100% {
      opacity: 0.25;
    }
    50% {
      opacity: 1;
    }
  }
  :global(.editor .cm-ghost-error) {
    color: #e0885f;
    font-style: normal;
    font-size: 0.85em;
    background: rgba(224, 136, 95, 0.1);
    border-radius: 3px;
    padding: 0 4px;
  }
  :global(.editor .cm-ghost-rewrite) {
    color: #b8b8b8;
    font-style: normal;
    background: rgba(120, 180, 200, 0.08);
    border-bottom: 1px dashed #4a6a78;
    padding: 0 1px;
  }
  :global(.editor .cm-search-match) {
    background: rgba(255, 200, 100, 0.18);
    color: #f4c97c;
    border-radius: 2px;
  }
  /*
   * Wikilink color states — !important on every color line because the
   * markdown grammar in @codemirror/lang-markdown treats [[Foo]] as a
   * reference-style link [Foo], and the oneDark theme's HighlightStyle
   * paints that token cyan-grey at a higher specificity. Without
   * !important all three states render the same theme color.
   *
   *   live     — sky blue ......................... existing note with content
   *   empty    — amber italic .................... existing note that's blank
   *   broken   — red dashed ...................... no matching note (yet)
   */
  :global(.editor .cm-wikilink) {
    color: #6cb6ff !important;
    text-decoration: underline !important;
    text-decoration-color: rgba(108, 182, 255, 0.55) !important;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  :global(.editor .cm-wikilink:hover) {
    text-decoration-color: rgba(108, 182, 255, 0.95) !important;
  }
  :global(.editor .cm-wikilink-empty) {
    color: #d6b06a !important;
    text-decoration: underline !important;
    text-decoration-color: rgba(214, 176, 106, 0.6) !important;
    font-style: italic !important;
  }
  :global(.editor .cm-wikilink-empty:hover) {
    color: #e6c685 !important;
    text-decoration-color: rgba(214, 176, 106, 1) !important;
  }
  :global(.editor .cm-wikilink-broken) {
    color: #c97a7a !important;
    text-decoration: underline dashed !important;
    text-decoration-color: rgba(201, 122, 122, 0.7) !important;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  :global(.editor .cm-wikilink-broken:hover) {
    text-decoration-color: rgba(201, 122, 122, 1) !important;
  }
  :global(.editor .cm-md-bold) {
    font-weight: 700;
  }
  :global(.editor .cm-md-italic) {
    font-style: italic;
  }
</style>
