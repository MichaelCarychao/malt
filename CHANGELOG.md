# Changelog

All notable changes to malt are documented here. Versioning follows
[semantic versioning](https://semver.org/) with pre-1.0 conventions: minor
bumps for meaningful feature batches, patch bumps for fixes.

## 0.2.5 — 2026-05-28

UX round: navigability, discoverability, sync awareness.

- **Settings split into tabs** — General / Shortcuts / Tags & queries /
  AI / About. The modal had outgrown a single scroll; tabs make it
  feel like proper preferences. Notes-folder controls moved from About
  to General. Vim shortcuts only render under Shortcuts when vim mode
  is enabled.
- **Find in current note** — `Ctrl+F` (or `Cmd+F`) inside the editor
  opens CodeMirror's native search panel: highlighted matches,
  next/prev navigation (`Ctrl+G` / `Ctrl+Shift+G`), regex toggle,
  case-sensitive toggle. Closes with `Esc`.
- **"Start writing" CTA** for empty-notes-folder state — replaces the
  terse "No notes yet" with a centered card explaining how to create
  the first note. Stale "(coming in M4b)" copy removed from the
  no-matches state.
- **Embedding model download indicator** — the first time fastembed
  loads (~33MB ONNX download), a pulsing amber `indexing…` pill
  appears in the status bar so users know what's happening. Disappears
  once the model is ready.
- **Sync conflict detection** — files matching Dropbox `(conflicted
  copy …)` or Syncthing `.sync-conflict-…` patterns get a `⚠` badge
  next to the title and a corpus-wide conflict count in the status
  bar. Click the count to clear the search and find them all. Manual
  merge for now; resolution UI is a future enhancement.

## 0.2.4 — 2026-05-27

**In-app auto-updater** — going forward, malt checks GitHub Releases for
new versions on startup and offers a one-click install + restart. The
multi-step "download → run installer → re-find in Start menu" friction
is gone for v0.2.4+.

- `tauri-plugin-updater` + `tauri-plugin-process` integrated.
- Endpoint: `https://github.com/MichaelCarychao/malt/releases/latest/download/latest.json`
  (the `/latest` URL resolves to the most recent *published* release —
  drafts are invisible to the updater, so internal testing builds don't
  leak out).
- Update integrity: each release bundle is signed with an ed25519 key
  in CI (`TAURI_SIGNING_PRIVATE_KEY` GitHub secret). The app verifies
  signatures locally with the embedded public key before installing.
- Silent background check 5 seconds after boot. If a newer version
  exists, a non-blocking amber toast appears bottom-right: "malt vX.Y.Z
  is available — Show details." Dismissable. Click to see full release
  notes + install/later choice.
- Manual "check for updates" button in Settings → about. Status label
  reflects current state (checking, up to date, downloading 47%, etc.).
- Install flow: download with progress bar → signature verification →
  in-place install → automatic restart into the new version. Typically
  10-15 seconds.

## 0.2.3 — 2026-05-27

Cosmetic polish pass.

- **Live version in Settings.** Pulled from Cargo.toml at compile time
  (single source of truth). About section adds repo + changelog links.
- **Window title shows the current note** — `malt — Note title` instead
  of just `malt`. Updates whenever you switch notes.
- **Smarter timestamps in the note list.** `3:14p` / `yest.` / `Tue` /
  `Mar 5` / `Mar '25` depending on age, instead of raw `2h` / `1d` /
  `3mo`. Reads at a glance.
- **First-run welcome notes** — a `Welcome to malt.md` + `Quick Tour.md`
  pair seeded into the notes folder on first launch (only if the dir is
  empty). Quietly self-deletes any future appearance via a `welcomed`
  flag file so trashing them isn't undone.
- **Restore last-open note on startup.** `selectedPath` persisted to
  localStorage; you land where you left off instead of always at the
  top of the sort.
- **Right-click any sidebar row** for a context menu: Open · Open in
  second pane · Rename · Duplicate · Reveal in file manager · Delete.
- **Save indicator** in the status bar — brief `saved` pulse after each
  autosave fires. Subtle confirmation your work is committed.
- **API key status dot** in the status bar — green when an Anthropic
  key is configured, gray when not. Click to jump to Settings.
- **Tooltip shortcuts** — hovering the gear shows the modifier+key for
  Settings; saved-search chips already showed their slot binding.
- **Boot splash** — a brief amber `m` on charcoal while content loads,
  fades out after ~200ms.

## 0.2.2 — 2026-05-27

macOS Gatekeeper fix + one keybinding change.

- Ad-hoc codesign the macOS bundle in CI (`APPLE_SIGNING_IDENTITY=-`).
  Without it, fully-unsigned + quarantined .dmg downloads trigger
  Gatekeeper's harsh "malt is damaged and can't be opened" rejection.
  With ad-hoc signing, users see the milder "cannot verify developer"
  warning and can right-click → Open to install.
- **AI completion keybinding changed from `Mod+Space` to `Mod+I`**.
  `Cmd+Space` is hardcoded to Spotlight on macOS — can't be intercepted.
  `Cmd/Ctrl+I` reads as "Insert / AI / Idea" and is free on both
  platforms. Same three behaviors: bare press = continue, with selection
  = rewrite, re-press = re-roll the current ghost suggestion.
- Real Developer ID signing + notarization is deferred until 1.0.

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
