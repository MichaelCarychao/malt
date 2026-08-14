<script lang="ts">
  // Brew pane: an editing cockpit that lives in the secondary pane.
  //
  // The streamed brew response is parsed into discrete suggestion items
  // (the brew prompt mandates one "- " bullet per suggestion), each with
  // an "implement" button that has the AI apply that one instruction to
  // the source note — the parent routes it into the primary editor's
  // inline diff review. Items are editable in place; applied items get
  // checked off. Below the AI's suggestions sits the user's own
  // checklist ("remove passive verbs"), persisted per vault.
  //
  // Sessions are per-note (see brewSessions.ts): the pane displays the
  // session for `sourcePath` and follows it as the parent navigates.

  import { Channel, invoke } from "@tauri-apps/api/core";
  import { onDestroy } from "svelte";
  import { sessionFor, type BrewItemState } from "./brewSessions";
  import { flushAllEditors } from "./editorRegistry";

  let {
    sourcePath = "",
    noteTitle = "",
    noteBody = "",
    brewNonce = 0,
    vaultPath = "",
    onClose,
    onAppendToSource,
    onSaveAsNote,
    onImplement,
    onImplementCancel,
  }: {
    /** Path of the note this pane is currently wed to (the primary
     * editor's note). Switching it swaps the displayed session. */
    sourcePath?: string;
    noteTitle?: string;
    noteBody?: string;
    /** Bumped by the parent each time an EXPLICIT fresh brew is wanted
     * (Cmd+Shift+B on a note with no cached session). Brewing keys on
     * this nonce alone — never on noteBody — so live editor edits don't
     * fire a brew_streaming call per keystroke. */
    brewNonce?: number;
    /** Active vault path — keys the persisted personal checklist. */
    vaultPath?: string;
    onClose?: () => void;
    /** Append the brew to the source note. Resolves to an error string to
     * show the user (e.g. the note is locked), or null on success. */
    onAppendToSource?: (brew: string) => void | Promise<string | null>;
    /** Save the brew as a new note (parent handles create + navigation). */
    onSaveAsNote?: (args: {
      brew: string;
      sourceTitle: string;
      linkBack: boolean;
    }) => void;
    /** Run one suggestion against the source note; resolves with the
     * review outcome ("accepted" checks the item off). */
    onImplement?: (instruction: string) => Promise<"accepted" | "cancelled" | "nochange">;
    /** Abort the in-flight implement / pending review from the pane. */
    onImplementCancel?: () => void;
  } = $props();

  // ── Displayed-session mirrors (reactive) ──────────────────────────
  let output = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let hasRun = $state(false);
  let interrupted = $state(false);
  let itemState = $state<Record<string, BrewItemState>>({});
  let scroller: HTMLDivElement | null = $state(null);
  let activeChannel: Channel<string> | null = null;
  let activeStreamId: number | null = null;
  // Which note the mirrors currently reflect (non-reactive tracker).
  let currentSource = "";

  function cancelActiveStream() {
    if (activeStreamId !== null) {
      void invoke("cancel_ai_stream", { streamId: activeStreamId }).catch(() => {});
      activeStreamId = null;
    }
  }

  // Swap the displayed session when the parent navigates the primary
  // pane. A stream still running for the OLD note is cancelled and the
  // old session marked interrupted — its partial output is kept.
  $effect(() => {
    const p = sourcePath;
    if (p === currentSource) return;
    if (activeChannel) {
      cancelActiveStream();
      if (currentSource) {
        const old = sessionFor(currentSource);
        old.interrupted = true;
      }
      activeChannel = null;
    }
    currentSource = p;
    const s = sessionFor(p);
    output = s.output;
    error = s.error;
    hasRun = s.hasRun;
    interrupted = s.interrupted;
    itemState = { ...s.itemState };
    busy = false;
  });

  // The brewNonce we last streamed for. Starts at -1 (never synced).
  // On the FIRST sync after mount, a note that already has a cached
  // session just displays it — the stale global nonce must not re-run
  // a brew the user only asked to reopen. The parent bumps the nonce
  // only when a fresh run is wanted.
  let lastNonce = -1;
  $effect(() => {
    const nonce = brewNonce;
    if (nonce === lastNonce) return;
    const firstSync = lastNonce === -1;
    lastNonce = nonce;
    if (firstSync && sessionFor(sourcePath || currentSource).hasRun) return;
    // Snapshot the body now so later edits don't mutate the in-flight run.
    const bodyAtTrigger = noteBody;
    void runBrew(bodyAtTrigger);
  });

  async function runBrew(body: string) {
    const p = currentSource || sourcePath;
    // Claim the display for this note (belt-and-braces vs effect order:
    // the source-sync effect must not treat this run as a stale stream).
    currentSource = p;
    const sess = sessionFor(p);
    if (!body.trim()) {
      error = "Nothing to brew — the note is empty.";
      sess.error = error;
      sess.hasRun = true;
      hasRun = true;
      return;
    }
    if (body.startsWith("MALT-ENC-v1:")) {
      error = "This note is encrypted — AI is disabled until it's unlocked.";
      sess.error = error;
      return;
    }
    cancelActiveStream();
    sess.output = "";
    sess.error = null;
    sess.hasRun = true;
    sess.interrupted = false;
    sess.itemState = {}; // re-run resets AI item state; custom items live elsewhere
    output = "";
    error = null;
    hasRun = true;
    interrupted = false;
    itemState = {};
    busy = true;
    const channel = new Channel<string>();
    activeChannel = channel;
    const streamId = Math.floor(Math.random() * 2 ** 48);
    activeStreamId = streamId;
    channel.onmessage = (chunk: string) => {
      if (activeChannel !== channel) return;
      sess.output += chunk;
      if (currentSource === p) {
        output = sess.output;
        if (scroller) scroller.scrollTop = scroller.scrollHeight;
      }
    };
    try {
      await invoke("brew_streaming", { title: noteTitle, body, streamId, onChunk: channel });
    } catch (e) {
      sess.error = String(e);
      if (activeChannel === channel && currentSource === p) {
        error = sess.error;
      }
    } finally {
      if (activeStreamId === streamId) activeStreamId = null;
      if (activeChannel === channel) {
        busy = false;
        activeChannel = null;
      }
    }
  }

  /** Freshest body for re-runs / empty-state brews on a note the pane
   * navigated to (the nonce path gets the live buffer via noteBody). */
  async function fetchCurrentBody(): Promise<string> {
    await flushAllEditors().catch(() => {});
    return await invoke<string>("read_note", { path: currentSource });
  }
  function rerun() {
    void fetchCurrentBody()
      .then((b) => runBrew(b))
      .catch((e) => (error = String(e)));
  }

  // ── Parsing the streamed markdown into sections + items ───────────
  type ParsedItem = { id: string; text: string };
  type ParsedSection = { title: string; intro: string[]; items: ParsedItem[] };
  type Parsed = { preamble: string[]; sections: ParsedSection[] };

  function parseBrew(text: string): Parsed {
    const preamble: string[] = [];
    const sections: ParsedSection[] = [];
    let current: ParsedSection | null = null;
    for (const raw of text.split("\n")) {
      const line = raw.trim();
      if (!line) continue;
      if (line.startsWith("## ")) {
        current = { title: line.slice(3).trim(), intro: [], items: [] };
        sections.push(current);
        continue;
      }
      const itemMatch = line.match(/^(?:[-*]|\d+\.)\s+(.*)$/);
      if (itemMatch && current) {
        current.items.push({
          id: `ai-${sections.length - 1}-${current.items.length}`,
          text: itemMatch[1],
        });
        continue;
      }
      (current ? current.intro : preamble).push(line);
    }
    return { preamble, sections };
  }
  const parsed = $derived(parseBrew(output));

  function aiText(item: ParsedItem): string {
    return itemState[item.id]?.text ?? item.text;
  }
  function persistAi() {
    const sess = sessionFor(currentSource);
    sess.itemState = JSON.parse(JSON.stringify(itemState));
  }

  // ── Personal checklist (persists per vault) ───────────────────────
  type CustomItem = { id: string; text: string; done?: boolean };
  let customItems = $state<CustomItem[]>([]);
  let newItemText = $state("");
  const storageKey = () => `malt.brewCustom.${vaultPath || "default"}`;
  $effect(() => {
    const k = storageKey();
    try {
      const raw = localStorage.getItem(k);
      customItems = raw ? (JSON.parse(raw) as CustomItem[]) : [];
    } catch {
      customItems = [];
    }
  });
  function persistCustom() {
    try {
      localStorage.setItem(storageKey(), JSON.stringify(customItems));
    } catch {
      /* quota/disabled — non-fatal */
    }
  }
  function addCustomItem() {
    const text = newItemText.trim();
    if (!text) return;
    customItems = [...customItems, { id: `custom-${Date.now()}`, text }];
    newItemText = "";
    persistCustom();
  }
  function removeCustomItem(id: string) {
    customItems = customItems.filter((c) => c.id !== id);
    persistCustom();
  }

  // ── Shared item interactions (AI + custom) ────────────────────────
  let implementing = $state<string | null>(null);
  let rowNote = $state<Record<string, string>>({});
  let editingId = $state<string | null>(null);
  let editText = $state("");

  function textFor(id: string, kind: "ai" | "custom", fallback: string): string {
    if (kind === "custom") return customItems.find((c) => c.id === id)?.text ?? fallback;
    return itemState[id]?.text ?? fallback;
  }
  function isDone(id: string, kind: "ai" | "custom"): boolean {
    if (kind === "custom") return !!customItems.find((c) => c.id === id)?.done;
    return !!itemState[id]?.done;
  }
  function toggleDone(id: string, kind: "ai" | "custom") {
    if (kind === "custom") {
      customItems = customItems.map((c) => (c.id === id ? { ...c, done: !c.done } : c));
      persistCustom();
    } else {
      itemState = { ...itemState, [id]: { ...itemState[id], done: !itemState[id]?.done } };
      persistAi();
    }
  }
  function startEdit(id: string, kind: "ai" | "custom", current: string) {
    if (implementing === id) return;
    editingId = id;
    editText = textFor(id, kind, current);
  }
  function commitEdit(kind: "ai" | "custom") {
    const id = editingId;
    if (id === null) return;
    editingId = null;
    const text = editText.trim();
    if (!text) return;
    if (kind === "custom") {
      customItems = customItems.map((c) => (c.id === id ? { ...c, text } : c));
      persistCustom();
    } else {
      itemState = { ...itemState, [id]: { ...itemState[id], text } };
      persistAi();
    }
  }
  async function implementItem(id: string, kind: "ai" | "custom", fallback: string) {
    if (!onImplement || implementing !== null) return;
    const instruction = textFor(id, kind, fallback);
    implementing = id;
    const { [id]: _drop, ...rest } = rowNote;
    rowNote = rest;
    try {
      const outcome = await onImplement(instruction);
      if (outcome === "accepted" && !isDone(id, kind)) {
        toggleDone(id, kind);
      } else if (outcome === "nochange") {
        rowNote = { ...rowNote, [id]: "no changes suggested" };
      }
    } catch (e) {
      rowNote = { ...rowNote, [id]: String(e) };
    } finally {
      implementing = null;
    }
  }

  /** Focus + select a just-mounted inline edit input. */
  function focusOnMount(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  // ── Header actions (unchanged behaviors) ──────────────────────────
  let saveLinked = $state(true);
  let saveFormOpen = $state(false);
  function copyToClipboard() {
    if (!output.trim()) return;
    void navigator.clipboard.writeText(output).catch(() => {});
  }
  async function appendToSource() {
    if (!output.trim() || !onAppendToSource) return;
    const err = await onAppendToSource(output);
    error = err ?? null;
  }
  function commitSaveAsNote() {
    if (!output.trim() || !onSaveAsNote) return;
    onSaveAsNote({ brew: output, sourceTitle: noteTitle, linkBack: saveLinked });
    saveFormOpen = false;
  }

  onDestroy(() => {
    // Stop the upstream generation; keep whatever streamed in the session.
    if (activeChannel && currentSource) {
      sessionFor(currentSource).interrupted = true;
    }
    cancelActiveStream();
    activeChannel = null;
  });
</script>

{#snippet itemRow(id: string, fallback: string, kind: "ai" | "custom")}
  <div class="brew-item" class:done={isDone(id, kind)}>
    <button
      class="brew-check"
      onclick={() => toggleDone(id, kind)}
      title={isDone(id, kind) ? "Mark as not done" : "Mark as done"}
    >{isDone(id, kind) ? "✓" : "○"}</button>
    {#if editingId === id}
      <input
        class="brew-item-edit"
        use:focusOnMount
        bind:value={editText}
        onkeydown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); commitEdit(kind); }
          else if (e.key === "Escape") { e.preventDefault(); editingId = null; }
        }}
        onblur={() => commitEdit(kind)}
      />
    {:else}
      <button
        class="brew-item-text"
        onclick={() => startEdit(id, kind, fallback)}
        title="Click to edit"
      >{textFor(id, kind, fallback)}</button>
    {/if}
    {#if kind === "custom"}
      <button class="brew-remove" onclick={() => removeCustomItem(id)} title="Remove from checklist">×</button>
    {/if}
    {#if implementing === id}
      <button class="brew-btn implementing" onclick={() => onImplementCancel?.()} title="Cancel this revision">
        cancel<span class="brew-dots">…</span>
      </button>
    {:else}
      <button
        class="brew-btn implement"
        onclick={() => void implementItem(id, kind, fallback)}
        disabled={busy || implementing !== null || !onImplement}
        title={isDone(id, kind) ? "Run again" : "Have the AI apply this to the note — you review the diff"}
      >implement</button>
    {/if}
  </div>
  {#if rowNote[id]}
    <div class="brew-row-note">{rowNote[id]}</div>
  {/if}
{/snippet}

<div class="brew-pane">
  <div class="brew-header">
    <span class="brew-accent"></span>
    <span class="brew-title">
      <span class="brew-label">brew —</span>
      {noteTitle || "(untitled)"}
    </span>
    <span class="brew-actions">
      <button class="brew-btn" onclick={rerun} disabled={busy || implementing !== null} title="Re-run for fresh suggestions">re-run</button>
      <button class="brew-btn" onclick={copyToClipboard} disabled={!output} title="Copy the brew to clipboard">copy</button>
      <button class="brew-btn" onclick={appendToSource} disabled={!output} title="Append the brew to the bottom of the source note">append</button>
      <button
        class="brew-btn"
        onclick={() => (saveFormOpen = !saveFormOpen)}
        disabled={!output}
        class:active={saveFormOpen}
        title="Save the brew as a new note in the vault"
      >save as note</button>
      <button class="brew-btn close-btn" onclick={() => onClose?.()} title="Close brew pane (Ctrl+W)">close</button>
    </span>
  </div>
  {#if saveFormOpen}
    <div class="brew-save-form">
      <label class="brew-save-link">
        <input type="checkbox" bind:checked={saveLinked} />
        link back to <strong>{noteTitle || "source"}</strong>
      </label>
      <span class="brew-save-actions">
        <button class="brew-btn" onclick={() => (saveFormOpen = false)}>cancel</button>
        <button class="brew-btn primary" onclick={commitSaveAsNote}>save</button>
      </span>
    </div>
  {/if}
  <div class="brew-body" bind:this={scroller}>
    {#if error}
      <div class="brew-error">{error}</div>
    {/if}
    {#if interrupted && !busy}
      <div class="brew-row-note">brew was interrupted — re-run for fresh suggestions</div>
    {/if}
    {#if !hasRun && !busy}
      <div class="brew-empty-state">
        <p>no brew yet for this note.</p>
        <button class="brew-btn primary" onclick={rerun}>brew this note</button>
      </div>
    {/if}
    {#if !output && busy}
      <div class="brew-empty">brewing<span class="brew-dots">…</span></div>
    {/if}
    {#each parsed.preamble as line, i (i)}
      <p class="brew-intro">{line}</p>
    {/each}
    {#each parsed.sections as section, si (si)}
      <h3 class="brew-section">{section.title}</h3>
      {#each section.intro as line, i (i)}
        <p class="brew-intro">{line}</p>
      {/each}
      {#each section.items as item (item.id)}
        {@render itemRow(item.id, item.text, "ai")}
      {/each}
    {/each}
    {#if busy && output}
      <span class="brew-cursor">▌</span>
    {/if}
    <div class="brew-custom-head">
      <h3 class="brew-section custom">your checklist</h3>
      <span class="brew-custom-hint">applies to any note · persists per vault</span>
    </div>
    {#if customItems.length === 0}
      <p class="brew-intro dim">standing editing moves — “tighten passive voice”, “rename X to Y” — add one below.</p>
    {/if}
    {#each customItems as c (c.id)}
      {@render itemRow(c.id, c.text, "custom")}
    {/each}
  </div>
  <div class="brew-add-row">
    <input
      class="brew-add-input"
      placeholder="add your own — e.g. remove passive verbs"
      bind:value={newItemText}
      onkeydown={(e) => {
        if (e.key === "Enter") { e.preventDefault(); addCustomItem(); }
      }}
    />
    <button class="brew-btn" onclick={addCustomItem} disabled={!newItemText.trim()}>add</button>
  </div>
</div>

<style>
  /* Warm dark tint — unmistakably a different mode from the #1a1a1a
     editor, in the gold family of the brew accent. */
  .brew-pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #1e1913;
    color: #e0e0e0;
    overflow: hidden;
  }
  .brew-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px 6px 0;
    border-bottom: 1px solid #33291a;
    font-size: 12px;
    background: #241d12;
  }
  .brew-accent {
    width: 3px;
    align-self: stretch;
    background: #d6b06a;
    margin-right: 4px;
  }
  .brew-title {
    flex: 1;
    color: #cfcfcf;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .brew-label {
    color: #d6b06a;
    text-transform: uppercase;
    font-size: 10px;
    letter-spacing: 0.1em;
    margin-right: 6px;
  }
  .brew-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }
  .brew-btn {
    background: transparent;
    border: 1px solid #3a3020;
    color: #aaa;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 2px 8px;
    cursor: pointer;
    border-radius: 2px;
  }
  .brew-btn:hover:not(:disabled) {
    border-color: #6a5a38;
    color: #e0e0e0;
  }
  .brew-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
  .brew-btn.close-btn {
    border-color: #2c2418;
    margin-left: 4px;
  }
  .brew-btn.active {
    border-color: #d6b06a;
    color: #e0e0e0;
    background: rgba(214, 176, 106, 0.08);
  }
  .brew-btn.primary {
    border-color: #6c6;
    color: #e0e0e0;
    background: rgba(108, 198, 108, 0.08);
  }
  .brew-btn.implement {
    border-color: #4a6a88;
    color: #9fc7ec;
  }
  .brew-btn.implement:hover:not(:disabled) {
    border-color: #6cb6ff;
    color: #6cb6ff;
  }
  .brew-btn.implementing {
    border-color: #d6b06a;
    color: #d6b06a;
  }
  .brew-save-form {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-bottom: 1px solid #33291a;
    background: #1a1610;
    font-size: 11px;
  }
  .brew-save-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: #cfcfcf;
    cursor: pointer;
    flex: 1;
  }
  .brew-save-link input {
    margin: 0;
  }
  .brew-save-link strong {
    color: #d6b06a;
    font-weight: normal;
    font-style: italic;
  }
  .brew-save-actions {
    display: flex;
    gap: 6px;
  }
  .brew-body {
    flex: 1;
    overflow-y: auto;
    padding: 12px 16px;
    min-height: 0;
  }
  .brew-empty,
  .brew-empty-state {
    color: #888;
    font-size: 12px;
    font-style: italic;
  }
  .brew-empty-state p {
    margin: 0 0 8px;
  }
  .brew-dots {
    display: inline-block;
    animation: brew-pulse 1.2s ease-in-out infinite;
  }
  @keyframes brew-pulse {
    0%, 100% { opacity: 0.3; }
    50% { opacity: 1; }
  }
  .brew-section {
    margin: 14px 0 6px;
    font-size: 11px;
    font-weight: normal;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: #d6b06a;
  }
  .brew-section:first-child {
    margin-top: 0;
  }
  .brew-custom-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-top: 18px;
    padding-top: 10px;
    border-top: 1px dashed #33291a;
  }
  .brew-custom-head .brew-section {
    margin: 0;
  }
  .brew-custom-hint {
    color: #6a5f4a;
    font-size: 10px;
  }
  .brew-intro {
    margin: 4px 0;
    font-size: 12.5px;
    line-height: 1.5;
    color: #b9b2a4;
  }
  .brew-intro.dim {
    color: #6a5f4a;
    font-style: italic;
  }
  .brew-item {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 3px 0;
  }
  .brew-item.done .brew-item-text {
    opacity: 0.45;
  }
  .brew-check {
    flex: 0 0 auto;
    background: transparent;
    border: none;
    color: #d6b06a;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    padding: 1px 2px;
    line-height: 1.5;
  }
  .brew-item-text {
    flex: 1 1 auto;
    background: transparent;
    border: none;
    color: #dcd5c6;
    font: inherit;
    font-size: 12.5px;
    line-height: 1.5;
    text-align: left;
    cursor: text;
    padding: 1px 2px;
    border-radius: 2px;
  }
  .brew-item-text:hover {
    background: rgba(214, 176, 106, 0.06);
  }
  .brew-item-edit {
    flex: 1 1 auto;
    background: #16120c;
    border: 1px solid #6a5a38;
    color: #e8e2d4;
    font: inherit;
    font-size: 12.5px;
    padding: 1px 4px;
    border-radius: 2px;
  }
  .brew-item-edit:focus {
    outline: none;
    border-color: #d6b06a;
  }
  .brew-remove {
    flex: 0 0 auto;
    background: transparent;
    border: none;
    color: #6a5f4a;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
    padding: 1px 2px;
  }
  .brew-remove:hover {
    color: #c97a7a;
  }
  .brew-row-note {
    margin: 0 0 4px 24px;
    font-size: 11px;
    font-style: italic;
    color: #8a7d5e;
  }
  .brew-add-row {
    display: flex;
    gap: 6px;
    padding: 8px 12px;
    border-top: 1px solid #33291a;
    background: #1a1610;
    flex: 0 0 auto;
  }
  .brew-add-input {
    flex: 1 1 auto;
    background: #16120c;
    border: 1px solid #3a3020;
    color: #e0e0e0;
    font: inherit;
    font-size: 12px;
    padding: 4px 8px;
    border-radius: 2px;
  }
  .brew-add-input:focus {
    outline: none;
    border-color: #d6b06a;
  }
  .brew-add-input::placeholder {
    color: #6a5f4a;
  }
  .brew-cursor {
    color: #d6b06a;
    animation: brew-blink 1s steps(2) infinite;
    margin-left: 2px;
  }
  @keyframes brew-blink {
    0% { opacity: 1; }
    50% { opacity: 0; }
  }
  .brew-error {
    background: rgba(201, 122, 122, 0.08);
    border: 1px solid rgba(201, 122, 122, 0.3);
    color: #c97a7a;
    padding: 10px 12px;
    border-radius: 3px;
    font-size: 12px;
    margin-bottom: 12px;
  }
</style>
