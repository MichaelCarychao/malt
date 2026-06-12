<script lang="ts">
  // Inline unlock form for a locked (encrypted) note. Lives INSIDE the
  // editor pane — never a window-covering modal — so the note list, search
  // bar, and the other pane stay fully interactive while a note is locked.
  // The parent supplies `onUnlock` (verify + cache the password; resolve to
  // an error string or null) and grabs a focus handle via onRegisterFocus
  // so Enter-from-search / row clicks can land the cursor here.

  let {
    title = "",
    onUnlock,
    onRegisterFocus,
  }: {
    title?: string;
    onUnlock?: (password: string) => Promise<string | null>;
    /** Called once with a function that focuses the password field. */
    onRegisterFocus?: (fn: () => void) => void;
  } = $props();

  let pw = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);
  let inputEl: HTMLInputElement | null = $state(null);

  $effect(() => {
    onRegisterFocus?.(() => inputEl?.focus());
  });

  async function submit() {
    if (!pw || busy || !onUnlock) return;
    busy = true;
    error = null;
    const err = await onUnlock(pw);
    busy = false;
    if (err) {
      error = err;
      inputEl?.select();
    } else {
      pw = "";
    }
  }
</script>

<div class="inline-unlock">
  <div class="unlock-title"><span class="unlock-icon">🔒</span>{title}</div>
  <div class="unlock-row">
    <input
      bind:this={inputEl}
      bind:value={pw}
      class="unlock-input"
      type="password"
      placeholder="password"
      spellcheck="false"
      autocomplete="off"
      onkeydown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          void submit();
        }
        // Esc is handled by the page-level router (returns focus to search).
      }}
    />
    <button class="unlock-btn" onclick={() => void submit()} disabled={busy || !pw}>
      {busy ? "unlocking…" : "unlock"}
    </button>
  </div>
  {#if error}
    <div class="unlock-error">{error}</div>
  {/if}
  <div class="unlock-hint">
    AES-256-GCM · Argon2id — no recovery without the password. <kbd>Esc</kbd> back to search.
  </div>
</div>

<style>
  .inline-unlock {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border: 1px solid #2e2e2e;
    border-radius: 4px;
    background: #1d1d1d;
    padding: 14px 18px;
    max-width: 340px;
  }
  .unlock-title {
    color: #cfcfcf;
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .unlock-icon {
    font-size: 14px;
  }
  .unlock-row {
    display: flex;
    gap: 6px;
  }
  .unlock-input {
    flex: 1;
    min-width: 0;
    background: #161616;
    border: 1px solid #2e2e2e;
    border-radius: 3px;
    color: #e0e0e0;
    font: inherit;
    font-size: 12px;
    padding: 5px 8px;
    outline: none;
  }
  .unlock-input:focus {
    border-color: #6cb6ff;
  }
  .unlock-btn {
    background: transparent;
    border: 1px solid #333;
    color: #aaa;
    font: inherit;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 2px 10px;
    border-radius: 3px;
    cursor: pointer;
  }
  .unlock-btn:hover:not(:disabled) {
    border-color: #6cb6ff;
    color: #e0e0e0;
  }
  .unlock-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }
  .unlock-error {
    color: #c97a7a;
    font-size: 11px;
  }
  .unlock-hint {
    color: #555;
    font-size: 10px;
  }
  .unlock-hint kbd {
    border: 1px solid #3a3a3a;
    border-bottom-width: 2px;
    border-radius: 3px;
    padding: 0 4px;
    font: inherit;
    font-size: 9px;
    color: #888;
  }
</style>
