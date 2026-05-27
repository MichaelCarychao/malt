<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";

  type BacklinkInfo = {
    source_path: string;
    source_title: string;
    snippet: string;
    link_text: string;
  };

  type RelatedNote = {
    path: string;
    title: string;
    snippet: string;
    similarity: number;
  };

  let {
    currentPath,
    collapsed = $bindable(false),
    onNavigate,
  }: {
    currentPath: string | null;
    collapsed?: boolean;
    onNavigate?: (path: string) => void;
  } = $props();

  let backlinks = $state<BacklinkInfo[]>([]);
  let related = $state<RelatedNote[]>([]);
  let unlistenNotes: UnlistenFn | null = null;
  let unlistenRelated: UnlistenFn | null = null;
  let fetchGen = 0;

  async function refresh() {
    const myGen = ++fetchGen;
    if (!currentPath) {
      backlinks = [];
      related = [];
      return;
    }
    const path = currentPath;
    try {
      const [bls, rels] = await Promise.all([
        invoke<BacklinkInfo[]>("find_backlinks", { path }),
        invoke<RelatedNote[]>("find_related", { path }),
      ]);
      if (myGen === fetchGen) {
        backlinks = bls;
        related = rels;
      }
    } catch {
      if (myGen === fetchGen) {
        backlinks = [];
        related = [];
      }
    }
  }

  $effect(() => {
    void currentPath;
    void refresh();
  });

  onMount(async () => {
    unlistenNotes = await listen("notes_changed", refresh);
    // The embedding worker emits this when a new vector lands — refresh so
    // related results light up as soon as the embed completes.
    unlistenRelated = await listen("related_changed", refresh);
  });

  onDestroy(() => {
    unlistenNotes?.();
    unlistenRelated?.();
  });

  function toggle() {
    collapsed = !collapsed;
  }

  function formatSim(s: number): string {
    return `${Math.round(s * 100)}%`;
  }
</script>

<div class="linkbacks">
  <div
    class="linkbacks-header"
    onclick={toggle}
    onkeydown={(e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        toggle();
      }
    }}
    role="button"
    tabindex="0"
    aria-expanded={!collapsed}
  >
    <span class="chevron">{collapsed ? "▶" : "▼"}</span>
    <span class="label">linkbacks</span>
    <span class="count">{backlinks.length}{related.length > 0 ? ` · ${related.length} related` : ""}</span>
  </div>
  {#if !collapsed}
    <div class="sections">
      <div class="section">
        <div class="section-label">backlinks</div>
        <ul class="bl-list">
          {#each backlinks as bl (bl.source_path + bl.snippet)}
            <li class="bl-row" onclick={() => onNavigate?.(bl.source_path)}>
              <span class="bl-title">{bl.source_title}</span>
              <span class="bl-snippet">{bl.snippet}</span>
            </li>
          {/each}
          {#if backlinks.length === 0}
            <li class="empty">No notes link to this one yet.</li>
          {/if}
        </ul>
      </div>
      <div class="section">
        <div class="section-label">related <span class="related-hint">by topic similarity</span></div>
        <ul class="bl-list">
          {#each related as r (r.path)}
            <li class="bl-row" onclick={() => onNavigate?.(r.path)}>
              <span class="bl-title">
                {r.title}
                <span class="sim">{formatSim(r.similarity)}</span>
              </span>
              <span class="bl-snippet">{r.snippet}</span>
            </li>
          {/each}
          {#if related.length === 0}
            <li class="empty">No related notes yet. (Embeddings may still be building.)</li>
          {/if}
        </ul>
      </div>
    </div>
  {/if}
</div>

<style>
  .linkbacks {
    display: flex;
    flex-direction: column;
    background: #181818;
    height: 100%;
    overflow: hidden;
  }
  .linkbacks-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    color: #888;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    cursor: pointer;
    user-select: none;
    flex-shrink: 0;
    border-bottom: 1px solid #2a2a2a;
  }
  .linkbacks-header:hover {
    color: #aaa;
  }
  .linkbacks-header:focus-visible {
    outline: 1px solid #555;
    outline-offset: -1px;
  }
  .chevron {
    color: #555;
    font-size: 9px;
    width: 10px;
  }
  .label {
    flex: 1;
  }
  .count {
    color: #555;
  }
  .sections {
    flex: 1;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }
  .section {
    border-bottom: 1px solid #222;
  }
  .section:last-child {
    border-bottom: 0;
  }
  .section-label {
    padding: 4px 12px;
    color: #555;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    background: #141414;
    border-bottom: 1px solid #222;
  }
  .related-hint {
    color: #3a3a3a;
    margin-left: 6px;
    text-transform: none;
    letter-spacing: 0;
    font-size: 9px;
    font-style: italic;
  }
  .bl-list {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .bl-row {
    padding: 5px 12px;
    cursor: pointer;
    border-bottom: 1px solid #232323;
  }
  .bl-row:hover {
    background: #232323;
  }
  .bl-title {
    color: #e0e0e0;
    font-size: 12px;
    display: block;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sim {
    color: #555;
    font-size: 10px;
    margin-left: 6px;
    font-variant-numeric: tabular-nums;
  }
  .bl-snippet {
    color: #666;
    font-size: 11px;
    display: block;
    margin-top: 1px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    padding: 8px 12px;
    color: #555;
    font-size: 11px;
    font-style: italic;
  }
</style>
