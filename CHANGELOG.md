# Changelog

All notable changes to malt are documented here. Versioning follows
[semantic versioning](https://semver.org/) with pre-1.0 conventions: minor
bumps for meaningful feature batches, patch bumps for fixes.

## 0.2.1 — 2026-05-27

Build fixes only — no app behavior changes from 0.2.0.

- Drop Intel Mac (x86_64) from the release matrix. ONNX Runtime (which
  fastembed pulls in) has no prebuilt Intel binaries on the current ort
  release, so the universal build can't link. Apple Silicon only for now.
  Intel Mac users can build from source.
- Bundle identifier changed from `com.malt.app` to `com.carychao.malt`.
  The previous one ended in `.app`, which collides with the macOS bundle
  extension convention. Effectively a one-time reinstall on systems that
  already installed 0.2.0 (Windows).

## 0.2.0 — 2026-05-27

First feature-complete release after the initial scaffold. Roughly everything
nvalt users expect, plus AI-augmented tagging and linking.

### Editor & navigation
- CodeMirror 6 with optional vim mode (in Settings)
- Sub-perceptible type-to-filter search powered by Tantivy (fuzzy + prefix)
- Split-pane editor: Cmd/Ctrl+click any list row or wikilink to open in a second pane
- Per-pane back/forward history (Cmd/Ctrl+`[` / `]`)
- Word/character count toggle in the status bar
- Configurable notes folder (set in Settings; restart required)

### Wikilinks
- `[[wikilink]]` autocomplete on `[[` (ranked by Tantivy)
- Live (resolved) vs broken visual distinction
- Click to navigate; Cmd/Ctrl+click opens in the other pane
- Click broken link → create note with that title
- Inline rename (Cmd/Ctrl+R) with atomic backlink rewrite
- Brackets hidden in render unless the cursor is in the link

### Tags
- Inline `#hashtags` with object-tag bias (see [research notes](https://zettelkasten.de/posts/object-tags-vs-topic-tags/))
- Type a tag anywhere; canonical-form tag line auto-relocates to the bottom
  of the file on save (hidden in render)
- Pill row above the editor body shows current tags; click to filter,
  hover-`×` to remove, right-click for menu
- Vocab vs ad-hoc visual distinction (amber italic for unsanctioned tags)
- Editable starter vocabulary in Settings

### Search & smart-views
- Saved searches with Cmd/Ctrl+S, activated via Cmd/Ctrl+1..9
- Query operators: `tag:foo`, `-tag:foo`, `modified:<7d`, `modified:>30d`,
  composable

### Linkbacks & semantic related
- Auto-maintained reverse wikilink index (linkbacks panel)
- Local semantic similarity via fastembed-rs (BGE-small-en-v1.5, 384-dim)
  + sqlite-vec; "Related" subsection shows top-5 by cosine similarity
- Background worker re-embeds on save and file watcher events

### AI assistance (BYO Anthropic API key)
- Ghost-text continuation at cursor (Cmd/Ctrl+Space)
- Selection rewrite with "unpack details, avoid clichés" (Cmd/Ctrl+Space
  on selection)
- Cmd/Ctrl+Shift+L: review modal proposing wikilinks for the current note
  — both deterministic title matches and AI-extracted entities worth
  promoting to their own notes
- All AI-bound text is stripped of malt-private markup (hashtags +
  wikilink brackets) before being sent

### Exports
- Cmd/Ctrl+Shift+E: per-note export modal
- Formats: clean `.md`, `.html`, `.epub`, `.txt`
- Clipboard variants: plain text, rich text (HTML)
- Optional "append linked notes" — packs the source + all resolved
  wikilink targets into one composite document (TOC-style sharing)

### Storage
- Plain `.md` files in a single flat folder (default `~/malt/`)
- No proprietary database — every feature is derived data
- Cross-platform sync via Dropbox / iCloud / etc. supported by design
  (use the configurable folder)

## 0.1.0 — initial scaffold

Empty Tauri 2 + SvelteKit + TypeScript window opens. Nothing functional yet.
