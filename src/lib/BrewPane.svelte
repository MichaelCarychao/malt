<script lang="ts">
  // Brew pane: a read-only streaming markdown view that lives in the
  // secondary pane while the user is brainstorming on a note.
  //
  // The parent passes `noteTitle` (just for the header), `noteBody`
  // (the source we send to Claude), and `onClose`. We open a streaming
  // IPC channel to `brew_streaming`, accumulate the response into
  // `output`, and render it as plain text — the brew prompt asks for
  // markdown, so visual hierarchy comes through naturally even without
  // a renderer (## headings, lists, italics-by-asterisks all stay
  // readable in a monospace block).
  //
  // The user can scroll. They can also append the whole brew to the
  // source note via the "append to note" action.

  import { Channel, invoke } from "@tauri-apps/api/core";
  import { onMount, onDestroy } from "svelte";

  let {
    noteTitle = "",
    noteBody = "",
    brewNonce = 0,
    onClose,
    onAppendToSource,
    onSaveAsNote,
  }: {
    noteTitle?: string;
    noteBody?: string;
    /** Bumped by the parent each time the user EXPLICITLY invokes brew
     * (Cmd+Shift+B). Brewing keys on this nonce alone — never on noteBody
     * — so live editor edits flowing in via noteBody don't fire a fresh
     * (and costly) brew_streaming call on every keystroke. */
    brewNonce?: number;
    onClose?: () => void;
    /** Append the brew to the source note. Resolves to an error string to
     * show the user (e.g. the note is locked), or null on success. */
    onAppendToSource?: (brew: string) => void | Promise<string | null>;
    /** Save the brew as a new note. The pane decides whether to add a
     * back-link to the source (the user toggles the checkbox below).
     * Parent handles `create_note` IPC + navigation to the new note. */
    onSaveAsNote?: (args: {
      brew: string;
      sourceTitle: string;
      linkBack: boolean;
    }) => void;
  } = $props();

  let output = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let scroller: HTMLDivElement | null = $state(null);
  let activeChannel: Channel<string> | null = null;
  // The brewNonce we last streamed for. Starts at -1 (no real nonce yet)
  // so the first explicit trigger always runs.
  let lastNonce = -1;

  // "Save as note" inline form state.
  let saveLinked = $state(true);
  let saveFormOpen = $state(false);

  async function runBrew(body: string) {
    if (!body.trim()) {
      output = "";
      error = "Nothing to brew — the note is empty.";
      return;
    }
    output = "";
    error = null;
    busy = true;
    const channel = new Channel<string>();
    activeChannel = channel;
    channel.onmessage = (chunk: string) => {
      // Drop late chunks from a previous run if the user navigated away.
      if (activeChannel !== channel) return;
      output += chunk;
      // Auto-scroll to bottom while streaming so new content stays in view.
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    };
    try {
      // Pass title alongside body — backend prepends it as a `# Title`
      // heading so the model knows what the note is about even when
      // the body is fragmentary.
      await invoke("brew_streaming", { title: noteTitle, body, onChunk: channel });
    } catch (e) {
      if (activeChannel === channel) {
        error = String(e);
      }
    } finally {
      if (activeChannel === channel) {
        busy = false;
        activeChannel = null;
      }
    }
  }

  // Re-run only on an EXPLICIT trigger: the parent bumps `brewNonce` each
  // time the user fires Cmd+Shift+B. We deliberately do NOT depend on
  // noteBody — the parent may stream live editor content into it, and we
  // must not kick off a brew_streaming call on every keystroke. The body
  // is captured here (at trigger time) and frozen for this run.
  $effect(() => {
    const nonce = brewNonce;
    if (nonce === lastNonce) return;
    lastNonce = nonce;
    // Snapshot the body now so later edits don't mutate the in-flight run.
    const bodyAtTrigger = noteBody;
    void runBrew(bodyAtTrigger);
  });

  function copyToClipboard() {
    if (!output.trim()) return;
    void navigator.clipboard.writeText(output).catch(() => {
      /* clipboard unavailable — ignore */
    });
  }
  async function appendToSource() {
    if (!output.trim() || !onAppendToSource) return;
    // The parent may refuse the append (e.g. the source note is locked and
    // we'd otherwise overwrite its encrypted envelope with plaintext) and
    // resolve to an error message. Surface it in the existing error slot
    // instead of silently doing nothing.
    const err = await onAppendToSource(output);
    error = err ?? null;
  }
  function rerun() {
    void runBrew(noteBody);
  }
  function commitSaveAsNote() {
    if (!output.trim() || !onSaveAsNote) return;
    onSaveAsNote({ brew: output, sourceTitle: noteTitle, linkBack: saveLinked });
    saveFormOpen = false;
  }

  onMount(() => {
    // No-op; the $effect above kicks off the initial run.
  });
  onDestroy(() => {
    // Mark any in-flight stream as superseded so chunks stop applying
    // to a destroyed component.
    activeChannel = null;
  });
</script>

<div class="brew-pane">
  <div class="brew-header">
    <span class="brew-accent"></span>
    <span class="brew-title">
      <span class="brew-label">brew —</span>
      {noteTitle || "(untitled)"}
    </span>
    <span class="brew-actions">
      <button class="brew-btn" onclick={rerun} disabled={busy} title="Re-run on the current note body">re-run</button>
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
    {#if !output && busy}
      <div class="brew-empty">brewing<span class="brew-dots">…</span></div>
    {/if}
    {#if output}
      <pre class="brew-output">{output}{#if busy}<span class="brew-cursor">▌</span>{/if}</pre>
    {/if}
  </div>
</div>

<style>
  .brew-pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #1a1a1a;
    color: #e0e0e0;
    overflow: hidden;
  }
  .brew-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px 6px 0;
    border-bottom: 1px solid #2a2a2a;
    font-size: 12px;
    background: #181818;
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
    border: 1px solid #333;
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
    border-color: #555;
    color: #e0e0e0;
  }
  .brew-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
  .brew-btn.close-btn {
    /* Slightly demoted styling so "close" doesn't fight with re-run /
       copy / append / save-as-note for attention. */
    border-color: #2a2a2a;
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
  .brew-save-form {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-bottom: 1px solid #2a2a2a;
    background: #161616;
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
    padding: 14px 18px;
    min-height: 0;
  }
  .brew-empty {
    color: #888;
    font-size: 12px;
    font-style: italic;
  }
  .brew-dots {
    display: inline-block;
    animation: brew-pulse 1.2s ease-in-out infinite;
  }
  @keyframes brew-pulse {
    0%, 100% { opacity: 0.3; }
    50% { opacity: 1; }
  }
  .brew-output {
    margin: 0;
    font-family: "Cascadia Mono", "SF Mono", Menlo, Consolas, monospace;
    font-size: 13px;
    line-height: 1.55;
    white-space: pre-wrap;
    word-wrap: break-word;
    color: #cfcfcf;
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
