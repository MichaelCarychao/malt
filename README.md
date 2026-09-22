# malt

> **Distill notes. Brew ideas.**

An open-source, keyboard-first writing app where you and a language model
(cloud or local) take turns, one paragraph at a time. A spiritual successor to
[nvALT](https://brettterpstra.com/projects/nvalt/): plain `.md` files, instant
type-to-filter search, and an AI layer that proposes continuations, tags, and
wikilinks without ever blocking the writer.

malt is for people who want the speed of nvALT with the connective tissue of
Roam/Obsidian/Tana, without locking their notes into a proprietary database.

![malt showing a sample worldbuilding vault — the note list, the editor with wikilinks and tags, and the backlinks / related-notes panel](docs/screenshot.png)

<sub>*A sample vault with invented notes — your notes stay plain `.md` files in a folder you choose.*</sub>

## Writing with a model, one paragraph at a time

I built malt to use AI in my fiction without losing the part where I'm the one
writing. Letting a model outline and draft can get you a novel draft in an
afternoon, but the resonance is off; that part still takes human aesthetic
judgment. So malt keeps the model to one short turn at a time:

1. **Write a paragraph.**
2. **Press `Cmd/Ctrl+;`** — the model proposes what comes next as ghost text:
   a few sentences, a short paragraph at most.
3. **Keep it, reshape it, or wave it off.** `Tab` (or an arrow key) accepts at
   the cursor, `Cmd/Ctrl+Enter` accepts from anywhere, `Esc` dismisses,
   re-pressing `Cmd/Ctrl+;` re-rolls. You can keep writing elsewhere while it
   thinks — the suggestion streams in behind you.
4. **It's your turn again.** Press `Cmd/Ctrl+Shift+;` to steer the next turn
   with a one-line direction ("make it darker", "pivot to the counterargument")
   — or pick a standing *house style* from any note you've tagged `#prompt`.

The paragraph is the unit of collaboration: big enough for the model to
surprise you, small enough to read every word, keep what belongs, and steer
what comes next. The author always has the next move.

The same philosophy runs through the bigger tools — select a passage and
`Cmd/Ctrl+;` rewrites just that; *Brew* turns a note into a checklist of
concrete edit suggestions you can apply one (or several) at a time, each
reviewed as an inline diff you accept or cancel. The model proposes; you
dispose.

## What's inside

- **Plain markdown, plain folder.** Your notes are `.md` files in `~/malt/`
  (configurable, e.g. point it at a Dropbox folder). No `.app/data/` or sqlite
  vault to extract from later. Delete malt tomorrow and your notes are still
  notes.
- **Instant search.** Tantivy-powered fuzzy + prefix filter on every keystroke.
  Real nvALT feel: the search bar both filters and creates (type a title, press
  Enter).
- **Wikilinks + backlinks.** `[[link]]` with autocomplete, broken-link styling,
  atomic rename + back-reference rewrite. A backlinks panel shows what links
  here, plus "unlinked mentions" (notes that name this one in prose) you can
  wire up in one click.
- **Tags as search shortcuts.** Inline `#hashtags` relocate to a hidden
  canonical line and render as pills. Vocabulary autocomplete, hierarchical
  `#a/b` tags, and optional per-tag **flair** (icon + color on note cards).
- **Saved searches + discovery reports.** `Cmd/Ctrl+S` to save a query,
  `Cmd/Ctrl+1..9` to recall. Operators compose: `tag:foo -tag:bar modified:<7d`.
  Built-in lenses: `is:orphan` (notes adrift from the link graph), `is:onthisday`,
  `is:duplicate` (near-identical twins, via embeddings), `is:encrypted`.
- **Semantic search & related notes.** A leading `~` ranks by meaning, not
  keywords. Related notes appear beside backlinks. All local, offline, per-vault
  — [fastembed-rs](https://crates.io/crates/fastembed) + sqlite-vec, no cloud.
- **Split panes + two-pane prompting.** `Cmd/Ctrl+click` a row or link to open
  the second pane. `Cmd/Ctrl+Shift+'` sends the other pane as a raw pre-prompt
  for the focused one — turn a note into a reusable AI prompt.
- **AI assistance** (optional — a local model, or bring your own key for
  Anthropic, OpenAI, Gemini, DeepSeek, or Grok):
  - `Cmd/Ctrl+;` — ghost-text continue at the cursor; with a selection, rewrite it.
  - `Cmd/Ctrl+Shift+;` — *steer*: a one-line direction, plus reusable house
    styles from `#prompt`-tagged notes.
  - `Cmd/Ctrl+Shift+B` — *Brew*: brainstorm a checklist of concrete edits for
    the current note; **implement** any of them (or several at once) and review
    the revision as an inline diff — strikeouts for what leaves, color for what
    arrives — before accepting. Each note keeps its brew session across
    restarts; add your own standing checklist items ("remove passive verbs").
  - `Cmd/Ctrl+Shift+L` — review proposed `[[wikilinks]]` (title matches + entities).
  - Optional background auto-tagging (off by default).
- **Local models, first-class.** The LM Studio provider works out of the box
  (`http://localhost:1234/v1`, no API key), and the endpoint field takes any
  OpenAI-compatible server — Ollama, llama.cpp, vLLM, or a box across your
  LAN/Tailscale. A *skip thinking* toggle asks reasoning models to answer
  directly, and every model you try is remembered as a one-click quick-swap
  chip. **With a local model, nothing leaves your machine.**
- **Per-note encryption.** AES-256-GCM + Argon2id, one note at a time. The file
  stays a single `MALT-ENC-v1:` line so sync tools keep working.
- **Daily notes, pinned notes, zen mode, task checkboxes, random note,** multiple
  **vaults,** and per-note **exports** (`.md` / `.html` / `.epub` / `.txt`).
- **Keyboard-first, dark, dense.** Every shortcut is documented in
  Settings → Shortcuts, and the in-app **Tips** (Settings → Tips) walk you
  through the surface.

See [CHANGELOG.md](CHANGELOG.md) for the full per-release feature list.

## Your notes are yours

malt never holds your content hostage. Notes are plain `.md` files you can read,
edit, sync, grep, or back up with any tool. Everything malt derives — the search
index, embeddings, config, pins — lives in a sidecar config directory, never
mixed into your notes folder.

**What the AI features send.** Nothing is sent anywhere until you invoke an AI
action. When you do, malt sends the current note (continuation and rewrite send
it split around your cursor or selection; Brew and wikilink suggestions send the
note body; two-pane prompting sends both panes) to whichever provider you
configured — hashtag markup is stripped first. Background auto-tagging sends
note content on a schedule, which is why it's **off by default**. API keys are
stored in your OS keychain (macOS Keychain / Windows Credential Manager), never
in config files. With a local model, all of this stays on your machine.

> ⚠️ **Encryption has no recovery.** Encrypted notes are protected by your
> password and nothing else — there is no backdoor or reset. Lose the password,
> lose the note. Keep important passwords backed up somewhere safe.

## Install

Pre-built installers ship from the [releases page](../../releases) — Windows
`.msi`/`.exe` and macOS `.dmg` (Apple Silicon). **Linux and Intel Macs:** build
from source (below); untested there, but nothing in malt is
architecture-specific. Builds are **unsigned**, so the OS will warn on first
launch:

- **Windows:** SmartScreen → "More info" → "Run anyway".
- **macOS:** right-click the `.app` → "Open" the first time (a plain
  double-click is blocked by Gatekeeper).

Once installed, **Settings → About → Check for Updates** uses a signed,
self-updating channel (the update payload is cryptographically verified even
though the installer itself is unsigned). Code-signing proper is a someday-maybe.

## Build from source

Prerequisites:

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 20+
- Platform Tauri prerequisites:
  [Windows](https://v2.tauri.app/start/prerequisites/#windows) /
  [macOS](https://v2.tauri.app/start/prerequisites/#macos) /
  [Linux](https://v2.tauri.app/start/prerequisites/#linux)

```bash
git clone https://github.com/MichaelCarychao/malt.git
cd malt
npm install
npm run tauri dev      # dev mode, hot reload
npm run tauri build    # release installer
```

The first build is slow (~5–15 min — fastembed + tantivy + onnxruntime pull in
a lot). Subsequent builds are fast. Before sending a patch, `npm run check` and
`cargo check --manifest-path src-tauri/Cargo.toml` should both be clean.

## Support & contributing

**Status:** malt is a personal project I use daily and develop in the open.
It's shared as-is, with no roadmap or support commitments — treat it like
someone's lovingly-maintained dotfiles, not a company's app. See
[SUPPORT.md](SUPPORT.md).

Questions and show-and-tell go in [Discussions](../../discussions); the
community helps each other there. Patches are welcome on the terms in
[CONTRIBUTING.md](CONTRIBUTING.md) (small PRs, MIT, possibly slow or no review).

## Lineage

malt descends from [nvALT](https://brettterpstra.com/projects/nvalt/) and
[Notational Velocity](http://notational.net/) before it — the apps that proved
a notes tool could be one search box and zero friction. The story behind malt:
[michaelcarychao.com/work/malt](https://michaelcarychao.com/work/malt/).

## License

[MIT](LICENSE) — do what you like; no warranty.
