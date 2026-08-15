# Changelog

All notable changes to malt are documented here. Versioning follows
[semantic versioning](https://semver.org/) with pre-1.0 conventions: minor
bumps for meaningful feature batches, patch bumps for fixes.

## 0.5.7 — 2026-08-14

No more silently cut-off brews.

### Changed

- **Brew's output ceiling raised 1k → 8k tokens** (≈16k total for LM
  Studio with reasoning headroom) — a customized brew prompt that runs
  long no longer gets chopped mid-list.
- **Truncation is now an error, not a mystery.** When the server stops
  at a token limit (`finish_reason: "length"` — typically the model's
  loaded context length in LM Studio being too small for a long note),
  malt says exactly that, with the fix, instead of presenting a
  cut-off result as complete. For implement this also protects your
  note: a truncated revision can never reach the accept stage, where
  its missing tail would have read as a deletion.
- The LM Studio provider note now recommends loading models with a
  16k+ context length.

## 0.5.6 — 2026-08-14

Brew pane feel: quieter affordances, true in-place editing.

### Changed

- **No more tooltips over the checklist.** The check glyphs (and item
  text) dropped their hover tooltips — they overlaid the very text you
  were reading. The glyph now signals clickability with a soft gold
  ring on hover instead.
- **Editing keeps the row's exact layout.** Clicking a suggestion's
  text no longer collapses it into a single-line input — the row stays
  in place with identical wrapping and spacing, and the caret lands at
  the spot you clicked. Enter commits, Esc reverts, as before.

## 0.5.5 — 2026-08-14

### New

- **Brews survive restarts and updates.** Brew sessions — suggestions,
  your in-place edits, checkmarks — now persist to local storage, so
  `Ctrl/⌘+Shift+B` on a previously-brewed note reopens its session
  even after quitting or updating malt. Sessions also survive vault
  switches (they're keyed per note). The store keeps the 40
  most-recently-touched sessions; re-run still gets fresh suggestions
  any time.

## 0.5.4 — 2026-08-14

Batch implement: check several suggestions, apply them in one pass.

### New

- **Implement checked.** Every brew item's circle is now a checkbox:
  check any number of suggestions (AI or your own), and an "implement
  checked" bar applies them all in ONE revision pass with one diff
  review. Since implement re-emits the whole note regardless, five
  instructions cost the same as one — the answer to local models
  taking minutes per pass. Accepting marks every checked item done.
- **Done rows read as done.** After an accepted implement the item's
  button relabels to "again" (it stays re-runnable); clicking a done
  item's ✓ resets it.

## 0.5.3 — 2026-08-14

### New

- **Live progress while a revision streams.** The "revising…" bar now
  shows characters received and elapsed seconds, so a slow local model
  re-emitting a long note no longer looks stuck. Climbing count =
  working; stuck at "waiting for the model" = still prompt-processing
  or thinking (a genuine stall errors out via the 90s idle timeout).

### Changed

- **Implement's output budget now scales with the note** (~2× its
  size, floor 1024 / ceiling 8192 tokens) instead of a flat 8192 —
  a runaway local generation that never emits an end-of-text can no
  longer grind toward 16k tokens on a short note.

## 0.5.2 — 2026-08-14

Implement actually works now — verified end to end against a live
server — plus a model quick-swap list for local models.

### Fixed

- **Implement no longer cancels itself (for real this time).** The
  0.5.1 watcher fix was correct but incidental: the actual bug was a
  reactivity loop — the editor's note-load effect tracked the review
  state through a guard inside `loadPath`, so locking the editor
  re-triggered the load, which cancelled the review it was reacting
  to. `loadPath` is now called untracked. Verified end to end with an
  automated pipeline test against a local streaming server: revision
  streamed, diff rendered (with the cursor scrolled to the first
  change), accept applied and saved the revision byte-for-byte.

### New

- **Model quick-swap list.** Every model name you type into a
  provider's model field is remembered and shown as a clickable chip
  under the field — switch between your local models in one click, ×
  to forget one. Capped at 20 per provider; built-in suggestions
  aren't duplicated.
- **LM Studio leads the provider list.** Settings → AI now orders
  providers local-first: LM Studio, then Anthropic, then the hosted
  rest.

## 0.5.1 — 2026-08-13

### Fixed

- **Implement no longer cancels itself instantly.** The implement flow
  saves the note before locking the editor, and malt's own file
  watcher echoed that save back as an "external change" — which the
  review guard treated as a reason to abort. The guard now recognizes
  malt's own save echoes and only cancels a review when the file
  genuinely changed under it (a sync or another app).

## 0.5.0 — 2026-08-13

Brew grows up: from a read-only brainstorm into an editing cockpit.
Suggestions you can run, an inline diff to review, and your own
standing checklist.

### New

- **Implement any brew suggestion.** Every suggestion in the brew pane
  now carries an **implement** button. Press it and the AI applies
  that one instruction to your note — you review the result in the
  editor as an inline diff: removed text struck through and dimmed,
  added text in blue. Accept (`Ctrl/⌘+Enter`) applies and saves;
  cancel (`Esc`) restores the original byte-for-byte. The editor is
  locked while a revision streams and while you review, and the
  in-between diff preview can never be saved to disk.
- **Suggestions are proposals now, not just questions.** The brew
  prompt was rewritten so each item pairs its provocation with a
  concrete edit ("the ending trails off → replace the last paragraph
  with a concrete image") — ready to run as-is, or click any item's
  text to reword it first. Applied items check off and dim, and can be
  re-run.
- **Your own editing checklist.** The bottom of the brew pane takes
  standing instructions of your own — "remove passive verbs", "rename
  X to Y" — implementable on any note, editable and removable, and
  remembered per vault across sessions.
- **Brews are wed to their note.** Each note keeps its own brew
  session: `Ctrl/⌘+Shift+B` on a brewed note reopens its suggestions
  (with your edits and checkmarks) instead of re-running; re-run is
  the explicit fresh-ideas action. Navigating while the pane is open
  swaps it to the new note's session, and brewing from the secondary
  pane brings that note into the primary first so the source is
  always visible beside its brew.
- **The pane looks like a mode now** — a warm, gold-tinted background
  distinct from the editor, with suggestions rendered as a real
  checklist instead of raw markdown.

### Fixed

- **Brew "append" targeted the wrong note** when brew had been opened
  from the secondary pane. Appends now always land on the note the
  brew belongs to.

## 0.4.27 — 2026-08-13

Give the AI a house style, and a model-list refresh across every
provider.

### New

- **House styles from your own notes.** Tag any note `#prompt` and it
  appears in a dropdown in the steer box (`Ctrl/⌘+Shift+;`). The
  selected note's body rides along as a standing style guide —
  injected into the system prompt after the feature prompt, so the
  output contract stays primary — for both ghost completion and
  rewrite. The selection persists across sessions; the note is read
  fresh on every generation, so edits to a style note apply instantly.
  Keep one note per voice: "Noir", "Spec register", "Punchy marketing".
- **Tips for the recent features.** New tips cover
  write-while-thinking, house styles, and the LM Studio provider; the
  ghost-accept and model-picker tips caught up with current behavior.

### Changed

- **Model suggestions refreshed to the August 2026 lineups.**
  Anthropic gains Sonnet 5, Opus 5, and Fable 5 (Haiku 4.5 stays the
  cost-effective default); OpenAI moves to gpt-5.5 with the 5.4
  mini/nano tiers; Gemini jumps to the 3.x series; Grok to 4.5/4.6;
  the LM Studio qwen suggestion now names Qwen3.6. These are seeds and
  quick-pick chips only — any typed model name still works.

## 0.4.26 — 2026-08-12

Make "skip thinking" actually stick for newer Qwen models.

### Fixed

- **Skip thinking now uses both switches.** The `/no_think` soft switch
  moved from the system prompt into the user message (where Qwen
  documents it — system-prompt placement is reported ignored), and
  LM Studio requests additionally send
  `chat_template_kwargs: {"enable_thinking": false}`, the template-level
  switch newer Qwen releases respect. Neither is honored by every
  model/server combination; if thinking still shows up, the reliable
  fix is one line at the top of the model's prompt template in LM
  Studio itself: `{%- set enable_thinking = false %}`.

## 0.4.25 — 2026-08-12

Keep writing while the AI thinks. Built for local reasoning models,
where a completion can take 20-30 seconds — but every AI feature
benefits.

### New

- **Write anywhere while a completion is pending.** The insertion point
  now rides along as you edit elsewhere in the note, and the response
  streams into the original spot when it arrives — no more freezing in
  place waiting for the model. Only an edit that touches the insertion
  point itself (or the selected range, for a rewrite) dismisses the
  pending completion, since the model's context is stale there — and
  dismissal also cancels the generation.
- **Gestures, updated to match.** `Tab` and arrow keys accept only when
  the cursor is at the completion — elsewhere they're ordinary editing
  keys. `Ctrl/⌘+Enter` accepts from anywhere without yanking your
  cursor to the insertion; `Esc` declines from anywhere.
- **A visible thinking indicator.** The pending marker is now three
  pulsing dots in the accent blue instead of a faint grey ellipsis —
  findable at a glance, even from the far end of the note.
- **Skip thinking (Settings → AI → LM Studio).** Appends Qwen's
  `/no_think` soft switch to every LM Studio prompt, asking hybrid
  reasoning models to answer directly — much faster completions when
  the direct answer is good enough. Best-effort: models without the
  convention ignore it.

### Fixed

- **Failed generations say why, where you're looking.** When a
  generation fails, a brief inline `⚠` notice appears where the dots
  were (auto-dismisses after a few seconds) instead of the dots
  silently vanishing.
- **Empty responses are errors, not silence.** A stream that finishes
  without visible text — typically a reasoning model that spent its
  whole token budget thinking — now reports exactly that, with
  remedies. Reasoning headroom for LM Studio also doubled (4096 →
  8192 tokens) so it happens far less.

## 0.4.24 — 2026-08-11

LM Studio actually works now, including with local reasoning models
(gpt-oss, Qwen3-style thinkers).

### Fixed

- **Pasted endpoint URLs work as-is.** Copying the full URL from LM
  Studio's UI (`…/v1/chat/completions`) no longer doubles the path —
  the endpoint field accepts the base URL with or without the
  `/chat/completions` suffix or a trailing slash.
- **Reasoning models produce output instead of silence.** Thinking
  tokens count against the response cap, so tight caps ended replies
  mid-thought — TEST reported `ok — replied: ""` and completions showed
  dots, then nothing. LM Studio calls now get generous headroom for
  hidden reasoning (local tokens are free; hosted providers keep exact
  caps).
- **Inline `<think>` blocks never reach your note.** If a model emits
  Qwen-style `<think>…</think>` reasoning inline and LM Studio's
  reasoning-section parsing is off, malt now strips the leading block
  client-side — streaming-safe, even when the tags split across
  network chunks.
- **The TEST button no longer calls an empty reply "ok".** A response
  with no visible content now reports an error with a hint about
  reasoning-model token budgets.

## 0.4.23 — 2026-08-11

Hotfix for 0.4.22's LM Studio support.

### Fixed

- **LM Studio provider was unselectable.** Every command taking a provider
  argument (test key, set provider) rejected `lmstudio` because the Rust
  enum serialized the variant as `lm_studio`. The wire name is now pinned
  to the canonical `lmstudio` id (the old spelling is still accepted when
  reading existing config files), and a round-trip test guards every
  provider id against future drift.

## 0.4.22 — 2026-08-11

Run malt's AI on your own hardware: LM Studio joins the provider list.

### New

- **LM Studio as an AI provider.** Point malt at any machine running
  [LM Studio](https://lmstudio.ai)'s local server and every AI feature —
  ghost completion, rewrite, brew, two-pane prompting, auto-tag, link
  suggestions — runs on your own hardware. The endpoint is configurable
  in Settings → AI (defaults to `http://localhost:1234/v1`), so a LAN or
  Tailscale hostname works just as well as localhost: run the model on
  your desktop, write on your laptop. No API key required — the **test**
  button doubles as a connectivity check — though a key is still sent if
  you store one (say, for an auth proxy in front of the server). The
  model field must match an ID loaded in LM Studio (its server lists
  them under `/v1/models`); keep the model loaded to avoid cold-start
  timeouts on one-shot calls like auto-tag.

## 0.4.21 — 2026-06-18

A small editing-comfort release: control how wide the writing column is,
drop into a full-screen focus mode, and a fix so a locked note no longer
gets in your way.

### New

- **Set the width of the editor text.** A slider in the bottom-right caps
  the writing column to a comfortable measure and centers it, instead of
  letting prose run the full width of the window. Drag it all the way right
  for edge-to-edge (the previous behavior); your setting is remembered
  between launches.
- **Full-screen focus mode (`Ctrl/⌘+Shift+Enter`).** Collapses everything —
  note list, search, panes, backlinks, chrome — down to just the current
  note's text, with a minimal bar showing the word/character count and the
  width slider. `Esc` brings the workspace back. (Deeper than zen mode,
  `Ctrl/⌘+Shift+F`, which only hides the note list.)

### Fixed

- **A locked note no longer takes over the whole window.** The password
  prompt for an encrypted note was being drawn over the entire app, so you
  couldn't click another note or the search bar until you dealt with it.
  It's now confined to its own editor pane — navigate away to any other
  note freely; the encrypted one stays locked until you enter its password.
- **`Esc` in that password prompt no longer bounces you back to search.**
  For an encrypted note the password field stands in for the note's
  content, so there's nothing to escape to — the key is inert there now.

## 0.4.20 — 2026-06-13

A correctness-and-speed release: a second full code review, with every
finding fixed. The headline is felt immediately — **typing and search are
noticeably faster** — but most of the work is quieter: closing ways your
notes could be corrupted or lost, and hardening the AI and encryption
edges. (0.4.19 was a release-plumbing bump with no user-facing changes.)

### Faster

- **Typing, searching, and switching notes no longer re-read your whole
  vault from disk.** malt now keeps the active vault's notes in memory and
  updates only what actually changed on each edit, instead of re-reading
  and re-parsing every file on every keystroke (it was doing several full
  passes per keypress while you typed). On a large vault the difference is
  the gap between "laggy" and "instant" — the nvalt feel restored. Search,
  the sidebar, tag counts, backlinks, and the link graph all read from the
  same in-memory source now.
- **Semantic search, "related notes", and the near-duplicates report fill
  in within seconds of opening a vault**, not minutes. The embedding worker
  processes its whole backlog each cycle instead of one note at a time.
- Saves, unlocks, and the vault-wide reports no longer run on the UI's
  message thread, so they can't briefly freeze the window.

### Your notes are safer

- **A `#tag` next to non-English text in an inline `` `code span` ``
  can no longer corrupt the note — or break the app.** A subtle offset bug
  meant a multibyte character (an accented letter, a CJK character, an
  emoji) inside backticks could shift where malt thought your tags were:
  at best it mangled a word on save, at worst it crashed the note listing
  for the whole vault until you hand-edited the file. Fixed on both the
  Rust and editor sides, with tests.
- **A transient hiccup reading a config file can no longer wipe your
  vault list, pins, saved searches, or custom prompts.** Previously any
  failure to read one of these files was treated as a first run and
  overwritten with defaults; now a missing file seeds defaults, a corrupt
  file is set aside as a `.bak` (your bytes preserved) before starting
  fresh, and a momentarily-unreadable file is left completely alone.
- **Malformed YAML frontmatter is preserved instead of silently deleted.**
  If the block at the top of a note didn't parse, malt used to drop it on
  the next rename / auto-tag / link edit. It now round-trips untouched.
- **Deleting a note you just typed in no longer brings it back.** A note
  deleted within the autosave window could be recreated by a stray save to
  the just-removed path.
- **Stray temporary files from a hard crash are cleaned up** at startup and
  on vault switch, so they don't linger (and sync via Dropbox).

### Auto-tagging (when enabled)

- **No longer re-tags your entire vault on every launch.** It remembers
  what it has already tagged (persisted across restarts) and skips
  unchanged notes — previously every launch re-sent the whole vault to the
  AI, twice per note, churning modified dates and triggering sync
  re-uploads. *One-time note:* the first launch after updating re-checks
  each note once to build that memory, then goes quiet.
- **No longer overwrites edits you make while it's thinking.** If you (or a
  sync tool) change a note during the API call, the tagger now skips its
  write rather than clobbering your edit with a stale version.

### AI

- **Dismissing a suggestion actually stops it.** Pressing Esc on a ghost
  completion, accepting it early, re-running a brew, or moving on now
  cancels the request server-side, so the provider stops generating — and
  stops billing — instead of finishing a response you'll never see.
- **Long generations no longer truncate, stall forever, or show `�`.**
  Streaming responses are reassembled correctly across network chunks
  (no more replacement characters mid-word), abort cleanly if the
  connection goes silent, and surface a real error if the provider returns
  one mid-stream instead of ending as a deceptively short success.
- **OpenAI's current models (gpt-5 era) work**, and tag/wikilink
  suggestions now use the model you picked in Settings rather than always
  the provider default.
- **AI features can't read or transmit a file outside your vault.** The
  wikilink-suggestion and export commands are now scoped to the active
  vault like every other note command.

### Wikilinks, tags & the editor

- **`[[Target|Alias]]` links work.** They display the alias, resolve to the
  target, follow it on rename, and clicking one opens the right note
  (instead of creating a junk note named "Target-Alias").
- **The cursor no longer jumps when a tag relocates.** Typing a `#tag`
  above where you're working used to teleport the caret to the end of the
  note on the next autosave.
- **"Review wikilinks" (`Cmd/Ctrl+Shift+L`) wraps the right text** on notes
  that have frontmatter — the offsets it inserts now match the editor
  exactly.
- **Renaming a note by case only ("note" → "Note") works**, and no longer
  leaves a ghost duplicate in the list.
- Tag relocation and auto-tagging only tidy the lines they actually
  touched, so trailing-space hard breaks and intentional blank lines
  elsewhere in the note survive.
- A note that merely *mentions* `MALT-ENC-v1:` in its text is no longer
  mistaken for an encrypted file and rendered unopenable.

### Encryption

- **Unlocking a note is now an inline form in the editor pane, not a
  window-covering modal** — the note list, search, and the other pane stay
  fully usable while a note is locked, and you can always get back out.
- **You can unlock from the keyboard:** arrow to a locked note and press
  Enter (it used to require a mouse click on the row, and the empty editor
  swallowed the keystroke).

### Other fixes & polish

- **Settings → notes folder works again** — picking a folder repoints the
  active vault live, instead of writing a setting nothing read and snapping
  back. No restart needed.
- A restrictive Content-Security-Policy is now set on the app window
  (defense-in-depth for synced/AI-authored content).
- Switching or removing a vault is more robust: the file watcher always
  follows the active vault, and a hiccup reindexing one subsystem no longer
  leaves the others pointed at the wrong vault.
- Exports that append linked notes use each note's own `# H1` as its
  section heading (no doubled titles) and leave a clear
  *(encrypted note omitted)* marker instead of silently skipping a locked
  target.
- Backlinks and related-notes rows are keyboard-focusable; the related /
  unlinked-mention scans don't run while the panel is collapsed.
- Fuzzy search no longer over-matches short non-English queries.
- The welcome and quick-tour notes teach the correct AI shortcut
  (`Ctrl/Cmd+;`) and mention all five supported providers.

## 0.4.18 — 2026-06-02

- **"Reveal in file manager" now reliably opens the right vault's folder.**
  Both the note row's *Reveal in file manager* and Settings' notes-folder
  *reveal* button were spawning `explorer.exe` / `open` directly from the app
  process, which was unreliable (often nothing appeared, especially for vaults
  outside the default location). They now route through the opener plugin's
  native shell APIs (Windows `SHOpenFolderAndSelectItems`, macOS Finder
  reveal), so reveal lands you in the active vault's folder — with the note
  selected — every time.

## 0.4.17 — 2026-06-02

- **Losing focus also finalizes an in-progress tag.** Extends 0.4.16: a
  half-typed `#tag` is now committed (filed to the canonical line, shown as a
  pill, and saved) the moment the editor loses focus — whether you click to
  another part of the app (search bar, a pill, a modal, the other pane) or
  switch to a different application window entirely. So a tag finalizes on any
  of: a following boundary, the caret moving off it, **or** focus leaving the
  editor.

## 0.4.16 — 2026-06-02

- **Hashtag pills stop forming mid-word.** A `#tag` is no longer finalized
  while you're still typing it. The pill row and the relocate-to-bottom pass
  now treat a hashtag as "in progress" whenever the caret is inside it or at
  its trailing edge, and only commit it once a boundary follows (a space,
  newline) **or the caret moves off it**. Previously a well-timed autosave
  could file a half-typed tag (`#app`) to the hidden line, jump the caret to
  the body, and strand the rest of the word. Inline tag coloring still updates
  live as typing feedback.

## 0.4.15 — 2026-06-02

- **Finalized tags can no longer be corrupted by typing.** Three leaks in
  the hidden canonical-tag-line machinery are fixed:
  - Adding a second tag at the end of a note no longer merges it into the
    first (`#foo` + `#bar` → `#foo#bar`). After tags relocate to the hidden
    bottom line, the caret is now parked at the end of the *visible* body
    instead of inside the hidden line, so the next keystroke can't land in
    a tag.
  - Backspacing at the end of a note can no longer "re-open" a finalized
    tag into editable text. The protected span now covers the tag line *and*
    its separator newline (matching exactly what's hidden), closing the
    one-character gap that let a deletion collapse the line back into view.
  - The pill's **×** remove button now works on tags that have already been
    filed to the bottom line (previously the edit was silently dropped by
    the tag-line guard). Removing a tag persists immediately.
  - Net effect: once a tag exists it's only editable/removable via its pill,
    never by editing the note text — as intended.

## 0.4.14 — 2026-05-31

- **"Check for Updates" stops crying wolf.** When no GitHub release is
  published yet (drafts only) the updater can't fetch a manifest — that's
  expected, not a failure. It now reports *"no published release yet —
  you're on the newest build"* instead of a red "Could not fetch a valid
  release JSON" error. Real failures (offline, GitHub down) still show as
  errors. Updates will work normally once a release is published.

## 0.4.13 — 2026-05-31

- **Two-pane prompting now strips tag markup before sending.** Both panes
  are run through the same AI-hygiene pass as every other AI path
  (drops `#hashtags` + the canonical tag line, unwraps `[[wikilinks]]`),
  so the model never sees malt's markup and can't echo or hallucinate
  `#tags` into the inserted reply. Still no prompt scaffolding otherwise.

## 0.4.12 — 2026-05-31

- **Two-pane prompting (Cmd/Ctrl+Shift+').** With the split open, this
  sends the OTHER pane's content as a raw pre-prompt *before* the focused
  pane's content — concatenated, with **no system prompt or scaffolding**,
  just the two editors' text — and streams the reply as ghost text into
  the focused note (Tab/Enter to keep, Esc to drop). Notes become reusable
  pre-prompts: keep instructions in one pane, your material in the other.
  Works across all AI providers; no-ops when there's no second note pane.

## 0.4.11 — 2026-05-31

- **First-line `# H1` becomes the note's display name.** When a note
  starts with a top-level heading, the sidebar card, the split-pane title
  bars, and the window title show that heading instead of the filename.
  Search-match highlighting follows the displayed name. The filename is
  still the note's identity — wikilinks, rename, and history are
  unchanged — so this is purely what you read.

## 0.4.10 — 2026-05-31

To-do list, batch 4 (the last two) — finishes the list.

- **Clickable task checkboxes.** `- [ ]` / `- [x]` markers (also `*`, `+`,
  `1.` lists) render as checkboxes you can click to toggle. The cursor
  reveals the raw `[ ]` when you're editing that line, and it's plain
  markdown underneath — your task lists stay in the file.
- **Bottom status bar.** The AI-key dot, "indexing…" pill, and "saved"
  flash moved out of the top toolbar into a slim bar along the bottom of
  the window, out of the way of the note count + sort controls.

## 0.4.9 — 2026-05-31

To-do list, batch 3 (pinning + cross-vault move — shared right-click menu).

- **Pinned notes.** Right-click → *Pin to top*. Pinned notes bubble to the
  top of the list and stay visible even when a text search would filter
  them out (they're skipped in `~`/`is:` report result sets, where they'd
  be noise). Subtle amber card tint + 📌 badge. Pins persist in config,
  follow renames, and clear on delete.
- **Move to another vault.** Right-click → *Move to vault → …* relocates
  the `.md` file into another vault's folder (collision-safe). The note's
  embedding is dropped from this vault and any pin cleared. Note: links
  don't follow across vaults — they're siloed by design.

## 0.4.8 — 2026-05-31

To-do list, batch 2 (layout + a vault-attach fix).

- **Resizable note list.** Drag the divider between the note list and the
  editor to set the sidebar width (200–700px); persisted across launches.
- **Zen mode (Cmd/Ctrl+Shift+F).** Collapse the note list so the editor
  fills the window. Toggle to bring it back — nothing is lost.
- **Attaching/switching a vault no longer flashes "No notes yet."** While
  the new vault reindexes, the sidebar shows "opening vault — indexing…"
  instead of the misleading empty-state CTA.

## 0.4.7 — 2026-05-31

- **A plain click on a note always opens it single-pane in the primary
  editor.** Removed the surprise auto-split: clicking a sync-conflict file
  used to auto-open its original in the second pane (no modifier needed,
  only for some notes — so it read as an intermittent bug that split the
  view). Gone. A plain click now also collapses any stray secondary pane,
  so you can't get stuck in a split. Cmd/Ctrl+click still opens a second
  pane deliberately; the ⚠ conflict badge still flags conflict files.

## 0.4.6 — 2026-05-31

To-do list, batch 1 (quick wins).

- **`is:encrypted` report.** Lists every password-locked note (mirrors
  the other `is:` lenses). Status badge + empty state + docs.
- **Double-click a saved-search chip to edit it.** Same rename / delete /
  reorder menu as right-click — handy on trackpads.
- **No more "indexing…" flicker on every keystroke.** The embedding model
  re-attempted loading on every autosave-triggered re-embed; if the model
  couldn't load (offline / download not finished), that flashed the status
  pill on each keystroke. Failed loads now back off (retry at most once
  every 2 min) so a broken model stays quiet. (If you see this a lot, your
  local embedding model isn't loading — semantic search / related /
  near-dupes won't work until it does.)

## 0.4.5 — 2026-05-31

Integration hardening — making malt safe to share a folder with another
program writing it concurrently (surfaced by a pre-integration review).

- **Atomic writes everywhere.** Every note write (save, encrypt/decrypt,
  password change, create, the rename cascade, auto-tag, link-mention)
  now stages a temp file and atomically renames it into place. A
  concurrent external reader never catches a truncated or half-written
  note — it sees the old complete file or the new one, never garbage.
- **No more echo writes.** Merely *viewing* a note another tool just
  edited no longer relocates its tags and writes malt's version back
  over it. Internal reloads + the tag-relocation pass are excluded from
  autosave; only real user edits write.
- **Rename detection won't mispair duplicates.** The external-rename
  cascade now fires only when the moved file's content is globally
  unique on both sides, so templates/stubs/boilerplate with identical
  content can't be misread as a rename and rewrite the wrong links.
- **Conflict-reload race guard.** Overlapping external-change handlers
  (sync storms) now carry a generation token, so a slow earlier disk
  read can't apply its result after a newer one.

## 0.4.4 — 2026-05-29

"Plays nice with other tools" batch — making malt safe as the editor over
a Markdown folder another system writes to, without either side stepping
on the other.

- **Conflict-safe external reload.** If a file changes on disk while the
  editor buffer has unsaved edits, malt no longer silently overwrites
  your work. A small bar appears — *keep mine* / *use theirs* — and
  autosave pauses until you choose. A clean buffer still fast-forwards to
  the new on-disk version live, as before. No cooperation required from
  the other writer.
- **External-rename cascade.** Rename a note *outside* malt (a script, a
  file manager, a sync tool) and malt now detects it and rewrites every
  `[[wikilink]]` pointing at the old name across the vault — the same
  atomic fix-up the in-app rename does. Detection is by content
  fingerprint, so it survives the delete+create event soup sync tools
  produce. (Limitation: if the rename coincides with a content edit in
  the same instant, the fingerprint won't match and links aren't
  rewritten.)
- **Coalesced reindexing.** The file watcher now collapses a burst of
  writes into a single reindex (quiet-period debounce, capped so a
  continuous stream still flushes a few times a minute). A tool
  batch-writing hundreds of files no longer triggers hundreds of
  rebuilds — important for a folder under heavy programmatic write load.

## 0.4.3 — 2026-05-29

Tag flair + a frontmatter-safety fix that makes malt a trustworthy editor
over a folder another tool also writes to.

- **Tag flair.** Give any tag an icon and an accent color in Settings →
  Tags & queries → tag flair. Every note *card* carrying that tag wears
  it — colored title, subtle tint, and a glyph before the name — so a
  flat folder mixing content types (e.g. `#element` / `#pitch` /
  `#story`) reads at a glance. When a note has several styled tags, the
  first listed wins the card color. The editor is deliberately left
  untouched. Stored per-install in config; applies across all vaults.
- **Frontmatter is now preserved verbatim.** Previously malt's YAML
  model only understood `tags`, so the rename, link-mention, and
  auto-tag paths — which rebuild a file from that model — silently
  dropped any other frontmatter key. Now every unknown key (`id`,
  `status`, image refs, anything an external system stores) round-trips
  intact. This makes malt safe to use as the prose editor over a
  Markdown folder that some other tool treats as a source of truth.

## 0.4.2 — 2026-05-29

Discovery reports + serendipity batch. Four new lenses for finding the
notes that normal search never surfaces.

- **The Orphanage (`is:orphan`).** Lists notes adrift from the link
  graph — no resolving `[[wikilink]]` out *and* no backlinks in. The
  stranded thoughts most in need of being woven in (or pruned). Skips
  encrypted notes, whose link structure is unknowable by design.
- **On This Day (`is:onthisday`).** Surfaces notes from this calendar
  day in years past — by an explicit `YYYY-MM-DD` title date when
  present, else the file's last-modified date. Today is excluded;
  most-recent-first. Computed in local time so it matches how daily
  notes are dated.
- **Near-duplicates (`is:duplicate`).** Flags notes with a near-
  identical twin (~0.9+ cosine similarity) — re-typed captures, forked
  drafts, double-pasted clippings. Runs the embedding KNN on a
  background worker so even a huge vault never stalls the UI.
- **Random note (Cmd/Ctrl+Shift+R).** Jumps to a random note in the
  vault — a serendipity engine for rediscovering what you've buried.
  Avoids re-landing on the note you're already reading.
- **Three new built-in saved searches.** The Orphanage, On This Day,
  and Near-duplicates join Empty Notes as built-in, reorderable,
  un-deletable saved-search chips — fresh installs land with all four
  bound to Cmd/Ctrl+1–4 as a guided tour of the report lenses.
- **Fix: Cmd/Ctrl+D now moves the editor to the new daily note.** The
  daily-note command cleared selection but an active search filter
  could snap it back to a visible row; the query is now cleared first.

## 0.4.1 — 2026-05-29

Refinement batch.

- **Steer the AI (Cmd/Ctrl+Shift+;).** A one-line modal lets you aim a
  generation — "make it darker", "pivot to the counterargument",
  "shorter" — injected as a `<direction>` note into the same
  completion/rewrite. Works across all providers; the default
  completion + rewrite prompts explain the tag.
- **Daily note no longer puts #journal in your way.** The seeded tag
  now lives on its own hidden canonical line with the cursor on an
  empty first line, ready to type.
- **Hidden tags can't be deleted by accident.** Backspacing past
  trailing newlines at the end of a note used to silently eat into
  the invisible tag line. A change-filter now protects those tag
  characters (the relocate transform + external reloads bypass it),
  and arrow/word motion skips the hidden region.
- **Unlinked-mention scan is built for scale.** Instead of reading
  every note on each note-open, the tantivy index narrows to
  candidate notes containing the title (a phrase query), and the
  precise check runs on a background worker so the UI never stalls —
  O(matches), not O(vault). Ready for vaults in the tens of thousands.

## 0.4.0 — 2026-05-28

Discovery + journaling batch (built overnight on the v0.3.3 review).

- **Semantic search (`~concept`).** A leading `~` switches the search
  bar to embedding-similarity ranking — "find what I meant, not the
  words I typed." Local model, per-vault, fully offline. Status line
  shows a "~ semantic · N near" badge; Enter opens the top result
  (never creates a `~…`-named note).
- **Unlinked mentions.** The linkbacks panel now surfaces notes that
  name the current note in plain prose without a `[[link]]` — ranked
  by mention count, with a one-click "link" that wraps the first
  occurrence in a wikilink (casing preserved, verify-before-mutate so
  it can't corrupt a file). Latent structure, surfaced.
- **Daily note (Cmd/Ctrl+D).** Opens (or creates) today's dated note
  (YYYY-MM-DD), seeded with `#journal` so days accumulate into a
  searchable journal. Toggle the tag in Settings → General.
- **Tag co-occurrence.** Filter by a single `tag:foo` and an "often
  with" row appears — the tags that most share notes with it, each a
  click away from narrowing to both. A map of your own structure.
- **Per-vault embedding DBs.** Each vault now has its own embedding
  store (`embeddings/vault-<hash>.db`), so Related Notes and semantic
  search are fully siloed by construction — no cross-vault leakage.
  Removing a vault reclaims its embedding file; switching repoints the
  connection live.
- **Encryption: cached key + salt reuse.** Re-saving an encrypted note
  reuses its salt and a process-wide derived-key cache, so autosave is
  a fast AES-GCM pass instead of a fresh (slow) Argon2 run. Password
  cache clears on vault switch.
- **Streaming no longer truncates.** Removed the 30 s total-request
  timeout from both AI clients' streaming paths (it was killing long
  brews mid-sentence); connect-timeout only now.
- **Provider-agnostic prompts + output sanitizer.** Prompt
  descriptions de-Claude'd and updated for the Cmd+; binding; ghost
  completions strip stray code-fences / wrapping quotes at accept-time
  so non-Anthropic models stay clean.
- **Housekeeping.** Removed orphaned `complete_text` /
  `set_completion_model` IPCs; type-check + cargo build are
  warning-free.

## 0.3.3 — 2026-05-28

Review/hardening pass on the v0.3.x feature surface.

- **Related Notes no longer leak across vaults.** The embedding store
  is a single shared SQLite file keyed by absolute path, and nothing
  purged it on vault switch — so `find_related` could surface another
  vault's note titles/snippets, breaking the siloing promise. Fixed
  by over-fetching KNN candidates and filtering to the active vault's
  path prefix in `find_related`. (A per-vault DB file is the
  fully-correct long-term fix; this kills the user-visible leak with
  no migration.)
- **OpenAI-compat streaming is more robust.** The SSE parser split
  only on `\n\n`; a gateway framing events with `\r\n\r\n` would have
  buffered forever and emitted nothing. Now normalizes CRLF→LF and
  accepts both `data: ` and `data:` (no-space) prefixes — covers
  proxies in front of OpenAI/DeepSeek/Grok/Gemini.
- **Type checker is finally clean (0 errors).** Resolved the
  long-standing `@replit/codemirror-vim` `handleKey` type mismatch
  with a documented cast — the CM6 build accepts the `EditorView` at
  runtime, the bundled types just describe the legacy adapter. Also
  removed a dead `.tip-close` CSS selector.

## 0.3.2 — 2026-05-28

- **Multi-provider AI.** New `openai_compat.rs` shared client covers
  OpenAI, DeepSeek, xAI Grok, and Google Gemini's OpenAI-compat
  endpoint with one code path (same `/chat/completions` shape, same
  SSE deltas — only base URL + key + model differ). Anthropic stays
  on its existing `/v1/messages` client. New `providers.rs` registry
  with `Provider` enum + per-provider defaults (model + suggested
  picks + base URL + one-line note). Per-provider key in keychain
  (one slot per provider — keep them all configured and switch on
  demand). Active provider lives in config; ghost completion,
  rewrite, brew, auto-tag, and AI link-suggestions all dispatch
  through it. Defaults seeded for May 2026: `gpt-5`,
  `gemini-2.5-flash`, `deepseek-v4-flash`, `grok-4.3`.
- **Settings → AI rebuilt as provider cards.** Each provider gets a
  card with: active-radio, label + key-status badge, one-line
  capability note, key field (with save / test / clear), model
  text-input plus suggested-pick chips. Editing the model field
  saves on blur. Auto-tag toggle moves to its own section since it
  now uses whichever provider is active.
- **Vault settings tab.** Rename in place, switch, or remove (files
  on disk are untouched — only malt's awareness of the folder is
  unlinked). The last vault can't be removed; the +page picker
  modal stays for fast keyboard-driven switching.
- **Vault switch no longer leaves the old note in the editor.** The
  auto-select effect was reading `notes[0]` from the stale list
  before the IPC came back and handing the editor a file that still
  existed on disk. Fix: clear `selectedPath` / `secondaryPath` /
  `rawResults` / `allNotes` / history BEFORE invoking the switch,
  then await both `refreshAllNotes()` and `performSearch()` so
  auto-select fires with the new vault's notes.
- **Cross-vault back/forward.** Each per-pane history entry now
  carries `{ path, vaultPath }`. `Cmd+[` / `Cmd+]` to an entry in a
  different vault switches vaults first (re-pointing the watcher,
  reindexing, refreshing the sidebar) before landing on the note.
  Handy for tab-style navigation across siloed corpora.
- **"Start writing →" actually creates a note now.** Used to clear
  the query and re-focus the search bar, which read as "did nothing"
  when the search bar was already focused. Now it creates an
  Untitled note in the active vault and drops you straight into the
  editor.
- **Updater note.** Still operational, not a code bug — v0.2.10 (and
  v0.2.5 if you want it discoverable) remain drafts on GitHub.
  `/releases/latest` 404s until at least one release is published.

## 0.3.1 — 2026-05-28

- **Vault switching (Cmd/Ctrl+Shift+V).** New `vaults.rs` registry:
  a list of named notes-folders, exactly one active at a time. Active
  vault's name shows as a chip at the bottom of the sidebar; click it
  for a dropdown of every vault. ⌘⇧V opens a filterable picker.
  "Add vault" wizard lets you point at any folder (creates it if
  missing). Switching repoints the file watcher live + rebuilds the
  search index, backlinks, and embedding queue; no restart needed.
  First-launch migration: pulls the legacy notes-dir into a single
  "Default" vault so existing installs keep their notes.
- **AI ghost moves from Cmd+J → Cmd+;.** Cmd+J turned out to be
  swallowed by the Tauri webview on macOS (and is a download
  shortcut in some browsers) — pressing it did nothing. Cmd+; is
  safe across browsers, OSes, and Tauri's webview, and stays close
  to the home row. Settings shortcut table + tip bank + on-screen
  labels updated.
- **Brew pane gains "save as note" + explicit close.** Inline form
  with a "link back to <source>" checkbox (default on). Saving
  creates a new note in the active vault titled "Brew of <source> —
  <date>", optionally prepended with `From [[source]]`, and
  navigates the primary pane to it.
- **Title-as-H1 prepended to AI prompts.** The auto-tagger and the
  brew prompt now both see the note as `# {title}\n\n{body}` instead
  of body-only. Crucial context for short / fragmentary notes —
  without it the model is guessing what the document is *about*.
- **Markdown rendering supports `*italic*`, `__bold__`, and nested
  bold/italic.** Old regex only handled `**bold**` and `_italic_`.
  New regexes capture either pair and run as two independent
  passes, which lets `**a *b* c**` decorate cleanly (italic inside
  bold via CodeMirror's nested mark spans). Boundary checks prevent
  `**foo**` from mis-matching as italic-of-"foo".
- **Updater note (still operational).** Both v0.2.5 and v0.2.10 on
  GitHub are still drafts; `/releases/latest` therefore 404s. Click
  **Publish release** on the v0.2.10 draft (and v0.2.5 if you want
  it discoverable) to make the updater see them. v0.3.1+ follows the
  same path once published.

## 0.3.0 — 2026-05-28

The "brew" half of the tagline arrives.

- **Brew Ideas (Cmd/Ctrl+Shift+B in the editor).** Streams a
  brainstorm from Claude into the secondary pane: three sections —
  threads to pull, where this connects, a few sharper framings.
  Read-only view with `re-run` / `copy` / `append` actions. The
  "append" button folds the brew into the source note under a
  `## Brew — [timestamp]` heading. Background: AI gets the note body
  (with malt-private markup stripped) plus a tight prompt that asks
  for scannable, action-oriented brainstorming and tells it to bail
  with a one-line message if the note is too thin.
- **Bold and italic markdown shortcuts.** ⌘B wraps the selection (or
  the word at the cursor) in `**…**`; ⌘I wraps in `_…_`. Toggling
  off works either way — the action detects existing wrappers and
  strips them.
- **WYSIWYG markdown rendering for bold / italic.** A new
  Prec.highest decoration plugin styles `**X**` and `_X_` spans
  visually (font-weight / font-style) and hides the `**` / `_`
  markers when the cursor isn't touching the span. Settings →
  general → "**/_ markers" toggle disables the hiding for plain-text
  purists. Same Prec trick as wikilinks: the markdown highlighter's
  inner span would otherwise win the visual.
- **AI ghost moved from Cmd+I to Cmd+J.** The italics-on-Cmd+I
  convention is too entrenched in markdown editors to ignore;
  hijacking it for AI was a non-starter. Cmd+J takes over AI
  continue / rewrite / re-roll. Settings shortcut table + tip bank
  + on-screen labels all updated.
- **Prompts management (Settings → Prompts).** Every system prompt
  malt sends to Claude is now listed, editable, and resettable per
  prompt. User overrides live in
  `~/.config/malt/prompts.json`; defaults ship in the binary so
  `reset to default` always works. Backend factored out into a
  dedicated `prompts.rs` module with a `PromptKey` enum;
  `ai.rs` reads via `prompts::get(...)` instead of compile-time
  constants. Covers tag / entities / completion / rewrite / brew
  out of the gate.

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
