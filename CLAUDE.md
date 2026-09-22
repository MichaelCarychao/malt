# malt — agent notes

Keyboard-first writing app (Tauri v2 + Rust backend, Svelte 5 runes +
CodeMirror 6 frontend) where the author and a language model take turns.
Plain `.md` notes in a flat folder. See README.md for the product story.

## Commands

```bash
npm run tauri dev      # run the app (hot reload)
npm run tauri build    # build the platform installer
npm run check          # svelte-check (must be clean before commit)
cargo test --manifest-path src-tauri/Cargo.toml   # Rust tests
npm run release -- X.Y.Z   # RELEASE — see below
```

## Releasing — use the script, never do it by hand

```bash
npm run release -- 0.6.1
```

`scripts/release.mjs` runs the entire ritual: verifies a
`## X.Y.Z — …` entry exists in CHANGELOG.md (write it first), bumps the
version in **three** manifests (package.json, src-tauri/Cargo.toml,
src-tauri/tauri.conf.json — written WITHOUT a BOM; Tauri's config parser
rejects a BOM), runs the test gates, commits, tags `vX.Y.Z`, pushes,
then **waits for the CI Release workflow to build the draft GitHub
release and PUBLISHES it**.

That last step is the one that gets forgotten when releasing by hand:
the tag push only creates a **draft** release, and the in-app
auto-updater reads `latest.json` from the latest **published** release —
a draft ships to nobody. If a release was tagged but never published,
finish it with:

```bash
npm run release -- --publish-only
```

## Landmines (hard-won — do not rediscover these)

- **Editor review lock:** every save/flush path in `src/lib/Editor.svelte`
  must check `review !== null` — during a brew-implement diff review the
  buffer holds a merged preview that must never reach disk.
- **`loadPath` is called `untrack()`ed** from the note-load `$effect`.
  Any reactive read inside a function called from an effect becomes a
  dependency of that effect; a tracked `loadPath` once re-ran on
  `review` changes and cancelled every review instantly.
- **`wordDiff.ts` is display-only by construction** — accept/cancel
  dispatch pristine strings, never text rebuilt from diff segments.
- **Brew sessions are wed to the primary pane's note** (keyed by
  absolute path, persisted in localStorage); the brew prompt mandates
  one `- ` bullet per suggestion — the pane's parser depends on it.
- **`handleExternalChange`:** react-to-external-change logic must sit
  BELOW the own-save-echo check (`fresh === lastSavedContent`) — malt's
  own saves echo back through the file watcher within milliseconds.
- **Windows/PowerShell:** never use `Set-Content -Encoding utf8` on the
  version manifests (writes a BOM). The release script handles this.
- Keys live in the OS keychain via the `keyring` crate (pinned v2 — v3's
  set→get round-trip silently fails on Windows).

## Conventions

- Version lives in the three manifests above and must stay in lockstep.
- Tags are `vX.Y.Z`; CHANGELOG.md entries are `## X.Y.Z — YYYY-MM-DD`
  written for users, not developers.
- All AI prompts live in `src-tauri/src/prompts.rs` with user overrides
  in the prompts registry — never inline prompt text elsewhere.
- Providers (cloud + LM Studio local) share `src-tauri/src/openai_compat.rs`;
  Anthropic has its own client in `ai.rs`.
