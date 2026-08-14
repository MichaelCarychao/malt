<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { flushAllEditors } from "./editorRegistry";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import {
    shouldSkipOnStartup,
    setSkipOnStartup,
    tipsBankSize,
    seenCount,
    resetSeenTips,
  } from "$lib/tips";

  // Saved searches: passed in from parent so we don't duplicate the list
  // fetching. Each entry has { id, name, query, slot }.
  type SavedSearchRef = { id: string; name: string; query: string; slot: number | null };

  let {
    open = $bindable(),
    onCheckForUpdates,
    updateStatusLabel = "",
    canCheckForUpdates = true,
    savedSearches = [],
    onActivateSavedSearch,
    onDeleteSavedSearch,
    onRenameSavedSearch,
    onReorderSavedSearch,
    onSavedSearchesUpdated,
    onLaunchTips,
    vaultsState = { vaults: [], active_index: 0 },
    onSwitchVault,
    onAddVault,
    onRenameVault,
    onRemoveVault,
  }: {
    open: boolean;
    onCheckForUpdates?: () => void;
    updateStatusLabel?: string;
    canCheckForUpdates?: boolean;
    savedSearches?: SavedSearchRef[];
    onActivateSavedSearch?: (s: SavedSearchRef) => void;
    onDeleteSavedSearch?: (id: string) => void;
    onRenameSavedSearch?: (id: string, currentName: string) => void;
    onReorderSavedSearch?: (id: string, currentSlot: number | null) => void;
    /** Called whenever Settings mutates the saved-searches list via IPC
     * (e.g. drag-and-drop reorder). Parent should set its state from the
     * passed list rather than re-fetching. */
    onSavedSearchesUpdated?: (list: SavedSearchRef[]) => void;
    /** Open the tips browser on demand (parent handles, including
     * closing this settings panel first). */
    onLaunchTips?: () => void;
    /** Parent's vault state, so the Vaults tab can render the active
     * vault badge and offer add/switch/rename/remove without a
     * separate IPC round-trip on every open. Parent owns the
     * authoritative list; we delegate mutations through callbacks. */
    vaultsState?: { vaults: { name: string; path: string }[]; active_index: number };
    onSwitchVault?: (index: number) => void;
    onAddVault?: () => void;
    onRenameVault?: (index: number, name: string) => void;
    onRemoveVault?: (index: number) => void;
  } = $props();

  const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);
  const mod = isMac ? "⌘" : "Ctrl";
  const shift = isMac ? "⇧" : "Shift";

  type Shortcut = { keys: string; action: string; status: "live" | "soon" };

  const maltShortcuts: Shortcut[] = [
    { keys: `${mod}+,`,                action: "Open / close settings",                       status: "live" },
    { keys: `${mod}+L`,                action: "Focus search field",                          status: "live" },
    { keys: `${mod}+N`,                action: "New note (clear & focus search)",             status: "live" },
    { keys: `${mod}+D`,                action: "Daily note — open/create today's dated note",  status: "live" },
    { keys: `${mod}+${shift}+R`,       action: "Random note — jump somewhere in the vault",   status: "live" },
    { keys: `${mod}+${shift}+F`,       action: "Zen mode — hide the note list, editor fills the window", status: "live" },
    { keys: `↑ / ↓`,                   action: "Move selection (in search field)",            status: "live" },
    { keys: `${mod}+↓ / ${mod}+;`,     action: "Next note (from anywhere)",                   status: "live" },
    { keys: `${mod}+↑ / ${mod}+K`,     action: "Previous note (from anywhere)",               status: "live" },
    { keys: `${mod}+[ / ${mod}+]`,     action: "Back / forward in focused pane's history (or primary)", status: "live" },
    { keys: `${mod}+Del / ${mod}+⌫`,   action: "Delete current note (confirmation; sidebar focus only)", status: "live" },
    { keys: `${mod}+R (in editor)`,    action: "Rename current note + rewrite all backlinks",  status: "live" },
    { keys: `Double-click row / title`, action: "Rename note inline (atomic backlink rewrite)", status: "live" },
    { keys: `${mod}+S (search focus)`, action: "Save current query as named search",            status: "live" },
    { keys: `${mod}+1 — ${mod}+9`,     action: "Activate saved search in that slot",           status: "live" },
    { keys: `Type # in editor`,        action: "Hashtag autocomplete (vocabulary + corpus)",   status: "live" },
    { keys: `${mod}+${shift}+L (in editor)`, action: "Suggest [[wikilinks]] for this note (review modal)", status: "live" },
    { keys: `${mod}+${shift}+B (in editor)`, action: "Brew ideas — AI brainstorm in the secondary pane", status: "live" },
    { keys: `${mod}+${shift}+V`,             action: "Open the vault picker (switch between notes folders)", status: "live" },
    { keys: `${mod}+${shift}+E`,             action: "Export current note (.md, .html, .epub, .txt, clipboard)", status: "live" },
    { keys: `${mod}+F`,                      action: "Find within current note (works from anywhere; press again to close)", status: "live" },
    { keys: `${mod}+B (in editor)`,          action: "Toggle **bold** around selection / word at cursor",     status: "live" },
    { keys: `${mod}+I (in editor)`,          action: "Toggle _italic_ around selection / word at cursor",     status: "live" },
    { keys: `tag:foo  -tag:foo`,       action: "Query operator: filter notes by hashtag",       status: "live" },
    { keys: `modified:<7d  <24h  >30d`, action: "Query operator: filter by recency",            status: "live" },
    { keys: `empty:true  empty:false`,  action: "Query operator: find blank notes (or only filled)", status: "live" },
    { keys: `~concept`,                 action: "Semantic search — rank notes by meaning, not keywords", status: "live" },
    { keys: `is:orphan`,                action: "Report: notes with no links in or out (The Orphanage)", status: "live" },
    { keys: `is:onthisday`,             action: "Report: notes from this calendar day in years past",    status: "live" },
    { keys: `is:duplicate`,             action: "Report: notes with a near-identical twin (embeddings)",  status: "live" },
    { keys: `is:encrypted`,             action: "Report: password-locked notes",                          status: "live" },
    { keys: `Enter (in search)`,       action: "Exact title → open; arrowed → open; else create new note", status: "live" },
    { keys: `Esc`,                     action: "Clear query + focus search (except in editor / modals)", status: "live" },
    { keys: `Tab (in search)`,         action: "Jump to editor",                              status: "live" },
    { keys: `Click row`,               action: "Open note in primary pane",                   status: "live" },
    { keys: `${mod}+Click row`,        action: "Open note in second pane (opens split)",      status: "live" },
    { keys: `Click [[wikilink]]`,      action: "Jump to linked note (same pane)",             status: "live" },
    { keys: `${mod}+Click [[wikilink]]`, action: "Open linked note in the OTHER pane",        status: "live" },
    { keys: `${mod}+W (in editor)`,    action: "Close the secondary editor pane",             status: "live" },
    { keys: `Type [[ in editor`,       action: "Wikilink autocomplete dropdown",              status: "live" },
    { keys: `${mod}+; (no selection)`,       action: "AI continue / infill at cursor (uses full doc)",  status: "live" },
    { keys: `${mod}+; (with selection)`,     action: "AI rewrite selection — unpack, avoid clichés",    status: "live" },
    { keys: `${mod}+; (re-press)`,           action: "Re-roll the current ghost suggestion",             status: "live" },
    { keys: `${mod}+${shift}+; (in editor)`, action: "Steer the AI — add a one-line direction note",     status: "live" },
    { keys: `${mod}+${shift}+' (split)`,     action: "Two-pane prompt — other pane is a raw pre-prompt for this one", status: "live" },
    { keys: `${mod}+Enter / Tab / Arrows / Click`, action: "Accept ghost completion",                    status: "live" },
    { keys: `Esc (in editor)`,               action: "Decline completion (or vim normal mode)",          status: "live" },
  ];

  const vimShortcuts: Shortcut[] = [
    { keys: "i / a / o",   action: "Insert before / after / new line below", status: "live" },
    { keys: "Esc",         action: "Back to normal mode",                    status: "live" },
    { keys: "h j k l",     action: "Cursor left / down / up / right",        status: "live" },
    { keys: "w / b",       action: "Word forward / back",                    status: "live" },
    { keys: "0 / $",       action: "Line start / end",                       status: "live" },
    { keys: "gg / G",      action: "Top / bottom of document",               status: "live" },
    { keys: "dd / yy / p", action: "Delete line / yank line / paste",        status: "live" },
    { keys: "u / Ctrl-r",  action: "Undo / redo",                            status: "live" },
    { keys: "/text",       action: "Search forward",                         status: "live" },
    { keys: ":w",          action: "Save (also autosaved)",                  status: "live" },
  ];

  let vimMode = $state(false);
  // Tips state mirrors localStorage; refresh whenever the panel opens so
  // toggling on the splash and re-opening Settings shows the new state.
  let tipsSkip = $state(false);
  let tipsSeen = $state(0);
  let tipsTotal = $state(0);
  function refreshTipsState() {
    tipsSkip = shouldSkipOnStartup();
    tipsSeen = seenCount();
    tipsTotal = tipsBankSize();
  }
  function toggleTipsSkip(e: Event) {
    const target = e.target as HTMLInputElement;
    setSkipOnStartup(target.checked);
    tipsSkip = target.checked;
  }
  function resetTipsHistory() {
    resetSeenTips();
    tipsSeen = 0;
  }
  let notesDirPath = $state("");
  let notesDirDirty = $state(false);
  let notesDirError = $state<string | null>(null);
  let tagVocabularyText = $state("");
  let tagVocabularyLoaded = $state(false);
  let tagVocabularyError = $state<string | null>(null);
  let tagVocabularySaved = $state(false);

  // ── Tag flair (per-tag icon + accent color on note cards) ──────────
  type TagStyle = { tag: string; icon: string; color: string };
  let tagStyles = $state<TagStyle[]>([]);
  let tagStylesLoaded = $state(false);
  let tagStylesError = $state<string | null>(null);
  let tagStylesSaved = $state(false);
  // Curated palettes. Icons are single glyphs/emoji (free text also
  // allowed); colors are muted tones that read on the dark theme.
  const FLAIR_ICONS = [
    "◆", "●", "▲", "■", "★", "✦", "❡", "✎", "⚑", "♦",
    "◐", "☾", "♻", "✓", "✗", "◷", "⬡", "❖",
  ];
  const FLAIR_COLORS = [
    "#d8b86a", "#7fb0d8", "#8fc99a", "#d88a8a", "#b89ad8",
    "#7fd8c9", "#d8a070", "#9aa0a8",
  ];
  let appVersion = $state("");
  type SettingsTab =
    | "general"
    | "shortcuts"
    | "vaults"
    | "searches"
    | "tags"
    | "ai"
    | "prompts"
    | "security"
    | "tips"
    | "about";
  function readInitialTab(): SettingsTab {
    if (typeof localStorage !== "undefined") {
      const t = localStorage.getItem("malt.settings.tab");
      if (
        t === "general" ||
        t === "shortcuts" ||
        t === "vaults" ||
        t === "searches" ||
        t === "tags" ||
        t === "ai" ||
        t === "prompts" ||
        t === "security" ||
        t === "tips" ||
        t === "about"
      ) {
        return t;
      }
    }
    return "general";
  }
  let activeTab = $state<SettingsTab>(readInitialTab());
  $effect(() => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.settings.tab", activeTab);
    }
  });

  // Drag-and-drop reorder inside the Settings saved-search list. Mirrors
  // the chip-bar version in +page.svelte; both call onReorderSavedSearch
  // through the parent (which knows the IPC + slot semantics).
  let settingsDragId = $state<string | null>(null);
  let settingsDragOverId = $state<string | null>(null);
  function settingsDragStart(e: DragEvent, id: string) {
    settingsDragId = id;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", id);
    }
  }
  function settingsDragOver(e: DragEvent, overId: string) {
    if (!settingsDragId || settingsDragId === overId) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    settingsDragOverId = overId;
  }
  function settingsDragLeave(overId: string) {
    if (settingsDragOverId === overId) settingsDragOverId = null;
  }
  async function settingsDrop(e: DragEvent, overId: string) {
    e.preventDefault();
    const sourceId = settingsDragId;
    settingsDragId = null;
    settingsDragOverId = null;
    if (!sourceId || sourceId === overId) return;
    const targetIdx = savedSearches.findIndex((x) => x.id === overId);
    if (targetIdx < 0) return;
    // Use invoke directly here — Settings already has access to the @tauri-apps/api
    // import for the rest of the panel and we want immediate feedback without
    // a round-trip through the parent component for drag-drop.
    try {
      const updated = await invoke<SavedSearchRef[]>("reorder_saved_search", {
        id: sourceId,
        targetPosition: targetIdx + 1,
      });
      onSavedSearchesUpdated?.(updated);
    } catch (err) {
      console.error("settings reorder saved search failed", err);
    }
  }
  function settingsDragEnd() {
    settingsDragId = null;
    settingsDragOverId = null;
  }

  // Pull the live version from the backend at mount — single source of truth
  // is Cargo.toml (env! macro reads CARGO_PKG_VERSION at compile time).
  $effect(() => {
    if (open && !appVersion) {
      invoke<string>("app_version")
        .then((v) => (appVersion = v))
        .catch(() => (appVersion = "(unknown)"));
    }
  });

  async function openRepo() {
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl("https://github.com/MichaelCarychao/malt");
    } catch {
      /* no-op */
    }
  }
  async function openChangelog() {
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl("https://github.com/MichaelCarychao/malt/blob/main/CHANGELOG.md");
    } catch {
      /* no-op */
    }
  }
  let taggingEnabled = $state(false);
  let dailyTag = $state(true);
  let configLoaded = $state(false);

  // ── Multi-provider AI state ───────────────────────────────────────
  type ProviderId = "anthropic" | "openai" | "deepseek" | "grok" | "gemini" | "lmstudio";
  type ProviderInfo = {
    id: ProviderId;
    label: string;
    default_model: string;
    suggested_models: string[];
    note: string;
    has_key: boolean;
    model: string;
    requires_key: boolean;
    base_url: string | null;
    saved_models: string[];
  };
  let providersList = $state<ProviderInfo[]>([]);
  let activeProviderId = $state<ProviderId>("anthropic");
  let providersLoaded = $state(false);
  let providerKeyInputs = $state<Record<string, string>>({});
  let providerModelInputs = $state<Record<string, string>>({});
  let providerBaseUrlInputs = $state<Record<string, string>>({});
  let providerTestResults = $state<Record<string, string>>({});
  let providerTestErrors = $state<Record<string, boolean>>({});
  let providerTesting = $state<Record<string, boolean>>({});
  let lmstudioNoThink = $state(false);

  async function loadProviders() {
    try {
      const list = await invoke<ProviderInfo[]>("list_providers");
      providersList = list;
      // Mirror model values into editable inputs so the user can
      // type-in a custom model name without immediately blowing away
      // the default in the suggested-pick row.
      const models: Record<string, string> = {};
      for (const p of list) models[p.id] = p.model;
      providerModelInputs = models;
      // Same mirror for the endpoint override (LM Studio's Tailscale /
      // LAN URL) — the backend reports the effective URL either way.
      const urls: Record<string, string> = {};
      for (const p of list) if (p.base_url !== null) urls[p.id] = p.base_url;
      providerBaseUrlInputs = urls;
      // Discover active provider + LM Studio prefs from config.
      const cfg = await invoke<{ active_provider: ProviderId; lmstudio_no_think: boolean }>("get_config");
      activeProviderId = cfg.active_provider;
      lmstudioNoThink = cfg.lmstudio_no_think;
    } catch (e) {
      console.error("loadProviders failed", e);
    } finally {
      providersLoaded = true;
    }
  }

  async function setActiveProvider(id: ProviderId) {
    try {
      await invoke("set_active_provider", { provider: id });
      activeProviderId = id;
    } catch (e) {
      console.error("set_active_provider failed", e);
    }
  }

  async function saveProviderKey(id: ProviderId) {
    const key = providerKeyInputs[id]?.trim();
    if (!key) return;
    try {
      await invoke("set_api_key_for", { provider: id, key });
      providerKeyInputs = { ...providerKeyInputs, [id]: "" };
      await loadProviders();
    } catch (e) {
      providerTestResults = { ...providerTestResults, [id]: String(e) };
      providerTestErrors = { ...providerTestErrors, [id]: true };
    }
  }
  async function clearProviderKey(id: ProviderId) {
    try {
      await invoke("clear_api_key_for", { provider: id });
      providerTestResults = { ...providerTestResults, [id]: "cleared" };
      providerTestErrors = { ...providerTestErrors, [id]: false };
      await loadProviders();
    } catch (e) {
      providerTestResults = { ...providerTestResults, [id]: String(e) };
      providerTestErrors = { ...providerTestErrors, [id]: true };
    }
  }
  async function testProviderKey(id: ProviderId) {
    providerTesting = { ...providerTesting, [id]: true };
    providerTestResults = { ...providerTestResults, [id]: "" };
    try {
      const reply = await invoke<string>("test_api_key_for", { provider: id });
      providerTestResults = { ...providerTestResults, [id]: `ok — replied: "${reply.trim()}"` };
      providerTestErrors = { ...providerTestErrors, [id]: false };
    } catch (e) {
      providerTestResults = { ...providerTestResults, [id]: String(e) };
      providerTestErrors = { ...providerTestErrors, [id]: true };
    } finally {
      providerTesting = { ...providerTesting, [id]: false };
    }
  }
  async function saveProviderModel(id: ProviderId) {
    const model = providerModelInputs[id]?.trim();
    if (!model) return;
    try {
      await invoke("set_provider_model", { provider: id, model });
      await loadProviders();
    } catch (e) {
      console.error("set_provider_model failed", e);
    }
  }
  async function saveProviderBaseUrl(id: ProviderId) {
    // Empty string is a valid submit — it clears the override and the
    // backend falls back to the provider default (localhost for LM Studio).
    const baseUrl = providerBaseUrlInputs[id]?.trim() ?? "";
    try {
      await invoke("set_provider_base_url", { provider: id, baseUrl });
      await loadProviders();
    } catch (e) {
      providerTestResults = { ...providerTestResults, [id]: String(e) };
      providerTestErrors = { ...providerTestErrors, [id]: true };
    }
  }
  async function toggleLmstudioNoThink() {
    const next = !lmstudioNoThink;
    lmstudioNoThink = next;
    try {
      await invoke("set_lmstudio_no_think", { enabled: next });
    } catch (e) {
      lmstudioNoThink = !next; // roll back on failure
      console.error("set_lmstudio_no_think failed", e);
    }
  }
  function pickSuggestedModel(id: ProviderId, model: string) {
    providerModelInputs = { ...providerModelInputs, [id]: model };
    void saveProviderModel(id);
  }
  /** Saved models minus the built-in suggestions (those already have
   * chips of their own — no duplicates). */
  function ownModels(p: ProviderInfo): string[] {
    return p.saved_models.filter((m) => !p.suggested_models.includes(m));
  }
  async function removeSavedModel(id: ProviderId, model: string) {
    try {
      await invoke("remove_saved_model", { provider: id, model });
      await loadProviders();
    } catch (e) {
      console.error("remove_saved_model failed", e);
    }
  }

  // Security tab state — same lazy-load pattern as the rest.
  let repromptOnBlur = $state(true);
  let securityLoaded = $state(false);

  // Always-show-markdown toggle (Editor reveals **/_ markers even when
  // the cursor isn't on the styled span). Stored in localStorage so the
  // Editor can read it synchronously without an IPC round-trip.
  let alwaysShowMarkdown = $state(
    typeof localStorage !== "undefined" &&
      localStorage.getItem("malt.alwaysShowMarkdown") === "1",
  );
  function toggleAlwaysShowMarkdown(e: Event) {
    const target = e.target as HTMLInputElement;
    alwaysShowMarkdown = target.checked;
    if (typeof localStorage !== "undefined") {
      if (alwaysShowMarkdown) localStorage.setItem("malt.alwaysShowMarkdown", "1");
      else localStorage.removeItem("malt.alwaysShowMarkdown");
    }
    // Tell every open editor to recompute its bold/italic decorations.
    if (typeof window !== "undefined") {
      window.dispatchEvent(new CustomEvent("malt:markdown-toggle-changed"));
    }
  }

  // Prompts tab state. The PromptInfo shape comes from the backend
  // verbatim; we keep an editable mirror per prompt so the textarea
  // tracks user edits before they hit "save".
  type PromptKey =
    | "tag"
    | "entities"
    | "completion"
    | "rewrite"
    | "brew"
    | "implement";
  type PromptInfo = {
    key: PromptKey;
    label: string;
    description: string;
    default: string;
    current: string;
    is_overridden: boolean;
  };
  let promptsLoaded = $state(false);
  let promptsList = $state<PromptInfo[]>([]);
  let promptDrafts = $state<Record<string, string>>({});
  let promptSaveStatus = $state<Record<string, "saved" | "error" | null>>({});

  async function loadPrompts() {
    try {
      const list = await invoke<PromptInfo[]>("list_prompts");
      promptsList = list;
      const drafts: Record<string, string> = {};
      for (const p of list) drafts[p.key] = p.current;
      promptDrafts = drafts;
    } catch {
      promptsList = [];
    } finally {
      promptsLoaded = true;
    }
  }

  async function savePrompt(key: PromptKey) {
    const content = promptDrafts[key] ?? "";
    try {
      await invoke("set_prompt", { key, content });
      promptSaveStatus = { ...promptSaveStatus, [key]: "saved" };
      // Re-load to refresh is_overridden flag + current text.
      await loadPrompts();
      // Clear the "saved" toast after 2s.
      setTimeout(() => {
        promptSaveStatus = { ...promptSaveStatus, [key]: null };
      }, 2000);
    } catch (e) {
      console.error("save_prompt failed", e);
      promptSaveStatus = { ...promptSaveStatus, [key]: "error" };
    }
  }

  async function resetPrompt(key: PromptKey) {
    try {
      await invoke("reset_prompt", { key });
      await loadPrompts();
      promptSaveStatus = { ...promptSaveStatus, [key]: "saved" };
      setTimeout(() => {
        promptSaveStatus = { ...promptSaveStatus, [key]: null };
      }, 2000);
    } catch (e) {
      console.error("reset_prompt failed", e);
      promptSaveStatus = { ...promptSaveStatus, [key]: "error" };
    }
  }

  onMount(() => {
    vimMode = localStorage.getItem("malt.vim") === "1";
  });

  $effect(() => {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("malt.vim", vimMode ? "1" : "0");
    }
    if (typeof window !== "undefined") {
      window.dispatchEvent(new CustomEvent("malt:vim-changed", { detail: vimMode }));
    }
  });

  $effect(() => {
    if (open && !notesDirPath) {
      invoke<string>("get_notes_dir")
        .then((p) => (notesDirPath = p))
        .catch(() => (notesDirPath = "(unknown)"));
    }
  });

  async function pickNotesDir() {
    notesDirError = null;
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        defaultPath: notesDirPath || undefined,
        title: "Choose notes folder",
      });
      if (!picked || typeof picked !== "string") return;
      // Repointing the active vault swaps the folder out from under any
      // open editor — flush first, abort if a pending edit can't be saved.
      if ((await flushAllEditors()) > 0) {
        notesDirError = "Couldn't save pending edits — folder change aborted.";
        return;
      }
      const effective = await invoke<string>("set_notes_dir", { path: picked });
      if (effective !== notesDirPath) {
        notesDirPath = effective;
        notesDirDirty = true;
      }
    } catch (e) {
      notesDirError = String(e);
    }
  }

  async function resetNotesDir() {
    notesDirError = null;
    try {
      if ((await flushAllEditors()) > 0) {
        notesDirError = "Couldn't save pending edits — folder change aborted.";
        return;
      }
      const effective = await invoke<string>("set_notes_dir", { path: null });
      if (effective !== notesDirPath) {
        notesDirPath = effective;
        notesDirDirty = true;
      }
    } catch (e) {
      notesDirError = String(e);
    }
  }

  async function revealNotesDir() {
    try {
      await invoke("reveal_notes_dir");
    } catch (e) {
      notesDirError = String(e);
    }
  }

  $effect(() => {
    if (open && !providersLoaded) {
      void loadProviders();
    }
    if (open && !configLoaded) {
      void loadConfig();
    }
    if (open && !securityLoaded) {
      void loadSecurityConfig();
    }
    if (open && !promptsLoaded) {
      void loadPrompts();
    }
    if (open) {
      // Tips state lives in localStorage; cheap to re-read every open
      // so the count reflects any splash interactions since last view.
      refreshTipsState();
    }
  });

  async function loadSecurityConfig() {
    try {
      const cfg = await invoke<{ reprompt_on_blur: boolean }>("get_security_config");
      repromptOnBlur = cfg.reprompt_on_blur;
    } catch {
      repromptOnBlur = true;
    } finally {
      securityLoaded = true;
    }
  }

  async function toggleRepromptOnBlur(e: Event) {
    const target = e.target as HTMLInputElement;
    const enabled = target.checked;
    try {
      await invoke("set_security_reprompt_on_blur", { enabled });
      repromptOnBlur = enabled;
      // Tell the page to sync its in-memory mirror without a full refresh.
      window.dispatchEvent(
        new CustomEvent("malt:security-changed", { detail: { reprompt_on_blur: enabled } }),
      );
    } catch (err) {
      target.checked = !enabled;
      console.error("set_security_reprompt_on_blur failed", err);
    }
  }

  $effect(() => {
    if (open && !tagVocabularyLoaded) {
      void loadTagVocabulary();
    }
    if (open && !tagStylesLoaded) {
      void loadTagStyles();
    }
  });

  async function loadTagStyles() {
    try {
      tagStyles = await invoke<TagStyle[]>("get_tag_styles");
    } catch {
      tagStyles = [];
    } finally {
      tagStylesLoaded = true;
    }
  }

  function addTagStyle() {
    tagStyles = [...tagStyles, { tag: "", icon: FLAIR_ICONS[0], color: FLAIR_COLORS[0] }];
    tagStylesSaved = false;
  }
  function removeTagStyle(i: number) {
    tagStyles = tagStyles.filter((_, idx) => idx !== i);
    tagStylesSaved = false;
    void saveTagStyles();
  }
  function setStyleField(i: number, field: keyof TagStyle, value: string) {
    tagStyles = tagStyles.map((s, idx) => (idx === i ? { ...s, [field]: value } : s));
    tagStylesSaved = false;
  }

  async function saveTagStyles() {
    tagStylesError = null;
    tagStylesSaved = false;
    // Canonicalize tag names client-side too (the backend re-canonicalizes
    // + drops no-ops, but this keeps the visible list tidy on save).
    const styles = tagStyles
      .map((s) => ({
        tag: s.tag.trim().replace(/^#/, "").toLowerCase(),
        icon: s.icon.trim(),
        color: s.color.trim(),
      }));
    try {
      const saved = await invoke<TagStyle[]>("set_tag_styles", { styles });
      tagStyles = saved;
      tagStylesSaved = true;
      // Live-apply on the page without a full reload.
      window.dispatchEvent(
        new CustomEvent("malt:tag-styles-changed", { detail: { styles: saved } }),
      );
    } catch (e) {
      tagStylesError = String(e);
    }
  }

  async function loadTagVocabulary() {
    try {
      const vocab = await invoke<string[]>("get_tag_vocabulary");
      tagVocabularyText = vocab.join(", ");
    } catch {
      tagVocabularyText = "";
    } finally {
      tagVocabularyLoaded = true;
    }
  }

  async function saveTagVocabulary() {
    tagVocabularyError = null;
    tagVocabularySaved = false;
    const vocabulary = tagVocabularyText
      .split(/[\s,]+/)
      .map((t) => t.trim().replace(/^#/, "").toLowerCase())
      .filter((t) => t.length > 0);
    try {
      await invoke("set_tag_vocabulary", { vocabulary });
      tagVocabularyText = vocabulary.join(", ");
      tagVocabularySaved = true;
    } catch (e) {
      tagVocabularyError = String(e);
    }
  }

  async function loadConfig() {
    try {
      const cfg = await invoke<{ tagging_enabled: boolean; daily_note_tag: boolean }>(
        "get_config",
      );
      taggingEnabled = cfg.tagging_enabled;
      dailyTag = cfg.daily_note_tag;
    } catch {
      taggingEnabled = false;
    } finally {
      configLoaded = true;
    }
  }

  async function toggleDailyTag(e: Event) {
    const target = e.target as HTMLInputElement;
    const enabled = target.checked;
    try {
      await invoke("set_daily_note_tag", { enabled });
      dailyTag = enabled;
      window.dispatchEvent(
        new CustomEvent("malt:daily-note-tag-changed", { detail: { enabled } }),
      );
    } catch (err) {
      target.checked = !enabled;
      console.error("set_daily_note_tag failed", err);
    }
  }

  async function toggleTagging(e: Event) {
    const target = e.target as HTMLInputElement;
    const enabled = target.checked;
    try {
      await invoke("set_tagging_enabled", { enabled });
      taggingEnabled = enabled;
    } catch (err) {
      target.checked = !enabled;
      console.error("set_tagging_enabled failed", err);
    }
  }

  function handleKey(e: KeyboardEvent) {
    if (open && e.key === "Escape") {
      e.preventDefault();
      open = false;
    }
  }

  function backdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) open = false;
  }

  onMount(() => {
    window.addEventListener("keydown", handleKey);
  });
  onDestroy(() => {
    window.removeEventListener("keydown", handleKey);
  });
</script>

{#if open}
  <div class="backdrop" onclick={backdropClick} role="presentation">
    <div class="panel" role="dialog" aria-modal="true" aria-label="Settings">
      <header class="panel-header">
        <span>settings</span>
        <button class="close" onclick={() => (open = false)} aria-label="Close settings">×</button>
      </header>

      <div class="panel-body">
        <nav class="panel-tabs">
          <button class="panel-tab" class:active={activeTab === "general"} onclick={() => (activeTab = "general")}>General</button>
          <button class="panel-tab" class:active={activeTab === "shortcuts"} onclick={() => (activeTab = "shortcuts")}>Shortcuts</button>
          <button class="panel-tab" class:active={activeTab === "vaults"} onclick={() => (activeTab = "vaults")}>Vaults</button>
          <button class="panel-tab" class:active={activeTab === "searches"} onclick={() => (activeTab = "searches")}>Saved searches</button>
          <button class="panel-tab" class:active={activeTab === "tags"} onclick={() => (activeTab = "tags")}>Tags &amp; queries</button>
          <button class="panel-tab" class:active={activeTab === "ai"} onclick={() => (activeTab = "ai")}>AI</button>
          <button class="panel-tab" class:active={activeTab === "prompts"} onclick={() => (activeTab = "prompts")}>Prompts</button>
          <button class="panel-tab" class:active={activeTab === "security"} onclick={() => (activeTab = "security")}>Security</button>
          <button class="panel-tab" class:active={activeTab === "tips"} onclick={() => (activeTab = "tips")}>Tips</button>
          <button class="panel-tab" class:active={activeTab === "about"} onclick={() => (activeTab = "about")}>About</button>
        </nav>

        <div class="panel-content">

      {#if activeTab === "general"}
      <section>
        <h3>
          <label>
            <input type="checkbox" bind:checked={vimMode} />
            vim mode
          </label>
        </h3>
        {#if vimMode}
          <p class="hint-text">Enabled. See the Shortcuts tab for the vim keymap.</p>
        {:else}
          <p class="hint-text">Off. Standard editor bindings apply.</p>
        {/if}
      </section>
      <section>
        <h3>markdown display</h3>
        <table>
          <tbody>
            <tr>
              <td class="keys">**/_ markers</td>
              <td class="action">
                <label class="toggle-label">
                  <input
                    type="checkbox"
                    checked={alwaysShowMarkdown}
                    onchange={toggleAlwaysShowMarkdown}
                  />
                  {alwaysShowMarkdown ? "always shown" : "hidden when cursor is away"} —
                  {alwaysShowMarkdown
                    ? "every ** and _ stays visible while you edit."
                    : "renders WYSIWYG; markers reveal when you touch the styled word."}
                </label>
              </td>
              <td class="status"></td>
            </tr>
          </tbody>
        </table>
      </section>
      <section>
        <h3>daily note</h3>
        <table>
          <tbody>
            <tr>
              <td class="keys">{mod}+D</td>
              <td class="action">Opens (or creates) today's note, titled with the date (YYYY-MM-DD).</td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">#journal tag</td>
              <td class="action">
                <label class="toggle-label">
                  <input
                    type="checkbox"
                    checked={dailyTag}
                    onchange={toggleDailyTag}
                  />
                  {dailyTag ? "on" : "off"} — seed a fresh daily note with #journal
                </label>
              </td>
              <td class="status"></td>
            </tr>
          </tbody>
        </table>
      </section>
      <section>
        <h3>notes folder</h3>
        <table>
          <tbody>
            <tr>
              <td class="keys">path</td>
              <td class="action ai-row">
                <span class="mono notes-path">{notesDirPath || "…"}</span>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys"></td>
              <td class="action ai-row">
                <button class="ai-btn" onclick={pickNotesDir}>change…</button>
                <button class="ai-btn" onclick={revealNotesDir} title="Open in file manager">reveal</button>
                <button class="ai-btn" onclick={resetNotesDir} title="Reset to ~/malt/">reset to default</button>
              </td>
              <td class="status"></td>
            </tr>
            {#if notesDirDirty}
              <tr>
                <td class="keys"></td>
                <td class="action test-result">Folder switched — the active vault now points here.</td>
                <td class="status"></td>
              </tr>
            {/if}
            {#if notesDirError}
              <tr>
                <td class="keys"></td>
                <td class="action test-result err">{notesDirError}</td>
                <td class="status"></td>
              </tr>
            {/if}
          </tbody>
        </table>
      </section>
      {/if}

      {#if activeTab === "shortcuts"}
      <section>
        <h3>malt shortcuts</h3>
        <table>
          <tbody>
            {#each maltShortcuts as s (s.action)}
              <tr class={s.status}>
                <td class="keys">{s.keys}</td>
                <td class="action">{s.action}</td>
                <td class="status">{s.status === "soon" ? "soon" : ""}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>

      {#if vimMode}
      <section>
        <h3>vim shortcuts</h3>
        <table>
          <tbody>
            {#each vimShortcuts as s (s.keys)}
              <tr class={s.status}>
                <td class="keys">{s.keys}</td>
                <td class="action">{s.action}</td>
                <td class="status">{s.status === "soon" ? "soon" : ""}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>
      {/if}
      {/if}

      {#if activeTab === "searches"}
      <section>
        <h3>saved searches</h3>
        <p class="hint-text">
          Saved searches are queries you've named. The first nine get keyboard
          shortcuts ({mod}+1 – {mod}+9) and appear on the chip bar above the
          note list. Beyond that you can save as many as you want — extras
          live here in Settings.
        </p>
        <table class="query-ops">
          <tbody>
            <tr>
              <td class="op">{mod}+S</td>
              <td>With focus in the search bar, save the current query. A small prompt asks for a name.</td>
            </tr>
            <tr>
              <td class="op">{mod}+1 – {mod}+9</td>
              <td>Activate the saved search in that slot. Slots track list order — drag to reorder.</td>
            </tr>
            <tr>
              <td class="op">right-click chip</td>
              <td>Activate / Rename / Reorder / Delete menu for any chip on the bar.</td>
            </tr>
            <tr>
              <td class="op">drag chip</td>
              <td>Drop on another chip (or row below) to move into that position; others shift.</td>
            </tr>
          </tbody>
        </table>

        {#if savedSearches.length === 0}
          <p class="hint-text" style="margin-top: 10px;">
            <em>No saved searches yet.</em> Try typing
            <span class="op">tag:draft modified:&lt;7d</span> in the search bar, then press {mod}+S.
          </p>
        {:else}
          <ul class="searches-list" aria-label="saved searches">
            {#each savedSearches as s (s.id)}
              <li
                class="search-item"
                class:dragging={settingsDragId === s.id}
                class:drag-over={settingsDragOverId === s.id && settingsDragId !== s.id}
                draggable={true}
                ondragstart={(e) => settingsDragStart(e, s.id)}
                ondragover={(e) => settingsDragOver(e, s.id)}
                ondragleave={() => settingsDragLeave(s.id)}
                ondrop={(e) => void settingsDrop(e, s.id)}
                ondragend={settingsDragEnd}
              >
                <span class="search-handle" aria-hidden="true">⋮⋮</span>
                <span class="search-slot">
                  {#if s.slot != null}
                    <span class="search-slot-badge">{mod}+{s.slot}</span>
                  {:else}
                    <span class="search-slot-none">—</span>
                  {/if}
                </span>
                <span class="search-meta">
                  <button
                    class="search-name"
                    onclick={() => onActivateSavedSearch?.(s)}
                    title="Activate this search"
                  >{s.name}</button>
                  <span class="search-query">{s.query}</span>
                </span>
                <span class="search-actions">
                  <button
                    class="ai-btn"
                    onclick={() => onRenameSavedSearch?.(s.id, s.name)}
                    title="Rename saved search"
                  >rename</button>
                  <button
                    class="ai-btn"
                    onclick={() => onReorderSavedSearch?.(s.id, s.slot)}
                    title="Move to a specific position"
                  >reorder</button>
                  <button
                    class="ai-btn"
                    onclick={() => onDeleteSavedSearch?.(s.id)}
                    title="Delete saved search"
                  >del</button>
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
      {/if}

      {#if activeTab === "vaults"}
      <section>
        <h3>vaults</h3>
        <p class="hint-text">
          A vault is just a folder of .md files. Each is fully siloed —
          its own index, embeddings, and watcher. Rename freely (the
          name is just malt's label, it doesn't touch the folder).
          Removing a vault here unlinks it from malt; the files on disk
          stay put.
        </p>
        <ul class="vault-list" aria-label="vaults">
          {#each vaultsState.vaults as v, i (v.path)}
            <li class="vault-row" class:active={i === vaultsState.active_index}>
              <span class="vault-row-status">
                {#if i === vaultsState.active_index}
                  <span class="vault-row-active">active</span>
                {:else}
                  <button
                    class="ai-btn"
                    onclick={() => onSwitchVault?.(i)}
                    title="Switch to this vault"
                  >switch</button>
                {/if}
              </span>
              <span class="vault-row-meta">
                <input
                  class="vault-row-name"
                  type="text"
                  value={v.name}
                  onblur={(e) => {
                    const nv = (e.target as HTMLInputElement).value.trim();
                    if (nv && nv !== v.name) onRenameVault?.(i, nv);
                  }}
                  onkeydown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                />
                <span class="vault-row-path" title={v.path}>{v.path}</span>
              </span>
              <span class="vault-row-actions">
                <button
                  class="ai-btn"
                  onclick={() => {
                    if (vaultsState.vaults.length <= 1) return;
                    if (confirm(`Remove "${v.name}" from malt? Files on disk are NOT deleted.`)) {
                      onRemoveVault?.(i);
                    }
                  }}
                  disabled={vaultsState.vaults.length <= 1}
                  title={vaultsState.vaults.length <= 1
                    ? "Can't remove the last vault"
                    : "Unlink this vault from malt (files on disk stay)"}
                >remove</button>
              </span>
            </li>
          {/each}
        </ul>
        <div class="vault-actions">
          <button class="ai-btn" onclick={() => onAddVault?.()}>add vault…</button>
        </div>
      </section>
      {/if}

      {#if activeTab === "ai"}
      <section>
        <h3>ai providers</h3>
        <p class="hint-text">
          malt can drive its AI features (ghost completion, brew,
          rewrite, auto-tag, link suggestions) through any of these.
          Each provider has its own keychain slot — store as many keys
          as you want and pick which one is active. The active provider
          handles every AI call across the app.
        </p>
        {#if !providersLoaded}
          <p class="hint-text">loading…</p>
        {:else}
          {#each providersList as p (p.id)}
            <div class="provider-card" class:active={p.id === activeProviderId}>
              <div class="provider-head">
                <label class="provider-title">
                  <input
                    type="radio"
                    name="active-provider"
                    value={p.id}
                    checked={p.id === activeProviderId}
                    onchange={() => void setActiveProvider(p.id)}
                  />
                  <span class="provider-label">{p.label}</span>
                  {#if p.id === activeProviderId}
                    <span class="provider-badge">active</span>
                  {/if}
                  {#if p.has_key}
                    <span class="provider-key-status">key set</span>
                  {/if}
                </label>
              </div>
              <div class="provider-note">{p.note}</div>
              {#if p.id === "lmstudio"}
                <div class="provider-row">
                  <span class="provider-row-label">endpoint</span>
                  <input
                    class="ai-input"
                    type="text"
                    bind:value={providerBaseUrlInputs[p.id]}
                    placeholder="http://localhost:1234/v1"
                    onblur={() => void saveProviderBaseUrl(p.id)}
                    onkeydown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }}
                  />
                </div>
                <div class="provider-row">
                  <label class="toggle-label">
                    <input
                      type="checkbox"
                      checked={lmstudioNoThink}
                      onchange={() => void toggleLmstudioNoThink()}
                    />
                    skip thinking — ask reasoning models to answer directly
                    (faster; Qwen-style models honor it, others ignore it)
                  </label>
                </div>
              {/if}
              <div class="provider-row">
                <span class="provider-row-label">key</span>
                {#if p.has_key}
                  <span class="badge">set</span>
                  <button
                    class="ai-btn"
                    onclick={() => void testProviderKey(p.id)}
                    disabled={providerTesting[p.id]}
                  >{providerTesting[p.id] ? "testing…" : "test"}</button>
                  <button class="ai-btn" onclick={() => void clearProviderKey(p.id)}>clear</button>
                {:else}
                  <input
                    type="password"
                    class="ai-input"
                    placeholder={p.id === "anthropic" ? "sk-ant-…" : p.id === "openai" ? "sk-…" : p.requires_key ? "key" : "optional"}
                    bind:value={providerKeyInputs[p.id]}
                    onkeydown={(e) => { if (e.key === "Enter") void saveProviderKey(p.id); }}
                  />
                  <button
                    class="ai-btn"
                    onclick={() => void saveProviderKey(p.id)}
                    disabled={!providerKeyInputs[p.id]?.trim()}
                  >save</button>
                  {#if !p.requires_key}
                    <button
                      class="ai-btn"
                      onclick={() => void testProviderKey(p.id)}
                      disabled={providerTesting[p.id]}
                    >{providerTesting[p.id] ? "testing…" : "test"}</button>
                  {/if}
                {/if}
              </div>
              {#if providerTestResults[p.id]}
                <div class="provider-test-result" class:err={providerTestErrors[p.id]}>{providerTestResults[p.id]}</div>
              {/if}
              <div class="provider-row">
                <span class="provider-row-label">model</span>
                <input
                  class="ai-input"
                  type="text"
                  bind:value={providerModelInputs[p.id]}
                  placeholder={p.default_model}
                  onblur={() => void saveProviderModel(p.id)}
                  onkeydown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }}
                />
              </div>
              <div class="provider-row provider-suggestions">
                <span class="provider-row-label"></span>
                {#each p.suggested_models as m (m)}
                  <button
                    class="ai-btn"
                    class:active={providerModelInputs[p.id] === m}
                    onclick={() => pickSuggestedModel(p.id, m)}
                  >{m}</button>
                {/each}
              </div>
              {#if ownModels(p).length > 0}
                <div class="provider-row provider-suggestions">
                  <span class="provider-row-label">yours</span>
                  {#each ownModels(p) as m (m)}
                    <span class="model-chip" class:active={providerModelInputs[p.id] === m}>
                      <button
                        class="model-chip-name"
                        onclick={() => pickSuggestedModel(p.id, m)}
                        title="Switch to this model"
                      >{m}</button>
                      <button
                        class="model-chip-x"
                        onclick={() => void removeSavedModel(p.id, m)}
                        title="Forget this model"
                        aria-label={`Remove ${m} from saved models`}
                      >×</button>
                    </span>
                  {/each}
                </div>
              {/if}
            </div>
          {/each}
        {/if}
      </section>
      <section>
        <h3>auto-tag</h3>
        <table>
          <tbody>
            <tr>
              <td class="keys">background</td>
              <td class="action">
                <label class="toggle-label">
                  <input
                    type="checkbox"
                    checked={taggingEnabled}
                    onchange={toggleTagging}
                    disabled={!configLoaded}
                  />
                  {taggingEnabled ? "on" : "off"} — append inline #hashtags at the bottom of each note (uses the active provider)
                </label>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">storage</td>
              <td class="action">OS keychain — one slot per provider (Windows Credential Manager / macOS Keychain)</td>
              <td class="status"></td>
            </tr>
          </tbody>
        </table>
      </section>
      {/if}

      {#if activeTab === "prompts"}
      <section>
        <h3>prompts</h3>
        <p class="hint-text">
          Every system prompt malt sends to Claude. Edit any of them to
          customize behavior, save to apply, or reset to restore the
          shipped default. Overrides live in
          <span class="op">~/.config/malt/prompts.json</span>.
        </p>
        {#if !promptsLoaded}
          <p class="hint-text">loading…</p>
        {:else}
          {#each promptsList as p (p.key)}
            <div class="prompt-card">
              <div class="prompt-head">
                <span class="prompt-title">
                  {p.label}
                  {#if p.is_overridden}
                    <span class="prompt-badge">customized</span>
                  {/if}
                </span>
                <span class="prompt-actions">
                  {#if promptSaveStatus[p.key] === "saved"}
                    <span class="vocab-status">saved</span>
                  {:else if promptSaveStatus[p.key] === "error"}
                    <span class="vocab-status err">error</span>
                  {/if}
                  <button
                    class="ai-btn"
                    onclick={() => void savePrompt(p.key)}
                    disabled={promptDrafts[p.key] === p.current}
                  >save</button>
                  <button
                    class="ai-btn"
                    onclick={() => void resetPrompt(p.key)}
                    disabled={!p.is_overridden}
                    title="Drop your override; revert to the default malt ships with"
                  >reset to default</button>
                </span>
              </div>
              <div class="prompt-desc">{p.description}</div>
              <textarea
                class="prompt-textarea"
                bind:value={promptDrafts[p.key]}
                spellcheck="false"
                rows="10"
              ></textarea>
            </div>
          {/each}
        {/if}
      </section>
      {/if}

      {#if activeTab === "tags"}
      <section>
        <h3>tags &amp; queries</h3>
        <table>
          <tbody>
            <tr>
              <td class="keys">vocabulary</td>
              <td class="action">
                <textarea
                  class="vocab-input"
                  bind:value={tagVocabularyText}
                  placeholder="draft, fleeting, waiting, archive"
                  rows="2"
                  spellcheck="false"
                ></textarea>
                <div class="vocab-actions">
                  <button class="ai-btn" onclick={() => void saveTagVocabulary()}>save</button>
                  {#if tagVocabularySaved}
                    <span class="vocab-status">saved</span>
                  {/if}
                  {#if tagVocabularyError}
                    <span class="vocab-status err">{tagVocabularyError}</span>
                  {/if}
                </div>
                <div class="vocab-hint">
                  Comma- or space-separated. Surfaced first in #-autocomplete.
                  Bias toward object/status tags (#draft, #meeting), not topics.
                </div>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">query syntax</td>
              <td class="action">
                <table class="query-ops">
                  <tbody>
                    <tr><td class="op">word words…</td><td>Fuzzy + prefix match on title and body (existing nvalt-style search).</td></tr>
                    <tr><td class="op">tag:foo</td><td>Notes tagged <span class="op">#foo</span>. Also matches <span class="op">#foo/bar</span> via slash nesting.</td></tr>
                    <tr><td class="op">-tag:foo</td><td>Exclude notes tagged <span class="op">#foo</span>.</td></tr>
                    <tr><td class="op">modified:&lt;7d</td><td>Modified within the last 7 days.</td></tr>
                    <tr><td class="op">modified:&gt;30d</td><td>Last modified more than 30 days ago.</td></tr>
                    <tr><td class="op">units</td><td><span class="op">h</span> hours, <span class="op">d</span> days, <span class="op">w</span> weeks, <span class="op">m</span> months, <span class="op">y</span> years.</td></tr>
                    <tr><td class="op">compose</td><td>Space-separate to AND: <span class="op">meeting tag:#draft modified:&lt;14d</span></td></tr>
                  </tbody>
                </table>
                <div class="vocab-hint">
                  Save the current query as a named smart-search with <span class="op">{mod}+S</span> · activate one with <span class="op">{mod}+1</span>–<span class="op">{mod}+9</span>.
                </div>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">tag flair</td>
              <td class="action">
                <div class="flair-list">
                  {#each tagStyles as s, i (i)}
                    <div class="flair-row">
                      <input
                        class="flair-tag"
                        placeholder="tag"
                        value={s.tag}
                        oninput={(e) => setStyleField(i, "tag", (e.target as HTMLInputElement).value)}
                        onchange={() => void saveTagStyles()}
                        spellcheck="false"
                      />
                      <div class="flair-icons" role="group" aria-label="icon">
                        {#each FLAIR_ICONS as ic}
                          <button
                            type="button"
                            class="flair-pick"
                            class:on={s.icon === ic}
                            title={`icon ${ic}`}
                            onclick={() => { setStyleField(i, "icon", ic); void saveTagStyles(); }}
                          >{ic}</button>
                        {/each}
                        <input
                          class="flair-icon-input"
                          maxlength="2"
                          placeholder="·"
                          value={s.icon}
                          oninput={(e) => setStyleField(i, "icon", (e.target as HTMLInputElement).value)}
                          onchange={() => void saveTagStyles()}
                          title="Or type/paste any glyph or emoji"
                        />
                      </div>
                      <div class="flair-colors" role="group" aria-label="color">
                        {#each FLAIR_COLORS as c}
                          <button
                            type="button"
                            class="flair-swatch"
                            class:on={s.color.toLowerCase() === c}
                            style={`background:${c}`}
                            title={c}
                            aria-label={`color ${c}`}
                            onclick={() => { setStyleField(i, "color", c); void saveTagStyles(); }}
                          ></button>
                        {/each}
                        <button
                          type="button"
                          class="flair-swatch none"
                          class:on={!s.color}
                          title="no color"
                          aria-label="no color"
                          onclick={() => { setStyleField(i, "color", ""); void saveTagStyles(); }}
                        >∅</button>
                      </div>
                      <span class="flair-preview" style={s.color ? `color:${s.color}` : ""}>
                        {#if s.icon}{s.icon} {/if}{s.tag || "tag"}
                      </span>
                      <button
                        type="button"
                        class="flair-remove"
                        title="remove this flair"
                        aria-label="remove"
                        onclick={() => removeTagStyle(i)}
                      >×</button>
                    </div>
                  {/each}
                </div>
                <div class="vocab-actions">
                  <button class="ai-btn" onclick={addTagStyle}>+ add flair</button>
                  <button class="ai-btn" onclick={() => void saveTagStyles()}>save</button>
                  {#if tagStylesSaved}
                    <span class="vocab-status">saved</span>
                  {/if}
                  {#if tagStylesError}
                    <span class="vocab-status err">{tagStylesError}</span>
                  {/if}
                </div>
                <div class="vocab-hint">
                  Give a tag an icon and accent color — it shows on every note
                  card carrying that tag (the editor is left untouched). Handy
                  for telling content types apart at a glance in a mixed folder
                  (e.g. <span class="op">#element</span>, <span class="op">#pitch</span>,
                  <span class="op">#story</span>). When a note has several styled
                  tags, the first listed wins the card color.
                </div>
              </td>
              <td class="status"></td>
            </tr>
          </tbody>
        </table>
      </section>
      {/if}

      {#if activeTab === "security"}
      <section>
        <h3>per-note encryption</h3>
        <p class="hint-text">
          Encrypt any note with a password. Body is AES-256-GCM with an
          Argon2id-derived key; the file stays a single line of
          <span class="op">MALT-ENC-v1:…</span> so sync apps still treat it
          as plain text. Filename remains visible — only the contents are
          opaque.
        </p>
        <table>
          <tbody>
            <tr>
              <td class="keys">how to encrypt</td>
              <td class="action">Right-click any note in the sidebar → <em>Encrypt…</em></td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">re-prompt on focus loss</td>
              <td class="action">
                <label class="toggle-label">
                  <input
                    type="checkbox"
                    checked={repromptOnBlur}
                    onchange={toggleRepromptOnBlur}
                    disabled={!securityLoaded}
                  />
                  {repromptOnBlur ? "on" : "off"} — drop cached passwords when malt loses focus
                </label>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">recovery</td>
              <td class="action">None. Lose the password, lose the note. Keep a backup somewhere safe.</td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">excluded</td>
              <td class="action">Encrypted notes skip the search index, AI tagger, and embeddings worker. They're findable by filename only.</td>
              <td class="status"></td>
            </tr>
          </tbody>
        </table>
      </section>
      {/if}

      {#if activeTab === "tips"}
      <section>
        <h3>tips</h3>
        <p class="hint-text">
          A rotating bank of short user-story tips, surfaced on the boot
          splash. Each tip is tagged with a Settings category so you can
          see at a glance which area it covers. The deck shuffles
          random-without-repeats until you've seen them all.
        </p>
        <table>
          <tbody>
            <tr>
              <td class="keys">launch</td>
              <td class="action ai-row">
                <button class="ai-btn" onclick={() => onLaunchTips?.()}>browse tips…</button>
                <span class="hint-text" style="margin: 0;">{tipsSeen} of {tipsTotal} seen · arrows navigate · Esc closes</span>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">on startup</td>
              <td class="action">
                <label class="toggle-label">
                  <input
                    type="checkbox"
                    checked={!tipsSkip}
                    onchange={(e) => {
                      const t = e.target as HTMLInputElement;
                      // Inverted: checkbox reads "show on startup" but the
                      // underlying preference is "skip on startup".
                      setSkipOnStartup(!t.checked);
                      tipsSkip = !t.checked;
                    }}
                  />
                  {tipsSkip ? "off" : "on"} — show a tip when malt opens
                </label>
              </td>
              <td class="status"></td>
            </tr>
            {#if tipsSeen > 0}
              <tr>
                <td class="keys">history</td>
                <td class="action ai-row">
                  <button class="ai-btn" onclick={resetTipsHistory}>reset seen list</button>
                  <span class="hint-text" style="margin: 0;">re-shuffle the deck from the top</span>
                </td>
                <td class="status"></td>
              </tr>
            {/if}
          </tbody>
        </table>
      </section>
      {/if}

      {#if activeTab === "about"}
      <section>
        <h3>about</h3>
        <table>
          <tbody>
            <tr>
              <td class="keys">version</td>
              <td class="action ai-row">
                <span class="badge version-badge">{appVersion || "…"}</span>
                <button class="ai-btn" onclick={() => void openChangelog()}>changelog</button>
                <button class="ai-btn" onclick={() => void openRepo()}>repo</button>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">updates</td>
              <td class="action ai-row">
                <button
                  class="ai-btn"
                  onclick={() => onCheckForUpdates?.()}
                  disabled={!canCheckForUpdates}
                >check for updates</button>
                {#if updateStatusLabel}
                  <span class="update-status-label">{updateStatusLabel}</span>
                {/if}
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">made by</td>
              <td class="action">Michael Carychao · plain markdown forever</td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">license</td>
              <td class="action">MIT</td>
              <td class="status"></td>
            </tr>
          </tbody>
        </table>
      </section>
      {/if}

        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    z-index: 100;
    padding: 48px 20px 20px;
  }
  .panel {
    background: #1a1a1a;
    border: 1px solid #333;
    color: #e0e0e0;
    width: min(820px, 100%);
    max-height: calc(100vh - 68px);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
  }
  .panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid #2a2a2a;
    color: #888;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .panel-body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .panel-tabs {
    flex: 0 0 140px;
    display: flex;
    flex-direction: column;
    background: #161616;
    border-right: 1px solid #2a2a2a;
    padding: 8px 0;
  }
  .panel-tab {
    background: transparent;
    border: 0;
    color: #888;
    font: inherit;
    font-size: 12px;
    text-align: left;
    padding: 7px 14px;
    cursor: pointer;
    border-left: 2px solid transparent;
  }
  .panel-tab:hover {
    color: #ccc;
    background: #1c1c1c;
  }
  .panel-tab.active {
    color: #e0e0e0;
    border-left-color: #d6b06a;
    background: #1a1a1a;
  }
  .panel-content {
    flex: 1;
    overflow-y: auto;
    min-width: 0;
  }
  .hint-text {
    color: #888;
    font-size: 12px;
    margin: 0 0 4px;
  }
  .close {
    background: transparent;
    border: 0;
    color: #888;
    font-size: 18px;
    line-height: 1;
    cursor: pointer;
    padding: 0 4px;
  }
  .close:hover {
    color: #e0e0e0;
  }
  section {
    padding: 12px 12px 8px;
    border-bottom: 1px solid #2a2a2a;
  }
  section:last-child {
    border-bottom: 0;
  }
  h3 {
    font-size: 11px;
    font-weight: normal;
    color: #888;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0 0 8px;
  }
  h3 label {
    color: #e0e0e0;
    text-transform: none;
    letter-spacing: 0;
    font-size: 13px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  h3 input {
    margin: 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    padding: 3px 0;
    vertical-align: top;
  }
  td.keys {
    width: 16ch;
    padding-right: 16px;
    color: #aaa;
    white-space: nowrap;
  }
  td.action {
    color: #e0e0e0;
  }
  .notes-path {
    flex: 1;
    min-width: 0;
    color: #aaa;
    word-break: break-all;
    font-size: 12px;
  }
  td.status {
    width: 5ch;
    color: #555;
    font-size: 10px;
    text-align: right;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  tr.soon td.action {
    color: #888;
  }
  .ai-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .ai-input {
    background: #0f0f0f;
    border: 1px solid #333;
    color: #e0e0e0;
    font: inherit;
    font-size: 12px;
    padding: 3px 6px;
    flex: 1;
    min-width: 220px;
    outline: 0;
  }
  .ai-input:focus {
    border-color: #555;
  }
  .ai-btn {
    background: transparent;
    border: 1px solid #333;
    color: #aaa;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 3px 8px;
    cursor: pointer;
  }
  .ai-btn:hover:not(:disabled) {
    border-color: #555;
    color: #e0e0e0;
  }
  .ai-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .ai-btn.active {
    border-color: #6c6;
    color: #e0e0e0;
    background: rgba(108, 198, 108, 0.08);
  }
  .badge {
    color: #6c6;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .version-badge {
    color: #d6b06a;
    background: rgba(214, 176, 106, 0.1);
    border: 1px solid rgba(214, 176, 106, 0.25);
    padding: 1px 8px;
    border-radius: 3px;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
  }
  .update-status-label {
    color: #888;
    font-size: 11px;
    font-style: italic;
  }
  .muted {
    color: #555;
  }
  .test-result {
    color: #6c6;
    font-size: 11px;
  }
  .test-result.err {
    color: #c66;
  }
  .toggle-label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }
  .toggle-label input {
    margin: 0;
  }
  .vocab-input {
    width: 100%;
    box-sizing: border-box;
    background: #0f0f0f;
    border: 1px solid #333;
    color: #e0e0e0;
    font: inherit;
    font-size: 12px;
    padding: 4px 6px;
    outline: 0;
    resize: vertical;
    min-height: 36px;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
  }
  .vocab-input:focus {
    border-color: #555;
  }
  .vocab-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 4px;
  }
  .vocab-status {
    color: #6c6;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .vocab-status.err {
    color: #c66;
  }
  .vocab-hint {
    color: #666;
    font-size: 10px;
    margin-top: 4px;
    line-height: 1.4;
  }
  /* ── Tag flair editor ───────────────────────────────────────────── */
  .flair-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 6px;
  }
  .flair-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 6px;
    border: 1px solid #2a2a2a;
    border-radius: 4px;
    background: #141414;
  }
  .flair-tag {
    width: 110px;
    background: #0f0f0f;
    border: 1px solid #333;
    color: #e0e0e0;
    font: inherit;
    font-size: 12px;
    padding: 3px 6px;
    outline: 0;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
  }
  .flair-tag:focus {
    border-color: #555;
  }
  .flair-icons,
  .flair-colors {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 3px;
    align-items: center;
  }
  .flair-pick {
    width: 20px;
    height: 20px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: #1c1c1c;
    border: 1px solid #2e2e2e;
    color: #b0b0b0;
    font-size: 12px;
    cursor: pointer;
    border-radius: 3px;
    padding: 0;
    line-height: 1;
  }
  .flair-pick:hover {
    border-color: #555;
    color: #e0e0e0;
  }
  .flair-pick.on {
    border-color: #6cb6ff;
    color: #fff;
    background: #233;
  }
  .flair-icon-input {
    width: 24px;
    text-align: center;
    background: #0f0f0f;
    border: 1px solid #333;
    color: #e0e0e0;
    font-size: 12px;
    padding: 2px;
    outline: 0;
  }
  .flair-swatch {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 2px solid transparent;
    cursor: pointer;
    padding: 0;
    box-shadow: 0 0 0 1px #2e2e2e;
  }
  .flair-swatch.on {
    border-color: #e0e0e0;
  }
  .flair-swatch.none {
    background: #1c1c1c;
    color: #777;
    font-size: 11px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    line-height: 1;
  }
  .flair-preview {
    flex: 1 1 auto;
    min-width: 60px;
    font-size: 12px;
    color: #888;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .flair-remove {
    background: transparent;
    border: 0;
    color: #777;
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
    padding: 0 4px;
  }
  .flair-remove:hover {
    color: #c66;
  }
  .query-ops {
    width: 100%;
    margin-bottom: 4px;
  }
  .query-ops td {
    padding: 2px 8px 2px 0;
    vertical-align: top;
    color: #ccc;
    font-size: 11px;
  }
  .query-ops td.op {
    width: 14ch;
    white-space: nowrap;
    color: #97b8d8;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
  }
  .query-ops td .op {
    color: #97b8d8;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    background: rgba(108, 182, 255, 0.06);
    padding: 0 4px;
    border-radius: 3px;
  }
  .searches-list {
    list-style: none;
    margin: 12px 0 0;
    padding: 0;
    border-top: 1px solid #2a2a2a;
  }
  .search-item {
    display: grid;
    grid-template-columns: auto auto 1fr auto;
    align-items: center;
    gap: 10px;
    padding: 7px 4px;
    border-bottom: 1px solid #1f1f1f;
    cursor: grab;
  }
  .search-item:active {
    cursor: grabbing;
  }
  .search-item.dragging {
    opacity: 0.4;
  }
  .search-item.drag-over {
    border-top: 2px solid #d6b06a;
    padding-top: 5px;
  }
  .search-handle {
    color: #444;
    font-size: 12px;
    line-height: 1;
    user-select: none;
    width: 12px;
    text-align: center;
  }
  .search-item:hover .search-handle {
    color: #888;
  }
  .search-slot {
    min-width: 5ch;
    text-align: center;
  }
  .search-slot-badge {
    display: inline-block;
    color: #6cb6ff;
    font-size: 10px;
    background: rgba(108, 182, 255, 0.12);
    border: 1px solid rgba(108, 182, 255, 0.3);
    padding: 1px 6px;
    border-radius: 3px;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-variant-numeric: tabular-nums;
  }
  .search-slot-none {
    color: #444;
    font-size: 10px;
  }
  .search-meta {
    display: flex;
    align-items: baseline;
    gap: 10px;
    flex-wrap: wrap;
    min-width: 0;
  }
  .search-name {
    background: transparent;
    border: 0;
    color: #d6b06a;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    padding: 0;
    text-align: left;
  }
  .search-name:hover {
    text-decoration: underline;
  }
  .search-query {
    color: #888;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .search-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }
  .prompt-card {
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid #2a2a2a;
  }
  .prompt-card:first-of-type {
    border-top: 0;
    margin-top: 4px;
    padding-top: 0;
  }
  .prompt-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }
  .prompt-title {
    flex: 1;
    color: #e0e0e0;
    font-size: 12px;
  }
  .prompt-badge {
    color: #d6b06a;
    background: rgba(214, 176, 106, 0.1);
    border: 1px solid rgba(214, 176, 106, 0.3);
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 6px;
    border-radius: 2px;
    margin-left: 6px;
    vertical-align: middle;
  }
  .prompt-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .prompt-desc {
    color: #888;
    font-size: 11px;
    line-height: 1.4;
    margin-bottom: 6px;
  }
  .prompt-textarea {
    width: 100%;
    box-sizing: border-box;
    background: #0f0f0f;
    border: 1px solid #333;
    color: #cfcfcf;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 11px;
    line-height: 1.5;
    padding: 8px 10px;
    outline: 0;
    resize: vertical;
    min-height: 100px;
  }
  .prompt-textarea:focus {
    border-color: #555;
  }
  .vault-list {
    list-style: none;
    margin: 0;
    padding: 0;
    border-top: 1px solid #2a2a2a;
  }
  .vault-row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 12px;
    padding: 8px 4px;
    border-bottom: 1px solid #1f1f1f;
  }
  .vault-row.active {
    background: rgba(214, 176, 106, 0.05);
  }
  .vault-row-status {
    min-width: 56px;
    text-align: center;
  }
  .vault-row-active {
    color: #d6b06a;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    padding: 2px 6px;
    border: 1px solid rgba(214, 176, 106, 0.3);
    border-radius: 2px;
  }
  .vault-row-meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .vault-row-name {
    background: transparent;
    border: 0;
    border-bottom: 1px dashed transparent;
    color: #e0e0e0;
    font: inherit;
    font-size: 12px;
    padding: 1px 2px;
    outline: 0;
    width: 100%;
  }
  .vault-row-name:hover {
    border-bottom-color: #333;
  }
  .vault-row-name:focus {
    border-bottom-color: #555;
  }
  .vault-row-path {
    color: #666;
    font-size: 10px;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .vault-row-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }
  .vault-actions {
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid #2a2a2a;
  }
  .provider-card {
    margin-top: 10px;
    padding: 10px 12px;
    border: 1px solid #2a2a2a;
    border-radius: 3px;
    background: #161616;
  }
  .provider-card.active {
    border-color: rgba(214, 176, 106, 0.4);
    background: rgba(214, 176, 106, 0.03);
  }
  .provider-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 4px;
  }
  .provider-title {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    flex: 1;
  }
  .provider-title input[type="radio"] {
    margin: 0;
  }
  .provider-label {
    color: #e0e0e0;
    font-size: 12px;
  }
  .provider-badge {
    color: #d6b06a;
    background: rgba(214, 176, 106, 0.1);
    border: 1px solid rgba(214, 176, 106, 0.3);
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 6px;
    border-radius: 2px;
  }
  .provider-key-status {
    color: #6c6;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .provider-note {
    color: #777;
    font-size: 11px;
    margin-bottom: 8px;
  }
  .provider-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 6px;
    flex-wrap: wrap;
  }
  .provider-row-label {
    color: #888;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    width: 6ch;
    flex-shrink: 0;
  }
  .provider-row .ai-input {
    flex: 1;
    min-width: 200px;
  }
  .provider-suggestions {
    gap: 4px;
    flex-wrap: wrap;
  }
  /* Quick-swap chips: every model the user has typed, one click to
     switch, × to forget. */
  .model-chip {
    display: inline-flex;
    align-items: stretch;
    border: 1px solid #333;
    border-radius: 2px;
    overflow: hidden;
  }
  .model-chip.active {
    border-color: #6cb6ff;
  }
  .model-chip-name {
    background: transparent;
    border: none;
    color: #aaa;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 2px 4px 2px 8px;
    cursor: pointer;
  }
  .model-chip.active .model-chip-name {
    color: #6cb6ff;
  }
  .model-chip-name:hover {
    color: #e0e0e0;
  }
  .model-chip-x {
    background: transparent;
    border: none;
    border-left: 1px solid #2a2a2a;
    color: #666;
    font: inherit;
    font-size: 11px;
    padding: 0 6px;
    cursor: pointer;
  }
  .model-chip-x:hover {
    color: #c97a7a;
  }
  .provider-test-result {
    color: #6c6;
    font-size: 11px;
    margin-top: 6px;
    margin-left: calc(6ch + 8px);
  }
  .provider-test-result.err {
    color: #c66;
  }
</style>
