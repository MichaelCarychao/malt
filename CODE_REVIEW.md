# malt — Full Code Review (2026-06-02)

Deep multi-angle audit of malt at v0.4.18, ahead of signed distribution. Every
finding cites real code. Severity is weighted toward the app's own values:
**data loss / corruption is the worst outcome**, then silent incorrectness, then
latency, then polish.

Scope reviewed in full: all 21 Rust modules (`src-tauri/src`, ~7.1k LOC) and the
Svelte/CodeMirror frontend (`src`, ~11.4k LOC), plus `tauri.conf.json` and
`capabilities/`.

> **Bottom line:** the architecture and the hard parts (crypto primitives,
> atomic writes, conflict handling, updater signing, capability scoping) are
> genuinely well built. But there is a **cluster of encryption-integrity bugs at
> the save/export boundary that can silently destroy or expose a user's
> encrypted notes**, plus durability and search-consistency gaps. Fix the
> Criticals before you put signed binaries in people's hands — a signed app that
> eats encrypted notes is worse than an unsigned one.

---

## Executive summary

| Tier | Count | Theme |
|---|---|---|
| **Critical** | 4 | Encrypted-note corruption/leak on save, switch, export, brew |
| **High** | 10 | Durability (no fsync), config-wipe, search-index staleness, wikilink rename corruption, UI-thread stalls, tagger provider bug |
| **Medium** | ~16 | CSP/path hardening, vault-switch atomicity, semantic-metric calibration, IME, keyboard safety |
| **Low** | ~15 | Reactivity smells, a11y, perf polish |

The Criticals are mostly one root cause wearing four hats: **the "is this note
encrypted, and with what password?" decision is made ad hoc at each call site,
and several call sites get it wrong or skip it.** A single funnel
(`read_note_plaintext` / `save_note_respecting_encryption`) would close most of
them at once.

---

## CRITICAL — fix before distributing

### C1. Switching notes writes the outgoing note with the *incoming* note's encryption codec
- **Where:** `src/lib/Editor.svelte:1724-1737` (`flushSave`), `:1801-1808` (`loadPath`); selection paths in `src/routes/+page.svelte:1116, 872, 1474` that set `selectedPath` with **no** preceding `await flushAllEditors()`.
- **What:** `loadPath` flushes the *outgoing* note (`currentPath`, line 1807), but `flushSave` resolves `isEncrypted`/`password` from **live props** (line 1727), which have already updated to the *incoming* note. On the common "edit a note, then click another within the 300 ms autosave window" path there is no earlier flush, so:
  - **encrypted (unlocked) A → plaintext B:** A's decrypted buffer is written via `save_note` to A's path → **A's encryption is silently removed and its plaintext is written to disk.**
  - **plaintext A → encrypted (unlocked) B:** A is written via `save_encrypted_note` under **B's** password → **A becomes unreadable** (encrypted under the wrong key, user never told).
  - Verified live, not latent: ordinary sidebar click / arrow-nav skip `flushAllEditors`.
- **Fix:** Bind the encryption identity to the *buffer*, not to live props. Snapshot `{isEncrypted, password}` at schedule/load time and pass them into `flushSave(path, content, isEncrypted, password)`. Mirror `currentPath`'s "source of truth is the value at edit time" discipline for `currentIsEncrypted`/`currentPassword`. Apply to the registered flusher and `onDestroy` flush too.

### C2. Export silently embeds encrypted-note ciphertext into shareable files
- **Where:** `src-tauri/src/export.rs:15-66` (`build_composite_markdown`), reachable from `export_as_string` / `export_to_file` (`lib.rs:247-290`).
- **What:** Every other content consumer guards `is_encrypted` (indexer, tagger, embedder, backlinks, mentions, lister). Export does **not**. An encrypted note on disk is one line `MALT-ENC-v1:<base64>`; export writes that envelope verbatim as if it were prose. Worse: with `append_links`, a TOC note that links to encrypted notes reads each target (`export.rs:37`) and appends its **ciphertext** into the exported `.md`/`.html`/`.epub` — content the user explicitly chose to encrypt, now baked into a file they're about to share. Also: `markdown_to_html_body` (`export.rs:69`) doesn't disable pulldown-cmark raw-HTML, so note-authored `<script>`/`<img onerror>` passes through into exported HTML/epub.
- **Fix:** In `build_composite_markdown`, `if encryption::is_encrypted(&raw) { return Err("cannot export an encrypted note") }`, and skip/er­ror per linked target inside the loop. Disable or sanitize raw HTML in the markdown→HTML path.

### C3. Brew append/save corrupts an encrypted source note
- **Where:** `src/routes/+page.svelte:1530-1540` (`appendBrewToSource`), `:1546-1569` (`saveBrewAsNote`); brew is reachable from an encrypted note (`openBrewForPrimary` has no guard).
- **What:** `appendBrewToSource` does `read_note` (raw) then `save_note`. On an encrypted source, `read_note` returns the `MALT-ENC-v1:` envelope; the function appends the brew text after it and writes it back as plaintext via `save_note`. The file now begins with the envelope but has trailing non-base64 garbage → **permanently undecryptable**, and the brew plaintext is on disk. `getPrePromptFor` (`:1349`) already handles the encrypted case correctly, so this is an inconsistency/oversight.
- **Fix:** Guard both brew-save paths on `is_encrypted`; use the cached password + `save_encrypted_note`, or refuse to append to an encrypted note.

### C4. Semantic search uses L2 distance but the code assumes cosine — every threshold is miscalibrated
- **Where:** `src-tauri/src/embeddings.rs:53` (schema) vs `:197-202, :267-280, :339-343`.
- **What:** The vector table is declared `vec0(embedding float[384])` with **no `distance_metric=`**, so sqlite-vec uses its default **L2 (Euclidean)** distance. But the code comments "cosine distance (0=identical, 2=opposite)" and computes `sim = 1.0 - dist/2.0`, then gates features on cosine-shaped thresholds: related ≥ `0.55` (`lib.rs:333`), near-duplicate ≥ `0.9` ("basically the same note", `lib.rs:388`). L2 is unbounded unless vectors are exactly unit-normalized (fastembed doesn't guarantee that at this layer), so `1 - dist/2` can go negative and the thresholds mean something other than intended. The "related notes" and "near-duplicate" features are silently calibrated against a metric the DB isn't using.
- **Fix:** Declare `vec0(embedding float[384] distance_metric=cosine)`. This is a schema change and `CREATE … IF NOT EXISTS` will keep the old L2 table on existing installs — gate it behind a `PRAGMA user_version` migration that drops+rebuilds `vec_notes` (embeddings are derived and cheap to recompute). See M9 (no schema versioning today).
- **Confidence:** High on the metric mismatch; the *practical* damage depends on whether fastembed happens to emit normalized vectors — worth an empirical check, but the fix is correct regardless.

---

## HIGH

### H1. `write_atomic` is atomic against readers but never fsyncs → power loss can zero out a note
- **Where:** `src-tauri/src/notes.rs:21-35`.
- **What:** `std::fs::write(tmp)` + `rename` is atomic w.r.t. concurrent readers but **not crash-safe**: neither the data nor the rename is flushed. On power loss / hard crash, common filesystem orderings persist the rename while the new file's data blocks were never written → the classic **0-length file where a complete note used to be**. This is the single write path for every save, AI re-tag, and wikilink cascade.
- **Fix:** `f.sync_all()` on the temp file before `rename`; on Unix, fsync the parent dir after. (Windows: the temp-file `sync_all` is the important part.)

### H2. Config / vaults / saved-searches / prompts: non-atomic writes that can wipe the file to empty
- **Where:** `config.rs:185-188`, `vaults.rs:92-95`, `saved_searches.rs:109-112`, `prompts.rs:110`.
- **What:** All persist via raw `std::fs::write` (truncate-then-write — crash mid-write corrupts the file) **and** use `serde_json::to_string_pretty(..).unwrap_or_default()`, so a serialization failure writes `""` over a valid file. For `vaults.json` that's "malt forgets where your notes live"; for `config.json`, lost pins/settings; for `saved_searches.json`, lost custom searches.
- **Fix:** Route all four through `write_atomic`; replace `unwrap_or_default()` with error propagation / skip-the-write.

### H3. The search index only updates via the debounced watcher — a dropped FS event leaves it silently stale forever
- **Where:** `src-tauri/src/index.rs:59-91` (`rebuild` is the *only* mutation path); write commands `lib.rs:98/408/425/497` never touch the index; watcher rebuild at `notes.rs:691` (`let _ = index.rebuild()`).
- **What:** `save_note`/`delete_note`/`rename_note`/`create_note` only write disk + enqueue embeddings; the index is refreshed solely by the watcher's debounced full `rebuild()`. Consequences: (a) search is stale for up to `MAX_WAIT = 3 s` after an edit; (b) if the OS coalesces/drops an event — exactly what happens under the Dropbox sync storms malt is designed to tolerate — the index goes stale with **no reconciliation, no periodic rescan, and the error is swallowed** (`let _ =`); (c) every change re-reads and re-tokenizes the **entire vault twice** (`list_notes()` already read every file, then `rebuild` reads them all again), directly against the "low latency" core value; (d) a panic in a tokenization helper on the watcher thread kills indexing for the rest of the session.
- **Fix:** Make write commands update the index synchronously via delete-by-term upsert/remove (the `path` field is already `STRING` and ready to be the primary key); keep the watcher for external changes; add an idle/focus full-rescan safety net; `catch_unwind` the watcher loop; stop discarding the rebuild `Result`. (Same full-rebuild-per-change pattern exists in `backlinks.rs:27` — fix together.)

### H4. Rename rewrites `[[links]]` inside fenced / inline code blocks
- **Where:** `src-tauri/src/backlinks.rs:156` (`rewrite_wikilinks_in_body`).
- **What:** The rename cascade walks raw bytes and rewrites *any* `[[old]]`, with no code-fence/backtick masking — even though `link_suggestions.rs` already builds exactly such a mask and the module docs list code exclusion as a goal. A note documenting wikilink syntax in a code block has its sample silently rewritten on rename. (Backlink *counting* via `scan_wikilinks` shares the root cause — code-block links wrongly count as backlinks.)
- **Fix:** Factor the exclusion-mask out of `link_suggestions` and skip masked spans in both `rewrite_wikilinks_in_body` and `scan_wikilinks`.

### H5. Rename can redirect links to a *different* note that merely slug-collides
- **Where:** `backlinks.rs:184` (`inner_slug == old_slug`) + `resolve:277`.
- **What:** `slugify` strips punctuation and case, so `slugify("Note 1") == slugify("Note-1") == "note1"`. Renaming "Note 1" rewrites `[[Note-1]]` even when a distinct file `Note-1.md` exists (the flat folder allows both). Links get silently pointed at the wrong note.
- **Fix:** Resolve each candidate link against the current note set; only rewrite when it resolves to the file being renamed. Prefer exact case-insensitive title match, slug only as fallback.

### H6. `[[link|alias]]` is unimplemented — aliased links never resolve, never backlink, and break on rename
- **Where:** `backlinks.rs:283/265/156`, `mentions.rs:31`, `link_suggestions.rs:242`.
- **What:** Nothing splits the inner text on `|`. `[[Quantum Mechanics|QM]]` resolves the whole string → matches no note → no backlink, counts as dangling, and rename never updates it. A standard wikilink feature (and listed design intent) that's silently absent.
- **Fix:** Split inner on the first `|`, resolve the left side, preserve `|alias` through rewrites.

### H7. The background tagger gates on the Anthropic key but runs the *active* provider
- **Where:** `tagger.rs:142` (`has_api_key()` → Anthropic) vs `:87-89` (`get_api_key_for(active_provider)`).
- **What:** After a user switches the active provider, auto-tagging either (a) silently does nothing forever (no Anthropic key → gate never passes), or (b) pops a path off the queue and errors every tick (Anthropic key present but active provider unconfigured) — dropping notes from the tag queue. This is exactly the multi-provider scenario the feature exists for.
- **Fix:** Gate on `has_api_key_for(active_provider)`; load config once per tick.

### H8. Search and semantic commands block the IPC/UI thread re-reading the whole vault
- **Where:** `lib.rs:57-89` (`search_notes`), `embeddings.rs:160` (`find_related`), `:236-265` (`search_text`); contrast the correct `spawn_blocking` in `list_near_duplicates` (`lib.rs:380`).
- **What:** `search_notes` calls `list_notes()` (reads every file) then does **another** `read_to_string` per hit for snippets — two full vault passes per keystroke-search, synchronously on the IPC thread, even though Tantivy already stores the fields. `find_related`/`semantic_search` are sync commands that hold the single DB mutex (and `search_text` the model mutex) across the query, contending with the background embed worker. Directly undercuts the #1 latency value.
- **Fix:** Make these `async` + `spawn_blocking`; serve `list_notes` summaries from a watcher-invalidated in-memory cache; pull snippets from Tantivy stored fields instead of a second disk read.

### H9. `loadPath` has no re-entrancy guard → rapid switches leak a second EditorView and can save to the wrong note
- **Where:** `src/lib/Editor.svelte:1801-1974`, driven by `$effect` at `:2039`.
- **What:** `loadPath` is async; a second path/password change during its `await read_note` starts a concurrent `loadPath`. Both pass the `if (view) destroy()` guard and both `new EditorView({parent: container})` — the loser's view is orphaned in the DOM with live update/blur/ResizeObserver listeners that can fire `scheduleSave`/`finalizeTagsOnBlur` against the *now-current* path (cross-note save). Unlike `handleExternalChange` (which has `externalChangeGen`), `loadPath` has no monotonic guard. Reachable by holding ↓ to scan notes or unlock flipping `password` mid-read.
- **Fix:** `loadGen` counter; bail after each await if superseded; await the read before destroying the old view.

### H10. BrewPane re-streams an LLM call on every source-body change
- **Where:** `src/lib/BrewPane.svelte:91-96`.
- **What:** The `$effect` keys on `` `${noteTitle}::${noteBody}` `` and re-runs `runBrew` whenever it changes. If the parent binds `noteBody` to live editor content, **every keystroke in the source note fires a fresh `brew_streaming` call** (cancelling the prior), hammering the provider and burning tokens. An expensive network action coupled to a reactive string diff with no debounce/gesture.
- **Fix:** Key on an explicit `brewNonce` the parent bumps when the user actually invokes brew, not on raw body content.

---

## MEDIUM

**Security hardening (the encryption boundary + WebView surface)**

- **M1. No backend guard against plaintext-clobbering an encrypted note.** `save_note` (`lib.rs:97`) writes unconditionally; the "don't write plaintext over ciphertext" invariant is frontend-only — which is what makes C1/C3 possible. Add a single funnel (`read_note_plaintext` / a save helper that respects on-disk encryption) so the guard can't be omitted per call site.
- **M2. AI streaming commands send the decrypted plaintext of encrypted notes to third-party APIs** (`lib.rs:700-808`, `suggest_wikilinks_ai`). Every other path skips encrypted content; the one surface that exfiltrates off-device doesn't know about encryption. Tag AI requests from encrypted notes and block / require explicit per-call confirmation.
- **M3. CSP is `null`** (`tauri.conf.json:22`). The WebView renders untrusted-ish content (synced `.md`, AI output) and has the full `invoke` surface (`delete_note`, `read_note` any path, `set_api_key`). With C2's raw-HTML passthrough this is a real XSS-into-privileged-origin path. Set a restrictive CSP; ensure markdown→HTML escapes/sanitizes raw HTML.
- **M4. Inconsistent path validation.** `read_note`/`save_note`/the encryption commands/`export_to_file` accept any absolute path; only `delete`/`rename`/`duplicate`/`move` are vault-scoped (and even those use lexical `starts_with` without `canonicalize`, which both lets symlinks escape and false-rejects case-variant paths on Win/macOS). Add a shared canonicalizing `vault_path_guard` to all note-path commands.
- **M5. Crypto hardening (low-ish):** the derived-key cache is keyed by a 64-bit `DefaultHasher` of (salt,password) — the comment's "never collides" is technically false (a collision returns a wrong key → undecryptable note); and keys/passwords are never zeroized and live for the process lifetime. Both are outside the stated "casual attacker who steals the file" threat model but worth hardening (`zeroize`, stronger cache key). Argon2 uses crate defaults (~19 MiB, t=2) = OWASP minimum; fine, but the versioned envelope makes raising it easy later.

**Concurrency / correctness**

- **M6. Vault switch is non-atomic across four independent locks** (`lib.rs:902-966`). "Active vault" lives in `vaults.json` and is re-resolved per subsystem; a fast double-switch (or the watcher thread racing) can leave index/embeddings/watcher pointed at different vaults, and the watcher cascade could rewrite `[[links]]` in the wrong vault. Hold one switch lock across the whole sequence and pass the captured `new_dir` to each subsystem.
- **M7. `active_path()`/`active_name()` index the vaults Vec unchecked** (`vaults.rs:98-109`) on the hottest path in the app (`notes_dir()` calls it per command, per watcher event). A malformed/empty registry panics the calling (or watcher) thread. Use `.get(..).unwrap_or_else(fallback_default_path)`.
- **M8. `notes_changed` list refreshes have no generation guard** (`+page.svelte:2524`). `refreshAllNotes`/`refreshTagMeta`/`refreshPinned` can resolve after a vault switch and repaint the new vault with the old vault's notes (then auto-select points the editor at a wrong-vault path). Add a vault-generation stamp like `queryGen`.
- **M9. No embedding-DB schema versioning** (`embeddings.rs:47`). `CREATE … IF NOT EXISTS` means the C4 cosine fix (and any future model/dim change) silently won't apply to existing installs. Add `PRAGMA user_version` + drop/rebuild on mismatch.
- **M10. Per-vault embedding DB filename uses `DefaultHasher`** (`embeddings.rs:538`), which has no cross-version stability guarantee — a Rust upgrade can silently remap every vault to a new DB file (all embeddings "gone", old files orphaned). Use a fixed-seed/stable hash (fnv/xxhash/sha1) for the filename.
- **M11. `embeddings.repoint()` race** (`:92`): a `process()` already in flight for an old-vault path can insert into the new vault's DB after a switch — cross-vault leak the per-vault-DB design exists to prevent. Re-check `notes_dir()` membership (or a vault token) immediately before the insert.
- **M12. Export is not written atomically** (`lib.rs:288`, raw `std::fs::write`) — a truncated re-export (esp. binary epub) silently replaces a good file. Route through an atomic bytes write.
- **M13. `flushAllEditors` swallows every flush error** (`editorRegistry.ts:16`). Destructive callers (`rename`/`move`/`switchVault`) `await` it specifically to guarantee no unsaved content is lost before mutating — but `.catch(()=>{})` means a failed save lets the destructive op proceed, losing content. Let it reject / report failures.

**Frontend correctness / UX**

- **M14. Global keydown handler doesn't exclude text inputs** (`+page.svelte:882-1086`). In the rename/save-search/vault modals, Cmd+1–9 yanks focus to search, Cmd+D opens the daily note, and **Cmd/Ctrl+Backspace (delete-word-left while editing a title) triggers the delete-note confirmation**. Early-return when `document.activeElement` is an input/textarea (except chosen combos like Esc).
- **M15. Autosave caret clamp can yank the cursor backward mid-edit** (`Editor.svelte:1744-1768`). `relocateTags(false)` runs on every debounced save and unconditionally clamps the caret to `min(caret, maxCaret)`; after an AI insert/paste that leaves the caret below the relocate point, the cursor jumps up ~300 ms after typing stops. Map the caret through the ChangeSet and clamp only when it would land in the hidden line.
- **M16. No IME composition gating** (`Editor.svelte`; no `compositionstart/end` anywhere). A debounced `relocateTags` full-doc replace during CJK candidate selection cancels composition and can drop/duplicate characters. Bail on `view.composing`.
- **M17. Whole-doc tag rescan on every keystroke *and* every cursor move** (`Editor.svelte:1423-1496`). `tagWatcher`/`tagPillPlugin` stringify the whole doc and `findInlineTags` over it on `selectionSet` too; scope to `view.visibleRanges` like the wikilink/markdown plugins do.
- **M18. Note list isn't virtualized; `flairAccent(note)` is called 3× per row** (`+page.svelte:2740-2772`) and `highlight` splits Unicode twice per row — re-run for the whole corpus on every keystroke. Memoize per-note flair, call accent once, virtualize as vaults grow.

---

## LOW / polish

- **AI stream:** no idle/read timeout (a stalled-but-open connection hangs forever) and no cancellation when the user moves on (keeps billing); empty/safety-filtered stream reported as success (silent no-op); per-chunk `from_utf8_lossy` can emit `�` for a multibyte token split across chunks; `dispatch_*` ignores the per-provider model override for tags/entities; tagger errors only `eprintln!` (invisible in a GUI build) — surface a throttled event. (`ai.rs`, `openai_compat.rs`, `tagger.rs`)
- **`merge_tags_into_file`** (`tags.rs:112`) has no internal `is_encrypted` guard — safe only because its one caller checks; same latent class as C2. Add the guard.
- **`forget_path`/`rename_path` self-lock the DB mutex** (`embeddings.rs`) — fine today, but one refactor from a non-reentrant deadlock; split `_locked` variants.
- **Reactivity smells:** auto-select `$effect` writes `selectedPath` it also reads (`+page.svelte:817`); `tipHistory` and the history stacks are plain `let`, not `$state`, so any UI bound to their length won't update (`:433`, `:542`). 
- **`isMac` via deprecated `navigator.platform`** defaults to Mac under SSR → Windows users briefly see ⌘ glyphs (relevant — you ship to Windows); duplicated inline ~6×. Centralize one `isMac()`.
- **A11y:** backlink/related rows are clickable `<li>` with no `role`/`tabindex`/keydown — not keyboard-reachable, in a keyboard-first app (`Linkbacks.svelte:147/161`); the unlinked-mention rows do it right — mirror them.
- **Raw `String(e)` backend errors shown in UI** (`+page.svelte` password/rename/export/vault) — map to friendly copy, log raw to console, avoid leaking FS paths.
- **BrewPane auto-scroll** forces bottom on every chunk, fighting a user who scrolls up mid-stream (`BrewPane.svelte:64`).
- **Settings flair editor** fires a full `set_tag_styles` IPC + global repaint on every keystroke (`Settings.svelte:1279`) — debounce / keep the explicit Save button.
- **`#[allow(dead_code)]` on `is_canonical_tag_line`** is stale — it's used internally (`tags.rs:381`).
- **`.expect()` on poisonable locks** in `backlinks.rs:70/76` and `notes.rs:442` (`repoint_watcher`) vs the poison-tolerant pattern `index.rs:60` uses — make them consistent so one panic can't cascade.
- **Maintainability:** `+page.svelte` at 4,844 lines is a genuine structural risk — it owns search, selection, history, pins, vaults, saved searches, encryption modal, export, tips, updater, brew. The encryption/save-on-switch tangle (C1/C3/M13) lives here precisely because responsibility is split with `Editor.svelte`. Extract a note-IO module that owns `(path, isEncrypted, password) → read/save`, one `enterVault(index)` primitive, and the saved-search subsystem as its own component.

---

## What's genuinely solid (so the picture is calibrated)

- **Crypto primitives** (`encryption.rs`): AES-256-GCM + Argon2id, **fresh random nonce every save** (correctly identifying nonce-reuse as the catastrophic case while safely reusing salt only to warm a CPU cache), non-leaking generic decrypt error, versioned envelope, RustCrypto (no OpenSSL), good test suite. The design is right — the bugs are all in the *integration*, not the math.
- **`write_atomic`** temp+rename with cleanup-on-failure, used by *every* note mutation (just needs fsync, H1).
- **External-change conflict handling** (`Editor.svelte`): monotonic `externalChangeGen` discards stale disk reads, `internalDocRewrite` annotation prevents echo-write loops, and a true divergence pauses autosave behind a non-destructive conflict bar instead of clobbering. This is the crux sync-storm scenario and it's handled with real care.
- **Watcher** debounce/coalescing (QUIET 300 ms / MAX_WAIT 3 s) and **conservative rename detection** (only pairs disappeared/reappeared files on globally-unique content) — refuses to guess.
- **Frontmatter round-trip** preserves unknown YAML keys (`#[serde(flatten)] extra`) with tests — correct "safe editor over a folder other tools own" behavior.
- **Encrypted notes consistently excluded** from index/tagger/embedder/backlinks/mentions (export, C2, is the one miss).
- **AI tag sanitization** (`canonicalize` → `[a-z0-9_/-]`) prevents YAML/frontmatter injection from model output; YAML written via `serde_yaml`, not string concat; no `unwrap` on any network/JSON path; API key only in headers, never logged.
- **Tantivy** queries built from hand-constructed `Term`s (no QueryParser injection) with the all-`MustNot`→`AllQuery` edge handled.
- **Updater** correctly enforces minisign signature verification (pubkey configured), HTTPS GitHub endpoint; **capabilities are tight** — no `fs:`/`shell:` grants, all file access funnels through audited commands (least privilege).
- **Embeddings** cached by content hash; stale purged on delete/rename/encrypt; model-load failure backs off with a status event.

---

## Recommended fix order

1. **C1** (save-on-switch encryption mix-up) — highest blast radius, common path, silent. Bind encryption identity to the buffer.
2. **C2 + C3 + M1** together — the encrypted-content funnel: one `is_encrypted`-respecting read/save helper kills export leak, brew corruption, and the missing backend guard at once.
3. **H1 + H2** — durability: fsync in `write_atomic`, atomic+non-truncating config/vault writes. Small, localized.
4. **H7** (tagger provider gate) and **H10** (brew re-stream) — cheap, user-facing, currently broken behaviors.
5. **H3** (search-index staleness/latency) — incremental upsert + safety-net rescan.
6. **H4/H5/H6** (wikilink rename correctness + aliases) — content-integrity and a missing headline feature.
7. **M3/M4** (CSP + path guard) — pair them; defense-in-depth before wide distribution.
8. **C4 + M9** — semantic metric fix behind a schema migration (verify empirically first).
9. The rest as polish / as the `+page.svelte` decomposition happens.
