#!/usr/bin/env node
// malt release script — the WHOLE ritual, including the step humans forget.
//
//   npm run release -- 0.6.1          bump → commit → tag → push → wait for
//                                     CI to build the draft → PUBLISH it
//   npm run release -- --publish-only finish a release whose tag is already
//                                     pushed (waits for the draft, publishes)
//   npm run release -- 0.6.1 --no-verify   skip the test/check gates
//
// Why this exists: pushing a tag triggers .github/workflows/release.yml,
// which builds all platforms and creates a DRAFT GitHub release. The
// auto-updater only sees PUBLISHED releases — a draft ships to nobody.
// This script closes that loop.
//
// Requirements: clean git tree on main, gh CLI authenticated, and a
// CHANGELOG.md entry ("## <version> — ...") for the new version.

import { execSync, spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

const POLL_SECONDS = 20;
const BUILD_TIMEOUT_MINUTES = 40;

function run(cmd, opts = {}) {
  return execSync(cmd, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], ...opts }).trim();
}
function runLoud(cmd) {
  // Inherit stdio so cargo/vite output streams to the terminal.
  const r = spawnSync(cmd, { shell: true, stdio: "inherit" });
  if (r.status !== 0) fail(`command failed: ${cmd}`);
}
function fail(msg) {
  console.error(`\nrelease: ${msg}`);
  process.exit(1);
}
const sleep = (s) => new Promise((r) => setTimeout(r, s * 1000));

// ── Parse args ───────────────────────────────────────────────────────
const args = process.argv.slice(2);
const publishOnly = args.includes("--publish-only");
const noVerify = args.includes("--no-verify");
const version = args.find((a) => /^\d+\.\d+\.\d+$/.test(a));
if (!publishOnly && !version) {
  fail("usage: npm run release -- <x.y.z>   (or --publish-only)");
}

// ── Preconditions ────────────────────────────────────────────────────
if (run("git rev-parse --abbrev-ref HEAD") !== "main") fail("not on main");
if (!publishOnly && run("git status --porcelain") !== "") {
  fail("working tree is not clean — commit or stash first");
}
try {
  run("gh auth status");
} catch {
  fail("gh CLI is not authenticated (run: gh auth login)");
}

const pkg = JSON.parse(readFileSync("package.json", "utf8"));
const current = pkg.version;
const target = publishOnly ? current : version;
const tag = `v${target}`;

// ── Bump + push (skipped in --publish-only) ──────────────────────────
if (!publishOnly) {
  if (target === current) fail(`already at ${current} — pick a new version`);
  const changelog = readFileSync("CHANGELOG.md", "utf8");
  if (!changelog.includes(`## ${target} `)) {
    fail(`CHANGELOG.md has no "## ${target} — ..." entry — write one first`);
  }

  // The three version manifests. Written WITHOUT a BOM — Tauri's config
  // parser rejects a BOM'd tauri.conf.json (hard-won lesson).
  const edits = [
    ["package.json", `"version": "${current}"`, `"version": "${target}"`],
    ["src-tauri/Cargo.toml", `version = "${current}"`, `version = "${target}"`],
    ["src-tauri/tauri.conf.json", `"version": "${current}"`, `"version": "${target}"`],
  ];
  for (const [file, from, to] of edits) {
    const text = readFileSync(file, "utf8");
    if (!text.includes(from)) fail(`${file}: expected ${from}`);
    writeFileSync(file, text.replace(from, to), { encoding: "utf8" });
  }
  console.log(`release: bumped ${current} → ${target} in 3 manifests`);

  if (!noVerify) {
    console.log("release: running gates (cargo test, svelte-check)…");
    runLoud("cargo test --manifest-path src-tauri/Cargo.toml");
    runLoud("npm run check");
  } else {
    // Still refresh Cargo.lock so the bump commit includes it.
    runLoud("cargo check --manifest-path src-tauri/Cargo.toml --quiet");
  }

  runLoud("git add -A");
  runLoud(`git commit -m "chore(release): bump to ${target}"`);
  runLoud("git push origin main");
  runLoud(`git tag ${tag}`);
  runLoud(`git push origin ${tag}`);
  console.log(`release: ${tag} pushed — CI is building the draft release now`);
}

// ── Wait for the CI draft, then publish it ───────────────────────────
console.log(
  `release: waiting for the ${tag} draft (poll ${POLL_SECONDS}s, timeout ${BUILD_TIMEOUT_MINUTES}m)…`,
);
const deadline = Date.now() + BUILD_TIMEOUT_MINUTES * 60 * 1000;
let published = false;
while (Date.now() < deadline) {
  // Abort early if the Release workflow for this tag failed outright.
  try {
    const runs = JSON.parse(
      run(`gh run list --workflow release.yml --limit 5 --json headBranch,status,conclusion`),
    );
    const ours = runs.find((r) => r.headBranch === tag);
    if (ours && ours.status === "completed" && ours.conclusion !== "success") {
      fail(`the Release workflow for ${tag} concluded: ${ours.conclusion} — fix CI, then rerun with --publish-only`);
    }
  } catch (e) {
    if (String(e).includes("concluded")) throw e;
    /* transient gh hiccup — keep polling */
  }

  let rel = null;
  try {
    rel = JSON.parse(run(`gh release view ${tag} --json isDraft,assets,url`));
  } catch {
    /* release not created yet */
  }
  if (rel) {
    const names = rel.assets.map((a) => a.name);
    if (!rel.isDraft) {
      console.log(`release: ${tag} is already published — nothing to do`);
      published = true;
      break;
    }
    // latest.json is uploaded by the workflow's final step, so its
    // presence means every installer + signature is already attached.
    if (names.includes("latest.json")) {
      run(`gh release edit ${tag} --draft=false --latest`);
      // Re-read for the URL: the draft's url is a placeholder until published.
      const url = JSON.parse(run(`gh release view ${tag} --json url`)).url;
      console.log(`release: PUBLISHED ${tag} (${names.length} assets) → ${url}`);
      console.log("release: the auto-updater will now serve this version.");
      published = true;
      break;
    }
    console.log(`release: draft exists, ${names.length} assets so far…`);
  } else {
    console.log("release: draft not created yet…");
  }
  await sleep(POLL_SECONDS);
}
if (!published) {
  fail(
    `timed out waiting for the ${tag} draft. Check the Actions tab, then finish with: npm run release -- --publish-only`,
  );
}
