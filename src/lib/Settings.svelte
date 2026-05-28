<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";

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
  }: {
    open: boolean;
    onCheckForUpdates?: () => void;
    updateStatusLabel?: string;
    canCheckForUpdates?: boolean;
    savedSearches?: SavedSearchRef[];
    onActivateSavedSearch?: (s: SavedSearchRef) => void;
    onDeleteSavedSearch?: (id: string) => void;
  } = $props();

  const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);
  const mod = isMac ? "⌘" : "Ctrl";
  const shift = isMac ? "⇧" : "Shift";

  type Shortcut = { keys: string; action: string; status: "live" | "soon" };

  const maltShortcuts: Shortcut[] = [
    { keys: `${mod}+,`,                action: "Open / close settings",                       status: "live" },
    { keys: `${mod}+L`,                action: "Focus search field",                          status: "live" },
    { keys: `${mod}+N`,                action: "New note (clear & focus search)",             status: "live" },
    { keys: `↑ / ↓`,                   action: "Move selection (in search field)",            status: "live" },
    { keys: `${mod}+↓ / ${mod}+J`,     action: "Next note (from anywhere)",                   status: "live" },
    { keys: `${mod}+↑ / ${mod}+K`,     action: "Previous note (from anywhere)",               status: "live" },
    { keys: `${mod}+[ / ${mod}+]`,     action: "Back / forward in focused pane's history (or primary)", status: "live" },
    { keys: `${mod}+Del / ${mod}+⌫`,   action: "Delete current note (confirmation; sidebar focus only)", status: "live" },
    { keys: `${mod}+R (in editor)`,    action: "Rename current note + rewrite all backlinks",  status: "live" },
    { keys: `Double-click row / title`, action: "Rename note inline (atomic backlink rewrite)", status: "live" },
    { keys: `${mod}+S (search focus)`, action: "Save current query as named search",            status: "live" },
    { keys: `${mod}+1 — ${mod}+9`,     action: "Activate saved search in that slot",           status: "live" },
    { keys: `Type # in editor`,        action: "Hashtag autocomplete (vocabulary + corpus)",   status: "live" },
    { keys: `${mod}+${shift}+L (in editor)`, action: "Suggest [[wikilinks]] for this note (review modal)", status: "live" },
    { keys: `${mod}+${shift}+E`,             action: "Export current note (.md, .html, .epub, .txt, clipboard)", status: "live" },
    { keys: `${mod}+F`,                      action: "Find within current note (works from anywhere; press again to close)", status: "live" },
    { keys: `tag:foo  -tag:foo`,       action: "Query operator: filter notes by hashtag",       status: "live" },
    { keys: `modified:<7d  <24h  >30d`, action: "Query operator: filter by recency",            status: "live" },
    { keys: `empty:true  empty:false`,  action: "Query operator: find blank notes (or only filled)", status: "live" },
    { keys: `Enter (in search)`,       action: "Exact title → open; arrowed → open; else create new note", status: "live" },
    { keys: `Esc`,                     action: "Clear query + focus search (except in editor / modals)", status: "live" },
    { keys: `Tab (in search)`,         action: "Jump to editor",                              status: "live" },
    { keys: `Click row`,               action: "Open note in primary pane",                   status: "live" },
    { keys: `${mod}+Click row`,        action: "Open note in second pane (opens split)",      status: "live" },
    { keys: `Click [[wikilink]]`,      action: "Jump to linked note (same pane)",             status: "live" },
    { keys: `${mod}+Click [[wikilink]]`, action: "Open linked note in the OTHER pane",        status: "live" },
    { keys: `${mod}+W (in editor)`,    action: "Close the secondary editor pane",             status: "live" },
    { keys: `Type [[ in editor`,       action: "Wikilink autocomplete dropdown",              status: "live" },
    { keys: `${mod}+I (no selection)`,       action: "AI continue / infill at cursor (uses full doc)",  status: "live" },
    { keys: `${mod}+I (with selection)`,     action: "AI rewrite selection — unpack, avoid clichés",    status: "live" },
    { keys: `${mod}+I (re-press)`,           action: "Re-roll the current ghost suggestion",             status: "live" },
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
  let notesDirPath = $state("");
  let notesDirDirty = $state(false);
  let notesDirError = $state<string | null>(null);
  let tagVocabularyText = $state("");
  let tagVocabularyLoaded = $state(false);
  let tagVocabularyError = $state<string | null>(null);
  let tagVocabularySaved = $state(false);
  let appVersion = $state("");
  type SettingsTab = "general" | "shortcuts" | "searches" | "tags" | "ai" | "about";
  function readInitialTab(): SettingsTab {
    if (typeof localStorage !== "undefined") {
      const t = localStorage.getItem("malt.settings.tab");
      if (t === "general" || t === "shortcuts" || t === "searches" || t === "tags" || t === "ai" || t === "about") {
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
  let hasAiKey = $state(false);
  let aiKeyLoaded = $state(false);
  let apiKeyInput = $state("");
  let testing = $state(false);
  let testResult = $state("");
  let testError = $state(false);
  let taggingEnabled = $state(false);
  let completionModel = $state("claude-haiku-4-5");
  let configLoaded = $state(false);

  const HAIKU = "claude-haiku-4-5";
  const SONNET = "claude-sonnet-4-6";
  const OPUS = "claude-opus-4-7";

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
    if (open && !aiKeyLoaded) {
      void loadHasKey();
    }
    if (open && !configLoaded) {
      void loadConfig();
    }
  });

  $effect(() => {
    if (open && !tagVocabularyLoaded) {
      void loadTagVocabulary();
    }
  });

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
      const cfg = await invoke<{ tagging_enabled: boolean; completion_model: string }>(
        "get_config"
      );
      taggingEnabled = cfg.tagging_enabled;
      completionModel = cfg.completion_model || HAIKU;
    } catch {
      taggingEnabled = false;
      completionModel = HAIKU;
    } finally {
      configLoaded = true;
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

  async function setModel(model: string) {
    const prev = completionModel;
    completionModel = model;
    try {
      await invoke("set_completion_model", { model });
    } catch (e) {
      completionModel = prev;
      console.error("set_completion_model failed", e);
    }
  }

  $effect(() => {
    if (!open) {
      testResult = "";
      apiKeyInput = "";
    }
  });

  async function loadHasKey() {
    try {
      hasAiKey = await invoke<boolean>("has_api_key");
    } catch {
      hasAiKey = false;
    } finally {
      aiKeyLoaded = true;
    }
  }

  async function saveKey() {
    if (!apiKeyInput.trim()) return;
    try {
      await invoke("set_api_key", { key: apiKeyInput });
      apiKeyInput = "";
      hasAiKey = true;
      testResult = "saved to OS keychain";
      testError = false;
    } catch (e) {
      testResult = String(e);
      testError = true;
    }
  }

  async function clearKey() {
    try {
      await invoke("clear_api_key");
      hasAiKey = false;
      testResult = "cleared";
      testError = false;
    } catch (e) {
      testResult = String(e);
      testError = true;
    }
  }

  async function testKey() {
    testing = true;
    testResult = "";
    try {
      const reply = await invoke<string>("test_api_key");
      testResult = `ok — claude replied: "${reply.trim()}"`;
      testError = false;
    } catch (e) {
      testResult = String(e);
      testError = true;
    } finally {
      testing = false;
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
          <button class="panel-tab" class:active={activeTab === "searches"} onclick={() => (activeTab = "searches")}>Saved searches</button>
          <button class="panel-tab" class:active={activeTab === "tags"} onclick={() => (activeTab = "tags")}>Tags &amp; queries</button>
          <button class="panel-tab" class:active={activeTab === "ai"} onclick={() => (activeTab = "ai")}>AI</button>
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
                <td class="action test-result">Restart malt for the new folder to take effect.</td>
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
          Saved searches are queries you've named so you can recall them with one keystroke.
          They live in <span class="op">~/malt/.malt/saved-searches.json</span> and are written
          to plain text — no DB, no cloud.
        </p>
        <table class="query-ops">
          <tbody>
            <tr>
              <td class="op">{mod}+S</td>
              <td>With focus in the search bar, save the current query. A small prompt asks for a name.</td>
            </tr>
            <tr>
              <td class="op">{mod}+1 — {mod}+9</td>
              <td>Activate the saved search bound to that slot. Slots are assigned in the order you save.</td>
            </tr>
            <tr>
              <td class="op">delete</td>
              <td>Remove a saved search from the list below; its slot is freed up for the next save.</td>
            </tr>
          </tbody>
        </table>

        {#if savedSearches.length === 0}
          <p class="hint-text" style="margin-top: 10px;">
            <em>No saved searches yet.</em> Try typing
            <span class="op">tag:draft modified:&lt;7d</span> in the search bar, then press {mod}+S.
          </p>
        {:else}
          <table class="searches-table">
            <thead>
              <tr>
                <th class="keys">slot</th>
                <th class="action">name &amp; query</th>
                <th class="status"></th>
              </tr>
            </thead>
            <tbody>
              {#each savedSearches as s (s.id)}
                <tr>
                  <td class="keys">{s.slot != null ? `${mod}+${s.slot}` : "—"}</td>
                  <td class="action search-row">
                    <button
                      class="search-name"
                      onclick={() => onActivateSavedSearch?.(s)}
                      title="Activate this search"
                    >{s.name}</button>
                    <span class="search-query">{s.query}</span>
                  </td>
                  <td class="status">
                    <button
                      class="ai-btn"
                      onclick={() => onDeleteSavedSearch?.(s.id)}
                      title="Delete saved search"
                    >del</button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>
      {/if}

      {#if activeTab === "ai"}
      <section>
        <h3>ai (claude)</h3>
        <table>
          <tbody>
            <tr>
              <td class="keys">api key</td>
              <td class="action ai-row">
                {#if !aiKeyLoaded}
                  <span class="muted">…</span>
                {:else if hasAiKey}
                  <span class="badge">set</span>
                  <button class="ai-btn" onclick={testKey} disabled={testing}>
                    {testing ? "testing…" : "test"}
                  </button>
                  <button class="ai-btn" onclick={clearKey}>clear</button>
                {:else}
                  <input
                    type="password"
                    class="ai-input"
                    bind:value={apiKeyInput}
                    placeholder="sk-ant-…"
                    onkeydown={(e) => e.key === "Enter" && saveKey()}
                  />
                  <button class="ai-btn" onclick={saveKey} disabled={!apiKeyInput.trim()}>
                    save
                  </button>
                {/if}
              </td>
              <td class="status"></td>
            </tr>
            {#if testResult}
              <tr>
                <td class="keys"></td>
                <td class="action test-result" class:err={testError}>{testResult}</td>
                <td class="status"></td>
              </tr>
            {/if}
            <tr>
              <td class="keys">model</td>
              <td class="action ai-row">
                <button
                  class="ai-btn"
                  class:active={completionModel === HAIKU}
                  onclick={() => setModel(HAIKU)}
                  disabled={!configLoaded}
                  title="claude-haiku-4-5 — $1/M in · 200K ctx · fastest, cheapest"
                >
                  haiku — fast
                </button>
                <button
                  class="ai-btn"
                  class:active={completionModel === SONNET}
                  onclick={() => setModel(SONNET)}
                  disabled={!configLoaded}
                  title="claude-sonnet-4-6 — $3/M in · 1M ctx · better loose-end detection"
                >
                  sonnet — smart
                </button>
                <button
                  class="ai-btn"
                  class:active={completionModel === OPUS}
                  onclick={() => setModel(OPUS)}
                  disabled={!configLoaded}
                  title="claude-opus-4-7 — $5/M in · 1M ctx · most literary attention; slowest"
                >
                  opus — best
                </button>
              </td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">storage</td>
              <td class="action">OS keychain (Windows Credential Manager / macOS Keychain)</td>
              <td class="status"></td>
            </tr>
            <tr>
              <td class="keys">auto-tag</td>
              <td class="action">
                <label class="toggle-label">
                  <input
                    type="checkbox"
                    checked={taggingEnabled}
                    onchange={toggleTagging}
                    disabled={!configLoaded}
                  />
                  {taggingEnabled ? "on" : "off"} — append inline #hashtags at the bottom of each note
                </label>
              </td>
              <td class="status"></td>
            </tr>
          </tbody>
        </table>
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
  .searches-table {
    width: 100%;
    margin-top: 12px;
    border-top: 1px solid #2a2a2a;
  }
  .searches-table th {
    color: #555;
    font-weight: normal;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    text-align: left;
    padding: 8px 0 4px;
    border-bottom: 1px solid #2a2a2a;
  }
  .searches-table th.status {
    text-align: right;
  }
  .searches-table td {
    padding: 5px 0;
    border-bottom: 1px solid #1f1f1f;
    vertical-align: middle;
  }
  .search-row {
    display: flex;
    align-items: baseline;
    gap: 12px;
    flex-wrap: wrap;
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
  }
</style>
