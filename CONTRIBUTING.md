# Contributing

Thanks for even considering it. malt is a personal project shared as-is (see
[SUPPORT.md](SUPPORT.md)), so contributions are welcome but come with honest
caveats.

## Honest expectations

- **I may be slow, or may not merge.** I maintain malt to fit my own hands. A
  clean, useful PR can still sit unreviewed or get declined because it pulls the
  app in a direction I don't want to maintain. No hard feelings either way —
  and if you disagree, **forking is encouraged.** It's MIT.
- **By contributing, you agree to license your contribution under the
  [MIT License](LICENSE).** No CLA, no paperwork — just that understanding.
- **Talk before you build anything big.** Open a [Discussion](../../discussions)
  for non-trivial changes before writing a lot of code, so neither of us wastes
  effort on something I won't merge.

## Good PRs

- **Small and focused.** One change per PR. A tight bug-fix with clear repro
  steps is the most likely thing to get merged quickly.
- **Match the surrounding style.** No sweeping reformatting, renames, or
  dependency churn bundled with a feature. Keep the diff legible.
- **Explain the why.** A sentence on the problem and your approach beats a wall
  of code.
- **No new heavy dependencies** without discussing it first — startup time and
  binary size matter for this app.

## Project shape

malt is [Tauri 2](https://v2.tauri.app/) with a Svelte 5 + TypeScript frontend
and a Rust backend.

- `src/` — Svelte/TS UI (`src/routes/+page.svelte` is the shell;
  `src/lib/` holds the editor, settings, panels).
- `src-tauri/src/` — Rust: `notes.rs` (files + watcher), `index.rs` (tantivy
  search), `backlinks.rs`, `embeddings.rs`, `ai.rs`, `config.rs`, `lib.rs`
  (IPC command registry).
- Notes are **plain `.md` files in a single flat folder — no subdirectories.**
  Derived state (index, embeddings, config) lives in a sidecar config dir, never
  in the notes folder. Please preserve both invariants.

## Before you open a PR

Both of these should be clean:

```bash
npm run check
cargo check --manifest-path src-tauri/Cargo.toml
```

Build instructions and prerequisites are in the [README](README.md#build-from-source).
The first build is slow; subsequent ones are fast.

## Reporting bugs

See [SUPPORT.md](SUPPORT.md). Include OS, malt version (Settings → About), and
exact steps to reproduce. For security issues, use private disclosure — don't
open a public issue.
