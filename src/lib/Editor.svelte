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
  } = $props();

  // Right-click pill menu: floating div anchored at cursor.
  let pillMenu = $state<{ tag: string; x: number; y: number } | null>(null);

  // Steer-generation modal (Cmd/Ctrl+Shift+;). The text is injected as a
  // <direction> note into the next completion/rewrite so the user can
  // nudge the model ("make it darker", "pivot to the counterargument").
  let steerOpen = $state(false);
  let steerText = $state("");
  let steerInputEl: HTMLInputElement | null = $state(null);

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
    void tick().then(() => steerInputEl?.focus());
  }
  function cancelSteer() {
    steerOpen = false;
    // Hand focus back to the editor so the user keeps typing.
    view?.focus();
  }
  function submitSteer() {
    const dir = steerText.trim();
    steerOpen = false;
    if (view) {
      view.focus();
      void fetchCompletion(view, dir);
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
    // Cancel any pending autosave so a tag-relocate doesn't shift the
    // positions out from under us while the user reviews suggestions.
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
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
    const changes: { from: number; to: number; insert: string }[] = [];
    const acceptedCreates: { title: string }[] = [];
    for (const s of [...linkSuggestions, ...linkAiSuggestions]) {
      if (!linkSuggestionChecked[suggestionKey(s)]) continue;
      for (const [from, to] of s.positions) {
        const original = view.state.doc.sliceString(from, to);
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
  let fetchGen = 0;
  // Monotonic guard so overlapping external-change handlers can't race:
  // only the most recent disk read is allowed to act on its result.
  let externalChangeGen = 0;
  let currentHighlightQuery = "";
  let currentAllNotes: NoteRef[] = [];

  const SAVE_DEBOUNCE_MS = 300;

  // ---------------------------------------------------------------
  // Ghost-text completion: state field, widget, decoration, keymap
  // ---------------------------------------------------------------

  type Ghost =
    | { mode: "insert"; text: string; pos: number }
    | { mode: "rewrite"; text: string; from: number; to: number };

  const setGhost = StateEffect.define<Ghost | null>();

  class GhostWidget extends WidgetType {
    readonly text: string;
    readonly isRewrite: boolean;
    constructor(text: string, isRewrite: boolean) {
      super();
      this.text = text;
      this.isRewrite = isRewrite;
    }
    eq(other: GhostWidget): boolean {
      return other.text === this.text && other.isRewrite === this.isRewrite;
    }
    toDOM(): HTMLElement {
      const span = document.createElement("span");
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

  function ghostStablePos(g: Ghost): number {
    return g.mode === "rewrite" ? g.from : g.pos;
  }

  const ghostField = StateField.define<Ghost | null>({
    create() {
      return null;
    },
    update(value, tr) {
      for (const effect of tr.effects) {
        if (effect.is(setGhost)) return effect.value;
      }
      if (tr.docChanged) return null;
      // If user moves cursor away from the ghost anchor, clear it.
      if (value && tr.selection) {
        const anchor = ghostStablePos(value);
        const head = tr.newSelection.main.head;
        const start = tr.newSelection.main.from;
        const end = tr.newSelection.main.to;
        if (value.mode === "insert" && head !== anchor) {
          return null;
        }
        if (value.mode === "rewrite" && (start !== value.from || end !== value.to)) {
          return null;
        }
      }
      return value;
    },
    provide: (f) =>
      EditorView.decorations.from(f, (ghost): DecorationSet => {
        if (!ghost) return Decoration.none;
        if (ghost.mode === "rewrite") {
          return Decoration.set([
            Decoration.replace({
              widget: new GhostWidget(ghost.text, true),
            }).range(ghost.from, ghost.to),
          ]);
        }
        return Decoration.set([
          Decoration.widget({
            widget: new GhostWidget(ghost.text, false),
            side: 1,
          }).range(ghost.pos),
        ]);
      }),
  });

  async function fetchCompletion(v: EditorView, direction = "") {
    const myGen = ++fetchGen;
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
        effects: setGhost.of({ mode: "rewrite", text: "…", from: sel.from, to: sel.to }),
      });

      let accumulated = "";
      let started = false;
      const channel = new Channel<string>();
      channel.onmessage = (chunk: string) => {
        if (myGen !== fetchGen || !view) return;
        const cur = view.state.selection.main;
        if (cur.from !== sel.from || cur.to !== sel.to) {
          v.dispatch({ effects: setGhost.of(null) });
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
            effects: setGhost.of({ mode: "rewrite", text: display, from: sel.from, to: sel.to }),
          });
        }
      };

      try {
        await invoke("rewrite_text_streaming", { before, selected, after, direction, onChunk: channel });
        if (myGen === fetchGen && !started && view) {
          v.dispatch({ effects: setGhost.of(null) });
        }
      } catch (e) {
        if (myGen === fetchGen && view) {
          v.dispatch({ effects: setGhost.of(null) });
        }
        console.error("rewrite_text_streaming failed", e);
      }
      return;
    }

    // INSERT / CONTINUATION mode — strip hashtags from context (same
    // reasoning as REWRITE above).
    const cursor = sel.head;
    const before = stripTagsForAI(v.state.doc.sliceString(0, cursor));
    const after = stripTagsForAI(v.state.doc.sliceString(cursor, docLen));
    if (!before.trim() && !after.trim()) return;

    v.dispatch({ effects: setGhost.of({ mode: "insert", text: "…", pos: cursor }) });

    let accumulated = "";
    let started = false;
    const channel = new Channel<string>();
    channel.onmessage = (chunk: string) => {
      if (myGen !== fetchGen || !view) return;
      if (view.state.selection.main.head !== cursor) {
        v.dispatch({ effects: setGhost.of(null) });
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
          effects: setGhost.of({ mode: "insert", text: display, pos: cursor }),
        });
      }
    };

    try {
      await invoke("complete_text_streaming", { before, after, direction, onChunk: channel });
      if (myGen === fetchGen && !started && view) {
        v.dispatch({ effects: setGhost.of(null) });
      }
    } catch (e) {
      if (myGen === fetchGen && view) {
        v.dispatch({ effects: setGhost.of(null) });
      }
      console.error("complete_text_streaming failed", e);
    }
  }

  // Two-pane prompting (Cmd/Ctrl+Shift+'): the OTHER pane's content is a
  // raw pre-prompt prepended to THIS pane's content; the whole concatenation
  // is sent with no scaffolding (see prompt_streaming). The response streams
  // as ghost text appended at the end of this note. No-ops when no second
  // note pane is open (getPrePrompt returns null).
  async function runTwoPanePrompt(v: EditorView) {
    if (!getPrePrompt) return;
    let prePrompt: string | null = null;
    try {
      prePrompt = await getPrePrompt();
    } catch {
      return;
    }
    if (prePrompt == null || !view) return;
    const myGen = ++fetchGen;
    // Strip hashtags (+ the canonical tag line and wikilink brackets) from
    // BOTH panes — same hygiene as the other AI paths — so the model never
    // sees our #tag markup and can't echo or invent tags in the reply.
    const focused = stripTagsForAI(v.state.doc.toString());
    const pre = stripTagsForAI(prePrompt);
    const prompt = `${pre}\n\n${focused}`.trim();
    if (!prompt) return;
    const pos = v.state.doc.length; // append the response at the end
    v.dispatch({ effects: setGhost.of({ mode: "insert", text: "…", pos }) });

    let accumulated = "";
    let started = false;
    const channel = new Channel<string>();
    channel.onmessage = (chunk: string) => {
      if (myGen !== fetchGen || !view) return;
      accumulated += chunk;
      const display = accumulated.replace(/\r/g, "").replace(/^\s+|\s+$/g, "");
      if (display) {
        started = true;
        v.dispatch({ effects: setGhost.of({ mode: "insert", text: display, pos }) });
      }
    };
    try {
      await invoke("prompt_streaming", { prompt, onChunk: channel });
      if (myGen === fetchGen && !started && view) {
        v.dispatch({ effects: setGhost.of(null) });
      }
    } catch (e) {
      if (myGen === fetchGen && view) {
        v.dispatch({ effects: setGhost.of(null) });
      }
      console.error("prompt_streaming failed", e);
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

  function acceptCompletion(v: EditorView): boolean {
    const ghost = v.state.field(ghostField, false);
    if (!ghost) return false;
    const clean = sanitizeCompletion(ghost.text);
    if (ghost.mode === "rewrite") {
      v.dispatch({
        changes: { from: ghost.from, to: ghost.to, insert: clean },
        selection: { anchor: ghost.from + clean.length },
        effects: setGhost.of(null),
      });
    } else {
      v.dispatch({
        changes: { from: ghost.pos, to: ghost.pos, insert: clean },
        selection: { anchor: ghost.pos + clean.length },
        effects: setGhost.of(null),
      });
    }
    return true;
  }

  function declineGhost(v: EditorView): boolean {
    const ghost = v.state.field(ghostField, false);
    if (!ghost) return false;
    v.dispatch({ effects: setGhost.of(null) });
    return true;
  }

  // Any "I want to interact with what I see" gesture accepts the ghost.
  // Arrow keys and mouse click pass through so the cursor still moves
  // afterward. Tab accepts and consumes (no tab character inserted).
  // Esc is the only explicit decline.
  function acceptThenPassThrough(v: EditorView): boolean {
    acceptCompletion(v);
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
      { key: "Tab", run: (v) => acceptCompletion(v) },
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
        const raw = linkEl.textContent ?? "";
        // The span text is the full "[[name]]" — strip the brackets.
        const target = raw.replace(/^\[\[/, "").replace(/\]\]$/, "").trim();
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
            const target = m[1].trim();
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
              dec: Decoration.mark({ class: cls }),
            });
            // Hide `[[` and `]]` unless the cursor / selection is anywhere
            // inside [start, end] (inclusive at both ends — so the cursor
            // arriving at the link's edge reveals the brackets too).
            const cursorInLink = sel.from <= end && sel.to >= start;
            if (!cursorInLink) {
              adds.push({ from: start, to: start + 2, dec: Decoration.replace({}) });
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

  // A hashtag is "in progress" while the caret sits inside it or at its
  // trailing edge — the user is still typing or editing it. An in-progress
  // tag is NOT finalized (no pill in the row, no relocation to the canonical
  // line) until a boundary follows it or the caret moves off it. `m` offsets
  // and `caret` are both doc-absolute.
  function isCaretInTag(m: { from: number; to: number }, caret: number): boolean {
    return caret > m.from && caret <= m.to;
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
        const canonical = findCanonicalTagLine(doc);
        const aboveText = canonical
          ? doc.split("\n").slice(0, canonical.lineIdx).join("\n")
          : doc;
        // Skip the tag the caret is still inside — no pill for a half-typed
        // hashtag; it appears once a boundary follows or the caret moves away.
        const inline = findInlineTags(aboveText)
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
          u.transactions.some((tr) => tr.effects.some((e) => e.is(tagPillRedraw)))
        ) {
          this.decorations = this.compute(u.view);
        }
      }
      compute(view: EditorView): DecorationSet {
        const builder = new RangeSetBuilder<Decoration>();
        const doc = view.state.doc.toString();
        const canonical = canonicalTagLineRange(doc);
        const matches = findInlineTags(doc);
        for (const m of matches) {
          // Skip matches inside the canonical tag line — that whole line is
          // hidden by tagLineHider, so styling individual pills there is wasted.
          if (canonical && m.from >= canonical.from && m.to <= canonical.to) continue;
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
    const doc = state.doc.toString();
    const range = canonicalTagLineRange(doc);
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
      if (!tr.docChanged) return value;
      return computeTagLineHide(tr.state);
    },
    provide: (f) => EditorView.decorations.from(f),
  });

  // Marks a transaction as an internal, programmatic rewrite (the
  // relocate-tags-to-bottom transform, external-change reloads, full-doc
  // resets) so the protect filter below lets it through. User keystrokes
  // carry no annotation and are subject to protection.
  const internalDocRewrite = Annotation.define<boolean>();

  // Protect the hidden canonical tag line from user edits. Without this,
  // backspacing past trailing newlines at the end of a note silently
  // eats into the (invisible) tag line — you can delete tags you can't
  // see. changeFilter returns the protected [from,to] range; CodeMirror
  // drops any user deletion that would touch it while still allowing
  // insertions adjacent to it and edits everywhere else. We protect from
  // the separator newline before the tag line through the end of the
  // document so the tags and their separator are both safe.
  const protectTagLine = EditorState.changeFilter.of((tr) => {
    if (tr.annotation(internalDocRewrite)) return true;
    const doc = tr.startState.doc.toString();
    const range = canonicalTagLineRange(doc);
    if (!range) return true;
    const line = tr.startState.doc.lineAt(range.from);
    // Protect the tag line AND the separator newline before it — the exact
    // span tagLineHider hides. Protecting only the tag chars left a gap: a
    // backspace at that boundary deleted the (hidden) separator and collapsed
    // the canonical line up into visible, editable text. canonicalTagLineRange
    // already returns null when there's no content above, so a tag-only note
    // still has an editable anchor and never reaches here.
    const from = line.from > 0 ? line.from - 1 : line.from;
    return [from, line.to];
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
    try {
      if (isEncrypted && password) {
        await invoke("save_encrypted_note", { path: p, content, password });
      } else {
        await invoke("save_note", { path: p, content });
      }
      lastSavedContent = content;
      onSaved?.();
    } catch (e) {
      console.error("save_note failed", e);
    }
  }

  function scheduleSave() {
    if (saveTimer) clearTimeout(saveTimer);
    if (!currentPath || !view) return;
    const p = currentPath;
    saveTimer = window.setTimeout(() => {
      if (!view) return;
      // A conflict is pending resolution — don't write either side until
      // the user chooses, or we'd silently clobber the on-disk version.
      if (externalConflict !== null) {
        saveTimer = null;
        return;
      }
      // Run the tag relocation transform before saving. If it changes the doc,
      // dispatch a single replace so the user sees their inline hashtags
      // migrate to the canonical bottom line (which is then hidden by the
      // tagLineHider decoration). Cursor clamps to the new doc length.
      const oldContent = view.state.doc.toString();
      // Don't finalize a tag the user is still typing. If the caret is inside
      // an inline hashtag, defer the whole relocate to a later save cycle (once
      // they add a boundary or move off it) — otherwise autosave would file a
      // half-typed tag to the hidden line mid-word and strand the rest.
      const caret = view.state.selection.main.head;
      const typingTag = findInlineTags(oldContent).some((m) => isCaretInTag(m, caret));
      const { body: newContent } = typingTag
        ? { body: oldContent }
        : relocateTagsToBottom(oldContent);
      if (newContent !== oldContent) {
        // Clamp the caret to the end of the VISIBLE body — never into or past
        // the now-hidden canonical tag line. If it lands inside that region the
        // next keystroke merges into a tag (#foo + #bar -> #foo#bar) and the
        // broken line stops being recognized as canonical, re-exposing it.
        const range = canonicalTagLineRange(newContent);
        const maxCaret = range
          ? newContent.slice(0, range.from).replace(/\s+$/, "").length
          : newContent.length;
        const cursor = Math.min(view.state.selection.main.head, maxCaret);
        view.dispatch({
          changes: { from: 0, to: oldContent.length, insert: newContent },
          selection: { anchor: cursor },
          // This rewrite legitimately rebuilds the canonical tag line, so
          // it must bypass the protect filter.
          annotations: internalDocRewrite.of(true),
        });
      }
      void flushSave(p, view.state.doc.toString());
      saveTimer = null;
    }, SAVE_DEBOUNCE_MS) as unknown as number;
  }

  async function loadPath(p: string | null) {
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
    }
    if (view && currentPath && currentPath !== p) {
      await flushSave(currentPath, view.state.doc.toString());
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
      if (isEncrypted) {
        if (!password) {
          // Locked: render an empty editor so the user sees nothing
          // sensitive. Parent will pop the password modal.
          body = "";
        } else {
          body = await invoke<string>("read_encrypted_note", { path: p, password });
        }
      } else {
        body = await invoke<string>("read_note", { path: p });
      }
    } catch (e) {
      console.error("read_note failed", e);
      body = "";
    }
    lastSavedContent = body;

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
    view = new EditorView({ state, parent: container });

    const active = document.activeElement;
    const inputFocused =
      active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement;
    if (!inputFocused) {
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
    let fresh = "";
    try {
      if (isEncrypted && password) {
        fresh = await invoke<string>("read_encrypted_note", {
          path: currentPath,
          password,
        });
      } else if (isEncrypted) {
        // Locked: don't try to read. Save would clobber anyway.
        return;
      } else {
        fresh = await invoke<string>("read_note", { path: currentPath });
      }
    } catch {
      return;
    }
    // A newer external-change handler started while we awaited the read —
    // let it win; acting on our now-stale `fresh` could clobber.
    if (myGen !== externalChangeGen) return;
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
    unlistenNotes = await listen("notes_changed", handleExternalChange);
    // Allow the parent to await all pending saves before a rename / other
    // path-mutating op so we don't write stale content to a renamed file.
    unregisterFlusher = registerEditorFlusher(async () => {
      if (saveTimer && currentPath && view) {
        clearTimeout(saveTimer);
        saveTimer = null;
        await flushSave(currentPath, view.state.doc.toString());
      }
    });
  });

  onDestroy(() => {
    window.removeEventListener("malt:vim-changed", handleVimChange);
    window.removeEventListener("malt:markdown-toggle-changed", handleMdToggle);
    window.removeEventListener("keydown", handlePillMenuEsc, true);
    window.removeEventListener("keydown", handleLinkSuggestionsKey, true);
    unlistenNotes?.();
    unregisterFlusher?.();
    if (saveTimer && currentPath && view) {
      void flushSave(currentPath, view.state.doc.toString());
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
          if (e.key === "Enter") { e.preventDefault(); submitSteer(); }
          else if (e.key === "Escape") { e.preventDefault(); cancelSteer(); }
        }}
      />
      <div class="steer-hint">
        A one-time nudge for the AI generation at your cursor (or your selection). Enter to generate · Esc to cancel.
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
  }
  :global(.editor .cm-content) {
    padding: 10px 0;
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
