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

  type UnlinkedMention = {
    path: string;
    title: string;
    snippet: string;
    count: number;
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
  let unlinked = $state<UnlinkedMention[]>([]);
  let unlistenNotes: UnlistenFn | null = null;
  let unlistenRelated: UnlistenFn | null = null;
  let fetchGen = 0;

  // Title of the current note (file stem) — the target whose mentions
  // we're hunting. Used as the wikilink text when linking a mention.
  let currentTitle = $derived.by(() => {
    if (!currentPath) return "";
    const slash = Math.max(currentPath.lastIndexOf("/"), currentPath.lastIndexOf("\\"));
    const base = slash >= 0 ? currentPath.slice(slash + 1) : currentPath;
    return base.toLowerCase().endsWith(".md") ? base.slice(0, -3) : base;
  });

  async function refresh() {
    const myGen = ++fetchGen;
    if (!currentPath) {
      backlinks = [];
      related = [];
      unlinked = [];
      return;
    }
    const path = currentPath;
    try {
      const [bls, rels, mentions] = await Promise.all([
        invoke<BacklinkInfo[]>("find_backlinks", { path }),
        invoke<RelatedNote[]>("find_related", { path }),
        invoke<UnlinkedMention[]>("find_unlinked_mentions", { path }),
      ]);
      if (myGen === fetchGen) {
        backlinks = bls;
        related = rels;
        unlinked = mentions;
      }
    } catch {
      if (myGen === fetchGen) {
        backlinks = [];
        related = [];
        unlinked = [];
      }
    }
  }

  async function linkMention(sourcePath: string) {
    if (!currentTitle) return;
    try {
      await invoke("link_unlinked_mention", {
        sourcePath,
        targetTitle: currentTitle,
      });
      // Optimistically drop it from the list; a full refresh follows
      // when the watcher fires notes_changed.
      unlinked = unlinked.filter((m) => m.path !== sourcePath);
    } catch (e) {
      console.error("link_unlinked_mention failed", e);
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
    <span class="count">{backlinks.length}{related.length > 0 ? ` · ${related.length} related` : ""}{unlinked.length > 0 ? ` · ${unlinked.length} unlinked` : ""}</span>
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
      {#if unlinked.length > 0}
        <div class="section">
          <div class="section-label">unlinked mentions <span class="related-hint">names this note without a link</span></div>
          <ul class="bl-list">
            {#each unlinked as m (m.path)}
              <li class="bl-row mention-row">
                <span class="mention-main" onclick={() => onNavigate?.(m.path)} role="button" tabindex="-1"
                  onkeydown={(e) => { if (e.key === "Enter") onNavigate?.(m.path); }}>
                  <span class="bl-title">
                    {m.title}
                    {#if m.count > 1}<span class="sim">{m.count}×</span>{/if}
                  </span>
                  <span class="bl-snippet">{m.snippet}</span>
                </span>
                <button
                  class="mention-link-btn"
                  onclick={(e) => { e.stopPropagation(); void linkMention(m.path); }}
                  title={`Wrap the first mention in [[${currentTitle}]]`}
                >link</button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}
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
  .mention-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .mention-main {
    flex: 1;
    min-width: 0;
    cursor: pointer;
  }
  .mention-link-btn {
    flex-shrink: 0;
    background: transparent;
    border: 1px solid #2e2e2e;
    color: #97a3b0;
    font: inherit;
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 2px 8px;
    border-radius: 3px;
    cursor: pointer;
  }
  .mention-link-btn:hover {
    border-color: #6cb6ff;
    color: #6cb6ff;
  }
</style>
