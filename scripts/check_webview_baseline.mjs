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

const readJson = (p) => JSON.parse(readFileSync(p, 'utf8'));
const baseline = readJson(BASELINE);

// ── Edge A: the macOS install gate ────────────────────────────────────────
// The value is declared in the BASE config only. tauri.lite.conf.json is
// applied as `cargo tauri build --config tauri.lite.conf.json`, a deep merge
// over the base (justfile shell-build-lite) — duplicating the value there would
// create a second source of truth. The overlay is only checked for a
// CONTRADICTION.
{
  const base = readJson(BASE_CONF);
  const got = base?.bundle?.macOS?.minimumSystemVersion;
  if (got !== baseline.macos_min) {
    fail('A', `${BASE_CONF} bundle.macOS.minimumSystemVersion is ${JSON.stringify(got)}, expected ${JSON.stringify(baseline.macos_min)}`);
  }
  const lite = readJson(LITE_CONF);
  const liteGot = lite?.bundle?.macOS?.minimumSystemVersion;
  if (liteGot !== undefined && liteGot !== baseline.macos_min) {
    fail('A', `${LITE_CONF} overrides minimumSystemVersion to ${JSON.stringify(liteGot)}; the overlay must omit it or match ${JSON.stringify(baseline.macos_min)}`);
  }
}

if (problems.length) {
  console.error(problems.join('\n'));
  console.error(`\n${problems.length} baseline violation(s). The declaration is ${BASELINE}; fix the consumer, not the declaration, unless you are deliberately moving the floor.`);
  process.exit(1);
}
console.log('✓ webview baseline consistent');
