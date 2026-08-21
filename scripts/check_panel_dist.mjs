#!/usr/bin/env node
// Guard: the committed panel dist (interfaces/webchat/dist) must be an
// internally-consistent js+wasm PAIR — every wasm export the wasm-bindgen glue
// references (`wasm.<name>`) must actually exist in aleph_panel_bg.wasm.
//
// Why this exists: v26.6.22 shipped a js-only rebuild whose aleph_panel.js
// referenced closure trampolines (wasm_bindgen__convert__closures_____invoke__*)
// absent from a stale aleph_panel_bg.wasm. The panel rendered but the connect
// coroutine invoked a missing trampoline (`TypeError: … is not a function`) and
// hung on "connecting" / blank against any remote core. CI embeds the committed
// dist verbatim (no WASM build), so the broken pair shipped to a release.
//
// This check catches that class of drift deterministically. It uses
// WebAssembly.Module.exports() (the authoritative export list — independent of
// the optional name section), so it stays correct even if `wasm-opt -g` is
// dropped. Run via `just check-dist` (also appended to `just wasm`) and in CI.
//
// Usage: node scripts/check_panel_dist.mjs [dist-dir]   (default: interfaces/webchat/dist)

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { brotliDecompressSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { MIN_BYTES } from './precompress_dist.mjs';

const dir = process.argv[2] || 'interfaces/webchat/dist';
const jsPath = `${dir}/aleph_panel.js`;
const wasmPath = `${dir}/aleph_panel_bg.wasm`;

let js;
let wasmBytes;
try {
  js = readFileSync(jsPath, 'utf8');
  wasmBytes = readFileSync(wasmPath);
} catch (e) {
  console.error(`✗ cannot read panel dist in '${dir}': ${e.message}`);
  console.error(
    `  These four files are TRACKED build outputs, not scratch: the release\n` +
      `  workflow embeds them verbatim (aleph-app-release.yml — "Panel WASM dist\n` +
      `  is pre-built and committed to git — no WASM build here"), so no release\n` +
      `  job owns a WASM toolchain and an empty dist/ ships an empty Panel.\n` +
      `  Fix: run \`just wasm\` and commit the result.\n` +
      `  If you meant to stop tracking dist/, that same change has to teach the\n` +
      `  release workflow to build the WASM first — dropping the files alone is\n` +
      `  what gated the pipeline shut at 033814185 (2026-08-13).`,
  );
  process.exit(1);
}

let exported;
try {
  const mod = new WebAssembly.Module(wasmBytes);
  exported = new Set(WebAssembly.Module.exports(mod).map((e) => e.name));
} catch (e) {
  console.error(`✗ ${wasmPath} is not a valid WebAssembly module: ${e.message}`);
  process.exit(1);
}

// The wasm-bindgen `--target web` glue holds the instance exports in a module
// variable named `wasm` and calls them as `wasm.<name>(...)`. Collect every
// such reference and confirm each resolves against an actual export.
const referenced = new Set(
  [...js.matchAll(/\bwasm\.([A-Za-z_$][A-Za-z0-9_$]*)/g)].map((m) => m[1]),
);

const missing = [...referenced].filter((name) => !exported.has(name)).sort();

if (missing.length > 0) {
  console.error(
    `✗ panel dist mismatch: aleph_panel.js references ${missing.length} wasm ` +
      `export(s) absent from aleph_panel_bg.wasm — js/wasm are NOT a matched pair.\n` +
      `  This is the v26.6.22 blank-panel bug class. Rebuild BOTH together with ` +
      `\`just wasm\` (never commit a js-only rebuild).\n` +
      `  Missing: ${missing.slice(0, 25).join(', ')}` +
      `${missing.length > 25 ? `, … (+${missing.length - 25} more)` : ''}`,
  );
  process.exit(1);
}

// ── Brotli siblings: both directions ──────────────────────────────────────
// dist/ is embedded verbatim by the release workflow, so a `.br` out of sync
// with its source ships WRONG BYTES — a strictly worse failure than the
// runtime compression it replaced. Both directions are required: checking only
// that existing siblings decompress correctly cannot tell you a NEW asset was
// never compressed, and checking only that large files have siblings cannot
// tell you an existing one is stale. (CLAUDE.md section 0: count the writers
// when you converge them.)
{
  const brProblems = [];

  // The two directions below only work because importing MIN_BYTES from
  // precompress_dist.mjs (above) is inert. That inertness is enforced by an
  // entry-point guard in that file's own source — a runtime check cannot
  // tell "the import did nothing because it is guarded" from "the import did
  // nothing because there was nothing to compress", so this has to read the
  // source text. If the guard is ever removed or renamed, the bare import
  // above silently re-runs the whole compression pass on every invocation of
  // this file and heals any corrupt/missing sibling before either direction
  // below ever runs — reintroducing, invisibly, the exact defect this file
  // exists to catch. Fail by name instead of letting that happen silently.
  const precompressPath = fileURLToPath(new URL('./precompress_dist.mjs', import.meta.url));
  const precompressSrc = readFileSync(precompressPath, 'utf8');
  if (
    !precompressSrc.includes('import.meta.url === pathToFileURL(process.argv[1]).href') ||
    !precompressSrc.includes('if (isMain)')
  ) {
    brProblems.push(
      `precompress_dist.mjs no longer guards its body with an entry-point check — ` +
        `importing MIN_BYTES from it (see the top of this file) would silently re-run ` +
        `the whole compression pass on every invocation of this guard and heal a ` +
        `corrupt/missing sibling before the checks below ever run. Restore ` +
        `\`if (isMain) { ... }\` gated by ` +
        `\`import.meta.url === pathToFileURL(process.argv[1]).href\`.`,
    );
  }

  const entries = readdirSync(dir).filter((n) => statSync(`${dir}/${n}`).isFile());

  // Direction 1: every sibling round-trips to its source.
  for (const name of entries.filter((n) => n.endsWith('.br'))) {
    const sourceName = name.slice(0, -3);
    if (!entries.includes(sourceName)) {
      brProblems.push(`${name} has no source file ${sourceName} — delete the stale sibling`);
      continue;
    }
    const source = readFileSync(`${dir}/${sourceName}`);
    let back;
    try {
      back = brotliDecompressSync(readFileSync(`${dir}/${name}`));
    } catch (e) {
      brProblems.push(`${name} is not valid brotli: ${e.message}`);
      continue;
    }
    if (!back.equals(source)) {
      brProblems.push(`${name} decompresses to something other than ${sourceName} — it is STALE, and serving it would send wrong bytes`);
    }
  }

  // Direction 2: every compressible source has a sibling.
  for (const name of entries.filter((n) => !n.endsWith('.br'))) {
    if (statSync(`${dir}/${name}`).size < MIN_BYTES) continue;
    if (!entries.includes(`${name}.br`)) {
      brProblems.push(
        `${name} is over ${MIN_BYTES} bytes but has no ${name}.br. Two possible ` +
          `causes: (1) the sibling is genuinely missing — run ` +
          `\`node scripts/precompress_dist.mjs\` to produce it; or (2) ${name} ` +
          `does not compress smaller than its source, so the producer correctly ` +
          `skipped it — in that case this check and the producer now disagree ` +
          `and both need extending together (see precompress_dist.mjs's own note ` +
          `on this), because re-running the producer alone will not help.`,
      );
    }
  }

  if (brProblems.length) {
    console.error(brProblems.map((p) => `✗ ${p}`).join('\n'));
    process.exit(1);
  }
}

console.log(
  `✓ panel dist OK: all ${referenced.size} wasm references in aleph_panel.js ` +
    `resolve against ${exported.size} exports in aleph_panel_bg.wasm.`,
);
