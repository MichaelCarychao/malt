# Changelog

All notable changes to malt are documented here. Versioning follows
[semantic versioning](https://semver.org/) with pre-1.0 conventions: minor
bumps for meaningful feature batches, patch bumps for fixes.

## 0.2.10 — 2026-05-28

- **Wikilink colors fixed at the root cause.** Three tries in, the
  actual fix: CodeMirror 6 nests overlapping mark decorations and uses
  facet precedence to decide which becomes the inner span. Higher
  precedence = inner. The markdown link-token decoration from
  `syntaxHighlighting(oneDark)` was outpriotizing our wikilink
  decoration, so oneDark's `tok-link` span sat inside our wikilink
  span and won the color cascade by being the innermost element with
  an explicit `color` rule. Hoisting `wikilinkPlugin` to `Prec.highest`
  flips the nesting — our span is now inner — and the class-based
  color rule wins without `!important`, inline styles, or descendant
  selectors. Live = sky blue, empty = amber italic, broken = red
  dashed, finally rendering as intended. Removed the inline-style
  workaround since the cause is gone.
- **New tagline.** "Distill notes. Brew ideas." replaces the previous
  line on the splash. Leans into the brewing metaphor in the name.

## 0.2.9 — 2026-05-28

- **Wikilink colors actually render now** — `!important` lost the
  cascade fight against `oneDark`'s HighlightStyle (which registers
  later in the document at equal specificity). Fix: paint the color
  via an inline `style` attribute on the decoration mark, which beats
  every external selector. Live = sky blue, empty = amber italic,
  broken = red dashed — all visibly distinct in the editor again.
- **Splash tagline.** "Plain markdown. AI when you want it." sits in
  italic grey below the wordmark.
- **Tip headlines.** Each tip now carries a max-8-word user-story
  headline (italic amber) above the body. Tip bank rewritten end to
  end so the headlines feel consistent — "Spin up today's note in
  one keystroke," "Rename a note without breaking links," etc.
- **OS-aware key combos in tips.** Tips author with the canonical
  ⌘ / ⇧ macOS symbols; a new `renderKeysForOS()` helper rewrites
  them to `Ctrl+` / `Shift+` on non-Mac platforms at render time.
  Windows users no longer see ⌘ glyphs on the splash.
- **Click anywhere on the splash dismisses.** Only the prev/next
  arrows and the skip checkbox stop propagation; everything else
  (logo, tagline, tip card, dismiss hint) closes the splash on
  click. Matches the "tap any key" affordance.
- **Esc closes the tips browser when launched from Settings.**
  `handleGlobalKey` was intercepting Esc before the tip handler
  ever ran. Tips now take priority in the Esc routing chain.
- **Tips get their own Settings tab.** Promoted from a sub-section
  under General to a dedicated "Tips" tab with launch button, seen
  count, on-startup toggle, and reset-seen-list action.

## 0.2.8 — 2026-05-28

- **Tips system.** Replaced the boot splash content with a rotating
  user-story-style tip carousel. Each tip is tagged with a Settings
  category and stored in a typed bank (`src/lib/tips.ts`). Selection
  algorithm: random from unseen pool until you've seen them all, then
  truly random with no immediate repeats. Arrow keys (or ‹ / ›
  buttons) navigate forward/back; back-stack works across sessions
  via `localStorage`. Tap any other key dismisses. New "don't show
  tips on startup" checkbox (default off) — reinstateable from
  Settings → general, which also gets a "browse tips…" button and a
  "reset seen list" action.
- **Boot splash uses the real bezeled icon image** instead of a
  squished monospace "m". Minimum on-screen duration is now 1 s
  (plus 320 ms fade) so fast boots no longer flash a frame of splash
  before the app pops in.
- **Wikilink colors now actually differentiate.** The markdown
  grammar in `@codemirror/lang-markdown` was treating `[[Foo]]` as a
  reference-style link and the `oneDark` theme's HighlightStyle was
  painting the inner text at higher specificity than our class.
  Added `!important` to every wikilink color rule so live / empty /
  broken are visibly distinct again: **blue** = ready,
  **amber italic** = empty draft, **red dashed** = missing.
- **Default save-search name = current query.** Open the save modal
  and the name field is pre-filled with whatever you typed; hit
  Enter to save without typing again. The text is pre-selected so
  any keystroke replaces it.
- **Double-click on a note row opens the full actions menu** instead
  of jumping straight to rename. Rename is one of the options, plus
  Open / Open in second pane / Duplicate / Reveal / Encrypt / Delete.
  Trackpad users without a right-click gesture now reach every
  action including encryption.

- **Per-note encryption.** Right-click any note → Encrypt… → set a
  password. Body is wrapped in a `MALT-ENC-v1:` envelope (AES-256-GCM
  with an Argon2id-derived key) so the file stays a single line of
  text — Dropbox / Syncthing / git all keep treating it as plain
  markdown. Filename is still visible; only contents are opaque. New
  context-menu items: Encrypt… / Change password… / Decrypt (remove
  password)…. Encrypted notes get a 🔒 in the sidebar, skip the search
  index + AI tagger + embedding worker, and are findable by filename
  only.
- **Settings → Security tab.** "Re-prompt for password on focus loss"
  toggle (default ON) — drops every cached password the moment malt
  loses focus, so encrypted notes re-lock when you tab away. Turn it
  off to keep unlocks for the whole session. Explains the recovery
  story: there isn't one. Lose the password, lose the note.
- **Empty-link color is now amber.** The "this note exists but is
  blank" wikilink state was a too-subtle grey-blue against the live
  blue. It's now the same `#d6b06a` accent malt uses elsewhere for
  "needs your attention" — clearly distinct from both the live blue
  ("ready") and the broken red ("missing").
- **Built-in "Empty Notes" saved search** (`empty:true`). Always
  present in the list — undeletable but renameable, reorderable, and
  removable from the quick bar. New users land with it at ⌘1.
- **"Remove from quick bar" right-click action.** Any saved search
  (built-in or user) can be unbound from its keyboard slot without
  deleting — it stays in Settings → Saved searches, just falls off the
  chip bar. Built-in saved searches show this *instead of* Delete.
- **Black-bg bezeled app icon.** The macOS dock / Windows taskbar
  needed the icon to read on light backgrounds. New `source.png` has
  a solid black canvas with rounded corners; all platform variants
  (icns / ico / Android / iOS) regenerated. Original transparent
  artwork preserved as `source-transparent.png` for future regenerations.
- **Updater note for v0.2.5 testers:** if "Check for updates" reports
  "could not fetch a valid release JSON," it's because the v0.2.5
  GitHub release is still a *draft*. The endpoint uses
  `/releases/latest/download/latest.json`, which only resolves to
  published non-draft releases. Publish the draft on GitHub and the
  updater will work for v0.2.5 testers; v0.2.6 onward will follow the
  same path once published.

## 0.2.6 — 2026-05-28

- **Cmd/Ctrl+F now toggles the find panel** instead of always opening
  it. Second press closes (same as Esc). Applied both inside the
  editor (override of CodeMirror's default Mod-f via `Prec.high`) and
  to the global forwarder, so the behavior is consistent regardless
  of where focus lives. The `onFinderReady` callback now exposes a
  toggle rather than an opener.
- **Empty-note awareness, three places.**
  - Sidebar rows for blank notes get a muted-italic title and a
    discreet `empty` glyph in the snippet column. Selection still
    highlights cleanly without being garish.
  - Wikilinks now have three states instead of two: live (default),
    `cm-wikilink-empty` (muted + italic, points to a note that exists
    but is blank), and `cm-wikilink-broken` (red, points nowhere). Lets
    you see at a glance which stubs are still unwritten.
  - New query operator `empty:true` / `empty:false` filters the note
    list to just blank (or just non-blank) notes. Composable with the
    rest — `empty:true modified:<7d` finds recent stubs you started
    but didn't get back to.
- **Cmd/Ctrl+Shift+L "create new notes if needed" checkbox.** The
  link-suggestions modal can now materialize each suggested wikilink
  as an empty `.md` file in one shot instead of leaving you with red
  broken links to chase. Preference is persisted in `localStorage`;
  off by default to keep the original review-first flow intact.
- **Saved searches: dedicated Settings tab.** The shortcuts table is
  too dense to learn from — new "Saved searches" tab explains how
  `Cmd+S` works, how the `Cmd+1`–`Cmd+9` slots get assigned, and
  shows your current list with one-click activate + delete. The tab
  closes settings and runs the search when you click a name.
- **Settings tab persistence.** The active tab is remembered across
  sessions in `localStorage` (`malt.settings.tab`) — open settings and
  you land where you left off.
- **Sync conflict files auto-open their canonical sibling in the
  secondary pane.** Click a Dropbox `(conflicted copy …)` or Syncthing
  `.sync-conflict-…` row and the original opens beside it so you can
  diff and merge by eye, then delete the conflict file. The pattern
  detection lives in `canonicalNameFromConflict()` and matches both
  vendors.
- **AI tagger pivots from YAML frontmatter to inline hashtags.** The
  background tagger now appends a `#tag` line at the bottom of each
  note instead of writing a YAML `tags:` block at the top. Existing
  YAML tags are merged in and the YAML key is wiped — single source
  of truth is the canonical inline line, which the editor hides and
  surfaces as pills. Tagger is still off by default; toggle lives in
  Settings → AI. The AI section's label now reads "append inline
  #hashtags at the bottom of each note" so the behavior matches the
  copy.
- **Saved searches: no slot cap, drag to reorder, right-click for
  options.** Right-clicking a chip used to delete-with-confirm — now
  it opens a proper menu: Activate / Rename / Reorder / Delete. Drag
  any chip onto another to drop it into that position; the rest shift
  to accommodate. The Reorder action opens a small modal that takes a
  1-indexed position number for keyboard-only reordering. You can save
  more than nine searches now — extras live in Settings → Saved
  searches (full drag-and-drop list with rename + reorder + delete
  per row); the chip bar only shows the first nine that have keyboard
  slots. Backend: `saved_searches.rs` rewritten so the JSON array
  order is canonical and slots are derived from position; new
  `rename_saved_search` and `reorder_saved_search` IPCs.

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
