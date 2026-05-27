# malt

A cross-platform AI-augmented [nvalt](https://brettterpstra.com/projects/nvalt/)
successor. Plain `.md` files in a single flat folder, instant type-to-filter
search, and a silent AI layer that proposes tags, wikilinks, and continuations
without ever blocking the writer.

malt is for people who want the speed of nvalt with the connective tissue of
Roam/Obsidian/Tana, without locking their notes into a proprietary database.

## What's inside

- **Plain markdown, plain folder.** Your notes are `.md` files in `~/malt/`
  (configurable). No `.app/data/` or sqlite vault to extract from later.
- **Instant search.** Tantivy-powered fuzzy + prefix filter on every
  keystroke. Real nvalt feel.
- **Wikilinks.** `[[link]]` with autocomplete, broken-link styling, atomic
  rename + back-reference rewrite, brackets hidden in render until you
  cursor-into the link.
- **Tags as search shortcuts.** Inline `#hashtags` with a vocab-vs-ad-hoc
  visual gradient. Tags relocate to a canonical line at the bottom of the
  file (hidden in render). Tag pills above the editor for quick filter / remove.
- **Saved searches.** Cmd/Ctrl+S to save current query, Cmd/Ctrl+1..9 to
  recall. Operators: `tag:foo`, `-tag:foo`, `modified:<7d`.
- **Split-pane editor.** Cmd/Ctrl+click any link or sidebar row → opens in
  the second pane. Cmd/Ctrl+W to close.
- **Semantic related notes.** Local embeddings (no cloud) via
  [fastembed-rs](https://crates.io/crates/fastembed). Shows top-5 cosine-similar
  notes alongside the explicit backlinks.
- **AI assistance** (optional, BYO Anthropic API key):
  - Ghost-text continuation at cursor (Cmd/Ctrl+Space)
  - Selection rewrite that unpacks generalities
  - Cmd/Ctrl+Shift+L: post-hoc review of proposed wikilinks for the current
    note — both deterministic title matches and AI-extracted entities
- **Exports.** Per-note `.md` / `.html` / `.epub` / `.txt`, clipboard
  variants, optional "append linked notes" for TOC-style sharing.

See [CHANGELOG.md](CHANGELOG.md) for the full feature list per release.

## Install

Pre-built installers ship from the
[GitHub releases page](../../releases) — Windows `.msi` and macOS `.dmg`.
Both are currently unsigned, so:

- **Windows**: SmartScreen will warn on first run. Click "More info" → "Run anyway".
- **macOS**: right-click the `.app` → "Open" the first time (regular
  double-click is blocked by Gatekeeper).

Signing is on the roadmap for the eventual 1.0.

## Build from source

Prerequisites:

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 20+
- Platform-specific Tauri prerequisites:
  [Windows](https://v2.tauri.app/start/prerequisites/#windows) /
  [macOS](https://v2.tauri.app/start/prerequisites/#macos) /
  [Linux](https://v2.tauri.app/start/prerequisites/#linux)

```bash
git clone https://github.com/<you>/malt.git
cd malt
npm install
npm run tauri dev          # dev mode with hot reload
npm run tauri build        # produces a release installer
```

The first build is slow (~5–15 minutes — fastembed + tantivy + onnxruntime
pull in a lot). Subsequent builds are fast.

## License

MIT.
