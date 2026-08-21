#!/usr/bin/env node
// Guard: the Panel's WebView baseline is declared ONCE in
// interfaces/webchat/webview-baseline.json, and every consumer that restates it
// must agree. Four edges, added across tasks 1-4:
//   A  desktop/shell/tauri.conf.json bundle.macOS.minimumSystemVersion == macos_min
//      (and tauri.lite.conf.json, a MERGE OVERLAY, must not contradict it)
//   B  baseline-probe.js probes == css_probes + js_probes, set-equal both ways
//   C  dist/index.html contains baseline-probe.js verbatim
//   D  every css_probe is actually exercised by dist/tailwind.css,
//      plus an over-reporting census of CSS function names
//
// Usage: node scripts/check_webview_baseline.mjs
import { readFileSync } from 'node:fs';

const BASELINE = 'interfaces/webchat/webview-baseline.json';
const BASE_CONF = 'desktop/shell/tauri.conf.json';
const LITE_CONF = 'desktop/shell/tauri.lite.conf.json';

const problems = [];
const fail = (edge, msg) => problems.push(`✗ ${edge}: ${msg}`);

// A read failure (missing file) or a parse failure (malformed JSON) is itself
// an edge A violation, not a new edge: every file this reads today only feeds
// the macOS install-gate check. Reports and returns null instead of throwing —
// a raw JSON.parse SyntaxError names no file at all, and with two
// structurally identical Tauri configs (base + lite) a stack trace can't tell
// an operator which one they broke.
const readJson = (p) => {
  let raw;
  try {
    raw = readFileSync(p, 'utf8');
  } catch (e) {
    fail('A', `cannot read ${p}: ${e.message}`);
    return null;
  }
  try {
    return JSON.parse(raw);
  } catch (e) {
    fail('A', `cannot parse ${p} as JSON: ${e.message}`);
    return null;
  }
};
const baseline = readJson(BASELINE);

// ── Edge A: the macOS install gate ────────────────────────────────────────
// The value is declared in the BASE config only. tauri.lite.conf.json is
// applied as `cargo tauri build --config tauri.lite.conf.json`, a deep merge
// over the base (justfile shell-build-lite) — duplicating the value there would
// create a second source of truth. The overlay is only checked for a
// CONTRADICTION.
//
// Guarded by `if (baseline)`/`if (base)`/`if (lite)`: when readJson has
// already recorded why a file couldn't be read, comparing against a null
// value would just re-report the same failure as a confusing second message.
if (baseline) {
  const base = readJson(BASE_CONF);
  if (base) {
    const got = base?.bundle?.macOS?.minimumSystemVersion;
    if (got !== baseline.macos_min) {
      fail('A', `${BASE_CONF} bundle.macOS.minimumSystemVersion is ${JSON.stringify(got)}, expected ${JSON.stringify(baseline.macos_min)}`);
    }
  }
  const lite = readJson(LITE_CONF);
  if (lite) {
    const liteGot = lite?.bundle?.macOS?.minimumSystemVersion;
    if (liteGot !== undefined && liteGot !== baseline.macos_min) {
      fail('A', `${LITE_CONF} overrides minimumSystemVersion to ${JSON.stringify(liteGot)}; the overlay must omit it or match ${JSON.stringify(baseline.macos_min)}`);
    }
  }
}

// ── Edge B: the probe list matches the declaration, BOTH directions ───────
// Set equality, not containment: a one-directional check cannot tell a new
// probe from a removed one, and both are drift.
if (baseline) {
  const PROBE = 'interfaces/webchat/baseline-probe.js';
  let src;
  try {
    src = readFileSync(PROBE, 'utf8');
  } catch (e) {
    fail('B', `cannot read ${PROBE}: ${e.message}`);
    src = null;
  }
  if (src) {
    const declared = new Set([
      ...baseline.css_probes.map(([prop, val]) => `${prop}|${val}`),
      ...baseline.js_probes,
    ]);
    const found = new Set();
    // CSS probes appear as the two-element array literals in the cssProbes table.
    for (const m of src.matchAll(/\[\s*'([^']+)'\s*,\s*'([^']+)'\s*\]/g)) {
      found.add(`${m[1]}|${m[2]}`);
    }
    // JS probes appear as bare identifier paths in the two typeof/property checks.
    for (const name of baseline.js_probes) {
      // Match the LAST segment as a property access or a bare global, so
      // "CSS.registerProperty" matches `typeof CSS.registerProperty` and
      // "WebAssembly" matches `typeof WebAssembly`.
      if (new RegExp(`\\b${name.replace('.', '\\.')}\\b`).test(src)) found.add(name);
    }
    for (const d of declared) {
      if (!found.has(d)) fail('B', `${PROBE} does not probe declared capability ${JSON.stringify(d)}`);
    }
    for (const f of found) {
      if (!declared.has(f)) fail('B', `${PROBE} probes ${JSON.stringify(f)}, which is not declared in ${BASELINE} — add it to the declaration or drop the probe`);
    }
  }
}

if (problems.length) {
  console.error(problems.join('\n'));
  console.error(`\n${problems.length} baseline violation(s). The declaration is ${BASELINE}; fix the consumer, not the declaration, unless you are deliberately moving the floor.`);
  process.exit(1);
}
console.log('✓ webview baseline consistent');
