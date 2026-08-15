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

import { readFileSync } from 'node:fs';

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

console.log(
  `✓ panel dist OK: all ${referenced.size} wasm references in aleph_panel.js ` +
    `resolve against ${exported.size} exports in aleph_panel_bg.wasm.`,
);
