<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onMount, onDestroy, tick } from "svelte";
  import Settings from "$lib/Settings.svelte";
  import Editor from "$lib/Editor.svelte";
  import Linkbacks from "$lib/Linkbacks.svelte";
  import { flushAllEditors } from "$lib/editorRegistry";

  type Note = {
    path: string;
    title: string;
    snippet: string;
    modified: number;
    tags?: string[];
    title_matches?: [number, number][];
    snippet_matches?: [number, number][];
  };

  type SavedSearch = {
    id: string;
    name: string;
    query: string;
    slot: number | null;
  };

  type TagCount = { name: string; count: number };

  function escapeHtml(s: string): string {
    return s
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function highlight(text: string, ranges?: [number, number][]): string {
    if (!ranges || ranges.length === 0) return escapeHtml(text);
    // ranges are char offsets, but JS string slicing is UTF-16 codeunit-based.
    // For ASCII text the two coincide; for occasional Unicode the highlight
    // may be slightly off but won't corrupt rendering.
    const chars = Array.from(text);
    let out = "";
    let cursor = 0;
    for (const [start, end] of ranges) {
      const s = Math.max(cursor, start);
      const e = Math.max(s, end);
      if (s > cursor) out += escapeHtml(chars.slice(cursor, s).join(""));
      out += "<mark>" + escapeHtml(chars.slice(s, e).join("")) + "</mark>";
      cursor = e;
    }
    if (cursor < chars.length) out += escapeHtml(chars.slice(cursor).join(""));
    return out;
  }
  type SortMode = "best" | "new" | "old" | "az" | "za";

  const SORT_OPTIONS: { id: SortMode; label: string }[] = [
    { id: "best", label: "best" },
    { id: "new", label: "new" },
    { id: "old", label: "old" },
    { id: "az", label: "a-z" },
    { id: "za", label: "z-a" },
  ];

  function readInitialSort(): SortMode {
    if (typeof localStorage === "undefined") return "best";
    const stored = localStorage.getItem("malt.sort");
    return SORT_OPTIONS.some((o) => o.id === stored) ? (stored as SortMode) : "best";
  }

  let query = $state("");
  let rawResults = $state<Note[]>([]);
  let allNotes = $state<Note[]>([]);
  let sortMode = $state<SortMode>(readInitialSort());
  let selectedPath = $state<string | null>(null);
  let settingsOpen = $state(false);
  let deleteConfirmOpen = $state(false);
  let cancelBtn: HTMLButtonElement | null = $state(null);
  let deleteBtn: HTMLButtonElement | null = $state(null);
  let editorWords = $state(0);
  let editorChars = $state(0);
  let secondaryPath = $state<string | null>(null);
  let focusedPane = $state<"primary" | "secondary">("primary");

  // Export modal — per-note. Lives at +page.svelte because it operates on
  // selectedPath and uses the native save dialog from the parent scope.
  let exportOpen = $state(false);
  let exportAppendLinks = $state(false);
  let exportLinkedCount = $state(0);
  let exportStatus = $state<string | null>(null);
  let exportBusy = $state(false);

  async function openExport() {
    if (!selectedPath) return;
    exportOpen = true;
    exportAppendLinks = false;
    exportStatus = null;
    exportLinkedCount = 0;
    try {
      exportLinkedCount = await invoke<number>("count_wikilink_targets", {
        path: selectedPath,
      });
    } catch {
      /* non-critical */
    }
  }

  function cancelExport() {
    if (exportBusy) return;
    exportOpen = false;
    exportStatus = null;
  }

  function exportSuggestedFilename(ext: string): string {
    const title = getTitleForPath(selectedPath) || "note";
    // Strip filesystem-unfriendly chars for the suggestion. User can edit.
    const safe = title.replace(/[\\/:*?"<>|]/g, "-").trim() || "note";
    return `${safe}.${ext}`;
  }

  async function exportToFile(format: "md" | "html" | "epub" | "txt") {
    if (!selectedPath || exportBusy) return;
    const extMap: Record<string, string> = {
      md: "md",
      html: "html",
      epub: "epub",
      txt: "txt",
    };
    const filterMap: Record<string, { name: string; extensions: string[] }> = {
      md: { name: "Markdown", extensions: ["md"] },
      html: { name: "HTML", extensions: ["html", "htm"] },
      epub: { name: "EPUB", extensions: ["epub"] },
      txt: { name: "Plain text", extensions: ["txt"] },
    };
    exportStatus = null;
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const picked = await save({
        title: `Export as ${format.toUpperCase()}`,
        defaultPath: exportSuggestedFilename(extMap[format]),
        filters: [filterMap[format]],
      });
      if (!picked) return; // user cancelled the save dialog
      exportBusy = true;
      await invoke("export_to_file", {
        path: selectedPath,
        format,
        appendLinks: exportAppendLinks,
        destPath: picked,
      });
      exportStatus = `Saved ${picked.split(/[\\/]/).pop()}`;
    } catch (e) {
      exportStatus = String(e);
    } finally {
      exportBusy = false;
    }
  }

  async function exportCopyPlain() {
    if (!selectedPath || exportBusy) return;
    exportStatus = null;
    try {
      exportBusy = true;
      const text = await invoke<string>("export_as_string", {
        path: selectedPath,
        format: "txt",
        appendLinks: exportAppendLinks,
      });
      await navigator.clipboard.writeText(text);
      exportStatus = "Copied as plain text";
    } catch (e) {
      exportStatus = String(e);
    } finally {
      exportBusy = false;
    }
  }

  async function exportCopyRich() {
    if (!selectedPath || exportBusy) return;
    exportStatus = null;
    try {
      exportBusy = true;
      const html = await invoke<string>("export_as_string", {
        path: selectedPath,
        format: "html_body",
        appendLinks: exportAppendLinks,
      });
      const plain = await invoke<string>("export_as_string", {
        path: selectedPath,
        format: "txt",
        appendLinks: exportAppendLinks,
      });
      // ClipboardItem with both MIME types so paste targets pick the best one.
      const item = new ClipboardItem({
        "text/html": new Blob([html], { type: "text/html" }),
        "text/plain": new Blob([plain], { type: "text/plain" }),
      });
      await navigator.clipboard.write([item]);
      exportStatus = "Copied as rich text";
    } catch (e) {
      exportStatus = String(e);
    } finally {
      exportBusy = false;
    }
  }

  // Saved searches: list + the "save current query" modal.
  let savedSearches = $state<SavedSearch[]>([]);
  let saveSearchOpen = $state(false);
  let saveSearchName = $state("");
  let saveSearchInputEl: HTMLInputElement | null = $state(null);
  let saveSearchSlot = $state<number | null>(null);

  // Tag vocabulary + corpus tags, fed to the editor for autocomplete.
  let tagVocabulary = $state<string[]>([]);
  let allTagCounts = $state<TagCount[]>([]);
  let allTagNames = $derived(allTagCounts.map((t) => t.name));

  // Rename modal state.
  let renameOpen = $state(false);
  let renameTargetPath = $state<string | null>(null);
  let renameInputVal = $state("");
  let renameBacklinkCount = $state(0);
  let renameError = $state<string | null>(null);
  let renameInputEl: HTMLInputElement | null = $state(null);
  let renameSaving = $state(false);
  function readInitialSplitFraction(): number {
    if (typeof localStorage !== "undefined") {
      const s = localStorage.getItem("malt.splitFraction");
      if (s) {
        const n = parseFloat(s);
        if (!isNaN(n) && n >= 0.2 && n <= 0.8) return n;
      }
    }
    return 0.5;
  }
  let splitFraction = $state(readInitialSplitFraction());
  let countMode = $state<"words" | "chars">(
    typeof localStorage !== "undefined" && localStorage.getItem("malt.countMode") === "chars"
      ? "chars"
      : "words"
  );
  function toggleCountMode() {
    countMode = countMode === "words" ? "chars" : "words";
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.countMode", countMode);
    }
  }
  let searchInput: HTMLInputElement | null = $state(null);
  // Tracks whether the user has explicitly arrowed in the result list since
  // the last query edit. When true, Enter opens the highlighted match.
  // When false, Enter falls back to "exact-title-match → open, else create".
  // Without this we'd never create a new note from search, because Tantivy's
  // fuzzy ranking always returns *some* result for any non-trivial query.
  let userNavigated = $state(false);

  // Linkbacks panel: collapsed flag + resizable height (persisted).
  let linkbacksCollapsed = $state(
    typeof localStorage !== "undefined" &&
      localStorage.getItem("malt.linkbacks.collapsed") === "1"
  );
  function readInitialLinkbacksHeight(): number {
    if (typeof localStorage !== "undefined") {
      const stored = localStorage.getItem("malt.linkbacks.height");
      if (stored) {
        const n = parseInt(stored, 10);
        if (!isNaN(n) && n >= 60 && n <= 1200) return n;
      }
    }
    return typeof window !== "undefined"
      ? Math.max(120, Math.round(window.innerHeight * 0.25))
      : 200;
  }
  let linkbacksHeight = $state(readInitialLinkbacksHeight());

  $effect(() => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.linkbacks.collapsed", linkbacksCollapsed ? "1" : "0");
    }
  });

  let resizingLinkbacks = false;
  let resizeStartY = 0;
  let resizeStartH = 0;
  function startLinkbacksResize(e: MouseEvent) {
    resizingLinkbacks = true;
    resizeStartY = e.clientY;
    resizeStartH = linkbacksHeight;
    window.addEventListener("mousemove", onLinkbacksResize);
    window.addEventListener("mouseup", endLinkbacksResize);
    e.preventDefault();
  }
  function onLinkbacksResize(e: MouseEvent) {
    if (!resizingLinkbacks) return;
    const dy = e.clientY - resizeStartY;
    // Dragging UP grows the linkbacks panel.
    const next = Math.max(80, Math.min(800, resizeStartH - dy));
    linkbacksHeight = next;
  }
  function endLinkbacksResize() {
    resizingLinkbacks = false;
    window.removeEventListener("mousemove", onLinkbacksResize);
    window.removeEventListener("mouseup", endLinkbacksResize);
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.linkbacks.height", String(linkbacksHeight));
    }
  }

  let unlisten: UnlistenFn | null = null;
  let queryGen = 0;
  let pendingFocusAfterLoad = false;

  // Navigation history — per pane. Explicit opens push; arrow nav doesn't.
  // Each pane has its own stack so back/forward operate on the focused pane.
  const HISTORY_CAP = 50;
  type PaneHistory = { stack: string[]; idx: number };
  let primaryHistory: PaneHistory = { stack: [], idx: -1 };
  let secondaryHistory: PaneHistory = { stack: [], idx: -1 };

  function pushToHistory(pane: "primary" | "secondary", path: string) {
    const h = pane === "primary" ? primaryHistory : secondaryHistory;
    if (h.idx >= 0 && h.stack[h.idx] === path) return;
    h.stack = h.stack.slice(0, h.idx + 1).concat(path);
    if (h.stack.length > HISTORY_CAP) h.stack.shift();
    h.idx = h.stack.length - 1;
  }

  function activePane(): "primary" | "secondary" {
    return focusedPane === "secondary" && secondaryPath ? "secondary" : "primary";
  }

  function goBack() {
    const pane = activePane();
    const h = pane === "primary" ? primaryHistory : secondaryHistory;
    if (h.idx <= 0) return;
    h.idx--;
    if (pane === "primary") {
      selectedPath = h.stack[h.idx];
      void scrollSelectedIntoView("nearest");
    } else {
      secondaryPath = h.stack[h.idx];
    }
  }

  function goForward() {
    const pane = activePane();
    const h = pane === "primary" ? primaryHistory : secondaryHistory;
    if (h.idx >= h.stack.length - 1) return;
    h.idx++;
    if (pane === "primary") {
      selectedPath = h.stack[h.idx];
      void scrollSelectedIntoView("nearest");
    } else {
      secondaryPath = h.stack[h.idx];
    }
  }

  function applySort(items: Note[], mode: SortMode): Note[] {
    switch (mode) {
      case "best":
        return items;
      case "new":
        return [...items].sort((a, b) => b.modified - a.modified);
      case "old":
        return [...items].sort((a, b) => a.modified - b.modified);
      case "az":
        return [...items].sort((a, b) =>
          a.title.localeCompare(b.title, undefined, { sensitivity: "base" })
        );
      case "za":
        return [...items].sort((a, b) =>
          b.title.localeCompare(a.title, undefined, { sensitivity: "base" })
        );
    }
  }

  let notes = $derived(applySort(rawResults, sortMode));

  async function performSearch(q: string) {
    const myGen = ++queryGen;
    const results = await invoke<Note[]>("search_notes", { query: q });
    if (myGen === queryGen) rawResults = results;
  }

  async function refreshAllNotes() {
    try {
      allNotes = await invoke<Note[]>("list_notes");
    } catch {
      /* keep stale list */
    }
  }

  $effect(() => {
    void performSearch(query);
  });

  // Reset the "user explicitly arrowed" flag whenever the query changes —
  // typing means new intent, the current highlight is no longer the user's
  // deliberate choice.
  $effect(() => {
    void query;
    userNavigated = false;
  });

  $effect(() => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.sort", sortMode);
    }
  });

  // Auto-select first match when current selection is filtered out (or none yet).
  // Note: this does NOT push to navigation history — it's reactive, not user intent.
  $effect(() => {
    if (notes.length === 0) return;
    if (!selectedPath || !notes.some((n) => n.path === selectedPath)) {
      selectedPath = notes[0].path;
      void scrollSelectedIntoView("nearest");
    }
  });

  function formatModified(secs: number): string {
    if (!secs) return "";
    const age = Math.floor(Date.now() / 1000) - secs;
    if (age < 60) return `${age}s`;
    if (age < 3600) return `${Math.floor(age / 60)}m`;
    if (age < 86400) return `${Math.floor(age / 3600)}h`;
    if (age < 86400 * 30) return `${Math.floor(age / 86400)}d`;
    return `${Math.floor(age / (86400 * 30))}mo`;
  }

  function navigate(delta: number) {
    if (notes.length === 0) return;
    const currentIdx = selectedPath ? notes.findIndex((n) => n.path === selectedPath) : -1;
    let newIdx = currentIdx === -1 ? (delta > 0 ? 0 : notes.length - 1) : currentIdx + delta;
    if (newIdx < 0) newIdx = 0;
    if (newIdx >= notes.length) newIdx = notes.length - 1;
    selectedPath = notes[newIdx].path;
    void scrollSelectedIntoView("nearest");
  }

  async function scrollSelectedIntoView(block: ScrollLogicalPosition = "nearest") {
    await tick();
    const el = document.querySelector(".note.selected") as HTMLElement | null;
    el?.scrollIntoView({ block });
  }

  function handleGlobalKey(e: KeyboardEvent) {
    // Bare Esc routing, in priority order:
    //   1. Rename modal open → cancel it
    //   2. Delete modal open → cancel it
    //   3. Settings open → let its own handler run (no-op here)
    //   4. Focus in editor → let editor handle (ghost-decline / vim normal)
    //   5. Otherwise → clear search query + focus search field
    if (e.key === "Escape" && !e.metaKey && !e.ctrlKey && !e.altKey) {
      if (exportOpen) {
        e.preventDefault();
        e.stopPropagation();
        cancelExport();
        return;
      }
      if (saveSearchOpen) {
        e.preventDefault();
        e.stopPropagation();
        cancelSaveSearch();
        return;
      }
      if (renameOpen) {
        e.preventDefault();
        e.stopPropagation();
        cancelRename();
        return;
      }
      if (deleteConfirmOpen) {
        e.preventDefault();
        e.stopPropagation();
        cancelDelete();
        return;
      }
      if (settingsOpen) return;
      const active = document.activeElement;
      const inEditor =
        active instanceof HTMLElement && !!active.closest(".cm-content");
      if (inEditor) return;
      e.preventDefault();
      e.stopPropagation();
      query = "";
      searchInput?.focus();
      searchInput?.select();
      return;
    }

    const mod = e.metaKey || e.ctrlKey;
    if (!mod) return;
    // Most shift-modified combos belong to the editor's keymap (e.g.
    // Mod+Shift+L for link suggestions). The export modal is the exception
    // — it's a page-level action so we own Mod+Shift+E.
    if (e.shiftKey) {
      const sKey = e.key.toLowerCase();
      if (sKey === "e") {
        e.preventDefault();
        e.stopPropagation();
        void openExport();
      }
      return;
    }
    const key = e.key.toLowerCase();

    if (key === ",") {
      e.preventDefault();
      settingsOpen = !settingsOpen;
      return;
    }
    if (key === "l") {
      e.preventDefault();
      searchInput?.focus();
      searchInput?.select();
      return;
    }
    if (key === "n") {
      e.preventDefault();
      query = "";
      searchInput?.focus();
      searchInput?.select();
      return;
    }
    if (key === "arrowdown" || key === "j") {
      e.preventDefault();
      e.stopPropagation();
      navigate(1);
      return;
    }
    if (key === "arrowup" || key === "k") {
      e.preventDefault();
      e.stopPropagation();
      navigate(-1);
      return;
    }
    if (key === "[") {
      e.preventDefault();
      e.stopPropagation();
      goBack();
      return;
    }
    if (key === "]") {
      e.preventDefault();
      e.stopPropagation();
      goForward();
      return;
    }
    if (key === "s") {
      // Save current query as a named search (only meaningful with a query).
      if (!query.trim()) return;
      e.preventDefault();
      e.stopPropagation();
      void openSaveSearchModal();
      return;
    }
    if (key.length === 1 && key >= "1" && key <= "9") {
      const slot = parseInt(key, 10);
      const has = savedSearches.some((s) => s.slot === slot);
      if (has) {
        e.preventDefault();
        e.stopPropagation();
        activateSlot(slot);
      }
      return;
    }
    if (key === "delete" || key === "backspace") {
      // Only fire when focus is outside the editor — there, Cmd+Backspace /
      // Ctrl+Del retain their native delete-word / delete-to-line-start meaning.
      const active = document.activeElement;
      const inEditor =
        active instanceof HTMLElement && !!active.closest(".cm-content");
      if (!inEditor) {
        e.preventDefault();
        e.stopPropagation();
        requestDelete();
      }
      return;
    }
  }

  function handleSearchKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return; // let modifier combos through to global handler
    if (e.key === "ArrowDown") {
      e.preventDefault();
      userNavigated = true;
      navigate(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      userNavigated = true;
      navigate(-1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      void tryEnter();
    } else if (e.key === "Tab" && !e.shiftKey) {
      e.preventDefault();
      focusEditor();
    }
    // Esc is now handled by the global capture-phase handler — always clears.
  }

  function openNote(path: string) {
    pushToHistory("primary", path);
    selectedPath = path;
    focusedPane = "primary";
    void scrollSelectedIntoView("nearest");
  }

  // List row click — bare opens in primary, Cmd/Ctrl+click opens in secondary.
  function handleNoteClick(e: MouseEvent, path: string) {
    if (e.metaKey || e.ctrlKey) {
      openInSecondary(path);
    } else {
      openNote(path);
    }
  }

  // Persist split fraction whenever it changes.
  $effect(() => {
    const f = splitFraction;
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.splitFraction", String(f));
    }
  });

  // Horizontal resize between primary and secondary editor panes.
  let resizingSplit = false;
  let splitStartX = 0;
  let splitStartFraction = 0.5;
  let editorsRowEl: HTMLDivElement | null = $state(null);
  function startSplitResize(e: MouseEvent) {
    resizingSplit = true;
    splitStartX = e.clientX;
    splitStartFraction = splitFraction;
    window.addEventListener("mousemove", onSplitResize);
    window.addEventListener("mouseup", endSplitResize);
    e.preventDefault();
  }
  function onSplitResize(e: MouseEvent) {
    if (!resizingSplit || !editorsRowEl) return;
    const rect = editorsRowEl.getBoundingClientRect();
    const dx = e.clientX - splitStartX;
    const dF = dx / rect.width;
    let next = splitStartFraction + dF;
    if (next < 0.2) next = 0.2;
    if (next > 0.8) next = 0.8;
    splitFraction = next;
  }
  function endSplitResize() {
    resizingSplit = false;
    window.removeEventListener("mousemove", onSplitResize);
    window.removeEventListener("mouseup", endSplitResize);
  }

  function requestDelete() {
    if (!selectedPath) return;
    deleteConfirmOpen = true;
  }

  function cancelDelete() {
    deleteConfirmOpen = false;
  }

  async function confirmDelete() {
    const path = selectedPath;
    if (!path) {
      deleteConfirmOpen = false;
      return;
    }
    deleteConfirmOpen = false;
    try {
      await invoke("delete_note", { path });
    } catch (e) {
      console.error("delete_note failed", e);
      return;
    }
    // Prune history in both panes.
    for (const h of [primaryHistory, secondaryHistory]) {
      h.stack = h.stack.filter((p) => p !== path);
      if (h.idx >= h.stack.length) h.idx = h.stack.length - 1;
    }
    if (secondaryPath === path) secondaryPath = null;
    // Pick next selection: next visible note in current list, else previous,
    // else null (auto-select-first will pick whatever survives after the
    // file watcher refreshes).
    const idx = notes.findIndex((n) => n.path === path);
    let next: string | null = null;
    if (idx >= 0) {
      next = notes[idx + 1]?.path ?? notes[idx - 1]?.path ?? null;
    }
    selectedPath = next;
    // The notes_changed watcher event will refresh rawResults + allNotes.
  }

  function getTitleForPath(path: string | null): string {
    if (!path) return "";
    const found = allNotes.find((n) => n.path === path) ?? notes.find((n) => n.path === path);
    return found?.title ?? path.split(/[\\/]/).pop() ?? path;
  }

  function handleEditorCount(words: number, chars: number) {
    editorWords = words;
    editorChars = chars;
  }

  function handleDeleteModalKey(e: KeyboardEvent) {
    if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
      e.preventDefault();
      cancelBtn?.focus();
    } else if (e.key === "ArrowRight" || e.key === "ArrowDown") {
      e.preventDefault();
      deleteBtn?.focus();
    }
  }

  function focusEditor() {
    const cm = document.querySelector(".cm-content") as HTMLElement | null;
    cm?.focus();
  }

  function handleEditorReady(view: { focus: () => void }) {
    if (pendingFocusAfterLoad) {
      pendingFocusAfterLoad = false;
      view.focus();
    }
  }

  async function tryEnter() {
    const trimmed = query.trim();
    // Empty query: jump into the editor on whatever's selected, if anything.
    if (!trimmed) {
      if (selectedPath) focusEditor();
      return;
    }
    // 1) User explicitly arrowed in the result list — honor their choice.
    if (userNavigated && selectedPath) {
      focusEditor();
      return;
    }
    // 2) The typed query is the exact title of an existing note (case-insensitive,
    //    Unicode-folded). Open it. This is the "I'm jumping to a note I know exists"
    //    flow. We check allNotes (not the filtered list) so Tantivy fuzzy-ranking
    //    quirks don't hide an exact match.
    const exact = allNotes.find(
      (n) => n.title.localeCompare(trimmed, undefined, { sensitivity: "base" }) === 0
    );
    if (exact) {
      pushToHistory("primary", exact.path);
      selectedPath = exact.path;
      focusedPane = "primary";
      void scrollSelectedIntoView("nearest");
      focusEditor();
      query = "";
      return;
    }
    // 3) Otherwise the user is asking to create a new note titled `trimmed`.
    //    The fuzzy match list might still be non-empty (Tantivy returns
    //    near-matches for nearly any input) — that's fine, we ignore it here.
    try {
      const newPath = await invoke<string>("create_note", { title: trimmed });
      // Optimistic insert so the auto-select-first effect doesn't override.
      const optimistic: Note = {
        path: newPath,
        title: trimmed,
        snippet: "",
        modified: Math.floor(Date.now() / 1000),
      };
      rawResults = [optimistic, ...rawResults];
      allNotes = [optimistic, ...allNotes];
      // Set BEFORE selectedPath so onReady sees the flag when the new view
      // mounts. The Editor handles vim-insert automatically for empty bodies.
      pendingFocusAfterLoad = true;
      pushToHistory("primary", newPath);
      selectedPath = newPath;
      focusedPane = "primary";
      query = "";
    } catch (e) {
      console.error("create_note failed", e);
      pendingFocusAfterLoad = false;
    }
  }

  function sortLabel(id: SortMode): string {
    switch (id) {
      case "best":
        return "Best match (relevance when filtering, recency otherwise)";
      case "new":
        return "Newest modified first";
      case "old":
        return "Oldest modified first";
      case "az":
        return "Title A → Z";
      case "za":
        return "Title Z → A";
    }
  }

  // Open in primary pane. Pushes to primary history, scrolls sidebar row to top.
  function openInPrimary(targetPath: string) {
    pushToHistory("primary", targetPath);
    selectedPath = targetPath;
    focusedPane = "primary";
    void scrollSelectedIntoView("start");
  }

  // Open in secondary pane (split). Pushes to secondary history; sidebar
  // highlight stays on primary's note (with green inset on the secondary).
  function openInSecondary(targetPath: string) {
    pushToHistory("secondary", targetPath);
    secondaryPath = targetPath;
    focusedPane = "secondary";
  }

  function closeSecondary() {
    secondaryPath = null;
    secondaryHistory = { stack: [], idx: -1 };
    focusedPane = "primary";
  }

  // Bare click on a wikilink: stay in clicked pane. Cmd/Ctrl+click: open in
  // the OTHER pane. The Editor doesn't know which pane it is — parent wires
  // direction here.
  function openWikilinkFromPrimary(target: string, alt: boolean) {
    if (alt) openInSecondary(target);
    else openInPrimary(target);
  }
  function openWikilinkFromSecondary(target: string, alt: boolean) {
    if (alt) openInPrimary(target);
    else openInSecondary(target);
  }

  async function createWikilinkInPane(title: string, alt: boolean, fromPane: "primary" | "secondary"): Promise<string | null> {
    const trimmed = title.trim();
    if (!trimmed) return null;
    try {
      const newPath = await invoke<string>("create_note", { title: trimmed });
      const optimistic: Note = {
        path: newPath,
        title: trimmed,
        snippet: "",
        modified: Math.floor(Date.now() / 1000),
      };
      rawResults = [optimistic, ...rawResults];
      allNotes = [optimistic, ...allNotes];
      // If the user wanted a split gesture, open in the OTHER pane.
      const openHere = !alt;
      const targetPane: "primary" | "secondary" =
        (openHere ? fromPane === "primary" : fromPane === "secondary") ? "primary" : "secondary";
      if (targetPane === "primary") {
        pushToHistory("primary", newPath);
        selectedPath = newPath;
        focusedPane = "primary";
        void scrollSelectedIntoView("start");
      } else {
        pushToHistory("secondary", newPath);
        secondaryPath = newPath;
        focusedPane = "secondary";
      }
      return newPath;
    } catch (e) {
      console.error("create_note (wikilink) failed", e);
      return null;
    }
  }
  const createFromPrimary = (title: string, alt: boolean) =>
    createWikilinkInPane(title, alt, "primary");
  const createFromSecondary = (title: string, alt: boolean) =>
    createWikilinkInPane(title, alt, "secondary");

  // When delete modal opens, focus the cancel button (safer default).
  $effect(() => {
    if (deleteConfirmOpen) {
      void tick().then(() => cancelBtn?.focus());
    }
  });

  // ----- Rename modal --------------------------------------------------

  async function openRename(path: string) {
    if (!path) return;
    renameTargetPath = path;
    renameInputVal = getTitleForPath(path);
    renameError = null;
    renameBacklinkCount = 0;
    renameSaving = false;
    renameOpen = true;
    await tick();
    renameInputEl?.focus();
    renameInputEl?.select();
    // Async backlink count for the prompt.
    try {
      const bls = await invoke<unknown[]>("find_backlinks", { path });
      if (renameOpen && renameTargetPath === path) {
        renameBacklinkCount = Array.isArray(bls) ? bls.length : 0;
      }
    } catch {
      /* count is just informational */
    }
  }

  function cancelRename() {
    renameOpen = false;
    renameTargetPath = null;
    renameError = null;
  }

  async function confirmRename() {
    if (!renameTargetPath || renameSaving) return;
    const newTitle = renameInputVal.trim();
    if (!newTitle) {
      renameError = "Title can't be empty";
      return;
    }
    const oldPath = renameTargetPath;
    const oldTitle = getTitleForPath(oldPath);
    if (newTitle === oldTitle) {
      cancelRename();
      return;
    }
    renameSaving = true;
    renameError = null;
    try {
      // Flush any pending autosaves first — otherwise the editor could
      // write stale content to the about-to-be-renamed path.
      await flushAllEditors();
      const newPath = await invoke<string>("rename_note", {
        path: oldPath,
        newTitle,
      });
      // Rewire history + selection in any pane that pointed at the old path.
      for (const h of [primaryHistory, secondaryHistory]) {
        h.stack = h.stack.map((p) => (p === oldPath ? newPath : p));
      }
      if (selectedPath === oldPath) selectedPath = newPath;
      if (secondaryPath === oldPath) secondaryPath = newPath;
      // Optimistic update so list reflects rename before the watcher fires.
      const stamp = Math.floor(Date.now() / 1000);
      const remap = (n: Note): Note =>
        n.path === oldPath ? { ...n, path: newPath, title: newTitle, modified: stamp } : n;
      rawResults = rawResults.map(remap);
      allNotes = allNotes.map(remap);
      renameOpen = false;
      renameTargetPath = null;
    } catch (e) {
      renameError = String(e);
    } finally {
      renameSaving = false;
    }
  }

  function handleRenameKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      void confirmRename();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancelRename();
    }
  }

  async function refreshSavedSearches() {
    try {
      savedSearches = await invoke<SavedSearch[]>("list_saved_searches");
    } catch {
      savedSearches = [];
    }
  }

  async function refreshTagMeta() {
    try {
      const [vocab, all] = await Promise.all([
        invoke<string[]>("get_tag_vocabulary"),
        invoke<TagCount[]>("list_all_tags"),
      ]);
      tagVocabulary = vocab;
      allTagCounts = all;
    } catch {
      /* keep stale */
    }
  }

  function activateSavedSearch(s: SavedSearch) {
    query = s.query;
    void tick().then(() => {
      searchInput?.focus();
      searchInput?.select();
    });
  }

  function activateSlot(slot: number) {
    const s = savedSearches.find((x) => x.slot === slot);
    if (s) activateSavedSearch(s);
  }

  async function openSaveSearchModal() {
    if (!query.trim()) return;
    saveSearchName = "";
    try {
      saveSearchSlot = await invoke<number | null>("next_free_search_slot");
    } catch {
      saveSearchSlot = null;
    }
    saveSearchOpen = true;
    await tick();
    saveSearchInputEl?.focus();
  }

  function cancelSaveSearch() {
    saveSearchOpen = false;
  }

  async function confirmSaveSearch() {
    const name = saveSearchName.trim();
    if (!name) return;
    const id =
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `s-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    try {
      savedSearches = await invoke<SavedSearch[]>("upsert_saved_search", {
        item: { id, name, query, slot: saveSearchSlot },
      });
      saveSearchOpen = false;
    } catch (e) {
      console.error("save search failed", e);
    }
  }

  async function deleteSavedSearch(id: string) {
    try {
      savedSearches = await invoke<SavedSearch[]>("delete_saved_search", { id });
    } catch (e) {
      console.error("delete saved search failed", e);
    }
  }

  function handleEditorTagClick(tag: string) {
    // Click a pill in the editor → filter the note list by that tag.
    query = `tag:${tag}`;
    void tick().then(() => searchInput?.focus());
  }

  async function handleEditorTagPromote(tag: string, addToVocab: boolean) {
    // Toggle vocab membership for `tag` and persist via IPC. The updated
    // vocab list trickles into both editors (they observe tagVocabulary).
    const next = addToVocab
      ? Array.from(new Set([...tagVocabulary, tag]))
      : tagVocabulary.filter((t) => t !== tag);
    try {
      await invoke("set_tag_vocabulary", { vocabulary: next });
      tagVocabulary = next;
    } catch (e) {
      console.error("set_tag_vocabulary failed", e);
    }
  }

  onMount(async () => {
    unlisten = await listen("notes_changed", async () => {
      await Promise.all([performSearch(query), refreshAllNotes(), refreshTagMeta()]);
    });
    window.addEventListener("keydown", handleGlobalKey, true);
    await Promise.all([refreshAllNotes(), refreshSavedSearches(), refreshTagMeta()]);
    await tick();
    searchInput?.focus();
  });

  onDestroy(() => {
    unlisten?.();
    window.removeEventListener("keydown", handleGlobalKey, true);
  });
</script>

<main>
  <header>
    <input
      bind:this={searchInput}
      bind:value={query}
      onkeydown={handleSearchKey}
      class="search"
      type="text"
      placeholder="type to filter…"
      spellcheck="false"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
    />
    <button
      class="gear"
      onclick={() => (settingsOpen = true)}
      aria-label="Open settings (Ctrl+,)"
      title="Settings (Ctrl+,)"
      tabindex="-1"
    >
      ⚙
    </button>
  </header>
  {#if savedSearches.length > 0}
    <div class="saved-row">
      {#each savedSearches as s (s.id)}
        <button
          class="saved-chip"
          class:active={query === s.query}
          onclick={() => activateSavedSearch(s)}
          oncontextmenu={(e) => {
            e.preventDefault();
            if (confirm(`Delete saved search "${s.name}"?`)) {
              void deleteSavedSearch(s.id);
            }
          }}
          title={`${s.query}${s.slot ? ` — Ctrl+${s.slot}` : ""}`}
          tabindex="-1"
        >
          <span class="saved-name">{s.name}</span>
          {#if s.slot}<span class="saved-slot">{s.slot}</span>{/if}
        </button>
      {/each}
    </div>
  {/if}
  <div class="status">
    <span class="count">
      {#if query}
        {notes.length} match{notes.length === 1 ? "" : "es"}
      {:else}
        {notes.length} note{notes.length === 1 ? "" : "s"}
      {/if}
      {#if selectedPath}
        <span class="sep">·</span>
        <button
          class="wordcount"
          onclick={toggleCountMode}
          title="Click to switch between word count and character count"
          tabindex="-1"
        >
          {#if countMode === "words"}
            <span class="wc-num">{editorWords.toLocaleString()}</span><span class="wc-unit">w</span>
          {:else}
            <span class="wc-num">{editorChars.toLocaleString()}</span><span class="wc-unit">c</span>
          {/if}
        </button>
      {/if}
    </span>
    <span class="sort">
      {#each SORT_OPTIONS as opt, i (opt.id)}
        {#if i > 0}<span class="sep">·</span>{/if}
        <button
          class="sort-btn"
          class:active={sortMode === opt.id}
          onclick={() => (sortMode = opt.id)}
          title={sortLabel(opt.id)}
          tabindex="-1"
        >
          {opt.label}
        </button>
      {/each}
    </span>
  </div>
  <div class="body">
    <ul class="notes">
      {#each notes as note (note.path)}
        <li
          class="note"
          class:selected={note.path === selectedPath}
          class:secondary={note.path === secondaryPath && note.path !== selectedPath}
          onclick={(e) => handleNoteClick(e, note.path)}
          ondblclick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            void openRename(note.path);
          }}
        >
          <span class="note-title">{@html highlight(note.title, note.title_matches)}</span>
          <span class="snippet">{@html highlight(note.snippet, note.snippet_matches)}</span>
          <span class="modified">{formatModified(note.modified)}</span>
        </li>
      {/each}
      {#if notes.length === 0 && query}
        <li class="empty">No matches. Press Enter to create "{query}" (coming in M4b)</li>
      {/if}
      {#if notes.length === 0 && !query}
        <li class="empty">No notes yet. Drop a .md file into ~/malt/</li>
      {/if}
    </ul>
    <div class="editor-pane">
      {#if selectedPath}
        <div class="editors-row" bind:this={editorsRowEl}>
          <div
            class="editor-wrapper"
            style:flex={secondaryPath ? `${splitFraction} 1 0` : "1 1 0"}
            onmousedowncapture={() => (focusedPane = "primary")}
            onfocusincapture={() => (focusedPane = "primary")}
          >
            {#if secondaryPath}
              <div class="pane-title" class:active={focusedPane === "primary"}>
                <span class="pane-accent primary-accent"></span>
                <span class="pane-title-text" title="Double-click to rename" role="button" tabindex="-1"
                  ondblclick={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (selectedPath) void openRename(selectedPath);
                  }}
                >{getTitleForPath(selectedPath)}</span>
              </div>
            {/if}
            <Editor
              path={selectedPath}
              query={query}
              allNotes={allNotes}
              onNavigate={openWikilinkFromPrimary}
              onCreate={createFromPrimary}
              onReady={handleEditorReady}
              onCount={handleEditorCount}
              onClose={closeSecondary}
              onRename={(p) => void openRename(p)}
              tagVocabulary={tagVocabulary}
              allTags={allTagNames}
              onTagClick={handleEditorTagClick}
              onTagPromote={handleEditorTagPromote}
            />
          </div>
          {#if secondaryPath}
            <div
              class="vresize-handle"
              onmousedown={startSplitResize}
              role="separator"
              aria-orientation="vertical"
              title="Drag to resize"
            ></div>
            <div
              class="editor-wrapper"
              style:flex={`${1 - splitFraction} 1 0`}
              onmousedowncapture={() => (focusedPane = "secondary")}
              onfocusincapture={() => (focusedPane = "secondary")}
            >
              <div class="pane-title" class:active={focusedPane === "secondary"}>
                <span class="pane-accent secondary-accent"></span>
                <span class="pane-title-text" title="Double-click to rename" role="button" tabindex="-1"
                  ondblclick={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (secondaryPath) void openRename(secondaryPath);
                  }}
                >{getTitleForPath(secondaryPath)}</span>
                <button
                  class="pane-close"
                  onclick={closeSecondary}
                  title="Close secondary pane (Ctrl+W)"
                  tabindex="-1"
                  aria-label="Close secondary pane"
                >×</button>
              </div>
              <Editor
                path={secondaryPath}
                query={query}
                allNotes={allNotes}
                onNavigate={openWikilinkFromSecondary}
                onCreate={createFromSecondary}
                onClose={closeSecondary}
                onRename={(p) => void openRename(p)}
                tagVocabulary={tagVocabulary}
                allTags={allTagNames}
                onTagClick={handleEditorTagClick}
              />
            </div>
          {/if}
        </div>
        {#if !linkbacksCollapsed}
          <div
            class="resize-handle"
            onmousedown={startLinkbacksResize}
            role="separator"
            aria-orientation="horizontal"
          ></div>
        {/if}
        <div
          class="linkbacks-wrapper"
          style:height={linkbacksCollapsed ? "auto" : `${linkbacksHeight}px`}
        >
          <Linkbacks
            currentPath={selectedPath}
            bind:collapsed={linkbacksCollapsed}
            onNavigate={openInPrimary}
          />
        </div>
      {:else}
        <div class="hint">Select a note on the left to open it. Edits autosave.</div>
      {/if}
    </div>
  </div>
</main>

<Settings bind:open={settingsOpen} />

{#if exportOpen && selectedPath}
  <div
    class="rename-backdrop"
    role="presentation"
    onclick={cancelExport}
  >
    <div
      class="rename-panel export-panel"
      role="dialog"
      aria-modal="true"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <div class="rename-label">export note</div>
      <div class="export-title">{getTitleForPath(selectedPath)}</div>

      {#if exportLinkedCount > 0}
        <label class="export-checkbox">
          <input type="checkbox" bind:checked={exportAppendLinks} />
          Append linked notes
          <span class="export-hint">({exportLinkedCount} note{exportLinkedCount === 1 ? "" : "s"} linked from this one)</span>
        </label>
      {/if}

      <div class="export-section-label">Save as file</div>
      <div class="export-buttons">
        <button class="export-btn" disabled={exportBusy} onclick={() => void exportToFile("md")}>Clean .md</button>
        <button class="export-btn" disabled={exportBusy} onclick={() => void exportToFile("html")}>.html</button>
        <button class="export-btn" disabled={exportBusy} onclick={() => void exportToFile("epub")}>.epub</button>
        <button class="export-btn" disabled={exportBusy} onclick={() => void exportToFile("txt")}>.txt</button>
      </div>

      <div class="export-section-label">Copy to clipboard</div>
      <div class="export-buttons">
        <button class="export-btn" disabled={exportBusy} onclick={() => void exportCopyPlain()}>Plain text</button>
        <button class="export-btn" disabled={exportBusy} onclick={() => void exportCopyRich()}>Rich text (HTML)</button>
      </div>

      {#if exportStatus}
        <div class="export-status">{exportStatus}</div>
      {/if}

      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={cancelExport} disabled={exportBusy}>close</button>
      </div>
    </div>
  </div>
{/if}

{#if saveSearchOpen}
  <div
    class="rename-backdrop"
    role="presentation"
    onclick={cancelSaveSearch}
  >
    <div
      class="rename-panel"
      role="dialog"
      aria-modal="true"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <div class="rename-label">save search</div>
      <input
        bind:this={saveSearchInputEl}
        bind:value={saveSearchName}
        class="rename-input"
        placeholder="Name (e.g. open loops)"
        spellcheck="false"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        onkeydown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); void confirmSaveSearch(); }
          if (e.key === "Escape") { e.preventDefault(); cancelSaveSearch(); }
        }}
      />
      <div class="rename-sub">
        Query: <span class="mono">{query}</span>
        {#if saveSearchSlot !== null}
          <br />Bound to {(typeof navigator !== "undefined" && /Mac/i.test(navigator.platform)) ? "⌘" : "Ctrl+"}{saveSearchSlot}.
        {:else}
          <br />No free slot (all 1-9 taken). Saved without a shortcut.
        {/if}
      </div>
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={cancelSaveSearch}>cancel</button>
        <button
          class="rename-btn confirm"
          onclick={() => void confirmSaveSearch()}
          disabled={!saveSearchName.trim()}
        >save</button>
      </div>
    </div>
  </div>
{/if}

{#if renameOpen && renameTargetPath}
  <div
    class="rename-backdrop"
    role="presentation"
    onclick={cancelRename}
    onkeydown={(e) => {
      if (e.key === "Escape") cancelRename();
    }}
  >
    <div
      class="rename-panel"
      role="dialog"
      aria-modal="true"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <div class="rename-label">Rename note</div>
      <input
        bind:this={renameInputEl}
        bind:value={renameInputVal}
        class="rename-input"
        spellcheck="false"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        onkeydown={handleRenameKey}
      />
      <div class="rename-sub">
        {#if renameError}
          <span class="rename-err">{renameError}</span>
        {:else if renameBacklinkCount > 0}
          {renameBacklinkCount} link{renameBacklinkCount === 1 ? "" : "s"} in other notes will be updated.
        {:else}
          No other notes link here.
        {/if}
      </div>
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={cancelRename}>cancel</button>
        <button
          class="rename-btn confirm"
          onclick={() => void confirmRename()}
          disabled={renameSaving}
        >{renameSaving ? "renaming…" : "rename"}</button>
      </div>
    </div>
  </div>
{/if}

{#if deleteConfirmOpen && selectedPath}
  <div
    class="delete-backdrop"
    role="presentation"
    onclick={cancelDelete}
    onkeydown={(e) => {
      if (e.key === "Escape") cancelDelete();
      if (e.key === "Enter") void confirmDelete();
    }}
  >
    <div
      class="delete-panel"
      role="dialog"
      aria-modal="true"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => {
        e.stopPropagation();
        handleDeleteModalKey(e);
      }}
    >
      <div class="delete-msg">
        Delete <strong>{getTitleForPath(selectedPath)}</strong>?
      </div>
      <div class="delete-sub">This removes the .md file from ~/malt/ and cannot be undone. Use ← / → to switch.</div>
      <div class="delete-actions">
        <button
          class="delete-btn delete-cancel"
          bind:this={cancelBtn}
          onclick={cancelDelete}
        >
          cancel
        </button>
        <button
          class="delete-btn delete-confirm"
          bind:this={deleteBtn}
          onclick={() => void confirmDelete()}
        >
          delete
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  :global(:root) {
    color-scheme: dark;
  }
  :global(html, body) {
    margin: 0;
    height: 100%;
    overflow: hidden;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 13px;
    background: #1a1a1a;
    color: #e0e0e0;
  }
  main {
    height: 100vh;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid #2a2a2a;
    flex-shrink: 0;
  }
  .search {
    flex: 1;
    background: transparent;
    border: 0;
    outline: 0;
    color: #e0e0e0;
    font: inherit;
    font-size: 15px;
    padding: 10px 12px;
    caret-color: #e0e0e0;
  }
  .search::placeholder {
    color: #555;
  }
  .gear {
    background: transparent;
    border: 0;
    color: #666;
    font-size: 14px;
    line-height: 1;
    padding: 0 12px;
    cursor: pointer;
  }
  .gear:hover {
    color: #e0e0e0;
    background: #232323;
  }
  .status {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 4px 12px;
    color: #555;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    border-bottom: 1px solid #2a2a2a;
    flex-shrink: 0;
  }
  .saved-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 4px 12px;
    border-bottom: 1px solid #2a2a2a;
    background: #161616;
    flex-shrink: 0;
  }
  .saved-chip {
    background: transparent;
    border: 1px solid #2e2e2e;
    color: #aaa;
    font: inherit;
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 3px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .saved-chip:hover {
    border-color: #555;
    color: #e0e0e0;
  }
  .saved-chip.active {
    border-color: #6cb6ff;
    color: #e0e0e0;
    background: rgba(108, 182, 255, 0.08);
  }
  .saved-slot {
    color: #6cb6ff;
    font-size: 9px;
    font-variant-numeric: tabular-nums;
    background: rgba(108, 182, 255, 0.15);
    padding: 0 3px;
    border-radius: 2px;
  }
  .mono {
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    color: #97b8d8;
  }
  .sort {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .sort-btn {
    background: transparent;
    border: 0;
    color: #555;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 0 2px;
    cursor: pointer;
  }
  .sort-btn:hover {
    color: #aaa;
  }
  .sort-btn.active {
    color: #e0e0e0;
  }
  .sep {
    color: #333;
  }
  .wordcount {
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    color: inherit;
    font: inherit;
    cursor: pointer;
    display: inline;
  }
  .wordcount:hover .wc-num {
    color: #ffffff;
  }
  .wc-num {
    color: #e0e0e0;
  }
  .wc-unit {
    color: #555;
    margin-left: 1px;
  }
  .body {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(260px, 32%) minmax(0, 1fr);
    overflow: hidden;
  }
  .notes {
    list-style: none;
    padding: 0;
    margin: 0;
    overflow-y: auto;
    border-right: 1px solid #2a2a2a;
  }
  .note {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    column-gap: 12px;
    row-gap: 1px;
    padding: 4px 12px;
    cursor: pointer;
    border-bottom: 1px solid transparent;
  }
  .note:hover {
    background: #232323;
  }
  .note.selected {
    background: #2c3a4a;
    box-shadow: inset 3px 0 0 0 #6cb6ff;
  }
  .note.secondary {
    box-shadow: inset 3px 0 0 0 #7ed29b;
  }
  .note.selected.secondary {
    /* Both panes show this note — combine accents */
    box-shadow: inset 3px 0 0 0 #6cb6ff, inset 6px 0 0 0 #7ed29b;
  }
  .note.selected .note-title {
    color: #ffffff;
  }
  .note.selected .snippet {
    color: #97a3b0;
  }
  .note.selected .modified {
    color: #7a8b9c;
  }
  .note-title {
    color: #e0e0e0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    grid-column: 1 / 2;
  }
  .snippet {
    color: #666;
    grid-column: 1 / 2;
    font-size: 11px;
    /* Allow up to 2 lines of snippet so highlighted matches past the first
       ~30 chars don't get hidden by single-line truncation. */
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    word-break: break-word;
    line-height: 1.35;
  }
  .modified {
    color: #555;
    font-variant-numeric: tabular-nums;
    grid-row: 1 / 2;
    grid-column: 2 / 3;
    align-self: start;
  }
  .note-title :global(mark),
  .snippet :global(mark) {
    background: rgba(255, 200, 100, 0.18);
    color: #f4c97c;
    padding: 0;
    border-radius: 0;
    font-weight: inherit;
  }
  .delete-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .delete-panel {
    background: #1a1a1a;
    border: 1px solid #333;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
    padding: 16px 20px;
    min-width: 320px;
    max-width: 480px;
  }
  .delete-msg {
    color: #e0e0e0;
    font-size: 13px;
    margin-bottom: 4px;
  }
  .delete-msg strong {
    color: #ffffff;
    font-weight: 600;
  }
  .delete-sub {
    color: #777;
    font-size: 11px;
    margin-bottom: 16px;
  }
  .delete-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .delete-btn {
    background: transparent;
    border: 1px solid #333;
    color: #aaa;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 6px 14px;
    cursor: pointer;
  }
  .delete-btn:hover {
    border-color: #555;
    color: #e0e0e0;
  }
  .delete-confirm:hover {
    border-color: #c66;
    color: #f88;
    background: rgba(200, 100, 100, 0.08);
  }
  .empty {
    padding: 24px 12px;
    color: #666;
  }
  .editor-pane {
    overflow: hidden;
    display: flex;
    flex-direction: column;
    background: #1a1a1a;
    min-width: 0;
  }
  .editors-row {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: row;
    overflow: hidden;
  }
  .editor-wrapper {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .pane-title {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    background: #161616;
    border-bottom: 1px solid #2a2a2a;
    color: #888;
    font-size: 11px;
    user-select: none;
    flex-shrink: 0;
  }
  .pane-title.active {
    color: #e0e0e0;
    background: #1c1c1c;
  }
  .pane-accent {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    opacity: 0.5;
  }
  .pane-title.active .pane-accent {
    opacity: 1;
  }
  .primary-accent {
    background: #6cb6ff;
  }
  .secondary-accent {
    background: #7ed29b;
  }
  .pane-title-text {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: text;
  }
  .pane-close {
    background: transparent;
    border: 0;
    color: #555;
    font-size: 14px;
    line-height: 1;
    padding: 0 4px;
    cursor: pointer;
  }
  .pane-close:hover {
    color: #e0e0e0;
  }
  .rename-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .rename-panel {
    background: #1a1a1a;
    border: 1px solid #333;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
    padding: 16px 20px;
    min-width: 380px;
    max-width: 560px;
  }
  .rename-label {
    color: #888;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 6px;
  }
  .rename-input {
    width: 100%;
    box-sizing: border-box;
    background: #111;
    border: 1px solid #333;
    color: #e0e0e0;
    font: inherit;
    font-size: 14px;
    padding: 6px 8px;
    outline: 0;
    caret-color: #e0e0e0;
  }
  .rename-input:focus {
    border-color: #555;
  }
  .rename-sub {
    color: #777;
    font-size: 11px;
    margin-top: 8px;
    margin-bottom: 16px;
    min-height: 1em;
  }
  .rename-err {
    color: #f88;
  }
  .rename-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }
  .rename-btn {
    background: transparent;
    border: 1px solid #333;
    color: #aaa;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 6px 14px;
    cursor: pointer;
  }
  .rename-btn:hover {
    border-color: #555;
    color: #e0e0e0;
  }
  .rename-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .rename-btn.confirm:hover {
    border-color: #6cb6ff;
    color: #cce6ff;
    background: rgba(108, 182, 255, 0.08);
  }
  .export-panel {
    min-width: 460px;
  }
  .export-title {
    color: #e0e0e0;
    font-size: 14px;
    margin-bottom: 14px;
    font-weight: 500;
  }
  .export-checkbox {
    display: flex;
    align-items: center;
    gap: 6px;
    color: #ccc;
    font-size: 12px;
    cursor: pointer;
    padding: 6px 10px;
    background: #161616;
    border: 1px solid #2a2a2a;
    border-radius: 3px;
    margin-bottom: 14px;
  }
  .export-checkbox input {
    margin: 0;
  }
  .export-hint {
    color: #666;
    font-size: 11px;
  }
  .export-section-label {
    color: #888;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    margin: 12px 0 6px;
  }
  .export-buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .export-btn {
    background: transparent;
    border: 1px solid #333;
    color: #ccc;
    font: inherit;
    font-size: 12px;
    padding: 6px 12px;
    cursor: pointer;
    border-radius: 3px;
  }
  .export-btn:hover:not(:disabled) {
    border-color: #6cb6ff;
    color: #cce6ff;
    background: rgba(108, 182, 255, 0.08);
  }
  .export-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .export-status {
    margin-top: 12px;
    padding: 6px 10px;
    background: rgba(108, 198, 108, 0.08);
    border: 1px solid rgba(108, 198, 108, 0.25);
    color: #9ed29e;
    font-size: 11px;
    border-radius: 3px;
  }
  .vresize-handle {
    flex: 0 0 4px;
    background: transparent;
    cursor: col-resize;
    border-left: 1px solid #2a2a2a;
    border-right: 1px solid #232323;
  }
  .vresize-handle:hover {
    background: rgba(108, 182, 255, 0.15);
  }
  .resize-handle {
    flex: 0 0 4px;
    background: transparent;
    cursor: row-resize;
    border-top: 1px solid #2a2a2a;
    border-bottom: 1px solid #232323;
  }
  .resize-handle:hover {
    background: rgba(108, 182, 255, 0.15);
  }
  .linkbacks-wrapper {
    flex: 0 0 auto;
    overflow: hidden;
    border-top: 1px solid #2a2a2a;
  }
  .hint {
    padding: 32px;
    color: #555;
    text-align: center;
    margin: auto;
  }
</style>
