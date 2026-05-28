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
    onClose,
    onAppendToSource,
  }: {
    noteTitle?: string;
    noteBody?: string;
    onClose?: () => void;
    onAppendToSource?: (brew: string) => void;
  } = $props();

  let output = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let scroller: HTMLDivElement | null = $state(null);
  let activeChannel: Channel<string> | null = null;
  // Hash of (title + body) so we re-stream when the parent swaps in a
  // different source note without unmounting the pane.
  let lastSource = "";

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
      await invoke("brew_streaming", { body, onChunk: channel });
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

  // Re-run when the source note changes (e.g. user re-fires Cmd+Shift+B
  // on a different note). Tracks both title and body so a body edit on
  // the same note also re-streams.
  $effect(() => {
    const key = `${noteTitle}::${noteBody}`;
    if (key === lastSource) return;
    lastSource = key;
    void runBrew(noteBody);
  });

  function copyToClipboard() {
    if (!output.trim()) return;
    void navigator.clipboard.writeText(output).catch(() => {
      /* clipboard unavailable — ignore */
    });
  }
  function appendToSource() {
    if (!output.trim() || !onAppendToSource) return;
    onAppendToSource(output);
  }
  function rerun() {
    void runBrew(noteBody);
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
      <button class="brew-close" onclick={() => onClose?.()} aria-label="Close brew pane" title="Close (Ctrl+W)">×</button>
    </span>
  </div>
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
  .brew-close {
    background: transparent;
    border: 0;
    color: #888;
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
    padding: 0 6px;
    margin-left: 4px;
  }
  .brew-close:hover {
    color: #e0e0e0;
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
