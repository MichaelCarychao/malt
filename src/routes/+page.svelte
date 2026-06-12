<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onMount, onDestroy, tick } from "svelte";
  import Settings from "$lib/Settings.svelte";
  import Editor from "$lib/Editor.svelte";
  import Linkbacks from "$lib/Linkbacks.svelte";
  import BrewPane from "$lib/BrewPane.svelte";
  import { flushAllEditors } from "$lib/editorRegistry";
  import {
    type Tip,
    pickNextTip,
    markSeen,
    getLastSeen,
    shouldSkipOnStartup,
    setSkipOnStartup,
    renderKeysForOS,
  } from "$lib/tips";

  type Note = {
    path: string;
    title: string;
    /** Display name: first-line `# H1` if present, else the filename
     * (`title`). Identity (wikilinks, rename, history) still uses `title`. */
    display_title?: string;
    snippet: string;
    modified: number;
    tags?: string[];
    is_conflict?: boolean;
    is_empty?: boolean;
    /** True when the file is wrapped in a `MALT-ENC-v1:` envelope.
     * Body can't be read without the password. */
    is_encrypted?: boolean;
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

  /** Per-tag visual flair (icon glyph + accent color) applied to note
   * cards in the sidebar. Loaded from config; managed in Settings. */
  type TagStyle = { tag: string; icon: string; color: string };

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
  // Secondary-pane mode: "editor" (default — shows the secondaryPath
  // note in a normal editor) or "brew" (shows the AI brainstorm pane
  // streaming from the primary note's body). When `brewActive` is
  // true the BrewPane component takes over the secondary slot;
  // `closeSecondary` also clears brew state.
  let brewActive = $state(false);
  let brewSourceTitle = $state("");
  let brewSourceBody = $state("");
  // Bumped on each EXPLICIT brew invocation (Cmd+Shift+B). BrewPane keys
  // its streaming effect on this nonce alone, so re-firing brew — even on
  // the same note — re-streams, while live body edits passed via
  // brewSourceBody never trigger a new LLM call.
  let brewNonce = $state(0);
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

  // Saved-search context menu (right-click on a chip). Mirrors rowMenu's
  // floating-menu pattern.
  let savedMenu = $state<{ id: string; name: string; slot: number | null; x: number; y: number } | null>(null);
  // Rename + reorder modals share state with savedMenu (the id we're acting on).
  let renameSearchOpen = $state(false);
  let renameSearchId = $state<string | null>(null);
  let renameSearchName = $state("");
  let renameSearchInputEl: HTMLInputElement | null = $state(null);
  let reorderSearchOpen = $state(false);
  let reorderSearchId = $state<string | null>(null);
  let reorderSearchPosition = $state(1);
  let reorderSearchInputEl: HTMLInputElement | null = $state(null);
  // Drag-and-drop reorder. dragId is the id being dragged; dragOverId is
  // the chip currently being hovered (for the drop indicator).
  let savedDragId = $state<string | null>(null);
  let savedDragOverId = $state<string | null>(null);

  // Tag vocabulary + corpus tags, fed to the editor for autocomplete.
  let tagVocabulary = $state<string[]>([]);
  let allTagCounts = $state<TagCount[]>([]);
  let allTagNames = $derived(allTagCounts.map((t) => t.name));

  // Per-tag flair (icon + accent color) for note cards. Lookup map is
  // keyed by lowercase tag for case-insensitive matching against a note's
  // (already-canonical) tags.
  let tagStyles = $state<TagStyle[]>([]);
  let tagStyleMap = $derived(
    new Map(tagStyles.map((s) => [s.tag.toLowerCase(), s])),
  );
  /** Styles that apply to a note, in tagStyles (priority) order. */
  function stylesForNote(note: Note): TagStyle[] {
    const tags = note.tags;
    if (!tags || tags.length === 0 || tagStyleMap.size === 0) return [];
    const out: TagStyle[] = [];
    for (const s of tagStyles) {
      if (tags.some((t) => t.toLowerCase() === s.tag.toLowerCase())) out.push(s);
    }
    return out;
  }
  /** Icons to show on a card (capped so a heavily-tagged note stays tidy). */
  function flairIcons(note: Note): string[] {
    return stylesForNote(note)
      .map((s) => s.icon)
      .filter((i) => i.length > 0)
      .slice(0, 3);
  }
  /** First matching style's color = the card's accent (or "" for none). */
  function flairAccent(note: Note): string {
    return stylesForNote(note).find((s) => s.color.length > 0)?.color ?? "";
  }

  // ─── Per-note encryption ──────────────────────────────────────────
  //
  // Password cache keyed by note path. Lives only in memory — never
  // persisted to disk. When the user opens an encrypted note we either
  // hit this cache (silent unlock) or pop the password modal.
  //
  // The cache is cleared on window blur when
  // securityRepromptOnBlur is true, so walking away from your laptop
  // re-locks everything.
  let unlockedPasswords = $state<Map<string, string>>(new Map());
  let securityRepromptOnBlur = $state(true);

  // Password prompt modal. Same modal serves four use cases —
  // distinguished by `kind` — so the user gets one consistent surface:
  //   unlock:  open an encrypted note (single password field)
  //   encrypt: set initial password on a plaintext note (confirm field)
  //   change:  rotate password on an encrypted note (current + new + confirm)
  //   decrypt: confirm current password to drop encryption
  type PasswordKind = "unlock" | "encrypt" | "change" | "decrypt";
  let pwOpen = $state(false);
  let pwKind = $state<PasswordKind>("unlock");
  let pwPath = $state<string | null>(null);
  let pwTitle = $state(""); // display name, for the modal header
  let pwCurrent = $state("");
  let pwNew = $state("");
  let pwConfirm = $state("");
  let pwError = $state<string | null>(null);
  let pwBusy = $state(false);
  let pwInputEl: HTMLInputElement | null = $state(null);
  // Separate ref for the "new password" field in encrypt mode — Svelte
  // doesn't allow conditional bind:this, so we keep distinct refs and
  // pick which to focus based on `pwKind`.
  let pwNewInputEl: HTMLInputElement | null = $state(null);
  // After unlock, where to land. "primary" or "secondary" pane.
  let pwTargetPane = $state<"primary" | "secondary">("primary");

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
  // Left sidebar (note list) width in px — drag-resizable, persisted.
  function readInitialSidebarWidth(): number {
    if (typeof localStorage !== "undefined") {
      const s = localStorage.getItem("malt.sidebarWidth");
      if (s) {
        const n = parseFloat(s);
        if (!isNaN(n) && n >= 200 && n <= 700) return n;
      }
    }
    return 300;
  }
  let sidebarWidth = $state(readInitialSidebarWidth());
  // Zen mode (Cmd/Ctrl+Shift+F): hide the note list so the editor fills
  // the window. Toggled; nothing is lost, the sidebar just collapses.
  let zenMode = $state(false);
  // Pinned note paths (absolute). Pinned notes bubble to the top of the
  // list and stay visible during text searches.
  let pinnedPaths = $state<string[]>([]);
  let pinnedSet = $derived(new Set(pinnedPaths));
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
  // Briefly true after an autosave fires — drives the saved-indicator pulse
  // in the status bar.
  let justSaved = $state(false);
  let savedPulseTimer: number | null = null;
  function flashSaved() {
    justSaved = true;
    if (savedPulseTimer) clearTimeout(savedPulseTimer);
    savedPulseTimer = window.setTimeout(() => { justSaved = false; }, 1200) as unknown as number;
  }
  // Tracks whether an Anthropic API key is configured. Renders a status dot.
  let hasApiKey = $state(false);
  // Brief boot splash that hides once the initial note list has loaded.
  let booting = $state(true);

  // ── Vaults ────────────────────────────────────────────────────────
  // Mirror of the backend vault registry. Refreshed on boot + after
  // any mutating IPC. The active vault's name shows in the sidebar
  // header next to the note count, and clicking it opens a dropdown
  // of every vault for one-click switching.
  type Vault = { name: string; path: string };
  type VaultsState = { vaults: Vault[]; active_index: number };
  let vaultsState = $state<VaultsState>({ vaults: [], active_index: 0 });
  let activeVaultName = $derived(
    vaultsState.vaults[vaultsState.active_index]?.name ?? "vault",
  );
  let vaultMenuOpen = $state(false);
  // True while a vault switch/attach is reindexing — drives an "indexing…"
  // notice instead of the misleading "No notes yet" empty state.
  let vaultSwitching = $state(false);
  let vaultPickerOpen = $state(false); // Cmd+Shift+V switcher modal
  let vaultPickerInputEl: HTMLInputElement | null = $state(null);
  let vaultPickerQuery = $state("");
  // "Add vault" modal
  let addVaultOpen = $state(false);
  let addVaultName = $state("");
  let addVaultPath = $state("");
  let addVaultError = $state<string | null>(null);

  // Tips system — shown on the boot splash unless the user has opted out
  // via the "don't show on startup" checkbox. Also accessible on demand
  // from Settings → general via "Launch tips". When `tipsOpen` is true
  // the boot splash stays mounted so the user can navigate; pressing
  // any key dismisses (boot path) or Esc dismisses (settings-launch path).
  let tipsOpen = $state(false);
  let tipCurrent = $state<Tip | null>(null);
  let tipHistory: Tip[] = []; // in-memory back-stack for ←
  let tipSkipOnStartup = $state(false);
  // Distinguish boot-time tips (auto-dismiss enabled) from on-demand
  // tips launched from Settings (Esc-only dismiss, no skip checkbox).
  let tipsLaunchedFromSettings = $state(false);
  // Track when the splash mounted so we can enforce the 1s minimum
  // duration even on fast boots — prevents a jarring flash.
  let bootMountedAt = $state<number>(0);
  // Right-click context menu on sidebar note rows.
  let rowMenu = $state<{ path: string; x: number; y: number } | null>(null);

  // Embedding worker status (loading / ready / error). The first run
  // downloads ~33MB of ONNX model; we want a visible hint that's happening.
  let embedStatus = $state<"idle" | "loading" | "ready" | "error">("idle");

  // Derived: how many sync-conflict files exist in the current corpus.
  let conflictCount = $derived(allNotes.filter((n) => n.is_conflict).length);

  // Per-pane "open find" functions exposed by the editor instances.
  // Used by the global Cmd+F handler to forward into whichever pane has
  // focus (primary as default).
  let primaryFinder: (() => void) | null = $state(null);
  let secondaryFinder: (() => void) | null = $state(null);
  function setPrimaryFinder(fn: () => void) { primaryFinder = fn; }
  function setSecondaryFinder(fn: () => void) { secondaryFinder = fn; }

  // Auto-updater state — see handleUpdateCheck for the flow.
  type UpdateState =
    | { kind: "idle" }
    | { kind: "checking" }
    | { kind: "up_to_date" }
    // No GitHub release is published yet (only drafts), so /releases/latest
    // 404s and there's no manifest to fetch. Benign — distinct from a real
    // error so we can say so calmly instead of flashing a failure.
    | { kind: "no_release" }
    | { kind: "available"; version: string; notes: string }
    | { kind: "downloading"; progress: number }
    | { kind: "installing" }
    | { kind: "error"; message: string };
  let updateState = $state<UpdateState>({ kind: "idle" });
  let updateToastDismissed = $state(false);
  let updateModalOpen = $state(false);
  // Holds the Update object between "available" and the install click.
  let pendingUpdate: { version: string; body?: string; downloadAndInstall: (cb: (e: unknown) => void) => Promise<void> } | null = null;

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
  // Bumped on every vault switch/attach/remove. Vault-wide refreshers
  // (refreshAllNotes / refreshTagMeta / refreshPinned) capture this at
  // entry and discard their result if it changed across the await — so a
  // slow old-vault listing can't repaint the new vault. Mirrors queryGen.
  let vaultGen = 0;
  let pendingFocusAfterLoad = false;

  // Navigation history — per pane. Explicit opens push; arrow nav doesn't.
  // Each pane has its own stack so back/forward operate on the focused
  // pane. Each entry remembers which vault it belonged to — back/forward
  // can therefore cross vaults: if the target entry's vault isn't the
  // active one, we switch vaults first, then land on the path.
  const HISTORY_CAP = 50;
  type HistoryEntry = { path: string; vaultPath: string };
  type PaneHistory = { stack: HistoryEntry[]; idx: number };
  let primaryHistory: PaneHistory = { stack: [], idx: -1 };
  let secondaryHistory: PaneHistory = { stack: [], idx: -1 };

  function pushToHistory(pane: "primary" | "secondary", path: string) {
    const h = pane === "primary" ? primaryHistory : secondaryHistory;
    const vaultPath = vaultsState.vaults[vaultsState.active_index]?.path ?? "";
    if (h.idx >= 0 && h.stack[h.idx].path === path && h.stack[h.idx].vaultPath === vaultPath) {
      return;
    }
    h.stack = h.stack.slice(0, h.idx + 1).concat({ path, vaultPath });
    if (h.stack.length > HISTORY_CAP) h.stack.shift();
    h.idx = h.stack.length - 1;
  }

  function activePane(): "primary" | "secondary" {
    return focusedPane === "secondary" && (secondaryPath || brewActive) ? "secondary" : "primary";
  }

  /** Navigate to a history entry, switching vaults if the entry
   * belongs to a different vault than the current one. After a vault
   * switch we replay the OTHER pane's most-recent entry that matches
   * the new vault, if any, so you don't lose your reading position
   * in the unrelated pane. */
  async function navigateToHistoryEntry(
    pane: "primary" | "secondary",
    entry: HistoryEntry,
  ) {
    const currentVaultPath = vaultsState.vaults[vaultsState.active_index]?.path ?? "";
    if (entry.vaultPath && entry.vaultPath !== currentVaultPath) {
      // The history entry lives in a different vault — switch first.
      const targetIdx = vaultsState.vaults.findIndex((v) => v.path === entry.vaultPath);
      if (targetIdx < 0) {
        // Vault was removed from the registry; can't cross. Skip silently.
        return;
      }
      // switchVault clears selection state + lists; we need to set
      // selection AFTER the refresh so the editor lands on the right
      // path. Pass through directly without going through switchVault
      // (which has its own clearing logic that would clobber what
      // we're about to set).
      // Crossing vaults swaps the editor's content; if a pending save
      // can't be flushed, abort the navigation rather than lose the edit.
      if ((await flushAllEditors()) > 0) {
        console.error("cross-vault navigation aborted: editor flush failed");
        alert("Couldn't save pending edits — navigation aborted to avoid losing changes.");
        return;
      }
      vaultsState = await invoke<VaultsState>("switch_vault", { index: targetIdx });
      // New vault is active — invalidate any in-flight old-vault refresh.
      vaultGen++;
      rawResults = [];
      allNotes = [];
      await Promise.all([refreshAllNotes(), performSearch(query)]);
    }
    if (pane === "primary") {
      selectedPath = entry.path;
      void scrollSelectedIntoView("nearest");
    } else {
      secondaryPath = entry.path;
    }
  }

  function goBack() {
    const pane = activePane();
    const h = pane === "primary" ? primaryHistory : secondaryHistory;
    if (h.idx <= 0) return;
    h.idx--;
    void navigateToHistoryEntry(pane, h.stack[h.idx]);
  }

  function goForward() {
    const pane = activePane();
    const h = pane === "primary" ? primaryHistory : secondaryHistory;
    if (h.idx >= h.stack.length - 1) return;
    h.idx++;
    void navigateToHistoryEntry(pane, h.stack[h.idx]);
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

  let notes = $derived.by(() => {
    const sorted = applySort(rawResults, sortMode);
    // Pinned notes bubble to the top — and stay visible even when a text
    // search would filter them out. We DON'T inject pins into computed
    // answer sets (semantic `~` / `is:` reports), where they'd be noise.
    if (semanticMode || specialReport || pinnedPaths.length === 0) return sorted;
    const pin = pinnedSet;
    const top = applySort(allNotes.filter((n) => pin.has(n.path)), sortMode);
    const rest = sorted.filter((n) => !pin.has(n.path));
    return [...top, ...rest];
  });

  // A leading ~ switches to semantic (embedding) search: "find what I
  // meant, not the words I used." Everything after the ~ is the concept.
  let semanticMode = $derived(query.trimStart().startsWith("~"));

  // Special "report" queries — computed lenses over the vault rather than
  // text search. Each is an exact directive; `reportFor` maps the raw
  // query to a report kind (or null). Used to route performSearch, guard
  // note-creation in tryEnter, and label the status bar.
  type ReportKind = "orphan" | "onthisday" | "duplicate" | "encrypted";
  function reportFor(q: string): ReportKind | null {
    const t = q.trim().toLowerCase();
    if (t === "is:orphan" || t === "is:orphans") return "orphan";
    if (t === "is:onthisday") return "onthisday";
    if (t === "is:duplicate" || t === "is:duplicates" || t === "is:dupe" || t === "is:dupes")
      return "duplicate";
    if (t === "is:encrypted" || t === "is:locked") return "encrypted";
    return null;
  }
  let specialReport = $derived(reportFor(query));

  /** Same calendar day (month + day) as today, in a prior year — by an
   * explicit YYYY-MM-DD title date when present, else the file's modified
   * date. Today itself is excluded. Local time throughout (matches how
   * daily notes are titled). Most-recent occurrence first. */
  function sameLocalDay(a: Date, b: Date): boolean {
    return (
      a.getFullYear() === b.getFullYear() &&
      a.getMonth() === b.getMonth() &&
      a.getDate() === b.getDate()
    );
  }
  function computeOnThisDay(items: Note[]): Note[] {
    const now = new Date();
    const month = now.getMonth();
    const day = now.getDate();
    const matched: { note: Note; key: number }[] = [];
    for (const n of items) {
      let when: Date | null = null;
      const tm = n.title.match(/^(\d{4})-(\d{2})-(\d{2})\b/);
      if (tm) {
        const dt = new Date(+tm[1], +tm[2] - 1, +tm[3]);
        if (!isNaN(dt.getTime())) when = dt;
      }
      if (!when && n.modified) when = new Date(n.modified * 1000);
      if (!when) continue;
      if (when.getMonth() !== month || when.getDate() !== day) continue;
      if (sameLocalDay(when, now)) continue; // exclude today
      matched.push({ note: n, key: when.getTime() });
    }
    matched.sort((a, b) => b.key - a.key);
    return matched.map((x) => x.note);
  }

  // When the query is exactly one `tag:foo` filter (no other terms), we
  // surface co-occurring tags as "often with" chips. This regex pulls
  // the bare tag out of that single-filter case; anything more complex
  // (extra words, a second operator) yields null and hides the chips.
  let singleTagFilter = $derived.by(() => {
    const m = query.trim().match(/^tag:#?([A-Za-z0-9_\/-]+)$/i);
    return m ? m[1].toLowerCase() : null;
  });
  let coTags = $state<TagCount[]>([]);
  $effect(() => {
    const tag = singleTagFilter;
    if (!tag) {
      coTags = [];
      return;
    }
    let cancelled = false;
    void invoke<TagCount[]>("tag_cooccurrence", { tag })
      .then((r) => {
        if (!cancelled) coTags = r;
      })
      .catch(() => {
        if (!cancelled) coTags = [];
      });
    return () => {
      cancelled = true;
    };
  });
  function addCoTag(name: string) {
    // Compose: append the co-tag as a second tag: filter (AND).
    query = `tag:${singleTagFilter} tag:${name}`;
    void tick().then(() => searchInput?.focus());
  }

  async function performSearch(q: string) {
    const myGen = ++queryGen;
    const trimmed = q.trimStart();
    if (trimmed.startsWith("~")) {
      const concept = trimmed.slice(1).trim();
      if (!concept) {
        // Bare "~" — show nothing yet; the user is mid-typing a concept.
        if (myGen === queryGen) rawResults = [];
        return;
      }
      try {
        const results = await invoke<Note[]>("semantic_search", { query: concept });
        if (myGen === queryGen) rawResults = results;
      } catch (e) {
        // Model/endpoint unavailable → empty rather than a broken bar.
        console.error("semantic_search failed", e);
        if (myGen === queryGen) rawResults = [];
      }
      return;
    }
    // Special report lenses (exact directives like `is:orphan`).
    const report = reportFor(q);
    if (report === "orphan") {
      try {
        const results = await invoke<Note[]>("list_orphans");
        if (myGen === queryGen) rawResults = results;
      } catch (e) {
        console.error("list_orphans failed", e);
        if (myGen === queryGen) rawResults = [];
      }
      return;
    }
    if (report === "duplicate") {
      try {
        const results = await invoke<Note[]>("list_near_duplicates");
        if (myGen === queryGen) rawResults = results;
      } catch (e) {
        console.error("list_near_duplicates failed", e);
        if (myGen === queryGen) rawResults = [];
      }
      return;
    }
    if (report === "onthisday") {
      // Computed client-side from the in-memory note list so the
      // date math uses *local* time (matching how daily notes are
      // titled) and needs no backend round-trip.
      const results = computeOnThisDay(allNotes);
      if (myGen === queryGen) rawResults = results;
      return;
    }
    if (report === "encrypted") {
      // The listing already knows which notes are encrypted; just filter.
      const results = allNotes.filter((n) => n.is_encrypted);
      if (myGen === queryGen) rawResults = results;
      return;
    }
    const results = await invoke<Note[]>("search_notes", { query: q });
    if (myGen === queryGen) rawResults = results;
  }

  async function refreshAllNotes() {
    const myGen = vaultGen;
    try {
      const result = await invoke<Note[]>("list_notes");
      // Discard a stale old-vault listing that resolved after a switch.
      if (myGen === vaultGen) allNotes = result;
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
    const now = new Date();
    const then = new Date(secs * 1000);
    const sameDay =
      now.getFullYear() === then.getFullYear() &&
      now.getMonth() === then.getMonth() &&
      now.getDate() === then.getDate();
    if (sameDay) {
      // "3:14p" / "11:02a" — short morning/afternoon marker, no zero-padded hour
      const h = then.getHours();
      const m = then.getMinutes().toString().padStart(2, "0");
      const ampm = h >= 12 ? "p" : "a";
      const h12 = h % 12 || 12;
      return `${h12}:${m}${ampm}`;
    }
    const yesterday = new Date(now);
    yesterday.setDate(now.getDate() - 1);
    if (
      yesterday.getFullYear() === then.getFullYear() &&
      yesterday.getMonth() === then.getMonth() &&
      yesterday.getDate() === then.getDate()
    ) {
      return "yest.";
    }
    // Within the last week: weekday abbreviation
    const ageMs = now.getTime() - then.getTime();
    const dayMs = 86400 * 1000;
    if (ageMs < 7 * dayMs) {
      return ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][then.getDay()];
    }
    // Within the same calendar year: "Mar 5"
    if (now.getFullYear() === then.getFullYear()) {
      const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
      return `${months[then.getMonth()]} ${then.getDate()}`;
    }
    // Older: "Mar '25"
    const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    return `${months[then.getMonth()]} '${(then.getFullYear() % 100).toString().padStart(2, "0")}`;
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
      // Tips splash takes priority over every other Esc target — it's
      // the topmost dialog while open. Without this, handleGlobalKey
      // (capture phase) would fall through to "clear search" and our
      // handleTipsKey would never see the event.
      if (tipsOpen) {
        e.preventDefault();
        e.stopPropagation();
        dismissTips();
        return;
      }
      if (updateModalOpen) {
        e.preventDefault();
        e.stopPropagation();
        closeUpdateModal();
        return;
      }
      if (rowMenu) {
        e.preventDefault();
        e.stopPropagation();
        dismissRowMenu();
        return;
      }
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
    // Text-entry guard: when focus is in a modal text field (rename,
    // save-search, add-vault, password, vault-picker — any <input>/
    // <textarea>/contentEditable), let the keystroke do normal text
    // editing instead of firing page-level shortcuts. Otherwise combos
    // like Cmd/Ctrl+Backspace (delete-word-left while editing a title)
    // would trigger the delete-note confirmation, and Cmd+1-9 / Cmd+D
    // would yank focus out of the modal.
    //
    // The editor (`.cm-content`) is contentEditable too, but it owns its
    // own keymap and the branches below already special-case it (e.g. the
    // `f` / Backspace `inEditor` checks), so we EXCLUDE it here and leave
    // that handling intact. Escape is already routed above and returns
    // before reaching this point, so it keeps working in modals.
    {
      const ae = document.activeElement;
      const inEditor = ae instanceof HTMLElement && !!ae.closest(".cm-content");
      const inTextEntry =
        !inEditor &&
        (ae instanceof HTMLInputElement ||
          ae instanceof HTMLTextAreaElement ||
          (ae instanceof HTMLElement && ae.isContentEditable));
      // The search bar is an <input> but its modifier combos are meant to
      // work globally (Cmd+L focuses it, Cmd+J/K navigate the list from it,
      // etc.) — handleSearchKey already lets modifier combos fall through to
      // here on purpose. So don't treat the search field as a text-entry
      // context; only modal/editor-adjacent fields suppress the shortcuts.
      if (inTextEntry && ae !== searchInput) return;
    }
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
      if (sKey === "v") {
        // Cmd/Ctrl+Shift+V opens the vault picker. Doesn't collide
        // with paste-without-formatting (which is Cmd/Ctrl+Shift+V
        // on Mac browsers too, but that only matters inside an input).
        e.preventDefault();
        e.stopPropagation();
        void openVaultPicker();
      }
      if (sKey === "r") {
        // Cmd/Ctrl+Shift+R jumps to a random note. (Plain Cmd+R is
        // rename; this is a page-level action, not an editor one.)
        e.preventDefault();
        e.stopPropagation();
        void openRandomNote();
      }
      if (sKey === "f") {
        // Cmd/Ctrl+Shift+F toggles zen mode (collapse the note list so
        // the editor fills the window). Plain Cmd+F is find-in-note.
        e.preventDefault();
        e.stopPropagation();
        zenMode = !zenMode;
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
    if (key === "d") {
      // Daily note: open (or create) today's dated note.
      e.preventDefault();
      e.stopPropagation();
      void openDailyNote();
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
    if (key === "f") {
      // Find in current note — open the focused editor's search panel.
      // If the cursor is already in the editor, the editor's own keymap
      // handles it; we only forward when focus is elsewhere (sidebar,
      // search bar, etc.).
      const active = document.activeElement;
      const inEditor =
        active instanceof HTMLElement && !!active.closest(".cm-content");
      if (!inEditor) {
        const fn = activePane() === "secondary" ? secondaryFinder : primaryFinder;
        if (fn) {
          e.preventDefault();
          e.stopPropagation();
          fn();
          return;
        }
      }
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
    // Encrypted-note guard: if locked, pop the password modal first.
    const note = allNotes.find((n) => n.path === path);
    if (note?.is_encrypted && !unlockedPasswords.has(path)) {
      openPasswordModal("unlock", path, note.title, "primary");
      return;
    }
    pushToHistory("primary", path);
    selectedPath = path;
    focusedPane = "primary";
    // A plain "open this note" means single-pane focus. Collapse any open
    // secondary NOTE pane so you can't get stuck in a split you didn't
    // ask for. (Cmd/Ctrl+click still opens a second pane deliberately; the
    // Brew pane is an explicit AI session, so leave it alone.)
    if (secondaryPath && !brewActive) {
      secondaryPath = null;
      secondaryHistory = { stack: [], idx: -1 };
    }
    void scrollSelectedIntoView("nearest");
  }

  // List row click — a plain click ALWAYS opens the note in the primary
  // editor, single-pane (openNote collapses any stray secondary split).
  // Cmd/Ctrl+click is the deliberate gesture to open in the secondary pane.
  // (We used to auto-open a sync-conflict file's original in the secondary
  // pane here; that surprised people — a plain click would unexpectedly
  // split the view for certain notes — so it's gone. The ⚠ badge still
  // flags conflicts; Cmd+click the original to compare.)
  function handleNoteClick(e: MouseEvent, path: string) {
    if (e.metaKey || e.ctrlKey) {
      openInSecondary(path);
      return;
    }
    openNote(path);
  }

  function handleNoteContextMenu(e: MouseEvent, path: string) {
    e.preventDefault();
    e.stopPropagation();
    rowMenu = { path, x: e.clientX, y: e.clientY };
  }

  function dismissRowMenu() {
    rowMenu = null;
  }

  async function rowMenuReveal(path: string) {
    rowMenu = null;
    try {
      await invoke("reveal_note", { path });
    } catch (err) {
      console.error("reveal_note failed", err);
    }
  }

  async function rowMenuDuplicate(path: string) {
    rowMenu = null;
    try {
      const newPath = await invoke<string>("duplicate_note", { path });
      // notes_changed event will refresh the list; jump to the new note.
      pushToHistory("primary", newPath);
      selectedPath = newPath;
      focusedPane = "primary";
      void scrollSelectedIntoView("nearest");
    } catch (err) {
      console.error("duplicate_note failed", err);
    }
  }

  function rowMenuRename(path: string) {
    rowMenu = null;
    void openRename(path);
  }

  async function refreshPinned() {
    const myGen = vaultGen;
    try {
      const result = await invoke<string[]>("get_pinned");
      if (myGen === vaultGen) pinnedPaths = result;
    } catch {
      /* keep stale */
    }
  }
  async function rowMenuTogglePin(path: string) {
    rowMenu = null;
    try {
      pinnedPaths = await invoke<string[]>("toggle_pin", { path });
    } catch (e) {
      console.error("toggle_pin failed", e);
    }
  }
  async function rowMenuMoveToVault(path: string, targetIndex: number) {
    rowMenu = null;
    try {
      // Moving the note to another vault relocates the file on disk. If a
      // pending save can't be flushed, abort rather than lose the edit.
      if ((await flushAllEditors()) > 0) {
        console.error("move_note_to_vault aborted: editor flush failed");
        alert("Couldn't save pending edits — move aborted to avoid losing changes.");
        return;
      }
      await invoke<string>("move_note_to_vault", { path, targetIndex });
      // The note left this vault — drop it from the open panes, then refresh.
      if (selectedPath === path) selectedPath = null;
      if (secondaryPath === path) secondaryPath = null;
      await Promise.all([refreshAllNotes(), performSearch(query), refreshPinned()]);
    } catch (e) {
      console.error("move_note_to_vault failed", e);
    }
  }

  function rowMenuDelete(path: string) {
    rowMenu = null;
    selectedPath = path;
    deleteConfirmOpen = true;
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

  // Left sidebar (note list) width resize.
  let resizingSidebar = false;
  let sidebarStartX = 0;
  let sidebarStartWidth = 300;
  function startSidebarResize(e: MouseEvent) {
    resizingSidebar = true;
    sidebarStartX = e.clientX;
    sidebarStartWidth = sidebarWidth;
    window.addEventListener("mousemove", onSidebarResize);
    window.addEventListener("mouseup", endSidebarResize);
    e.preventDefault();
  }
  function onSidebarResize(e: MouseEvent) {
    if (!resizingSidebar) return;
    let next = sidebarStartWidth + (e.clientX - sidebarStartX);
    if (next < 200) next = 200;
    if (next > 700) next = 700;
    sidebarWidth = next;
  }
  function endSidebarResize() {
    resizingSidebar = false;
    window.removeEventListener("mousemove", onSidebarResize);
    window.removeEventListener("mouseup", endSidebarResize);
  }
  // Persist sidebar width.
  $effect(() => {
    const w = sidebarWidth;
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.sidebarWidth", String(w));
    }
  });

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
      // Sync editor save-state BEFORE deleting: if the open note has
      // unsaved edits, the pane switch after deletion would flush them
      // back to the just-deleted path — save_note recreates files, so the
      // "deleted" note resurrected with the buffer's content. Flushing
      // first makes that flush a no-op; the malt:note-deleted event below
      // belt-and-suspenders it by marking the buffer clean either way.
      await flushAllEditors();
      await invoke("delete_note", { path });
    } catch (e) {
      console.error("delete_note failed", e);
      return;
    }
    // Tell every editor instance this path is gone on purpose, so none of
    // them re-saves stale content onto it during their own teardown.
    window.dispatchEvent(
      new CustomEvent("malt:note-deleted", { detail: { path } }),
    );
    // Prune history in both panes. With cross-vault entries the
    // filter compares by path only — if you delete a note, any history
    // entries referencing it (in this vault or another) are stale.
    for (const h of [primaryHistory, secondaryHistory]) {
      h.stack = h.stack.filter((e) => e.path !== path);
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

  /** Like getTitleForPath but prefers the note's display name (first-line
   * `# H1`). Use for what the user *reads* — pane titles, window title.
   * Identity-bound callers (rename, wikilinks, history) keep using
   * getTitleForPath (the filename). */
  function getDisplayTitle(path: string | null): string {
    if (!path) return "";
    const found = allNotes.find((n) => n.path === path) ?? notes.find((n) => n.path === path);
    return found?.display_title || found?.title || path.split(/[\\/]/).pop() || path;
  }

  /** Two-pane prompting: the "pre-prompt" for a pane is the OTHER pane's
   * note content (flushed to disk, read back live). Null when there's no
   * second NOTE pane open (no split, or the other pane is the Brew pane,
   * or it's an encrypted note we can't read). The Editor prepends this to
   * its own content and sends the lot raw. */
  async function getPrePromptFor(pane: "primary" | "secondary"): Promise<string | null> {
    if (brewActive) return null;
    const otherPath = pane === "primary" ? secondaryPath : selectedPath;
    if (!otherPath) return null;
    try {
      await flushAllEditors();
      if (allNotes.find((n) => n.path === otherPath)?.is_encrypted) {
        const pw = unlockedPasswords.get(otherPath);
        if (!pw) return null; // locked — can't use as a pre-prompt
        return await invoke<string>("read_encrypted_note", { path: otherPath, password: pw });
      }
      return await invoke<string>("read_note", { path: otherPath });
    } catch {
      return null;
    }
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
    // Semantic mode (~concept) and report lenses (is:orphan, is:onthisday,
    // is:duplicate): Enter never *creates* a note named after the directive.
    // It opens the top-ranked / selected result, or no-ops if there are
    // none. Creating happens only in plain lexical mode.
    if (semanticMode || specialReport) {
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
    // Encrypted-note guard for secondary pane.
    const note = allNotes.find((n) => n.path === targetPath);
    if (note?.is_encrypted && !unlockedPasswords.has(targetPath)) {
      openPasswordModal("unlock", targetPath, note.title, "secondary");
      return;
    }
    pushToHistory("secondary", targetPath);
    secondaryPath = targetPath;
    focusedPane = "secondary";
  }

  function closeSecondary() {
    secondaryPath = null;
    secondaryHistory = { stack: [], idx: -1 };
    brewActive = false;
    brewSourceTitle = "";
    brewSourceBody = "";
    focusedPane = "primary";
  }

  // Cmd+Shift+B in the primary editor — open the brew pane and stream
  // a brainstorm from the current note body. Reuses the secondary
  // pane's real estate (closing it first if a regular note was open).
  // `body` arrives from the editor pre-stringified.
  // M2 — AI is disabled on encrypted notes. Returns true (and offers to
  // decrypt, which needs the password) when `path` is an encrypted note;
  // false to let the AI action proceed. Decrypting rewrites the note as
  // plaintext, after which AI works normally. Blocks even when the note is
  // unlocked in-session: "encrypted on disk" is the gate, not readability.
  function blockAiForEncrypted(path: string | null): boolean {
    if (!path) return false;
    const note = allNotes.find((n) => n.path === path);
    if (!note?.is_encrypted) return false;
    const title = getTitleForPath(path);
    if (
      confirm(
        "AI features are disabled for encrypted notes.\n\nDecrypt this note to use AI? You'll need its password, and the note will be saved as plaintext.",
      )
    ) {
      openPasswordModal("decrypt", path, title);
    }
    return true;
  }

  function openBrewForPrimary(body: string) {
    if (blockAiForEncrypted(selectedPath)) return;
    const title = selectedPath ? getTitleForPath(selectedPath) : "(untitled)";
    secondaryPath = null;
    secondaryHistory = { stack: [], idx: -1 };
    brewSourceTitle = title;
    brewSourceBody = body;
    brewActive = true;
    // Explicit invocation — bump the nonce so BrewPane (re)streams even if
    // it was already open on this same note body.
    brewNonce++;
    focusedPane = "secondary";
  }
  // Same as above but triggered from the secondary editor — brews on
  // the secondary note's body. Output replaces the secondary's editor
  // view; the primary stays put.
  function openBrewForSecondary(body: string) {
    if (blockAiForEncrypted(secondaryPath)) return;
    const title = secondaryPath ? getTitleForPath(secondaryPath) : "(untitled)";
    secondaryPath = null;
    secondaryHistory = { stack: [], idx: -1 };
    brewSourceTitle = title;
    brewSourceBody = body;
    brewActive = true;
    // Explicit invocation — bump the nonce so BrewPane (re)streams even if
    // it was already open on this same note body.
    brewNonce++;
    focusedPane = "secondary";
  }
  // Append the streamed brew to the bottom of the source note. Writes
  // the augmented file to disk; the watcher will reload editors that
  // happen to be showing it. Returns an error string the BrewPane can
  // surface, or null on success.
  //
  // Encryption invariant: read_note / save_note operate on RAW bytes. On
  // an encrypted note read_note hands back the "MALT-ENC-v1:" envelope and
  // save_note would then persist plaintext-appended-to-ciphertext, leaving
  // the file permanently undecryptable. So we mirror getPrePromptFor: when
  // selectedPath is encrypted we round-trip through read_encrypted_note /
  // save_encrypted_note with the cached password, and if no password is
  // cached we abort (rather than corrupt the file) and tell the user.
  async function appendBrewToSource(brew: string): Promise<string | null> {
    if (!selectedPath) return "No source note to append to.";
    const path = selectedPath;
    const encrypted = !!allNotes.find((n) => n.path === path)?.is_encrypted;
    const password = encrypted ? unlockedPasswords.get(path) : undefined;
    if (encrypted && !password) {
      // Locked note with no cached password — refuse rather than write
      // plaintext over the encrypted envelope.
      return "That note is locked — unlock it before appending the brew.";
    }
    try {
      // Flush pending edits so we append onto the latest on-disk content,
      // not a stale snapshot.
      await flushAllEditors();
      const current = encrypted
        ? await invoke<string>("read_encrypted_note", { path, password })
        : await invoke<string>("read_note", { path });
      const stamp = new Date().toLocaleString();
      const augmented = `${current.trimEnd()}\n\n## Brew — ${stamp}\n\n${brew.trim()}\n`;
      if (encrypted) {
        await invoke("save_encrypted_note", { path, content: augmented, password });
      } else {
        await invoke("save_note", { path, content: augmented });
      }
      return null;
    } catch (e) {
      console.error("appendBrewToSource failed", e);
      return `Couldn't append the brew: ${String(e)}`;
    }
  }

  // Save the brew as a brand-new note in the vault. Title is
  // "Brew of <source title> — <date>" so the user can find them
  // chronologically; if `linkBack` is on, prepend a wikilink to the
  // source so the new note shows up in the source's linkbacks panel.
  async function saveBrewAsNote(args: {
    brew: string;
    sourceTitle: string;
    linkBack: boolean;
  }) {
    const stamp = new Date().toLocaleDateString();
    const title = `Brew of ${args.sourceTitle || "untitled"} — ${stamp}`;
    try {
      const newPath = await invoke<string>("create_note", { title });
      const linkLine = args.linkBack && args.sourceTitle
        ? `From [[${args.sourceTitle}]]\n\n`
        : "";
      const body = `${linkLine}${args.brew.trim()}\n`;
      await invoke("save_note", { path: newPath, content: body });
      // Navigate to the new note in the primary pane; close the brew
      // pane (the user just saved what was in it).
      closeSecondary();
      pushToHistory("primary", newPath);
      selectedPath = newPath;
      focusedPane = "primary";
      void scrollSelectedIntoView("nearest");
    } catch (e) {
      console.error("saveBrewAsNote failed", e);
    }
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
      // write stale content to the about-to-be-renamed path. If a flush
      // FAILS, abort: renaming moves the file out from under the editor,
      // so an unsaved edit would be lost to the old (now gone) path.
      if ((await flushAllEditors()) > 0) {
        renameError = "Couldn't save pending edits — rename aborted to avoid losing changes.";
        renameSaving = false;
        return;
      }
      const newPath = await invoke<string>("rename_note", {
        path: oldPath,
        newTitle,
      });
      // Rewire history + selection in any pane that pointed at the old path.
      // (Vault context stays the same — a rename never crosses vaults.)
      for (const h of [primaryHistory, secondaryHistory]) {
        h.stack = h.stack.map((e) =>
          e.path === oldPath ? { path: newPath, vaultPath: e.vaultPath } : e,
        );
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
    const myGen = vaultGen;
    try {
      const [vocab, all, styles] = await Promise.all([
        invoke<string[]>("get_tag_vocabulary"),
        invoke<TagCount[]>("list_all_tags"),
        invoke<TagStyle[]>("get_tag_styles"),
      ]);
      // Drop results from a vault we've since switched away from.
      if (myGen !== vaultGen) return;
      tagVocabulary = vocab;
      allTagCounts = all;
      tagStyles = styles;
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
    // Pre-fill name with the query itself so you can hit Enter to save
    // without typing anything. Common case: you already type queries
    // that read like names ("tag:draft", "modified:<7d", "meeting"),
    // and a literal-name save is rarely worse than nothing.
    saveSearchName = query.trim();
    try {
      saveSearchSlot = await invoke<number | null>("next_free_search_slot");
    } catch {
      saveSearchSlot = null;
    }
    saveSearchOpen = true;
    await tick();
    saveSearchInputEl?.focus();
    saveSearchInputEl?.select();
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

  // ── Saved-search context menu + rename/reorder modals ────────────────

  function openSavedSearchMenu(e: MouseEvent, s: SavedSearch) {
    e.preventDefault();
    savedMenu = { id: s.id, name: s.name, slot: s.slot, x: e.clientX, y: e.clientY };
  }
  function dismissSavedMenu() {
    savedMenu = null;
  }
  function startRenameSavedSearch(id: string, currentName: string) {
    savedMenu = null;
    renameSearchId = id;
    renameSearchName = currentName;
    renameSearchOpen = true;
    void tick().then(() => {
      renameSearchInputEl?.focus();
      renameSearchInputEl?.select();
    });
  }
  function cancelRenameSavedSearch() {
    renameSearchOpen = false;
    renameSearchId = null;
  }
  async function confirmRenameSavedSearch() {
    const id = renameSearchId;
    const name = renameSearchName.trim();
    if (!id || !name) return;
    try {
      savedSearches = await invoke<SavedSearch[]>("rename_saved_search", { id, name });
    } catch (e) {
      console.error("rename saved search failed", e);
    }
    renameSearchOpen = false;
    renameSearchId = null;
  }
  function startReorderSavedSearch(id: string, currentSlot: number | null) {
    savedMenu = null;
    reorderSearchId = id;
    // Default to current slot if any, else to current position + 1 (end of list).
    if (currentSlot != null) {
      reorderSearchPosition = currentSlot;
    } else {
      const idx = savedSearches.findIndex((x) => x.id === id);
      reorderSearchPosition = idx >= 0 ? idx + 1 : savedSearches.length;
    }
    reorderSearchOpen = true;
    void tick().then(() => {
      reorderSearchInputEl?.focus();
      reorderSearchInputEl?.select();
    });
  }
  function cancelReorderSavedSearch() {
    reorderSearchOpen = false;
    reorderSearchId = null;
  }
  async function confirmReorderSavedSearch() {
    const id = reorderSearchId;
    const target = Math.max(1, Math.min(savedSearches.length, Math.floor(reorderSearchPosition)));
    if (!id || !Number.isFinite(target)) return;
    try {
      savedSearches = await invoke<SavedSearch[]>("reorder_saved_search", {
        id,
        targetPosition: target,
      });
    } catch (e) {
      console.error("reorder saved search failed", e);
    }
    reorderSearchOpen = false;
    reorderSearchId = null;
  }
  function deleteSavedSearchWithConfirm(id: string, name: string) {
    savedMenu = null;
    if (confirm(`Delete saved search "${name}"?`)) {
      void deleteSavedSearch(id);
    }
  }

  // ── Drag and drop reorder ────────────────────────────────────────────

  function savedDragStart(e: DragEvent, id: string) {
    savedDragId = id;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      // Set some payload so the browser actually treats this as a drag.
      e.dataTransfer.setData("text/plain", id);
    }
  }
  function savedDragOver(e: DragEvent, overId: string) {
    if (!savedDragId || savedDragId === overId) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    savedDragOverId = overId;
  }
  function savedDragLeave(overId: string) {
    if (savedDragOverId === overId) savedDragOverId = null;
  }
  async function savedDrop(e: DragEvent, overId: string) {
    e.preventDefault();
    const sourceId = savedDragId;
    savedDragId = null;
    savedDragOverId = null;
    if (!sourceId || sourceId === overId) return;
    const targetIdx = savedSearches.findIndex((x) => x.id === overId);
    if (targetIdx < 0) return;
    try {
      savedSearches = await invoke<SavedSearch[]>("reorder_saved_search", {
        id: sourceId,
        targetPosition: targetIdx + 1,
      });
    } catch (err) {
      console.error("reorder saved search (drop) failed", err);
    }
  }
  function savedDragEnd() {
    savedDragId = null;
    savedDragOverId = null;
  }

  // ─── Password modal helpers ────────────────────────────────────────

  function openPasswordModal(
    kind: PasswordKind,
    path: string,
    title: string,
    targetPane: "primary" | "secondary" = "primary",
  ) {
    pwKind = kind;
    pwPath = path;
    pwTitle = title;
    pwTargetPane = targetPane;
    pwCurrent = "";
    pwNew = "";
    pwConfirm = "";
    pwError = null;
    pwBusy = false;
    pwOpen = true;
    void tick().then(() => {
      if (kind === "encrypt") pwNewInputEl?.focus();
      else pwInputEl?.focus();
    });
  }
  function cancelPasswordModal() {
    pwOpen = false;
    pwPath = null;
    pwCurrent = "";
    pwNew = "";
    pwConfirm = "";
    pwError = null;
  }
  async function confirmPasswordModal() {
    const path = pwPath;
    if (!path) return;
    pwError = null;
    pwBusy = true;
    try {
      if (pwKind === "unlock") {
        // Verify by attempting decryption.
        await invoke<string>("read_encrypted_note", { path, password: pwCurrent });
        unlockedPasswords = new Map(unlockedPasswords).set(path, pwCurrent);
        pwOpen = false;
        // Finally route to the originally-requested pane.
        if (pwTargetPane === "secondary") {
          openInSecondary(path);
        } else {
          openNote(path);
        }
      } else if (pwKind === "encrypt") {
        if (pwNew.length < 4) {
          pwError = "password must be at least 4 characters";
          return;
        }
        if (pwNew !== pwConfirm) {
          pwError = "passwords don't match";
          return;
        }
        // Read current plaintext, then encrypt with the new password.
        const plaintext = await invoke<string>("read_note", { path });
        await invoke("change_note_password", {
          path,
          oldPassword: "",
          newPassword: pwNew,
        });
        // Cache password so the note can be saved without re-prompting.
        unlockedPasswords = new Map(unlockedPasswords).set(path, pwNew);
        pwOpen = false;
        // notes_changed event will refresh the list; nothing else to do.
        // Avoid unused-variable lint:
        void plaintext;
      } else if (pwKind === "change") {
        if (pwNew.length < 4) {
          pwError = "new password must be at least 4 characters";
          return;
        }
        if (pwNew !== pwConfirm) {
          pwError = "new passwords don't match";
          return;
        }
        await invoke("change_note_password", {
          path,
          oldPassword: pwCurrent,
          newPassword: pwNew,
        });
        unlockedPasswords = new Map(unlockedPasswords).set(path, pwNew);
        pwOpen = false;
      } else if (pwKind === "decrypt") {
        await invoke("decrypt_existing_note", { path, password: pwCurrent });
        const next = new Map(unlockedPasswords);
        next.delete(path);
        unlockedPasswords = next;
        pwOpen = false;
      }
    } catch (e) {
      pwError = String(e);
    } finally {
      pwBusy = false;
    }
  }

  // Row context-menu entry points.
  function rowMenuEncrypt(path: string) {
    rowMenu = null;
    const note = allNotes.find((n) => n.path === path);
    if (!note) return;
    openPasswordModal("encrypt", path, note.title);
  }
  function rowMenuChangePassword(path: string) {
    rowMenu = null;
    const note = allNotes.find((n) => n.path === path);
    if (!note) return;
    openPasswordModal("change", path, note.title);
  }
  function rowMenuDecrypt(path: string) {
    rowMenu = null;
    const note = allNotes.find((n) => n.path === path);
    if (!note) return;
    openPasswordModal("decrypt", path, note.title);
  }

  // Window blur: when the focus-reprompt toggle is on, drop the entire
  // password cache so encrypted notes re-lock when the user walks away.
  // We re-render after clearing so editors of encrypted notes display
  // their "(locked)" state immediately.
  function handleWindowBlur() {
    if (!securityRepromptOnBlur) return;
    if (unlockedPasswords.size === 0) return;
    unlockedPasswords = new Map();
  }

  // ─── Tips system ─────────────────────────────────────────────────

  function nextTip() {
    if (tipCurrent) tipHistory.push(tipCurrent);
    const t = pickNextTip(tipCurrent);
    if (t) {
      tipCurrent = t;
      markSeen(t.id);
    }
  }
  function prevTip() {
    const t = tipHistory.pop();
    if (t) {
      tipCurrent = t;
      // markSeen is intentional here — viewing again counts as a view.
      markSeen(t.id);
    }
  }
  function dismissTips() {
    const wasLaunchedFromSettings = tipsLaunchedFromSettings;
    tipsOpen = false;
    tipsLaunchedFromSettings = false;
    // When launched from Settings, the splash isn't really "booting" any
    // more — just hide it immediately. The boot path has its own 1s
    // minimum enforced elsewhere (and the user actively dismissed the
    // tip anyway, signalling they've seen enough).
    if (wasLaunchedFromSettings) return;
    if (booting) booting = false;
  }
  function launchTipsFromSettings() {
    tipsLaunchedFromSettings = true;
    // Resume from the last-seen tip if we have one; otherwise start fresh.
    const last = getLastSeen();
    tipCurrent = last ?? pickNextTip(null);
    if (tipCurrent) markSeen(tipCurrent.id);
    tipHistory = [];
    tipsOpen = true;
  }
  function toggleSkipTipsOnStartup(e: Event) {
    const target = e.target as HTMLInputElement;
    tipSkipOnStartup = target.checked;
    setSkipOnStartup(tipSkipOnStartup);
  }
  function handleTipsKey(e: KeyboardEvent) {
    if (!tipsOpen) return;
    if (e.key === "ArrowRight") {
      e.preventDefault();
      nextTip();
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      prevTip();
    } else if (e.key === "Escape") {
      e.preventDefault();
      dismissTips();
    } else if (!tipsLaunchedFromSettings) {
      // On the boot splash, any other key dismisses too — matches the
      // "tap any key" prompt at the bottom of the splash.
      e.preventDefault();
      dismissTips();
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

  // -------------------- auto-updater --------------------

  async function checkForUpdates(opts: { silent: boolean }) {
    if (updateState.kind === "checking" || updateState.kind === "downloading") return;
    updateState = { kind: "checking" };
    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      if (update) {
        updateState = {
          kind: "available",
          version: update.version,
          notes: update.body ?? "",
        };
        pendingUpdate = update as typeof pendingUpdate;
        updateToastDismissed = false;
      } else {
        updateState = { kind: "up_to_date" };
        // If this was a silent background check, fade back to idle so we
        // don't leave a permanent "up to date" badge in the status bar.
        if (opts.silent) {
          setTimeout(() => {
            if (updateState.kind === "up_to_date") updateState = { kind: "idle" };
          }, 4000);
        }
      }
    } catch (e) {
      const msg = String(e);
      // The most common "failure" is benign: no published GitHub release
      // yet (drafts only), so `/releases/latest` 404s and the plugin can't
      // fetch a valid release manifest. Surface that calmly, not as a red
      // error. Genuine failures (offline, GitHub down) still show as errors.
      if (/valid release json|releases\/latest|404|not found/i.test(msg)) {
        updateState = { kind: "no_release" };
        if (opts.silent) {
          setTimeout(() => {
            if (updateState.kind === "no_release") updateState = { kind: "idle" };
          }, 4000);
        }
        return;
      }
      updateState = { kind: "error", message: msg };
      // Don't pester on background failures (offline, GitHub down, etc.).
      if (opts.silent) {
        setTimeout(() => {
          if (updateState.kind === "error") updateState = { kind: "idle" };
        }, 1000);
      }
    }
  }

  async function installPendingUpdate() {
    if (!pendingUpdate) return;
    updateState = { kind: "downloading", progress: 0 };
    let total = 0;
    let downloaded = 0;
    try {
      await pendingUpdate.downloadAndInstall((event: unknown) => {
        // Event shape: { event: "Started", data: { contentLength } } |
        //              { event: "Progress", data: { chunkLength } }    |
        //              { event: "Finished" }
        const e = event as { event: string; data?: { contentLength?: number; chunkLength?: number } };
        if (e.event === "Started") {
          total = e.data?.contentLength ?? 0;
          downloaded = 0;
          updateState = { kind: "downloading", progress: 0 };
        } else if (e.event === "Progress") {
          downloaded += e.data?.chunkLength ?? 0;
          const pct = total > 0 ? Math.min(99, Math.round((downloaded / total) * 100)) : 0;
          updateState = { kind: "downloading", progress: pct };
        } else if (e.event === "Finished") {
          updateState = { kind: "installing" };
        }
      });
      // Restart into the new version.
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
    } catch (e) {
      updateState = { kind: "error", message: String(e) };
      pendingUpdate = null;
    }
  }

  function dismissUpdateToast() {
    updateToastDismissed = true;
  }

  function openUpdateModal() {
    if (updateState.kind === "available") {
      updateModalOpen = true;
      updateToastDismissed = true;
    }
  }

  function closeUpdateModal() {
    updateModalOpen = false;
  }

  async function refreshApiKeyStatus() {
    try {
      hasApiKey = await invoke<boolean>("has_api_key");
    } catch {
      hasApiKey = false;
    }
  }

  // Create an Untitled note and jump into the editor. Wired to the
  // "Start writing →" CTA on the empty-vault state — previously that
  // button just focused the search bar, which read as "did nothing"
  // to anyone who already had the search bar focused (which is
  // ~always, since malt boots there).
  async function startFirstNote() {
    try {
      const newPath = await invoke<string>("create_note", { title: "Untitled" });
      // Wait for the notes_changed event to refresh allNotes so the
      // new note exists in the sidebar before we navigate to it.
      await refreshAllNotes();
      selectedPath = newPath;
      pushToHistory("primary", newPath);
      focusedPane = "primary";
      void tick().then(() => focusEditor());
    } catch (e) {
      console.error("startFirstNote failed", e);
    }
  }

  // Daily note: Cmd/Ctrl+D opens today's note (YYYY-MM-DD), creating it
  // if needed. A fresh daily note is seeded with #journal unless the
  // user disabled that in Settings → General.
  let dailyNoteTag = $state(true);
  function todayTitle(): string {
    const d = new Date();
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, "0");
    const day = String(d.getDate()).padStart(2, "0");
    return `${y}-${m}-${day}`;
  }
  async function openDailyNote() {
    const title = todayTitle();
    const existing = allNotes.find(
      (n) => n.title.localeCompare(title, undefined, { sensitivity: "base" }) === 0,
    );
    if (existing) {
      // Clear any active query first: if the search bar is filtering to
      // something the dated note doesn't match, the auto-select effect
      // would snap selection back to a visible row and the editor would
      // never move to the daily note. An empty query lists everything.
      query = "";
      await performSearch("");
      pushToHistory("primary", existing.path);
      selectedPath = existing.path;
      focusedPane = "primary";
      void scrollSelectedIntoView("nearest");
      void tick().then(() => focusEditor());
      return;
    }
    try {
      const newPath = await invoke<string>("create_note", { title });
      if (dailyNoteTag) {
        // Seed the journal tag on its OWN line at the bottom, with an
        // empty first line for the cursor. The editor's tagLineHider
        // hides the canonical bottom tag line + shows it as a pill, so
        // #journal never sits in the typing path. The cursor defaults
        // to offset 0 (the empty first line), ready to write.
        await invoke("save_note", { path: newPath, content: "\n#journal\n" });
      }
      // Clear the query (same reason as above) and refresh so the new
      // note is present + visible before we select it.
      query = "";
      await Promise.all([refreshAllNotes(), performSearch("")]);
      pushToHistory("primary", newPath);
      selectedPath = newPath;
      focusedPane = "primary";
      void tick().then(() => focusEditor());
    } catch (e) {
      console.error("openDailyNote failed", e);
    }
  }

  /** Jump to a random note in the active vault — a serendipity tool for
   * rediscovering what you've forgotten. Picks from the whole vault (not
   * the current filter) and clears the query so the pick is selectable.
   * Avoids re-landing on the note you're already reading when possible. */
  async function openRandomNote() {
    const pool =
      allNotes.length > 1 && selectedPath
        ? allNotes.filter((n) => n.path !== selectedPath)
        : allNotes;
    if (pool.length === 0) return;
    const pick = pool[Math.floor(Math.random() * pool.length)];
    query = "";
    await performSearch("");
    pushToHistory("primary", pick.path);
    selectedPath = pick.path;
    focusedPane = "primary";
    void scrollSelectedIntoView("nearest");
    void tick().then(() => focusEditor());
  }

  // ─── Vault management ────────────────────────────────────────────

  async function refreshVaults() {
    try {
      vaultsState = await invoke<VaultsState>("list_vaults");
    } catch {
      vaultsState = { vaults: [], active_index: 0 };
    }
  }
  async function switchVault(index: number) {
    vaultMenuOpen = false;
    vaultPickerOpen = false;
    if (index === vaultsState.active_index) return;
    vaultSwitching = true;
    try {
      // Flush any pending edits before swapping out from under the editor.
      // If a flush FAILS, abort the switch: the editor's content belongs to
      // the current vault and would be lost once we clear state + repoint.
      if ((await flushAllEditors()) > 0) {
        console.error("switch_vault aborted: editor flush failed");
        alert("Couldn't save pending edits — vault switch aborted to avoid losing changes.");
        vaultSwitching = false;
        return;
      }
      // CRITICAL: clear selection state AND the sidebar lists BEFORE
      // invoking the switch. Otherwise the auto-select effect (which
      // fires on selectedPath = null) reads notes[0] from the stale
      // old-vault list, hands the Editor a full path that still exists
      // on disk in the old vault, and we end up showing the wrong
      // note in the new vault. Clearing rawResults makes the derived
      // `notes` empty so auto-select bails until the refresh lands.
      selectedPath = null;
      secondaryPath = null;
      primaryHistory = { stack: [], idx: -1 };
      secondaryHistory = { stack: [], idx: -1 };
      rawResults = [];
      allNotes = [];
      // Drop every cached encrypted-note password — they belong to the
      // vault we're leaving. (Paths are unique so this isn't strictly a
      // correctness bug, but holding another vault's note passwords in
      // memory after switching away is poor hygiene.)
      unlockedPasswords = new Map();
      vaultsState = await invoke<VaultsState>("switch_vault", { index });
      // New vault is live — bump the generation so any old-vault refresh
      // still in flight (e.g. from a notes_changed event) discards itself.
      vaultGen++;
      // Pull the new vault's listing now so the auto-select effect
      // has something to pick on the next reactive cycle.
      await Promise.all([refreshAllNotes(), performSearch(query)]);
    } catch (e) {
      console.error("switch_vault failed", e);
    } finally {
      vaultSwitching = false;
    }
  }
  async function openVaultPicker() {
    await refreshVaults();
    vaultPickerOpen = true;
    vaultPickerQuery = "";
    await tick();
    vaultPickerInputEl?.focus();
  }
  function closeVaultPicker() {
    vaultPickerOpen = false;
  }
  let vaultPickerFiltered = $derived.by(() => {
    const q = vaultPickerQuery.trim().toLowerCase();
    if (!q) return [...vaultsState.vaults].sort((a, b) => a.name.localeCompare(b.name));
    return vaultsState.vaults
      .filter((v) => v.name.toLowerCase().includes(q) || v.path.toLowerCase().includes(q))
      .sort((a, b) => a.name.localeCompare(b.name));
  });
  function openAddVault() {
    vaultPickerOpen = false;
    addVaultName = "";
    addVaultPath = "";
    addVaultError = null;
    addVaultOpen = true;
  }
  async function pickAddVaultPath() {
    addVaultError = null;
    try {
      const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Choose a folder for this vault",
      });
      if (typeof picked === "string") addVaultPath = picked;
    } catch (e) {
      addVaultError = String(e);
    }
  }
  async function confirmAddVault() {
    addVaultError = null;
    if (!addVaultPath.trim()) {
      addVaultError = "pick a folder first";
      return;
    }
    vaultSwitching = true;
    try {
      // Adding a vault makes it active (a vault switch under the hood). If a
      // pending save can't be flushed, abort before we clear state + repoint
      // so the current vault's unsaved edit isn't lost.
      if ((await flushAllEditors()) > 0) {
        addVaultError = "Couldn't save pending edits — aborted to avoid losing changes.";
        vaultSwitching = false;
        return;
      }
      // Same staleness fix as switchVault: clear UI state BEFORE the
      // backend swap. add_vault auto-makes the new vault active.
      selectedPath = null;
      secondaryPath = null;
      primaryHistory = { stack: [], idx: -1 };
      secondaryHistory = { stack: [], idx: -1 };
      rawResults = [];
      allNotes = [];
      vaultsState = await invoke<VaultsState>("add_vault", {
        name: addVaultName.trim(),
        path: addVaultPath.trim(),
      });
      addVaultOpen = false;
      // add_vault on the backend just registers — it doesn't reindex.
      // Explicitly switch so the watcher + index + embeddings repoint.
      await invoke("switch_vault", { index: vaultsState.active_index });
      // New vault is active — invalidate any in-flight old-vault refresh.
      vaultGen++;
      await Promise.all([refreshAllNotes(), performSearch(query)]);
    } catch (e) {
      addVaultError = String(e);
    } finally {
      vaultSwitching = false;
    }
  }

  async function renameVaultAt(index: number, name: string) {
    try {
      vaultsState = await invoke<VaultsState>("rename_vault", { index, name });
    } catch (e) {
      console.error("rename_vault failed", e);
    }
  }
  async function removeVaultAt(index: number) {
    // Removing the active vault triggers an index rebuild on the
    // backend; the notes_changed event will refresh the sidebar.
    // Clear UI state first so we don't briefly render the removed
    // vault's last note in the editor.
    try {
      // Removing the active vault repoints the index off the current
      // notes. Abort if a pending save can't be flushed first.
      if ((await flushAllEditors()) > 0) {
        console.error("remove_vault aborted: editor flush failed");
        alert("Couldn't save pending edits — vault removal aborted to avoid losing changes.");
        return;
      }
      selectedPath = null;
      secondaryPath = null;
      primaryHistory = { stack: [], idx: -1 };
      secondaryHistory = { stack: [], idx: -1 };
      rawResults = [];
      allNotes = [];
      unlockedPasswords = new Map();
      vaultsState = await invoke<VaultsState>("remove_vault", { index });
      // Active vault may have changed — invalidate in-flight old refreshers.
      vaultGen++;
      await Promise.all([refreshAllNotes(), performSearch(query)]);
    } catch (e) {
      console.error("remove_vault failed", e);
    }
  }

  async function refreshSecurityConfig() {
    try {
      const cfg = await invoke<{ reprompt_on_blur: boolean }>("get_security_config");
      securityRepromptOnBlur = cfg.reprompt_on_blur;
    } catch {
      // Default already true; nothing to do on failure.
    }
  }

  async function refreshGeneralConfig() {
    try {
      const cfg = await invoke<{ daily_note_tag: boolean }>("get_config");
      dailyNoteTag = cfg.daily_note_tag;
    } catch {
      // Default already true.
    }
  }

  // Persist the last open primary note across restarts so users land where
  // they left off instead of always at "most recent".
  const LAST_OPEN_KEY = "malt.lastOpenPath";
  function persistLastOpen(path: string | null) {
    if (typeof localStorage === "undefined") return;
    if (path) localStorage.setItem(LAST_OPEN_KEY, path);
    else localStorage.removeItem(LAST_OPEN_KEY);
  }
  function readLastOpen(): string | null {
    if (typeof localStorage === "undefined") return null;
    return localStorage.getItem(LAST_OPEN_KEY);
  }

  // Push the current note's title into the window title bar.
  async function setWindowTitle(noteTitle: string | null) {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const w = getCurrentWindow();
      const t = noteTitle ? `malt — ${noteTitle}` : "malt";
      await w.setTitle(t);
    } catch {
      /* setTitle not critical; ignore */
    }
  }
  // Watch selectedPath; update title + persist.
  $effect(() => {
    const p = selectedPath;
    persistLastOpen(p);
    void setWindowTitle(p ? getDisplayTitle(p) : null);
  });

  let unlistenEmbedStatus: UnlistenFn | null = null;

  onMount(async () => {
    unlisten = await listen("notes_changed", async () => {
      await Promise.all([performSearch(query), refreshAllNotes(), refreshTagMeta(), refreshPinned()]);
    });
    unlistenEmbedStatus = await listen<string>("embedding_status", (e) => {
      const p = e.payload;
      if (p === "loading" || p === "ready" || p === "error") embedStatus = p;
    });
    window.addEventListener("keydown", handleGlobalKey, true);
    window.addEventListener("blur", handleWindowBlur);
    window.addEventListener("keydown", handleTipsKey);
    // Settings → Security tab broadcasts when the toggle changes so we
    // sync the in-memory mirror without re-querying.
    window.addEventListener("malt:security-changed", ((e: Event) => {
      const detail = (e as CustomEvent<{ reprompt_on_blur: boolean }>).detail;
      if (detail) securityRepromptOnBlur = detail.reprompt_on_blur;
    }) as EventListener);
    window.addEventListener("malt:daily-note-tag-changed", ((e: Event) => {
      const detail = (e as CustomEvent<{ enabled: boolean }>).detail;
      if (detail) dailyNoteTag = detail.enabled;
    }) as EventListener);
    // Live-apply tag flair edits from Settings without a re-query.
    window.addEventListener("malt:tag-styles-changed", ((e: Event) => {
      const detail = (e as CustomEvent<{ styles: TagStyle[] }>).detail;
      if (detail) tagStyles = detail.styles;
    }) as EventListener);
    // Splash + tip prep. Record the mount time so we can enforce a
    // minimum 1 second on-screen duration even on fast boots — without
    // it, instant boots flash the splash for a frame or two before the
    // app pops in, which reads as a glitch.
    bootMountedAt = Date.now();
    tipSkipOnStartup = shouldSkipOnStartup();
    if (!tipSkipOnStartup) {
      tipCurrent = pickNextTip(null);
      if (tipCurrent) markSeen(tipCurrent.id);
      tipsOpen = true;
    }
    await Promise.all([
      refreshAllNotes(),
      refreshSavedSearches(),
      refreshTagMeta(),
      refreshApiKeyStatus(),
      refreshSecurityConfig(),
      refreshGeneralConfig(),
      refreshVaults(),
      refreshPinned(),
    ]);
    // After list loads, try to restore the last-open note. The auto-select
    // effect would otherwise jump to whatever's first.
    const last = readLastOpen();
    if (last && allNotes.some((n) => n.path === last)) {
      selectedPath = last;
      void scrollSelectedIntoView("nearest");
    }
    await tick();
    searchInput?.focus();
    // If the user opted out of tips, drop the splash after the 1 second
    // minimum + 320ms fade-out animation. Otherwise the splash stays up
    // until the user dismisses (any key tap or arrow nav, handled in
    // handleTipsKey).
    if (tipSkipOnStartup) {
      const elapsed = Date.now() - bootMountedAt;
      const remaining = Math.max(0, 1320 - elapsed);
      setTimeout(() => { booting = false; }, remaining);
    }
    // Silent background update check 5s after boot. Errors are swallowed
    // (offline, GitHub down, no published release yet → all fine to ignore).
    setTimeout(() => { void checkForUpdates({ silent: true }); }, 5000);
  });

  onDestroy(() => {
    unlisten?.();
    unlistenEmbedStatus?.();
    window.removeEventListener("keydown", handleGlobalKey, true);
    window.removeEventListener("keydown", handleTipsKey);
    window.removeEventListener("blur", handleWindowBlur);
  });
</script>

<main class:zen={zenMode}>
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
      title={`Settings (${(typeof navigator !== "undefined" && /Mac/i.test(navigator.platform)) ? "⌘" : "Ctrl+"},)`}
      tabindex="-1"
    >
      ⚙
    </button>
  </header>
  {#if savedSearches.some((s) => s.slot != null)}
    <div class="saved-row">
      {#each savedSearches.filter((s) => s.slot != null) as s (s.id)}
        <button
          class="saved-chip"
          class:active={query === s.query}
          class:dragging={savedDragId === s.id}
          class:drag-over={savedDragOverId === s.id && savedDragId !== s.id}
          onclick={() => activateSavedSearch(s)}
          oncontextmenu={(e) => openSavedSearchMenu(e, s)}
          ondblclick={(e) => openSavedSearchMenu(e, s)}
          draggable={true}
          ondragstart={(e) => savedDragStart(e, s.id)}
          ondragover={(e) => savedDragOver(e, s.id)}
          ondragleave={() => savedDragLeave(s.id)}
          ondrop={(e) => void savedDrop(e, s.id)}
          ondragend={savedDragEnd}
          title={`${s.query}${s.slot ? ` — Ctrl+${s.slot} · double- or right-click to edit · drag to reorder` : ""}`}
          tabindex="-1"
        >
          <span class="saved-name">{s.name}</span>
          {#if s.slot}<span class="saved-slot">{s.slot}</span>{/if}
        </button>
      {/each}
    </div>
  {/if}
  {#if singleTagFilter && coTags.length > 0}
    <div class="cotag-row">
      <span class="cotag-label">often with</span>
      {#each coTags as ct (ct.name)}
        <button
          class="cotag-chip"
          onclick={() => addCoTag(ct.name)}
          title={`Also require #${ct.name} (${ct.count} note${ct.count === 1 ? "" : "s"} share both)`}
          tabindex="-1"
        >#{ct.name}<span class="cotag-count">{ct.count}</span></button>
      {/each}
    </div>
  {/if}
  <div class="status">
    <span class="count">
      <button
        class="vault-chip"
        onclick={() => (vaultMenuOpen = !vaultMenuOpen)}
        title={`Vault: ${activeVaultName} — click to switch · ${(typeof navigator !== "undefined" && /Mac/i.test(navigator.platform)) ? "⌘" : "Ctrl+"}⇧V opens picker`}
        tabindex="-1"
      >{activeVaultName}</button>
      <span class="sep">·</span>
      {#if semanticMode}
        <span class="semantic-badge" title="Semantic search — ranked by meaning, not keywords">~ semantic</span>
        <span class="sep">·</span>
        {notes.length} near
      {:else if specialReport === "orphan"}
        <span class="semantic-badge" title="The Orphanage — notes with no links in or out">orphanage</span>
        <span class="sep">·</span>
        {notes.length} adrift
      {:else if specialReport === "onthisday"}
        <span class="semantic-badge" title="On This Day — notes from this calendar day in years past">on this day</span>
        <span class="sep">·</span>
        {notes.length} from the past
      {:else if specialReport === "duplicate"}
        <span class="semantic-badge" title="Near-duplicates — notes with a near-identical twin (~0.9+ similarity)">near-dupes</span>
        <span class="sep">·</span>
        {notes.length} flagged
      {:else if specialReport === "encrypted"}
        <span class="semantic-badge" title="Encrypted notes — password-protected (AES-256-GCM)">🔒 encrypted</span>
        <span class="sep">·</span>
        {notes.length} locked
      {:else if query}
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
      {#if conflictCount > 0}
        <span class="sep">·</span>
        <button
          class="conflict-pill"
          onclick={() => (query = "")}
          title={`${conflictCount} sync-conflict file${conflictCount === 1 ? "" : "s"} in your notes folder — open each to merge manually`}
        >⚠ {conflictCount} conflict{conflictCount === 1 ? "" : "s"}</button>
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
  <div class="body" style:grid-template-columns={zenMode ? "minmax(0, 1fr)" : `${sidebarWidth}px 5px minmax(0, 1fr)`}>
    <ul class="notes">
      {#each notes as note (note.path)}
        <li
          class="note"
          class:selected={note.path === selectedPath}
          class:secondary={note.path === secondaryPath && note.path !== selectedPath}
          class:conflict={note.is_conflict}
          class:empty-note={note.is_empty}
          class:encrypted={note.is_encrypted}
          class:flaired={flairAccent(note) !== ""}
          class:pinned={pinnedSet.has(note.path)}
          style={flairAccent(note) ? `--flair-accent: ${flairAccent(note)}` : ""}
          onclick={(e) => handleNoteClick(e, note.path)}
          ondblclick={(e) => {
            // Double-click pops the full actions menu (Rename / Encrypt /
            // Delete / etc.) at the cursor — same surface as right-click,
            // so trackpad users without a right-click gesture still get
            // every action including encryption.
            e.preventDefault();
            e.stopPropagation();
            handleNoteContextMenu(e, note.path);
          }}
          oncontextmenu={(e) => handleNoteContextMenu(e, note.path)}
        >
          <span class="note-title">
            {#if pinnedSet.has(note.path)}<span class="pin-badge" title="Pinned to top — right-click to unpin">📌</span>{/if}
            {#if note.is_conflict}<span class="conflict-badge" title="Sync conflict — manually merge with the original">⚠</span>{/if}
            {#if note.is_encrypted}<span class="encrypted-badge" title={unlockedPasswords.has(note.path) ? "Unlocked this session" : "Encrypted — click to unlock"}>{unlockedPasswords.has(note.path) ? "🔓" : "🔒"}</span>{/if}
            {#each flairIcons(note) as ic}<span class="flair-icon" style={flairAccent(note) ? `color: ${flairAccent(note)}` : ""}>{ic}</span>{/each}
            {@html highlight(note.display_title ?? note.title, note.title_matches)}
          </span>
          <span class="snippet">{@html highlight(note.snippet, note.snippet_matches)}</span>
          <span class="modified">{formatModified(note.modified)}</span>
        </li>
      {/each}
      {#if notes.length === 0 && vaultSwitching}
        <li class="empty">opening vault — indexing…</li>
      {:else if notes.length === 0 && query && semanticMode}
        <li class="empty">No semantically-similar notes. (Needs the embedding model + at least a couple of indexed notes.)</li>
      {:else if notes.length === 0 && specialReport === "orphan"}
        <li class="empty">No orphans — every note is woven into your web of links.</li>
      {:else if notes.length === 0 && specialReport === "onthisday"}
        <li class="empty">Nothing from this calendar day in years past — yet. Check back as your archive grows.</li>
      {:else if notes.length === 0 && specialReport === "duplicate"}
        <li class="empty">No near-duplicates. (Needs the embedding model + a couple of indexed notes; very similar notes show up here.)</li>
      {:else if notes.length === 0 && specialReport === "encrypted"}
        <li class="empty">No encrypted notes. Right-click any note → Encrypt… to lock one.</li>
      {:else if notes.length === 0 && query}
        <li class="empty">No matches. Press Enter to create "{query}".</li>
      {/if}
      {#if notes.length === 0 && !query && !vaultSwitching}
        <li class="empty empty-cta">
          <div class="empty-cta-title">No notes yet</div>
          <div class="empty-cta-desc">
            Type a title in the search bar and press <kbd>Enter</kbd> to create your first note —
            or click below to start with an Untitled note you can rename later.
          </div>
          <button
            class="empty-cta-btn"
            onclick={() => void startFirstNote()}
          >Start writing →</button>
        </li>
      {/if}
    </ul>
    <div
      class="sidebar-resize-handle"
      onmousedown={startSidebarResize}
      role="separator"
      aria-orientation="vertical"
      title="Drag to resize the note list"
    ></div>
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
                >{getDisplayTitle(selectedPath)}</span>
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
              onSaved={flashSaved}
              onFinderReady={setPrimaryFinder}
              isEncrypted={selectedPath ? !!allNotes.find((n) => n.path === selectedPath)?.is_encrypted : false}
              password={selectedPath ? unlockedPasswords.get(selectedPath) ?? null : null}
              onBrew={openBrewForPrimary}
              getPrePrompt={() => getPrePromptFor("primary")}
              onEncryptedAiBlocked={blockAiForEncrypted}
            />
          </div>
          {#if secondaryPath || brewActive}
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
              {#if !brewActive}
                <div class="pane-title" class:active={focusedPane === "secondary"}>
                  <span class="pane-accent secondary-accent"></span>
                  <span class="pane-title-text" title="Double-click to rename" role="button" tabindex="-1"
                    ondblclick={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      if (secondaryPath) void openRename(secondaryPath);
                    }}
                  >{getDisplayTitle(secondaryPath)}</span>
                  <button
                    class="pane-close"
                    onclick={closeSecondary}
                    title="Close secondary pane (Ctrl+W)"
                    tabindex="-1"
                    aria-label="Close secondary pane"
                  >×</button>
                </div>
              {/if}
              {#if brewActive}
                <BrewPane
                  noteTitle={brewSourceTitle}
                  noteBody={brewSourceBody}
                  brewNonce={brewNonce}
                  onClose={closeSecondary}
                  onAppendToSource={(brew) => appendBrewToSource(brew)}
                  onSaveAsNote={(args) => void saveBrewAsNote(args)}
                />
              {:else}
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
                  onTagPromote={handleEditorTagPromote}
                  onFinderReady={setSecondaryFinder}
                  isEncrypted={secondaryPath ? !!allNotes.find((n) => n.path === secondaryPath)?.is_encrypted : false}
                  password={secondaryPath ? unlockedPasswords.get(secondaryPath) ?? null : null}
                  onBrew={openBrewForSecondary}
                  getPrePrompt={() => getPrePromptFor("secondary")}
                  onEncryptedAiBlocked={blockAiForEncrypted}
                />
              {/if}
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
  <div class="bottom-bar">
    <span
      class="status-dot api-dot"
      class:on={hasApiKey}
      title={hasApiKey ? "Anthropic API key configured — click for settings" : "No Anthropic API key — click to set up"}
      onclick={() => (settingsOpen = true)}
      role="button"
      tabindex="-1"
    ></span>
    {#if embedStatus === "loading"}
      <span class="status-pill" title="Downloading semantic-search model (one-time, ~33MB)">indexing…</span>
    {/if}
    <span class="bottom-bar-spacer"></span>
    {#if justSaved}
      <span class="saved-flash" title="Note autosaved">saved</span>
    {/if}
  </div>
</main>

<Settings
  bind:open={settingsOpen}
  onCheckForUpdates={() => void checkForUpdates({ silent: false })}
  updateStatusLabel={
    updateState.kind === "checking" ? "checking…" :
    updateState.kind === "up_to_date" ? "you're on the latest version" :
    updateState.kind === "no_release" ? "no published release yet — you're on the newest build" :
    updateState.kind === "available" ? `v${updateState.version} available — see toast` :
    updateState.kind === "downloading" ? `downloading… ${updateState.progress}%` :
    updateState.kind === "installing" ? "installing — restart imminent" :
    updateState.kind === "error" ? `error: ${updateState.message}` : ""
  }
  canCheckForUpdates={updateState.kind !== "checking" && updateState.kind !== "downloading" && updateState.kind !== "installing"}
  savedSearches={savedSearches}
  onActivateSavedSearch={(s) => {
    settingsOpen = false;
    activateSavedSearch(s);
  }}
  onDeleteSavedSearch={(id) => {
    const s = savedSearches.find((x) => x.id === id);
    if (s) deleteSavedSearchWithConfirm(id, s.name);
  }}
  onRenameSavedSearch={(id, currentName) => startRenameSavedSearch(id, currentName)}
  onReorderSavedSearch={(id, currentSlot) => startReorderSavedSearch(id, currentSlot)}
  onSavedSearchesUpdated={(list) => (savedSearches = list)}
  onLaunchTips={() => {
    settingsOpen = false;
    launchTipsFromSettings();
  }}
  vaultsState={vaultsState}
  onSwitchVault={(i) => {
    settingsOpen = false;
    void switchVault(i);
  }}
  onAddVault={() => {
    settingsOpen = false;
    openAddVault();
  }}
  onRenameVault={(i, name) => void renameVaultAt(i, name)}
  onRemoveVault={(i) => void removeVaultAt(i)}
/>

{#if rowMenu}
  <div
    class="pill-menu-backdrop"
    role="presentation"
    onclick={dismissRowMenu}
    oncontextmenu={(e) => { e.preventDefault(); dismissRowMenu(); }}
  ></div>
  <div
    class="row-menu"
    style:left={`${rowMenu.x}px`}
    style:top={`${rowMenu.y}px`}
    role="menu"
  >
    <button class="row-menu-item" onclick={() => openNote(rowMenu!.path)}>Open</button>
    <button class="row-menu-item" onclick={() => openInSecondary(rowMenu!.path)}>Open in second pane</button>
    <button class="row-menu-item" onclick={() => void rowMenuTogglePin(rowMenu!.path)}>
      {pinnedSet.has(rowMenu!.path) ? "Unpin" : "Pin to top"}
    </button>
    <div class="row-menu-sep"></div>
    <button class="row-menu-item" onclick={() => rowMenuRename(rowMenu!.path)}>Rename…</button>
    <button class="row-menu-item" onclick={() => void rowMenuDuplicate(rowMenu!.path)}>Duplicate</button>
    <button class="row-menu-item" onclick={() => void rowMenuReveal(rowMenu!.path)}>Reveal in file manager</button>
    <div class="row-menu-sep"></div>
    {#if allNotes.find((n) => n.path === rowMenu!.path)?.is_encrypted}
      <button class="row-menu-item" onclick={() => rowMenuChangePassword(rowMenu!.path)}>Change password…</button>
      <button class="row-menu-item" onclick={() => rowMenuDecrypt(rowMenu!.path)}>Decrypt (remove password)…</button>
    {:else}
      <button class="row-menu-item" onclick={() => rowMenuEncrypt(rowMenu!.path)}>Encrypt…</button>
    {/if}
    {#if vaultsState.vaults.length > 1}
      <div class="row-menu-sep"></div>
      <div class="row-menu-label">Move to vault</div>
      {#each vaultsState.vaults as v, i (v.path)}
        {#if i !== vaultsState.active_index}
          <button class="row-menu-item" onclick={() => void rowMenuMoveToVault(rowMenu!.path, i)}>→ {v.name}</button>
        {/if}
      {/each}
    {/if}
    <div class="row-menu-sep"></div>
    <button class="row-menu-item danger" onclick={() => rowMenuDelete(rowMenu!.path)}>Delete…</button>
  </div>
{/if}

{#if savedMenu}
  <div
    class="pill-menu-backdrop"
    role="presentation"
    onclick={dismissSavedMenu}
    oncontextmenu={(e) => { e.preventDefault(); dismissSavedMenu(); }}
  ></div>
  <div
    class="row-menu"
    style:left={`${savedMenu.x}px`}
    style:top={`${savedMenu.y}px`}
    role="menu"
  >
    <button class="row-menu-item" onclick={() => {
      const s = savedSearches.find((x) => x.id === savedMenu!.id);
      dismissSavedMenu();
      if (s) activateSavedSearch(s);
    }}>Activate</button>
    <div class="row-menu-sep"></div>
    <button class="row-menu-item" onclick={() => startRenameSavedSearch(savedMenu!.id, savedMenu!.name)}>Rename…</button>
    <button class="row-menu-item" onclick={() => startReorderSavedSearch(savedMenu!.id, savedMenu!.slot)}>Reorder…</button>
    <div class="row-menu-sep"></div>
    <button class="row-menu-item danger" onclick={() => deleteSavedSearchWithConfirm(savedMenu!.id, savedMenu!.name)}>Delete</button>
  </div>
{/if}

{#if renameSearchOpen}
  <div
    class="rename-backdrop"
    role="presentation"
    onclick={cancelRenameSavedSearch}
  >
    <div
      class="rename-panel"
      role="dialog"
      aria-modal="true"
      aria-label="Rename saved search"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="rename-label">rename saved search</div>
      <input
        class="rename-input"
        type="text"
        bind:this={renameSearchInputEl}
        bind:value={renameSearchName}
        onkeydown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); void confirmRenameSavedSearch(); }
          else if (e.key === "Escape") { e.preventDefault(); cancelRenameSavedSearch(); }
        }}
      />
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={cancelRenameSavedSearch}>cancel</button>
        <button
          class="rename-btn confirm"
          onclick={() => void confirmRenameSavedSearch()}
          disabled={!renameSearchName.trim()}
        >rename</button>
      </div>
    </div>
  </div>
{/if}

{#if pwOpen}
  <div
    class="rename-backdrop"
    role="presentation"
    onclick={cancelPasswordModal}
  >
    <div
      class="rename-panel pw-panel"
      role="dialog"
      aria-modal="true"
      aria-label="Note password"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => {
        if (e.key === "Escape") { e.preventDefault(); cancelPasswordModal(); }
      }}
    >
      <div class="rename-label">
        {#if pwKind === "unlock"}🔓 unlock — {pwTitle}
        {:else if pwKind === "encrypt"}🔒 encrypt — {pwTitle}
        {:else if pwKind === "change"}🔁 change password — {pwTitle}
        {:else}🔓 decrypt — {pwTitle}
        {/if}
      </div>
      {#if pwKind === "unlock" || pwKind === "decrypt" || pwKind === "change"}
        <input
          class="rename-input"
          type="password"
          placeholder={pwKind === "change" ? "current password" : "password"}
          bind:this={pwInputEl}
          bind:value={pwCurrent}
          onkeydown={(e) => {
            if (e.key === "Enter" && pwKind !== "change") {
              e.preventDefault();
              void confirmPasswordModal();
            }
          }}
        />
      {/if}
      {#if pwKind === "encrypt" || pwKind === "change"}
        <input
          class="rename-input"
          type="password"
          placeholder={pwKind === "change" ? "new password" : "password"}
          bind:value={pwNew}
          bind:this={pwNewInputEl}
        />
        <input
          class="rename-input"
          type="password"
          placeholder="confirm"
          bind:value={pwConfirm}
          onkeydown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void confirmPasswordModal();
            }
          }}
        />
      {/if}
      {#if pwError}
        <div class="pw-error">{pwError}</div>
      {/if}
      <div class="pw-hint">
        {#if pwKind === "unlock"}Locked notes use AES-256-GCM with an Argon2id-derived key. Each save round-trips through the password.
        {:else if pwKind === "encrypt"}Once set, the note's body is unreadable without this password. There's no recovery — losing it loses the note. {securityRepromptOnBlur ? "" : "(Re-prompt on focus loss is OFF.)"}
        {:else if pwKind === "change"}Re-encrypts the note with a new key. The old password is required to decrypt; both encrypted-on-disk and in-memory cache update.
        {:else}Removes encryption. The note will be written back to disk as plaintext markdown.
        {/if}
      </div>
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={cancelPasswordModal} disabled={pwBusy}>cancel</button>
        <button
          class="rename-btn confirm"
          onclick={() => void confirmPasswordModal()}
          disabled={pwBusy ||
            (pwKind === "unlock" && !pwCurrent) ||
            (pwKind === "decrypt" && !pwCurrent) ||
            (pwKind === "encrypt" && (!pwNew || !pwConfirm)) ||
            (pwKind === "change" && (!pwCurrent || !pwNew || !pwConfirm))}
        >
          {#if pwBusy}working…
          {:else if pwKind === "unlock"}unlock
          {:else if pwKind === "encrypt"}encrypt
          {:else if pwKind === "change"}change
          {:else}decrypt
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if reorderSearchOpen}
  <div
    class="rename-backdrop"
    role="presentation"
    onclick={cancelReorderSavedSearch}
  >
    <div
      class="rename-panel"
      role="dialog"
      aria-modal="true"
      aria-label="Reorder saved search"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="rename-label">move saved search to position</div>
      <input
        class="rename-input"
        type="number"
        min="1"
        max={savedSearches.length}
        bind:this={reorderSearchInputEl}
        bind:value={reorderSearchPosition}
        onkeydown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); void confirmReorderSavedSearch(); }
          else if (e.key === "Escape") { e.preventDefault(); cancelReorderSavedSearch(); }
        }}
      />
      <div class="reorder-hint">
        1 = first on the bar. Positions past 9 stay in Settings only;
        everything else shifts to accommodate.
      </div>
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={cancelReorderSavedSearch}>cancel</button>
        <button class="rename-btn confirm" onclick={() => void confirmReorderSavedSearch()}>move</button>
      </div>
    </div>
  </div>
{/if}

{#if vaultMenuOpen}
  <div class="pill-menu-backdrop" role="presentation" onclick={() => (vaultMenuOpen = false)}></div>
  <div class="vault-menu" role="menu">
    <div class="vault-menu-head">Vaults</div>
    {#each [...vaultsState.vaults].map((v, i) => ({ v, i })).sort((a, b) => a.v.name.localeCompare(b.v.name)) as item (item.v.path)}
      <button
        class="vault-menu-item"
        class:active={item.i === vaultsState.active_index}
        onclick={() => void switchVault(item.i)}
        title={item.v.path}
      >
        <span class="vault-menu-name">{item.v.name}</span>
        {#if item.i === vaultsState.active_index}<span class="vault-menu-current">active</span>{/if}
      </button>
    {/each}
    <div class="row-menu-sep"></div>
    <button class="vault-menu-item" onclick={() => { vaultMenuOpen = false; void openVaultPicker(); }}>
      Open picker… <span class="vault-menu-hint">⇧+(⌘/Ctrl)+V</span>
    </button>
    <button class="vault-menu-item" onclick={openAddVault}>Add vault…</button>
  </div>
{/if}

{#if vaultPickerOpen}
  <div class="rename-backdrop" role="presentation" onclick={closeVaultPicker}>
    <div class="rename-panel vault-picker" role="dialog" aria-modal="true" onclick={(e) => e.stopPropagation()}>
      <div class="rename-label">switch vault</div>
      <input
        class="rename-input"
        type="text"
        placeholder="filter by name or path…"
        bind:this={vaultPickerInputEl}
        bind:value={vaultPickerQuery}
        onkeydown={(e) => {
          if (e.key === "Escape") { e.preventDefault(); closeVaultPicker(); }
          if (e.key === "Enter" && vaultPickerFiltered.length > 0) {
            e.preventDefault();
            const first = vaultPickerFiltered[0];
            const idx = vaultsState.vaults.findIndex((v) => v.path === first.path);
            if (idx >= 0) void switchVault(idx);
          }
        }}
      />
      <ul class="vault-picker-list" aria-label="vaults">
        {#each vaultPickerFiltered as v (v.path)}
          {@const realIdx = vaultsState.vaults.findIndex((x) => x.path === v.path)}
          <li>
            <button
              class="vault-picker-item"
              class:active={realIdx === vaultsState.active_index}
              onclick={() => void switchVault(realIdx)}
            >
              <span class="vault-picker-name">{v.name}</span>
              <span class="vault-picker-path">{v.path}</span>
            </button>
          </li>
        {/each}
        {#if vaultPickerFiltered.length === 0}
          <li class="vault-picker-empty">No vaults match.</li>
        {/if}
      </ul>
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={closeVaultPicker}>close</button>
        <button class="rename-btn confirm" onclick={openAddVault}>add vault…</button>
      </div>
    </div>
  </div>
{/if}

{#if addVaultOpen}
  <div class="rename-backdrop" role="presentation" onclick={() => (addVaultOpen = false)}>
    <div class="rename-panel" role="dialog" aria-modal="true" onclick={(e) => e.stopPropagation()}>
      <div class="rename-label">add vault</div>
      <input
        class="rename-input"
        type="text"
        placeholder="vault name (e.g. Work, Personal, Recipes)"
        bind:value={addVaultName}
        onkeydown={(e) => {
          if (e.key === "Escape") { e.preventDefault(); addVaultOpen = false; }
        }}
      />
      <div class="add-vault-path-row">
        <input
          class="rename-input"
          type="text"
          placeholder="folder path"
          bind:value={addVaultPath}
        />
        <button class="rename-btn" onclick={() => void pickAddVaultPath()}>browse…</button>
      </div>
      <div class="reorder-hint">
        Each vault is a folder of .md files — fully siloed from the others.
        Each has its own index, embeddings, and saved searches… well,
        eventually. For now, saved searches are shared globally.
      </div>
      {#if addVaultError}
        <div class="pw-error">{addVaultError}</div>
      {/if}
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={() => (addVaultOpen = false)}>cancel</button>
        <button
          class="rename-btn confirm"
          onclick={() => void confirmAddVault()}
          disabled={!addVaultPath.trim()}
        >add</button>
      </div>
    </div>
  </div>
{/if}

{#if booting || tipsOpen}
  <div
    class="boot-splash"
    class:tips-open={tipsOpen}
    class:settings-launch={tipsLaunchedFromSettings}
    role="presentation"
    onclick={tipsOpen ? dismissTips : undefined}
  >
    <div class="boot-card" role="presentation">
      <img class="boot-logo-img" src="/malt-icon.png" alt="malt" />
      <div class="boot-label">malt</div>
      <div class="boot-tagline">Distill notes. Brew ideas.</div>

      {#if tipsOpen && tipCurrent}
        <div class="tip-card">
          <div class="tip-category">{tipCurrent.category}</div>
          <div class="tip-headline">{tipCurrent.headline}</div>
          <div class="tip-story">{renderKeysForOS(tipCurrent.story)}</div>
        </div>
        <div class="tip-controls">
          <button
            class="tip-arrow"
            onclick={(e) => { e.stopPropagation(); prevTip(); }}
            disabled={tipHistory.length === 0}
            title="Previous tip (←)"
            aria-label="Previous tip"
          >‹</button>
          <button
            class="tip-arrow"
            onclick={(e) => { e.stopPropagation(); nextTip(); }}
            title="Next tip (→)"
            aria-label="Next tip"
          >›</button>
        </div>
        {#if !tipsLaunchedFromSettings}
          <div class="tip-dismiss-hint">click or tap any key to dismiss</div>
          <label class="tip-skip-label" onclick={(e) => e.stopPropagation()}>
            <input
              type="checkbox"
              checked={tipSkipOnStartup}
              onchange={toggleSkipTipsOnStartup}
            />
            don't show tips on startup
          </label>
        {:else}
          <div class="tip-dismiss-hint">click or press Esc to close</div>
        {/if}
      {/if}
    </div>
  </div>
{/if}

{#if updateState.kind === "available" && !updateToastDismissed && !updateModalOpen}
  <div class="update-toast" role="status">
    <span class="update-toast-text">
      <strong>malt {updateState.version}</strong> is available
    </span>
    <button class="update-toast-btn primary" onclick={openUpdateModal}>Show details</button>
    <button class="update-toast-btn" onclick={dismissUpdateToast} title="Dismiss until next launch">×</button>
  </div>
{/if}

{#if updateState.kind === "downloading" || updateState.kind === "installing"}
  <div class="update-toast" role="status">
    <span class="update-toast-text">
      {#if updateState.kind === "downloading"}
        Downloading update… {updateState.progress}%
      {:else}
        Installing… malt will restart in a moment.
      {/if}
    </span>
  </div>
{/if}

{#if updateModalOpen && updateState.kind === "available"}
  <div
    class="rename-backdrop"
    role="presentation"
    onclick={closeUpdateModal}
  >
    <div
      class="rename-panel update-panel"
      role="dialog"
      aria-modal="true"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="rename-label">update available</div>
      <div class="update-version">malt {updateState.version}</div>
      {#if updateState.notes}
        <div class="update-notes-label">What's new</div>
        <pre class="update-notes">{updateState.notes}</pre>
      {/if}
      <div class="rename-actions">
        <button class="rename-btn cancel" onclick={closeUpdateModal}>later</button>
        <button class="rename-btn confirm" onclick={() => void installPendingUpdate()}>install + restart</button>
      </div>
    </div>
  </div>
{/if}

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
  .vault-chip {
    background: transparent;
    border: 0;
    color: #d6b06a;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 0;
    cursor: pointer;
  }
  .vault-chip:hover {
    text-decoration: underline;
  }
  .semantic-badge {
    color: #97b8d8;
    background: rgba(108, 182, 255, 0.1);
    border: 1px solid rgba(108, 182, 255, 0.25);
    padding: 0 5px;
    border-radius: 3px;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .cotag-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    padding: 4px 10px 6px;
    border-bottom: 1px solid #1c1c1c;
  }
  .cotag-label {
    color: #555;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .cotag-chip {
    background: transparent;
    border: 1px solid #2e2e2e;
    color: #b59b6b;
    font: inherit;
    font-size: 10px;
    padding: 1px 7px;
    border-radius: 10px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .cotag-chip:hover {
    border-color: #d6b06a;
    color: #d6b06a;
  }
  .cotag-count {
    color: #555;
    font-size: 9px;
    font-variant-numeric: tabular-nums;
  }
  .vault-menu {
    position: fixed;
    bottom: 28px;
    left: 12px;
    background: #1a1a1a;
    border: 1px solid #333;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.6);
    z-index: 201;
    min-width: 220px;
    padding: 4px 0;
    font-size: 12px;
  }
  .vault-menu-head {
    color: #555;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    padding: 4px 12px 6px;
  }
  .vault-menu-item {
    display: flex;
    width: 100%;
    background: transparent;
    border: 0;
    color: #cfcfcf;
    font: inherit;
    font-size: 12px;
    padding: 5px 12px;
    cursor: pointer;
    text-align: left;
    align-items: center;
    gap: 8px;
  }
  .vault-menu-item:hover {
    background: #222;
    color: #e0e0e0;
  }
  .vault-menu-item.active .vault-menu-name {
    color: #d6b06a;
  }
  .vault-menu-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .vault-menu-current {
    color: #d6b06a;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }
  .vault-menu-hint {
    margin-left: auto;
    color: #555;
    font-size: 10px;
  }
  .vault-picker {
    min-width: 480px;
  }
  .vault-picker-list {
    list-style: none;
    margin: 8px 0;
    padding: 0;
    max-height: 320px;
    overflow-y: auto;
    border-top: 1px solid #2a2a2a;
    border-bottom: 1px solid #2a2a2a;
  }
  .vault-picker-item {
    display: flex;
    flex-direction: column;
    width: 100%;
    background: transparent;
    border: 0;
    border-bottom: 1px solid #1f1f1f;
    color: #cfcfcf;
    font: inherit;
    padding: 6px 8px;
    cursor: pointer;
    text-align: left;
    gap: 2px;
  }
  .vault-picker-item:hover {
    background: #222;
  }
  .vault-picker-item.active {
    background: rgba(214, 176, 106, 0.06);
  }
  .vault-picker-name {
    color: #e0e0e0;
    font-size: 12px;
  }
  .vault-picker-path {
    color: #777;
    font-size: 10px;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .vault-picker-empty {
    color: #666;
    font-size: 11px;
    padding: 12px 8px;
    font-style: italic;
    list-style: none;
  }
  .add-vault-path-row {
    display: flex;
    gap: 6px;
    margin-top: 6px;
  }
  .add-vault-path-row .rename-input {
    flex: 1;
  }
  .saved-chip.dragging {
    opacity: 0.4;
  }
  .saved-chip.drag-over {
    /* Left edge accent indicates "drop here, this position" — items push right. */
    border-left: 2px solid #d6b06a;
    padding-left: 7px;
  }
  .reorder-hint {
    color: #888;
    font-size: 11px;
    line-height: 1.4;
    margin-top: -4px;
    margin-bottom: 6px;
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
  .status-dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #444;
    cursor: pointer;
    vertical-align: middle;
    margin: 0 2px 1px;
    transition: background 120ms ease;
  }
  .status-dot.api-dot.on {
    background: #6c6;
    box-shadow: 0 0 4px rgba(108, 198, 108, 0.5);
  }
  .status-dot:hover {
    filter: brightness(1.3);
  }
  /* Slim bottom status bar: AI-key dot, indexing pill, saved flash. */
  .bottom-bar {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 10px;
    border-top: 1px solid #2a2a2a;
    background: #161616;
    min-height: 20px;
    font-size: 11px;
    color: #6a6a6a;
  }
  .bottom-bar-spacer {
    flex: 1 1 auto;
  }
  .saved-flash {
    color: #6c6;
    margin-left: 6px;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    animation: saved-fade 1.2s ease-out;
  }
  @keyframes saved-fade {
    0% { opacity: 0; transform: translateY(2px); }
    20% { opacity: 1; transform: translateY(0); }
    80% { opacity: 1; }
    100% { opacity: 0; }
  }
  .pill-menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 199;
  }
  .row-menu {
    position: fixed;
    z-index: 200;
    background: #1c1c1c;
    border: 1px solid #333;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.5);
    padding: 4px;
    min-width: 200px;
    display: flex;
    flex-direction: column;
  }
  .row-menu-item {
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
  .row-menu-item:hover {
    background: #2a2a2a;
    color: #fff;
  }
  .row-menu-item.danger:hover {
    background: rgba(200, 100, 100, 0.12);
    color: #f88;
  }
  .row-menu-sep {
    height: 1px;
    background: #2a2a2a;
    margin: 4px 0;
  }
  .row-menu-label {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: #6a6a6a;
    padding: 2px 12px;
  }
  .boot-splash {
    position: fixed;
    inset: 0;
    background: #1a1a1a;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    z-index: 300;
    pointer-events: auto;
  }
  /* No-tip path: the splash fades out automatically after the 1s minimum
     duration. With tips open, the splash stays put until dismissed. */
  .boot-splash:not(.tips-open) {
    animation: boot-fade 320ms ease forwards;
    animation-delay: 1000ms;
    pointer-events: none;
  }
  .boot-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    max-width: 560px;
    padding: 24px;
  }
  .boot-logo-img {
    /* Use the actual bezeled icon so we don't render a squished
       monospace letter as a logo. width:height auto = native aspect. */
    width: 96px;
    height: 96px;
    display: block;
    image-rendering: -webkit-optimize-contrast;
  }
  .boot-label {
    color: #555;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.3em;
  }
  .boot-tagline {
    color: #888;
    font-size: 12px;
    font-style: italic;
    margin-top: 6px;
    letter-spacing: 0.02em;
  }
  .tip-card {
    background: #222;
    border: 1px solid #333;
    border-left: 3px solid #d6b06a;
    border-radius: 3px;
    padding: 14px 18px;
    margin-top: 20px;
    max-width: 460px;
    text-align: left;
  }
  .tip-category {
    color: #d6b06a;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.15em;
    margin-bottom: 4px;
  }
  .tip-headline {
    color: #d6b06a;
    font-size: 13px;
    font-style: italic;
    margin-bottom: 8px;
    line-height: 1.35;
  }
  .tip-story {
    color: #cfcfcf;
    font-size: 13px;
    line-height: 1.55;
  }
  .tip-controls {
    display: flex;
    gap: 10px;
    margin-top: 12px;
  }
  .tip-arrow {
    background: transparent;
    border: 1px solid #333;
    color: #888;
    font: inherit;
    font-size: 16px;
    line-height: 1;
    padding: 4px 12px;
    cursor: pointer;
    border-radius: 2px;
  }
  .tip-arrow:hover:not(:disabled) {
    color: #e0e0e0;
    border-color: #555;
  }
  .tip-arrow:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
  .tip-dismiss-hint {
    color: #666;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.15em;
    margin-top: 14px;
  }
  .tip-skip-label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-top: 4px;
    color: #888;
    font-size: 11px;
    cursor: pointer;
  }
  .tip-skip-label input {
    margin: 0;
  }
  @keyframes boot-fade {
    from { opacity: 1; }
    to { opacity: 0; visibility: hidden; }
  }
  .update-toast {
    position: fixed;
    bottom: 16px;
    right: 16px;
    z-index: 180;
    background: #1c1c1c;
    border: 1px solid #d6b06a;
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.6);
    padding: 10px 12px;
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12px;
    color: #e0e0e0;
    max-width: 360px;
    animation: toast-slide 240ms ease;
  }
  @keyframes toast-slide {
    from { opacity: 0; transform: translateY(10px); }
    to { opacity: 1; transform: translateY(0); }
  }
  .update-toast-text {
    flex: 1;
  }
  .update-toast-text strong {
    color: #d6b06a;
    font-weight: 600;
  }
  .update-toast-btn {
    background: transparent;
    border: 1px solid #333;
    color: #aaa;
    font: inherit;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 3px 8px;
    cursor: pointer;
    border-radius: 3px;
  }
  .update-toast-btn:hover {
    border-color: #555;
    color: #e0e0e0;
  }
  .update-toast-btn.primary {
    border-color: #d6b06a;
    color: #f1d394;
  }
  .update-toast-btn.primary:hover {
    background: rgba(214, 176, 106, 0.1);
  }
  .update-panel {
    min-width: 460px;
    max-width: 580px;
  }
  .update-version {
    color: #d6b06a;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 18px;
    margin-bottom: 12px;
  }
  .update-notes-label {
    color: #888;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 8px 0 4px;
  }
  .update-notes {
    background: #111;
    border: 1px solid #2a2a2a;
    border-radius: 3px;
    padding: 10px 12px;
    margin: 0 0 16px;
    color: #ccc;
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 280px;
    overflow-y: auto;
    font-family: inherit;
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
  /* Grab strip just right of the list↔editor divider. */
  .sidebar-resize-handle {
    cursor: col-resize;
    background: transparent;
    transition: background 0.12s;
  }
  .sidebar-resize-handle:hover {
    background: rgba(108, 182, 255, 0.35);
  }
  /* Zen mode (Cmd/Ctrl+Shift+F): collapse the note list so the editor
     fills the window. display:none removes them as grid items, leaving
     the editor-pane as the lone column. */
  main.zen .notes,
  main.zen .sidebar-resize-handle {
    display: none;
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
  /* ── Per-tag flair (note cards only; editor left alone) ──────────────
     A styled tag tints its note cards and colors the title + icon with
     an accent (--flair-accent, set inline from config). Selection and
     secondary-pane states still win the left stripe and background;
     flair survives as the colored icon so you never lose the selection
     cue. color-mix degrades gracefully (no tint) on old webviews — the
     colored title carries the signal regardless. */
  .flair-icon {
    font-size: 11px;
    margin-right: 5px;
    opacity: 0.92;
  }
  .note.flaired:not(.selected):not(.secondary) {
    background: color-mix(in srgb, var(--flair-accent) 9%, transparent);
  }
  .note.flaired:not(.selected):not(.secondary):hover {
    background: color-mix(in srgb, var(--flair-accent) 15%, #232323);
  }
  .note.flaired:not(.selected) .note-title {
    color: var(--flair-accent);
  }
  /* Pinned notes: a subtle amber card tint + a 📌 badge. (Placed after the
     flair rules so a pinned+flaired note reads as pinned.) */
  .pin-badge {
    font-size: 10px;
    margin-right: 4px;
    opacity: 0.85;
  }
  .note.pinned:not(.selected):not(.secondary) {
    background: rgba(216, 184, 106, 0.08);
  }
  .note.pinned:not(.selected):not(.secondary):hover {
    background: rgba(216, 184, 106, 0.14);
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
  .empty-cta {
    text-align: center;
    padding: 36px 16px;
  }
  .empty-cta-title {
    color: #e0e0e0;
    font-size: 14px;
    font-weight: 500;
    margin-bottom: 6px;
  }
  .empty-cta-desc {
    color: #888;
    font-size: 12px;
    margin-bottom: 16px;
    line-height: 1.5;
  }
  .empty-cta-desc kbd {
    background: #232323;
    border: 1px solid #333;
    border-radius: 3px;
    padding: 1px 5px;
    font-size: 10px;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    color: #d6b06a;
  }
  .empty-cta-btn {
    background: transparent;
    border: 1px solid #d6b06a;
    color: #d6b06a;
    font: inherit;
    font-size: 12px;
    padding: 6px 16px;
    border-radius: 3px;
    cursor: pointer;
  }
  .empty-cta-btn:hover {
    background: rgba(214, 176, 106, 0.1);
    color: #f1d394;
  }
  .conflict-badge {
    color: #e8a33d;
    margin-right: 4px;
    font-size: 11px;
    cursor: help;
  }
  .note.conflict .note-title {
    color: #e8a33d;
  }
  .note.conflict.selected .note-title {
    color: #ffc879;
  }
  /* Empty notes: dim the title so they recede until you intentionally fill
     them. Selected state still wins to make the focused row readable. */
  .note.empty-note .note-title {
    color: #888;
    font-style: italic;
  }
  .note.empty-note.selected .note-title {
    color: #c8c8c8;
    font-style: italic;
  }
  .note.empty-note .snippet::after {
    content: "empty";
    color: #555;
    font-style: italic;
    font-size: 10px;
  }
  /* Encrypted notes get a small lock glyph and a desaturated snippet so
     they read as opaque-by-design rather than missing data. */
  .encrypted-badge {
    margin-right: 4px;
    font-size: 11px;
    cursor: help;
  }
  .note.encrypted .snippet {
    color: #555;
    font-style: italic;
  }
  .pw-panel .rename-input + .rename-input {
    margin-top: 6px;
  }
  .pw-hint {
    color: #777;
    font-size: 11px;
    line-height: 1.4;
    margin-top: 6px;
  }
  .pw-error {
    color: #c66;
    font-size: 11px;
    margin-top: 6px;
  }
  .conflict-pill {
    background: rgba(232, 163, 61, 0.1);
    border: 1px solid rgba(232, 163, 61, 0.3);
    color: #e8a33d;
    font: inherit;
    font-size: 10px;
    padding: 1px 7px;
    border-radius: 9px;
    cursor: pointer;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .conflict-pill:hover {
    background: rgba(232, 163, 61, 0.18);
    color: #f4b955;
  }
  .status-pill {
    background: rgba(214, 176, 106, 0.08);
    border: 1px solid rgba(214, 176, 106, 0.25);
    color: #d6b06a;
    font-size: 10px;
    padding: 1px 7px;
    border-radius: 9px;
    text-transform: none;
    letter-spacing: 0;
    font-style: italic;
    animation: pulse-soft 1.6s ease-in-out infinite;
  }
  @keyframes pulse-soft {
    0%, 100% { opacity: 0.7; }
    50% { opacity: 1; }
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
