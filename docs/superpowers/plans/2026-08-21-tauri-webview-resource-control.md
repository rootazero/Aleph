# Tauri Cross-Platform WebView Resource Control — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Declare and enforce the Panel's WebView floor, halve the cold-load payload, add Range/206 to both byte routes, and give Linux a codec diagnosis and a glass-degradation trigger.

**Architecture:** Every fact lands in the single layer that knows it. A JSON declaration (`webview-baseline.json`) drives the install gate, the build gate, and the runtime probe. An inline probe script resolves the platform, writes `data-platform` and `data-flat`, then checks the CSS baseline. Brotli is produced at build time and committed; the server negotiates it and never compresses at runtime. A shared `byte_range.rs` serves both byte routes. No new runtime dependency is added.

**Tech Stack:** Rust (axum 0.8, tower-http 0.6, tokio), Node ≥18 (built-in `zlib`, `WebAssembly`), Tauri v2, Leptos 0.8 / WASM, Tailwind v4.2, `just`.

**Spec:** `docs/superpowers/specs/2026-08-21-tauri-webview-resource-control-design.md`

---

## Deviations from the spec (decided while planning; verified against the code)

Two spec statements assumed a shape the repo does not have. Both are corrected
here, and the reason is recorded so a reader of the spec is not confused.

1. **Spec §4.2 says the four-edge baseline guard is "a Rust test".** It is a
   **node script** (`scripts/check_webview_baseline.mjs`) instead. Reasons:
   `desktop/shell` will not build without `just _stage-shell-placeholders` first
   (tauri-build requires the `externalBin` placeholder files), so a Rust test
   there is expensive to run; the guard's four inputs are JSON, JS, JSON and CSS,
   all of which node reads natively; and the repo already places exactly this
   class of dist-adjacent guard in node (`scripts/check_panel_dist.mjs`). Same
   four assertions, same failure names.

2. **Spec §4.2 says "both `tauri.conf.json` files' `minimumSystemVersion`".**
   `tauri.lite.conf.json` is applied as `cargo tauri build --config
   tauri.lite.conf.json` — a **merge overlay** on the base config, not a
   standalone file (`justfile:153-160`, `.github/workflows/aleph-app-release.yml:171`).
   So the value is declared **once, in the base config**, and the guard asserts
   the base matches the JSON *and* that the overlay does not contradict it.
   Duplicating it into the overlay would create the second source of truth this
   design exists to avoid.

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Support floor:** macOS **13.3** / Safari **16.4** / WebKitGTK **2.42**. These
  exact strings live in `interfaces/webchat/webview-baseline.json` and are never
  retyped anywhere else.
- **No new runtime dependency.** Brotli is produced at build time with node's
  built-in `zlib.brotliCompressSync`. The Range helper is hand-written; do not
  reach for `tower-http`'s `fs` feature.
- **`src/harness/` has zero delta.** If a task appears to need a change there,
  stop and escalate.
- **`interfaces/webchat/dist/` is a git-tracked build output.** The release
  workflow embeds it verbatim and no job owns a WASM toolchain. Anything
  generated into `dist/` must be committed, and anything committed there must be
  paired-guarded against its source.
- **Verification is Windows-local.** Linux and macOS get scripted assertions the
  user runs. Never mark a Linux/macOS behaviour "verified" in a commit message.
- **Commit format:** `<scope>: <description>`, English, e.g.
  `gateway: serve precompressed brotli panel assets`.
- **Minimum verification set** (CLAUDE.md §10) — run before declaring any task
  done that touches Rust:
  ```
  cargo test -p alephcore --lib --no-run
  cargo clippy --all-targets
  ```
  Tasks touching `interfaces/webchat/` additionally run
  `cargo test -p aleph-panel --lib` (NOT `cargo check` — it cannot see that
  crate's test modules).
- **Every guard added by this plan must be broken once and observed to go RED**
  before its task is committed. A guard that has never been falsified is not a
  guard. Each task names its mutation.

---

## File Structure

| Path | Responsibility |
|---|---|
| `interfaces/webchat/webview-baseline.json` | **New.** The one baseline declaration. Nothing else states these numbers. |
| `interfaces/webchat/baseline-probe.js` | **New.** Resolve platform → compute `data-flat` → probe the CSS baseline → render the fallback page. Runs synchronously before the WASM module script. |
| `scripts/check_webview_baseline.mjs` | **New.** The four-edge build gate plus the over-reporting CSS function census. |
| `scripts/precompress_dist.mjs` | **New.** Brotli sibling producer, with round-trip verification. |
| `scripts/check_panel_dist.mjs` | Modified. Gains bidirectional `.br` pairing assertions. |
| `src/gateway/server/byte_range.rs` | **New.** Pure single-range parser. No I/O, no axum types in the core function. |
| `src/gateway/server/artifact_route.rs` | Modified. Range wiring after the gates; Range rate bucket; CSP on 206/416. |
| `src/gateway/server/canvas_asset_route.rs` | Modified. Same wiring; keeps its own `Cache-Control`. |
| `src/gateway/control_plane/server.rs` | Modified. Accept-Encoding negotiation, identity-derived ETag, `Vary`. |
| `src/diagnostics/checks/media_codecs.rs` | **New.** Linux GStreamer element probe, three-state, cancellable. |
| `interfaces/webchat/src/platform_host.rs` | **New.** `host()` — a pure reader of `data-platform`. No UA fallback (the probe owns that). |
| `interfaces/webchat/styles/tailwind.css` | Modified. Two degradation blocks re-keyed from `@media` to `html[data-flat="1"]`. |
| `.../chat/voice_playback.rs` | Modified. Narrow-predicate decoder receipt + `canPlayType` pre-check. |
| `.../chat/state.rs`, `.../chat/composer/mod.rs` | Modified. `voice_notice` signal + its banner, mirroring `send_error` / `SendErrorBanner`. |
| `desktop/shell/tauri.conf.json` | Modified. `bundle.macOS.minimumSystemVersion`, `bundle.windows.webviewInstallMode`. |
| `desktop/shell/src/main.rs` | Modified. `SHELL_MARKER_JS` completed for all three platforms. |
| `justfile` | Modified. wasm-opt required + feature allow-list; probe inlining; brotli step; `check-baseline` recipe. |
| `qa/webview_compat/run.sh` | **New.** Linux/macOS effect assertions. |

---

## Task Dependency Order

```
T1 ─┬─ T2 ─┬─ T3 ── T4
    │      └─ T6 ── T7
    │
T5 (independent)
T8 ── T9 ── T10
T11 ─┬─ T12
     └─ T13
T14 (independent)
T6 ── T15
T16 (after T1–T15)
T17 (last — rebuild dist + full sweep)
```

---

### Task 1: Baseline declaration and the install gate

**Files:**
- Create: `interfaces/webchat/webview-baseline.json`
- Create: `scripts/check_webview_baseline.mjs`
- Modify: `desktop/shell/tauri.conf.json:18-37` (the `bundle` object)
- Modify: `justfile` (add a `check-baseline` recipe)

**Interfaces:**
- Consumes: nothing.
- Produces: `interfaces/webchat/webview-baseline.json` with keys
  `macos_min: string`, `safari_min: string`, `webkitgtk_min: string`,
  `css_probes: [string, string][]`, `js_probes: string[]`.
  `scripts/check_webview_baseline.mjs` exits non-zero on any violation and
  prints `✗ <edge>: <observed> != <expected>`.

- [ ] **Step 1: Write the declaration**

Create `interfaces/webchat/webview-baseline.json`:

```json
{
  "_comment": "The ONE declaration of the Panel's minimum WebView. Three consumers derive from it and one guard covers all their edges: desktop/shell/tauri.conf.json (macOS install gate), interfaces/webchat/baseline-probe.js (runtime probe), scripts/check_webview_baseline.mjs (build gate). Never retype these values anywhere else. Rationale and the asymmetry of the guard's forward/reverse assertions: docs/superpowers/specs/2026-08-21-tauri-webview-resource-control-design.md sections 3.3 and 4.2.",
  "macos_min": "13.3",
  "safari_min": "16.4",
  "webkitgtk_min": "2.42",
  "css_probes": [
    ["color", "oklch(0 0 0)"],
    ["color", "color-mix(in oklab, red, red)"]
  ],
  "js_probes": ["CSS.registerProperty", "WebAssembly"]
}
```

- [ ] **Step 2: Write the failing guard (edge A only)**

Create `scripts/check_webview_baseline.mjs`:

```js
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
```

- [ ] **Step 3: Run it and verify it FAILS**

```bash
node scripts/check_webview_baseline.mjs
```

Expected: `✗ A: desktop/shell/tauri.conf.json bundle.macOS.minimumSystemVersion is undefined, expected "13.3"` and exit 1.

- [ ] **Step 4: Add the install gate to the base config**

In `desktop/shell/tauri.conf.json`, inside `"bundle"`, add a `macOS` key and
extend the existing `windows` key. The `windows` block currently reads:

```json
    "windows": {
      "nsis": {
        "installerIcon": "icons/icon.ico"
      }
    }
```

Replace it with:

```json
    "macOS": {
      "minimumSystemVersion": "13.3"
    },
    "windows": {
      "webviewInstallMode": {
        "type": "downloadBootstrapper",
        "silent": true
      },
      "nsis": {
        "installerIcon": "icons/icon.ico"
      }
    }
```

`minimumSystemVersion` is the hardest of the three baseline gates and costs no
code: macOS refuses the install itself. It applies to both products because the
lite build merges over this file. `webviewInstallMode` makes today's implicit
default explicit and silent; fixed-runtime bundling is deliberately not used
(it would add roughly 180 MB to the Windows installer).

- [ ] **Step 5: Run it and verify it PASSES**

```bash
node scripts/check_webview_baseline.mjs
```

Expected: `✓ webview baseline consistent`, exit 0.

- [ ] **Step 6: Prove the guard goes RED**

Temporarily change `minimumSystemVersion` to `"13.2"`, re-run, confirm the
failure names the file and both values, then change it back and re-run to green.

- [ ] **Step 7: Wire it into `just`**

Add to `justfile`, immediately after the existing `check-dist` recipe:

```makefile
# Verify the Panel's declared WebView baseline is consistent across every
# consumer. Run by `just wasm`, and in CI on any change under
# interfaces/webchat/ or desktop/shell/.
check-baseline:
    node scripts/check_webview_baseline.mjs
```

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/webview-baseline.json scripts/check_webview_baseline.mjs desktop/shell/tauri.conf.json justfile
git commit -m "desktop: declare the Panel WebView baseline and gate macOS installs on it"
```

---

### Task 2: The inline probe

**Files:**
- Create: `interfaces/webchat/baseline-probe.js`
- Modify: `scripts/check_webview_baseline.mjs` (add edge B)

**Interfaces:**
- Consumes: `webview-baseline.json` (Task 1).
- Produces: a script that, when evaluated synchronously in a document, sets
  `document.documentElement.dataset.platform` to one of `"macos" | "windows" |
  "linux"`, sets `dataset.flat` to `"1"` or removes it, and on baseline failure
  sets `dataset.webviewUnsupported = "1"` and replaces `document.body`. Exposes
  nothing on `window` — later Rust code reads only the attributes.

- [ ] **Step 1: Write the probe**

Create `interfaces/webchat/baseline-probe.js`. The probe list here is asserted
set-equal to `webview-baseline.json` by edge B, so the two can never drift.

```js
/* Aleph Panel WebView baseline probe.
 *
 * Runs as a SYNCHRONOUS inline <script> ahead of the module script that boots
 * the WASM (module scripts are deferred, so ordering is guaranteed). Three jobs,
 * in this order — the ordering is load-bearing, see the spec section 4.3:
 *
 *   1. Resolve and WRITE data-platform. It cannot simply be read: the shell's
 *      SHELL_MARKER_JS is an `initialization_script`, which runs before page
 *      scripts only for SAME-ORIGIN pages. A panel-only shell pointed at a
 *      remote Gateway re-asserts the marker from `on_page_load`, which fires at
 *      PageLoadEvent::Finished — AFTER this script. A plain browser never gets
 *      the marker at all. So this script owns the resolution and everything
 *      else (platform_host.rs) is a pure reader.
 *   2. Compute data-flat, which drives the glass degradation. Depends on 1.
 *   3. Probe the CSS baseline and, on failure, replace the page.
 *
 * Steps 1 and 2 run unconditionally, before and independently of the probe
 * verdict: a supported browser still needs its platform and flat attributes.
 *
 * The probe list is kept set-equal to interfaces/webchat/webview-baseline.json
 * by scripts/check_webview_baseline.mjs (edge B).
 */
(function () {
  var el = document.documentElement;

  // ── 1. Platform ─────────────────────────────────────────────────────────
  // Only three buckets are needed; this is not a general UA parser. The
  // ambiguous case resolves to "linux", which is the SAFE direction: flat
  // rendering is a degradation, never a hazard.
  function resolvePlatform() {
    var declared = el.getAttribute('data-platform');
    if (declared === 'macos' || declared === 'windows' || declared === 'linux') {
      return declared;
    }
    var ua = (navigator.userAgent || '') + ' ' + (navigator.platform || '');
    if (/Mac|iPhone|iPad|iPod/i.test(ua)) return 'macos';
    if (/Win/i.test(ua)) return 'windows';
    return 'linux';
  }
  var platform = resolvePlatform();
  el.setAttribute('data-platform', platform);

  // ── 2. Flat mode ────────────────────────────────────────────────────────
  // Two inputs, one attribute, one CSS rule block. `platform` is the value
  // resolved above — NOT a second read of the attribute, which may not have
  // existed a moment ago.
  var reduced = null;
  try {
    reduced = window.matchMedia('(prefers-reduced-transparency: reduce)');
  } catch (e) {
    reduced = null;
  }
  function applyFlat() {
    var flat = platform === 'linux' || !!(reduced && reduced.matches);
    if (flat) {
      el.setAttribute('data-flat', '1');
    } else {
      el.removeAttribute('data-flat');
    }
  }
  applyFlat();
  if (reduced) {
    // A mid-session OS change must still apply. addEventListener is the modern
    // form; addListener is the Safari 13 fallback and costs two lines.
    if (reduced.addEventListener) {
      reduced.addEventListener('change', applyFlat);
    } else if (reduced.addListener) {
      reduced.addListener(applyFlat);
    }
  }

  // ── 3. Baseline probes ──────────────────────────────────────────────────
  var missing = [];
  var cssProbes = [
    ['color', 'oklch(0 0 0)'],
    ['color', 'color-mix(in oklab, red, red)']
  ];
  for (var i = 0; i < cssProbes.length; i++) {
    var p = cssProbes[i];
    var ok = false;
    try {
      ok = !!(window.CSS && CSS.supports && CSS.supports(p[0], p[1]));
    } catch (e) {
      ok = false;
    }
    if (!ok) missing.push(p[0] + ': ' + p[1]);
  }
  if (!(window.CSS && typeof CSS.registerProperty === 'function')) {
    missing.push('CSS.registerProperty');
  }
  if (typeof WebAssembly !== 'object') {
    missing.push('WebAssembly');
  }
  if (missing.length === 0) return;

  el.setAttribute('data-webview-unsupported', '1');

  // The fallback page carries its OWN styles. It must not depend on
  // tailwind.css — that stylesheet is precisely what has failed, because its
  // ~328 oklch() token definitions go invalid at computed-value time and the
  // whole palette collapses. Hex colours and basic layout only.
  var esc = function (s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  };
  var items = missing.map(function (m) { return '<li><code>' + esc(m) + '</code></li>'; }).join('');
  document.body.innerHTML =
    '<div style="font:16px/1.6 -apple-system,Segoe UI,Roboto,sans-serif;' +
    'max-width:34rem;margin:12vh auto;padding:0 1.5rem;color:#1a1a1f;background:#fff">' +
    '<h1 style="font-size:1.4rem;margin:0 0 .75rem">This system&rsquo;s WebView is too old for the Aleph Panel</h1>' +
    '<p style="margin:0 0 1rem;color:#4a4a55">The Panel needs a browser engine that supports modern CSS colour. ' +
    'These capabilities are missing here:</p>' +
    '<ul style="margin:0 0 1.25rem;color:#4a4a55">' + items + '</ul>' +
    '<p style="margin:0 0 1rem;color:#4a4a55"><strong>Minimum:</strong> macOS 13.3+ &middot; WebKitGTK 2.42+ &middot; ' +
    'any evergreen Chromium or Edge WebView2.</p>' +
    '<p style="margin:0;color:#4a4a55">Aleph itself is still running. You can keep working through the ' +
    '<code>aleph</code> CLI, the <code>aleph-tui</code> terminal client, or the Panel on a phone or another ' +
    'machine &mdash; the core is one service with many front ends.</p></div>';
  document.body.setAttribute('style', 'margin:0;background:#fff');
})();
```

- [ ] **Step 2: Extend the guard with edge B, and watch it fail first**

Append to `scripts/check_webview_baseline.mjs`, immediately before the
`if (problems.length)` block:

```js
// ── Edge B: the probe list matches the declaration, BOTH directions ───────
// Set equality, not containment: a one-directional check cannot tell a new
// probe from a removed one, and both are drift.
{
  const PROBE = 'interfaces/webchat/baseline-probe.js';
  const src = readFileSync(PROBE, 'utf8');
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
```

Run `node scripts/check_webview_baseline.mjs`. If edge B is written correctly
against the probe from Step 1, it passes immediately — so **prove it works by
mutation instead**: delete the `['color', 'oklch(0 0 0)']` line from
`baseline-probe.js`, re-run, and confirm:

```
✗ B: interfaces/webchat/baseline-probe.js does not probe declared capability "color|oklch(0 0 0)"
```

Then restore it. Also add `["color", "lch(0 0 0)"]` to the JSON's `css_probes`
temporarily and confirm the same edge reports it in the other direction, then
remove it.

- [ ] **Step 3: Run the full guard to green**

```bash
node scripts/check_webview_baseline.mjs
```

Expected: `✓ webview baseline consistent`.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/baseline-probe.js scripts/check_webview_baseline.mjs
git commit -m "panel: add the WebView baseline probe and pin it to the declaration"
```

---

### Task 3: Inline the probe into the generated index.html

**Files:**
- Modify: `justfile:201-227` (the `wasm` recipe's step 4 heredoc)
- Modify: `scripts/check_webview_baseline.mjs` (add edge C)

**Interfaces:**
- Consumes: `interfaces/webchat/baseline-probe.js` (Task 2).
- Produces: `interfaces/webchat/dist/index.html` containing the probe source
  verbatim inside a `<script>` element that precedes the `<script type="module">`.

- [ ] **Step 1: Change the recipe to compose index.html in two parts**

In `justfile`, the `wasm` recipe's step 4 currently writes the whole
`index.html` from a single quoted heredoc. Split it so the probe is injected
between the head and the module script. Replace the block that starts
`# 4. Runtime index.html` and ends at the `HTMLEOF` line with:

```makefile
    # 4. Runtime index.html. Written in three parts so baseline-probe.js is
    #    inlined VERBATIM: it must run synchronously before the module script
    #    (module scripts are deferred), and it must be byte-identical to its
    #    source so scripts/check_webview_baseline.mjs edge C can pair them.
    cat > {{panel_dist}}/index.html << 'HTMLHEAD'
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>Aleph Panel</title>
        <!-- Inline so the browser never issues its default GET /favicon.ico:
             nothing serves that path, so every page load logged a 404 that cost a
             QA round to chase. A data: URI is inside the Panel CSP
             (img-src 'self' data: https:) and needs no new dist asset. -->
        <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Crect width='32' height='32' rx='7' fill='%230d0d12'/%3E%3Ctext x='16' y='23' text-anchor='middle' font-family='Georgia,serif' font-size='21' fill='%23e8e6e1'%3E%E2%84%B5%3C/text%3E%3C/svg%3E" />
        <link rel="stylesheet" href="/tailwind.css" />
      </head>
      <body class="bg-surface text-text-primary">
        <noscript>This application requires JavaScript to run.</noscript>
        <script>
    HTMLHEAD
    cat {{panel_dir}}/baseline-probe.js >> {{panel_dist}}/index.html
    cat >> {{panel_dist}}/index.html << 'HTMLTAIL'
        </script>
        <script type="module">
          import init from '/aleph_panel.js';
          await init({ module_or_path: '/aleph_panel_bg.wasm' });
        </script>
      </body>
    </html>
    HTMLTAIL
    # 4.5 Baseline consistency (edges A-D).
    node scripts/check_webview_baseline.mjs
```

- [ ] **Step 2: Add edge C to the guard**

Append to `scripts/check_webview_baseline.mjs` before the `if (problems.length)`
block:

```js
// ── Edge C: dist/index.html carries the probe VERBATIM ────────────────────
// Same class as the js/wasm pairing guard in check_panel_dist.mjs: catches a
// one-sided rebuild where the probe changed but dist did not, or vice versa.
{
  const INDEX = 'interfaces/webchat/dist/index.html';
  const probe = readFileSync('interfaces/webchat/baseline-probe.js', 'utf8');
  let index;
  try {
    index = readFileSync(INDEX, 'utf8');
  } catch (e) {
    fail('C', `cannot read ${INDEX}: ${e.message} — run \`just wasm\``);
    index = null;
  }
  if (index !== null) {
    if (!index.includes(probe.trimEnd())) {
      fail('C', `${INDEX} does not contain baseline-probe.js verbatim — run \`just wasm\` and commit dist/`);
    }
    // Ordering is load-bearing: the probe must precede the module script.
    const probeAt = index.indexOf(probe.trimEnd());
    const moduleAt = index.indexOf('<script type="module">');
    if (probeAt >= 0 && moduleAt >= 0 && probeAt > moduleAt) {
      fail('C', `${INDEX} runs the module script before the probe; the probe must be first (it is synchronous, the module is deferred)`);
    }
  }
}
```

- [ ] **Step 3: Rebuild and verify**

```bash
just wasm
```

Expected: the recipe completes and prints `✓ webview baseline consistent`
followed by `✓ WASM: interfaces/webchat/dist/`.

- [ ] **Step 4: Prove edge C goes RED**

Append a space to the last line of `interfaces/webchat/baseline-probe.js`
without rebuilding, then run `node scripts/check_webview_baseline.mjs`. Expected:

```
✗ C: interfaces/webchat/dist/index.html does not contain baseline-probe.js verbatim — run `just wasm` and commit dist/
```

Revert the edit and re-run to green.

- [ ] **Step 5: Verify the fallback page renders (Windows real machine)**

```bash
cargo run --bin aleph-server
```

Open `http://127.0.0.1:18790` in Edge, then in DevTools console run:

```js
CSS.supports = () => false; location.reload();
```

Expected: the fallback page renders with both `color:` capabilities listed, the
`macOS 13.3+ · WebKitGTK 2.42+` line, and the CLI/TUI/phone escape hatches. It
must be readable — if it appears unstyled, the fallback is leaning on
`tailwind.css` and the whole point is lost.

- [ ] **Step 6: Commit**

```bash
git add justfile scripts/check_webview_baseline.mjs interfaces/webchat/dist/index.html
git commit -m "panel: inline the baseline probe ahead of the wasm module script"
```

---

### Task 4: The CSS baseline census (edge D)

**Files:**
- Modify: `scripts/check_webview_baseline.mjs` (add edge D)

**Interfaces:**
- Consumes: `webview-baseline.json`, `interfaces/webchat/dist/tailwind.css`.
- Produces: no new exports; the guard gains two assertions.

**Why this edge is shaped the way it is.** The forward assertion — "every modern
capability in the CSS must be covered by a probe" — **cannot be made honest**: a
scanner only recognizes the patterns it was taught, which is the "enumeration
only covers the world as it was on the day it was written" failure in CLAUDE.md
§0. So this task ships two things: a *reverse* assertion that can be honest
(every probe is still load-bearing), and a deliberately **over-reporting**
census whose failure direction is false-positive, never false-negative.

- [ ] **Step 1: Add edge D**

Append to `scripts/check_webview_baseline.mjs` before the `if (problems.length)`
block:

```js
// ── Edge D: probes are load-bearing, and no unreviewed CSS function appears ──
{
  const CSS_PATH = 'interfaces/webchat/dist/tailwind.css';
  let css;
  try {
    css = readFileSync(CSS_PATH, 'utf8');
  } catch (e) {
    fail('D', `cannot read ${CSS_PATH}: ${e.message} — run \`just wasm\``);
    css = null;
  }

  if (css !== null) {
    // D1 (reverse, honest): every declared CSS probe must still be exercised by
    // the built stylesheet. This catches a probe list rotting into a stale
    // licence — a capability we still gate on that the CSS stopped using.
    for (const [, value] of baseline.css_probes) {
      const fn = value.slice(0, value.indexOf('('));
      if (!fn || !css.includes(`${fn}(`)) {
        fail('D', `${CSS_PATH} no longer uses ${fn}(), but ${BASELINE} still gates on it — drop the probe or find out why the CSS changed`);
      }
    }

    // D2 (forward, over-reporting): every CSS function name in the built
    // stylesheet must be on the reviewed list. A Tailwind upgrade that emits a
    // new function goes RED and a human decides whether the baseline moves.
    // False positives are the intended failure direction: a new name is cheap
    // to review, a silently-shipped capability cliff is not.
    const REVIEWED = new Set([
      // colour
      'oklch', 'oklab', 'color-mix', 'rgb', 'rgba', 'hsl', 'hsla', 'color',
      'light-dark',
      // math / sizing
      'calc', 'min', 'max', 'clamp', 'round', 'var', 'env',
      // layout / transforms
      'translate', 'translateX', 'translateY', 'translateZ', 'translate3d',
      'rotate', 'rotateX', 'rotateY', 'rotateZ', 'scale', 'scaleX', 'scaleY',
      'scale3d', 'skewX', 'skewY', 'perspective', 'matrix', 'matrix3d',
      // filters / effects
      'blur', 'brightness', 'contrast', 'grayscale', 'hue-rotate', 'invert',
      'opacity', 'saturate', 'sepia', 'drop-shadow',
      // gradients / images
      'linear-gradient', 'radial-gradient', 'conic-gradient',
      'repeating-linear-gradient', 'repeating-radial-gradient', 'url', 'image-set',
      // selectors / at-rule conditions
      'not', 'is', 'where', 'has', 'nth-child', 'nth-last-child', 'nth-of-type',
      'selector', 'supports', 'lang', 'dir', 'host', 'slotted',
      // animation
      'cubic-bezier', 'steps', 'attr', 'counter', 'format', 'local',
    ]);
    const seen = new Set();
    for (const m of css.matchAll(/(?:^|[^\w-])([a-zA-Z][\w-]*)\(/g)) {
      seen.add(m[1]);
    }
    const novel = [...seen].filter((n) => !REVIEWED.has(n)).sort();
    if (novel.length) {
      fail('D', `${CSS_PATH} uses CSS function(s) not on the reviewed list: ${novel.join(', ')}.\n` +
        `      This is an OVER-REPORTING census: it goes red on anything new, by design.\n` +
        `      Decide for each one whether it is inside the ${baseline.safari_min} / WebKitGTK ${baseline.webkitgtk_min} floor.\n` +
        `      If yes, add it to REVIEWED in this file. If no, the floor moves and ${BASELINE} changes.`);
    }
  }
}
```

- [ ] **Step 2: Run it and reconcile the list**

```bash
node scripts/check_webview_baseline.mjs
```

The first run will very likely report several function names the `REVIEWED` set
does not have. **This is the census doing its job — its first output is the true
size of the class, not a bug.** For each reported name, look it up against
Safari 16.4 / WebKitGTK 2.42. Every name that is inside the floor gets added to
`REVIEWED` with no further action. Any name outside the floor stops this task:
report it to the user, because the floor decision (D1 in the spec) would have to
change.

- [ ] **Step 3: Verify green**

```bash
node scripts/check_webview_baseline.mjs
```

Expected: `✓ webview baseline consistent`.

- [ ] **Step 4: Prove both halves go RED**

For D1: temporarily add `["color", "lch(0 0 0)"]` to `css_probes` in the JSON —
`lch(` does not appear in the built CSS. Expected:

```
✗ D: interfaces/webchat/dist/tailwind.css no longer uses lch(), but interfaces/webchat/webview-baseline.json still gates on it — ...
```

(Note this also trips edge B, which is correct — the probe does not have it
either.) Revert.

For D2: remove `'oklch'` from `REVIEWED`, re-run, confirm it is reported as
novel, then restore it.

- [ ] **Step 5: Commit**

```bash
git add scripts/check_webview_baseline.mjs
git commit -m "panel: census the built stylesheet against the declared WebView floor"
```

---

### Task 5: wasm-opt becomes a required correctness gate

**Files:**
- Modify: `justfile:194-200` (the wasm recipe's step 3.5)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing importable; `just wasm` now fails when binaryen is absent.

**Context.** Rust 1.82+ enables the WASM 2.0 feature set by default on
`wasm32-unknown-unknown`. That set is inside the Safari 16.4 floor today, so
nothing is broken now — the fence exists so a future toolchain bump cannot
silently enable a feature outside it, whose symptom is the same white screen G1
is about. Because the step now carries correctness, it can no longer skip
silently: a gate that skips when a tool is missing is a gate that exists only on
the author's machine.

- [ ] **Step 1: Replace the optional shrink step**

In `justfile`, replace the whole `# 3.5 Shrink wasm ...` block (the
`if command -v wasm-opt` conditional) with:

```makefile
    # 3.5 Shrink wasm AND fence its feature set.
    #
    # This step is REQUIRED, not an optional optimisation. The --enable-* list
    # is an allow-list: wasm-opt validates the module against exactly these
    # features, so a toolchain bump that silently enables a new one (Rust 1.82+
    # already turns on the WASM 2.0 set by default) fails here instead of white-
    # screening on an older WebKit. Notably absent: --enable-simd. If you are
    # adding a feature to this list, you are moving the floor declared in
    # interfaces/webchat/webview-baseline.json — change that too.
    #
    # -g keeps the name section for crash diagnostics.
    if ! command -v wasm-opt >/dev/null 2>&1; then
        echo "✗ wasm-opt (binaryen) is required." >&2
        echo "  It is not a size optimisation any more: it fences the WASM feature set" >&2
        echo "  against the declared WebView baseline. Install it with one of:" >&2
        echo "    cargo install wasm-opt" >&2
        echo "    brew install binaryen        # macOS" >&2
        echo "    apt install binaryen         # Debian/Ubuntu" >&2
        echo "    winget install WebAssembly.binaryen   # Windows" >&2
        exit 1
    fi
    wasm-opt -Oz -g \
        --enable-bulk-memory --enable-sign-ext --enable-nontrapping-float-to-int \
        --enable-mutable-globals --enable-multivalue --enable-reference-types \
        {{panel_dist}}/aleph_panel_bg.wasm -o {{panel_dist}}/aleph_panel_bg.wasm
    echo "✓ wasm-opt applied (feature set fenced)"
```

- [ ] **Step 2: Verify the happy path still builds**

```bash
just wasm
```

Expected: `✓ wasm-opt applied (feature set fenced)` and the recipe completes.

- [ ] **Step 3: Prove the fence goes RED**

Rebuild the wasm with an out-of-floor feature and confirm wasm-opt rejects it:

```bash
RUSTFLAGS="-C target-feature=+simd128" cargo build -p aleph-panel --lib \
  --target wasm32-unknown-unknown --profile wasm-release
```

Then run the `wasm-bindgen` and `wasm-opt` lines from the recipe by hand against
that artifact. Expected: wasm-opt exits non-zero with a validation error naming
a SIMD instruction. Rebuild cleanly (`just wasm`) afterwards so `dist/` is not
left holding a SIMD binary.

- [ ] **Step 4: Prove the missing-tool branch goes RED**

Temporarily rename the wasm-opt binary (or prepend an empty dir to `PATH`), run
`just wasm`, and confirm it exits 1 with the four install commands. Restore.

- [ ] **Step 5: Commit**

```bash
git add justfile
git commit -m "build: make wasm-opt required and fence the panel's wasm feature set"
```

---

### Task 6: Panel platform reader and the shell's three-platform marker

**Files:**
- Create: `interfaces/webchat/src/platform_host.rs`
- Modify: `interfaces/webchat/src/lib.rs:36` (module list)
- Modify: `desktop/shell/src/main.rs:88-95` (`SHELL_MARKER_JS`)

**Interfaces:**
- Consumes: the `data-platform` attribute written by `baseline-probe.js` (Task 2).
- Produces:
  ```rust
  pub enum HostPlatform { MacOs, Windows, Linux }
  pub fn host() -> HostPlatform
  ```
  in `crate::platform_host`. `host()` is a **pure reader** — it contains no UA
  fallback, because the probe already resolved and wrote the attribute. An
  unrecognised or absent attribute yields `Linux`, matching the probe's safe
  direction.

- [ ] **Step 1: Write the failing test**

Create `interfaces/webchat/src/platform_host.rs`:

```rust
//! Which platform's WebView is rendering this Panel.
//!
//! A **pure reader** of the `data-platform` attribute on `<html>`. The
//! resolution — including the UA fallback for pages the shell's
//! `initialization_script` never reached — belongs to `baseline-probe.js`,
//! which runs synchronously before the WASM boots and writes the attribute
//! (see the spec, section 3.2). Duplicating that fallback here would be a
//! second implementation of the same decision.
//!
//! Sibling of [`crate::platform::wide::views::voice::audio::is_native_shell`],
//! which reads `data-shell` the same way and for the same reason: these facts
//! are declared by the host, not derived in WASM, where `cfg(target_os)` is
//! always `unknown`.

/// The three WebView engines Aleph ships against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    /// WKWebView.
    MacOs,
    /// Edge WebView2 (Chromium).
    Windows,
    /// WebKitGTK.
    Linux,
}

impl HostPlatform {
    /// Parse the `data-platform` value. Anything unrecognised — including a
    /// missing attribute — is `Linux`: that is the direction the probe already
    /// chose for its own ambiguous case, because flat rendering is a
    /// degradation and never a hazard.
    #[must_use]
    pub fn from_attribute(value: Option<&str>) -> Self {
        match value {
            Some("macos") => Self::MacOs,
            Some("windows") => Self::Windows,
            _ => Self::Linux,
        }
    }
}

/// This document's host platform.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn host() -> HostPlatform {
    let attr = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|e| e.get_attribute("data-platform"));
    HostPlatform::from_attribute(attr.as_deref())
}

/// Non-wasm builds (unit tests on the host toolchain) have no document.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn host() -> HostPlatform {
    HostPlatform::Linux
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values_map_to_their_platform() {
        assert_eq!(HostPlatform::from_attribute(Some("macos")), HostPlatform::MacOs);
        assert_eq!(HostPlatform::from_attribute(Some("windows")), HostPlatform::Windows);
        assert_eq!(HostPlatform::from_attribute(Some("linux")), HostPlatform::Linux);
    }

    #[test]
    fn absent_or_unknown_resolves_to_linux_the_safe_direction() {
        // Flat rendering is a degradation, never a hazard, so an unknown host
        // gets the conservative answer — the same choice baseline-probe.js
        // makes for an unrecognised user agent.
        assert_eq!(HostPlatform::from_attribute(None), HostPlatform::Linux);
        assert_eq!(HostPlatform::from_attribute(Some("")), HostPlatform::Linux);
        assert_eq!(HostPlatform::from_attribute(Some("haiku")), HostPlatform::Linux);
    }
}
```

- [ ] **Step 2: Run the tests and verify they FAIL**

```bash
cargo test -p aleph-panel --lib platform_host
```

Expected: FAIL — `error[E0433]: failed to resolve: use of undeclared crate or module 'platform_host'`, because the module is not declared yet.

- [ ] **Step 3: Declare the module**

In `interfaces/webchat/src/lib.rs`, add in alphabetical position (after
`pub mod platform;` on line 36):

```rust
pub mod platform_host;
```

- [ ] **Step 4: Run the tests and verify they PASS**

```bash
cargo test -p aleph-panel --lib platform_host
```

Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Complete the shell marker for all three platforms**

In `desktop/shell/src/main.rs`, replace the two `SHELL_MARKER_JS` definitions
(currently macOS-only for `data-platform`) with three:

```rust
#[cfg(target_os = "macos")]
const SHELL_MARKER_JS: &str = "var e=document.documentElement;\
    e.setAttribute('data-shell','aleph-tauri');\
    e.setAttribute('data-platform','macos');";
#[cfg(target_os = "windows")]
const SHELL_MARKER_JS: &str = "var e=document.documentElement;\
    e.setAttribute('data-shell','aleph-tauri');\
    e.setAttribute('data-platform','windows');";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const SHELL_MARKER_JS: &str = "var e=document.documentElement;\
    e.setAttribute('data-shell','aleph-tauri');\
    e.setAttribute('data-platform','linux');";
```

Update the doc comment above them: it currently says the platform is recorded
"on macOS" so the Panel's CSS can leave room for the traffic lights. Extend it:

```rust
/// Injected into every document the webview loads (splash and Panel).
/// Marks the page as shell-hosted and records the platform.
///
/// macOS uses it to leave room for the overlay traffic lights and let the
/// vibrancy material show through; Linux uses it to drop glass to opaque
/// solids (`html[data-flat="1"]`, see the Panel's tailwind.css); Windows uses
/// it for neither today, and is declared anyway so the attribute is never
/// absent on a shell-hosted page — a reader that has to distinguish "Windows"
/// from "the marker did not run" has no way to.
///
/// This is NOT the only writer. `baseline-probe.js` resolves and writes the
/// same attribute before the WASM boots, because this script is an
/// `initialization_script` and therefore runs before page scripts only for
/// SAME-ORIGIN pages — a panel-only shell pointed at a remote Gateway does not
/// get it until `on_page_load`, which is too late. The two agree by
/// construction: the probe keeps a value it finds already set.
```

- [ ] **Step 6: Verify the shell still compiles**

```bash
just _stage-shell-placeholders
cargo check -p aleph-desktop-shell
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/platform_host.rs interfaces/webchat/src/lib.rs desktop/shell/src/main.rs
git commit -m "desktop: declare data-platform on all three hosts and read it from the Panel"
```

---

### Task 7: Re-key the glass degradation to `data-flat`

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css:1084` and `:1155`

**Interfaces:**
- Consumes: the `data-flat` attribute written by `baseline-probe.js` (Task 2).
- Produces: no exports. After this task, glass is opaque whenever
  `html[data-flat="1"]`, which is Linux OR reduced-transparency.

**Why not simply zero the tokens.** `backdrop-filter: blur(0px)` still creates a
compositing layer and still costs on WebKitGTK — which is the exact thing being
fixed. The `backdrop-filter: none` half is required, so the rule block cannot be
avoided, and CSS cannot OR an `@media` condition with an attribute selector.
Hence a JS-computed attribute with the two inputs folded into it.

- [ ] **Step 1: Re-key the main block**

At `interfaces/webchat/styles/tailwind.css:1084`, replace the opening line

```css
@media (prefers-reduced-transparency: reduce) {
```

with

```css
/* Flat mode. Two inputs, one attribute, one block:
     - macOS/GNOME "Reduce transparency" (prefers-reduced-transparency), and
     - Linux, where WebKitGTK pays a large and sometimes catastrophic cost for
       backdrop-filter on machines without hardware acceleration.
   `data-flat` is computed by interfaces/webchat/baseline-probe.js, which runs
   synchronously before the WASM boots and re-evaluates on a matchMedia change.
   It is deliberately NOT an @media query any more: CSS cannot OR a media
   condition with a selector, and duplicating this ~70-line block for Linux
   would be a second source of truth. Accessibility note: degradation now
   depends on JS, but the risk is bounded — the probe is a synchronous inline
   script ahead of the module script, so if it did not run the Panel did not
   load either. There is no state where the page renders and the degradation
   is missing. */
html[data-flat="1"] {
```

Everything inside the block is unchanged. **Verify the closing brace still
balances**: the block previously ended at line 1153 with `}` closing the rules
and line 1154 `}` closing the `@media`. A selector block needs only one closing
brace, so the now-surplus `}` must be removed — read lines 1148-1156 and delete
exactly the one that closed the `@media`.

- [ ] **Step 2: Re-key the dark-mode block**

At line 1155 (before the edit; re-locate it by searching for
`prefers-reduced-transparency`), the compound query

```css
@media (prefers-reduced-transparency: reduce) and (prefers-color-scheme: dark) {
  :root:not(.light), :root:not(.light)[data-material] {
    --mat-raised: oklch(0.20 0.018 310) !important;
  }
}
```

becomes — only the transparency half moves to the attribute; the colour-scheme
half stays a media query, because that is what it is:

```css
@media (prefers-color-scheme: dark) {
  :root[data-flat="1"]:not(.light), :root[data-flat="1"]:not(.light)[data-material] {
    --mat-raised: oklch(0.20 0.018 310) !important;
  }
}
```

- [ ] **Step 3: Verify no `prefers-reduced-transparency` remains**

```bash
grep -n "prefers-reduced-transparency" interfaces/webchat/styles/tailwind.css
```

Expected: no output. Any remaining occurrence is a third trigger and a second
source of truth.

- [ ] **Step 4: Rebuild and verify the rule survived minification**

```bash
just wasm
grep -c 'data-flat' interfaces/webchat/dist/tailwind.css
```

Expected: a non-zero count. Zero means Tailwind dropped the block (usually a
brace imbalance from Step 1) — go back and re-read the block boundaries.

- [ ] **Step 5: Verify on the Windows real machine**

Start the server, open the Panel in Edge, and in the console:

```js
document.documentElement.setAttribute('data-flat','1');
getComputedStyle(document.querySelector('.glass')).backdropFilter
```

Expected: `"none"`. Then remove the attribute and confirm it returns to a
`blur(...)` value. This proves the re-key works even though Windows is not a
platform that triggers it.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/styles/tailwind.css interfaces/webchat/dist/tailwind.css
git commit -m "panel: key glass degradation on data-flat so Linux shares the reduced-transparency path"
```

---

### Task 8: Build-time brotli precompression

**Files:**
- Create: `scripts/precompress_dist.mjs`
- Modify: `justfile` (wasm recipe, new step after the baseline check)

**Interfaces:**
- Consumes: `interfaces/webchat/dist/*`.
- Produces: `interfaces/webchat/dist/<name>.br` for every dist file larger than
  `MIN_BYTES` whose brotli output is actually smaller. Exits non-zero if any
  round-trip check fails.

- [ ] **Step 1: Write the producer**

Create `scripts/precompress_dist.mjs`:

```js
#!/usr/bin/env node
// Produce brotli siblings for the committed panel dist.
//
// Why build-time and not runtime: the gateway currently gzips on every ETag
// miss, which means zstd-decompressing ~22 MB out of the binary and then
// gzip-compressing it again to send ~5 MB. Precompressing moves that work to
// `just wasm` (paid once) and drops the wire payload by roughly a third.
//
// Why the outputs are COMMITTED: interfaces/webchat/dist/ is a git-tracked
// build output — the release workflow embeds it verbatim and no job owns a
// WASM toolchain (see the header of scripts/check_panel_dist.mjs). A `.br`
// that is out of sync with its source would serve WRONG BYTES, which is worse
// than the problem this fixes, so check_panel_dist.mjs pairs them in both
// directions.
//
// Usage: node scripts/precompress_dist.mjs [dist-dir]
import { readdirSync, readFileSync, writeFileSync, statSync, unlinkSync } from 'node:fs';
import { brotliCompressSync, brotliDecompressSync, constants } from 'node:zlib';

const dir = process.argv[2] || 'interfaces/webchat/dist';

// Below one TCP initial window there is nothing to win, and a `.br` larger than
// its source is pure loss. The criterion is size plus measured benefit — NOT an
// extension allow-list, which would silently miss the next asset type.
export const MIN_BYTES = 4096;

const failures = [];
let written = 0;
let skipped = 0;

for (const name of readdirSync(dir).sort()) {
  if (name.endsWith('.br')) continue;
  const path = `${dir}/${name}`;
  const st = statSync(path);
  if (!st.isFile()) continue;

  const brPath = `${path}.br`;

  if (st.size < MIN_BYTES) {
    // Remove a stale sibling if the file shrank below the threshold, so the
    // bidirectional guard in check_panel_dist.mjs cannot trip on a leftover.
    try { unlinkSync(brPath); } catch { /* nothing to remove */ }
    skipped++;
    continue;
  }

  const source = readFileSync(path);
  const compressed = brotliCompressSync(source, {
    params: {
      [constants.BROTLI_PARAM_QUALITY]: 11,
      // 24 is the maximum window for STANDARD brotli. Anything above it is the
      // Large-Window extension, which `Content-Encoding: br` does not cover.
      [constants.BROTLI_PARAM_LGWIN]: 24,
      [constants.BROTLI_PARAM_SIZE_HINT]: source.length,
    },
  });

  if (compressed.length >= source.length) {
    try { unlinkSync(brPath); } catch { /* nothing to remove */ }
    console.log(`  ${name}: brotli is not smaller (${compressed.length} >= ${source.length}), skipped`);
    skipped++;
    continue;
  }

  // Round-trip immediately. A corrupt sibling is the one failure mode that
  // reaches users as wrong bytes rather than as an error.
  const back = brotliDecompressSync(compressed);
  if (!back.equals(source)) {
    failures.push(`${name}: brotli round-trip did not reproduce the source`);
    continue;
  }

  writeFileSync(brPath, compressed);
  const pct = ((1 - compressed.length / source.length) * 100).toFixed(1);
  console.log(`  ${name}: ${source.length} -> ${compressed.length} (-${pct}%)`);
  written++;
}

if (failures.length) {
  console.error(failures.map((f) => `✗ ${f}`).join('\n'));
  process.exit(1);
}
console.log(`✓ precompressed ${written} file(s), skipped ${skipped}`);
```

- [ ] **Step 2: Run it and record the real numbers**

```bash
node scripts/precompress_dist.mjs
```

Expected: `aleph_panel_bg.wasm`, `aleph_panel.js` and `tailwind.css` each get a
`.br`; `index.html` is skipped (under 4 KiB). **Write the observed wasm figure
down** — it replaces the estimate in the spec and in the commit message. It
should land near 3.4 MB against the 5,020,809-byte gzip baseline.

- [ ] **Step 3: Verify the round-trip independently**

```bash
node -e "const z=require('zlib'),f=require('fs');const a=f.readFileSync('interfaces/webchat/dist/aleph_panel_bg.wasm');const b=z.brotliDecompressSync(f.readFileSync('interfaces/webchat/dist/aleph_panel_bg.wasm.br'));console.log(a.equals(b)?'IDENTICAL':'MISMATCH')"
```

Expected: `IDENTICAL`.

- [ ] **Step 4: Wire it into the recipe**

In `justfile`, immediately after the `node scripts/check_webview_baseline.mjs`
line added in Task 3, add:

```makefile
    # 4.6 Brotli siblings. Committed alongside dist/ because the release
    #     workflow embeds dist verbatim; check_panel_dist.mjs pairs them.
    node scripts/precompress_dist.mjs
```

- [ ] **Step 5: Verify the full recipe**

```bash
just wasm
```

Expected: completes, with the precompress output between the baseline check and
the dist pair check.

- [ ] **Step 6: Commit** (the `.br` artifacts are committed in Task 17 with the
final rebuild; this commit is the tooling only)

```bash
git add scripts/precompress_dist.mjs justfile
git commit -m "build: precompress the panel dist with brotli at build time"
```

---

### Task 9: Pair the brotli siblings in the dist guard

**Files:**
- Modify: `scripts/check_panel_dist.mjs`

**Interfaces:**
- Consumes: `scripts/precompress_dist.mjs`'s `MIN_BYTES` (imported, not
  retyped — a second copy of the threshold is a second source of truth).
- Produces: two new assertions in the existing guard.

- [ ] **Step 1: Add both directions**

At the top of `scripts/check_panel_dist.mjs`, extend the imports:

```js
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { brotliDecompressSync } from 'node:zlib';
import { MIN_BYTES } from './precompress_dist.mjs';
```

Then, immediately before the script's final success message, add:

```js
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
      brProblems.push(`${name} is over ${MIN_BYTES} bytes but has no ${name}.br — run \`node scripts/precompress_dist.mjs\``);
    }
  }

  if (brProblems.length) {
    console.error(brProblems.map((p) => `✗ ${p}`).join('\n'));
    process.exit(1);
  }
}
```

Note: direction 2 will report a file whose brotli output was not smaller than
its source (the producer skips those). If that ever happens for a real asset,
the producer and the guard disagree — extend both together, never just the
guard.

- [ ] **Step 2: Run to green**

```bash
node scripts/check_panel_dist.mjs
```

Expected: the existing js/wasm pair message plus no brotli complaints.

- [ ] **Step 3: Prove direction 1 goes RED**

Two mutations, because direction 1 covers two distinct failures — bytes that
are not brotli at all, and bytes that are valid brotli of the wrong content.

```bash
# 1a — invalid: replace the stream with garbage
printf 'not brotli' > interfaces/webchat/dist/tailwind.css.br
node scripts/check_panel_dist.mjs   # ✗ tailwind.css.br is not valid brotli: ...

# 1b — stale: valid brotli, wrong content
node -e "const z=require('zlib'),f=require('fs');f.writeFileSync('interfaces/webchat/dist/tailwind.css.br',z.brotliCompressSync(Buffer.from('stale content')))"
node scripts/check_panel_dist.mjs   # ✗ tailwind.css.br decompresses to something other than tailwind.css — it is STALE
```

Restore with `node scripts/precompress_dist.mjs` after each.

> **Do not use `printf '\x00' >> …`.** An earlier version of this step said to,
> and it cannot work: a brotli decoder stops at the final block and ignores
> trailing bytes, so the file still decompresses to a byte-identical copy of
> the source and the guard correctly stays green. Measured 2026-08-22.
> A falsification that cannot fail proves nothing about the guard.

- [ ] **Step 4: Prove direction 2 goes RED**

```bash
rm interfaces/webchat/dist/tailwind.css.br
node scripts/check_panel_dist.mjs
```

Expected: `✗ tailwind.css is over 4096 bytes but has no tailwind.css.br — run ...`.
Restore with `node scripts/precompress_dist.mjs`.

- [ ] **Step 5: Commit**

```bash
git add scripts/check_panel_dist.mjs
git commit -m "build: pair brotli siblings with their sources in the dist guard"
```

---

### Task 10: Serve the precompressed representation

**Files:**
- Modify: `src/gateway/control_plane/server.rs:42-96` (`serve_static_or_index`)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `dist/<path>.br` embedded via `ControlPlaneAssets` (Tasks 8–9).
- Produces: no new exports. Behaviour: `GET /<path>` with
  `Accept-Encoding: br` and an embedded `<path>.br` returns the compressed bytes
  with `Content-Encoding: br`; the `ETag` is **always** derived from the
  identity asset and the response **always** carries `Vary: Accept-Encoding`.

**The correctness trap.** An ETag that follows the served representation gives a
client that switched `Accept-Encoding` a false 304 — it receives brotli bytes
believing they are identity. The ETag must be computed from the identity asset
before any representation is chosen.

**Why `CompressionLayer` is not removed.** tower-http's compression layer passes
through any response that already carries a `Content-Encoding`. Setting the
header is therefore sufficient to suppress runtime compression for these
assets, while files without a `.br` keep their gzip path and a client that does
not advertise `br` is unaffected. **This is asserted below with a real request
rather than taken from documentation.**

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/gateway/control_plane/server.rs`:

```rust
    /// The wire fact the whole precompression design rests on.
    #[tokio::test]
    async fn brotli_is_served_when_the_client_accepts_it() {
        let Some(name) = ControlPlaneAssets::iter().find(|n| {
            ControlPlaneAssets::get(&format!("{n}.br")).is_some()
        }) else {
            // No precompressed asset embedded in this build (dist not built);
            // skip rather than fail — the guard for that is check_panel_dist.
            return;
        };
        let path = name.to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "br, gzip".parse().unwrap());
        let resp = serve_static_or_index(headers, AxumPath(path.clone())).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br",
            "a client advertising br must receive the precompressed sibling"
        );
        assert_eq!(
            resp.headers().get(header::VARY).unwrap(),
            "accept-encoding",
            "without Vary a shared cache would hand brotli bytes to an identity client"
        );
    }

    /// A client that does not advertise brotli keeps the old behaviour.
    #[tokio::test]
    async fn identity_is_served_when_brotli_is_not_accepted() {
        let Some(name) = ControlPlaneAssets::iter().find(|n| {
            ControlPlaneAssets::get(&format!("{n}.br")).is_some()
        }) else {
            return;
        };
        let path = name.to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "gzip".parse().unwrap());
        let resp = serve_static_or_index(headers, AxumPath(path)).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().get(header::CONTENT_ENCODING).is_none(),
            "a gzip-only client must get identity bytes and let CompressionLayer decide"
        );
    }

    /// The trap: the validator must describe the RESOURCE, not the encoding.
    #[tokio::test]
    async fn the_etag_does_not_change_with_the_accepted_encoding() {
        let Some(name) = ControlPlaneAssets::iter().find(|n| {
            ControlPlaneAssets::get(&format!("{n}.br")).is_some()
        }) else {
            return;
        };
        let path = name.to_string();

        let mut br = HeaderMap::new();
        br.insert(header::ACCEPT_ENCODING, "br".parse().unwrap());
        let with_br = serve_static_or_index(br, AxumPath(path.clone())).await;

        let plain = serve_static_or_index(HeaderMap::new(), AxumPath(path)).await;

        assert_eq!(
            with_br.headers().get(header::ETAG),
            plain.headers().get(header::ETAG),
            "an encoding-dependent ETag lets a client that switched Accept-Encoding \
             take a 304 and then decode brotli bytes as identity"
        );
    }
```

- [ ] **Step 2: Run and verify they FAIL**

```bash
cargo test -p alephcore --lib control_plane::server
```

Expected: `brotli_is_served_when_the_client_accepts_it` fails on the
`CONTENT_ENCODING` unwrap (None), and the `VARY` assertion likewise.

- [ ] **Step 3: Implement the negotiation**

Replace the body of `serve_static_or_index` from the `match ControlPlaneAssets::get(&path)`
line down to the closing brace of its `Some(content) => { ... }` arm with:

```rust
    // Try to serve as static asset first
    match ControlPlaneAssets::get(&path) {
        Some(content) => {
            // Content-hash ETag over the IDENTITY representation — never over
            // whichever encoding we happen to serve. An encoding-dependent
            // validator lets a client that switched `Accept-Encoding` take a
            // 304 and then decode brotli bytes as identity. Weak because the
            // wire representation varies with Content-Encoding, which is
            // exactly what `Vary` announces.
            let etag = format!("W/\"{}\"", hex::encode(content.metadata.sha256_hash()));

            // Revalidation hit: the client already holds this exact asset → 304
            // with no body. This turns a repeat open from a multi-MB download
            // into a tiny round-trip.
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|inm| inm.split(',').any(|t| t.trim() == etag))
            {
                return (
                    StatusCode::NOT_MODIFIED,
                    [
                        (header::ETAG, etag.as_str()),
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::VARY, "accept-encoding"),
                    ],
                )
                    .into_response();
            }

            let mime = mime_guess::from_path(&path).first_or_octet_stream();

            // Precompressed sibling, produced by `just wasm` and committed
            // alongside dist/ (scripts/precompress_dist.mjs). Serving it sets
            // `Content-Encoding`, which makes tower-http's CompressionLayer
            // pass the response straight through — so the 22 MB WASM is neither
            // gzipped at request time nor sent uncompressed. Assets without a
            // sibling fall through to the layer's gzip exactly as before.
            let brotli = accepts_brotli(&headers)
                .then(|| ControlPlaneAssets::get(&format!("{path}.br")))
                .flatten();

            match brotli {
                Some(compressed) => (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, mime.as_ref()),
                        (header::CONTENT_ENCODING, "br"),
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::ETAG, etag.as_str()),
                        (header::VARY, "accept-encoding"),
                    ],
                    compressed.data,
                )
                    .into_response(),
                None => (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, mime.as_ref()),
                        // Cacheable but must revalidate via ETag before reuse:
                        // always fresh after a deploy, never re-transfers
                        // unchanged bytes.
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::ETAG, etag.as_str()),
                        (header::VARY, "accept-encoding"),
                    ],
                    content.data,
                )
                    .into_response(),
            }
        }
```

Add the helper above `serve_index`:

```rust
/// Does this request advertise brotli?
///
/// A deliberately simple token scan rather than full q-value negotiation: the
/// only decision here is "precompressed sibling or not", and a client that
/// sends `br;q=0` while also sending it as a token is not a real client. If a
/// weighted decision is ever needed, that is a different function, not a
/// widening of this one.
fn accepts_brotli(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.split(';').next().unwrap_or_default().trim() == "br")
        })
}
```

- [ ] **Step 4: Run and verify they PASS**

```bash
cargo test -p alephcore --lib control_plane::server
```

Expected: all five tests in the module pass (the two pre-existing plus the three
new).

- [ ] **Step 5: Verify on the wire, Windows real machine**

The `CompressionLayer` pass-through claim is the one assumption this task rests
on, and it must be measured, not read:

```bash
cargo run --bin aleph-server
```

In another shell:

```bash
curl -sS -o NUL -D - -H "Accept-Encoding: br" http://127.0.0.1:18790/aleph_panel_bg.wasm
curl -sS -o NUL -D - -H "Accept-Encoding: gzip" http://127.0.0.1:18790/aleph_panel_bg.wasm
```

Expected: the first shows `content-encoding: br` and a `content-length` near the
figure recorded in Task 8 Step 2 — **not** `br` wrapped in `gzip`, which would
mean the layer double-encoded. The second shows `content-encoding: gzip` and a
length near 5.0 MB.

- [ ] **Step 6: Refresh the stale comment**

The router's `CompressionLayer` comment still claims "the WASM alone is ~15.5 MB
uncompressed → ~3.7 MB gzipped". Replace it with the measured figures and the
new division of labour:

```rust
        // Runtime gzip for assets WITHOUT a precompressed sibling (index.html,
        // anything under the 4 KiB threshold). The large payloads — the ~22 MB
        // WASM above all — are served from committed `.br` files by
        // `serve_static_or_index`, and this layer passes those through
        // untouched because they already carry a `Content-Encoding`.
        // 304 revalidations carry no body, so nothing runs on a cache hit.
        .layer(CompressionLayer::new())
```

- [ ] **Step 7: Commit**

```bash
git add src/gateway/control_plane/server.rs
git commit -m "gateway: serve precompressed brotli panel assets with an identity-derived ETag"
```

---

### Task 11: The shared Range parser

**Files:**
- Create: `src/gateway/server/byte_range.rs`
- Modify: `src/gateway/server/mod.rs` (module declaration)

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  pub enum RangeVerdict {
      Whole,
      Satisfiable { start: u64, end: u64 },  // end INCLUSIVE
      Unsatisfiable,
  }
  pub fn parse_range(header: Option<&str>, total: u64) -> RangeVerdict;
  impl RangeVerdict { pub fn content_range(&self, total: u64) -> String; }
  ```

**Design rules, all load-bearing:**
- Single range only. `multipart/byteranges` is deliberately unimplemented —
  browser media elements and GStreamer issue single ranges. A multi-range
  request yields `Whole` (RFC 9110 permits ignoring a Range you do not support
  and answering 200), **not** 416: refusing a request we could satisfy in full
  would be a regression.
- Anything malformed also yields `Whole`, for the same reason.
- `end` is inclusive, matching the HTTP wire format.

- [ ] **Step 1: Write the failing tests**

Create `src/gateway/server/byte_range.rs`:

```rust
//! Single-range HTTP `Range` parsing, shared by every byte route.
//!
//! Both `/artifact` and `/canvas-asset` need this and neither may grow its own
//! copy. Without Range support, WebKitGTK — which plays media through
//! GStreamer — cannot seek: the scrub bar does nothing, audio does not buffer,
//! and large files can fail outright.
//!
//! # What is deliberately not here
//!
//! `multipart/byteranges`. Browser media elements and GStreamer issue single
//! ranges; multi-range buys nothing and costs a whole response encoding. A
//! multi-range request is answered with the WHOLE resource (RFC 9110 lets a
//! server ignore a `Range` it does not support), never with 416 — refusing a
//! request we can satisfy in full would be a regression, not a safety measure.
//!
//! # This is a representation concern
//!
//! Callers must apply it AFTER every authorization gate. A range must never be
//! the reason a byte is reachable.

/// What to do with a request's `Range` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeVerdict {
    /// No usable range — send 200 with the entire body. Covers "no header",
    /// "malformed", and "multi-range".
    Whole,
    /// Send 206 with `[start, end]`, both inclusive.
    Satisfiable { start: u64, end: u64 },
    /// Send 416 with `Content-Range: bytes */<total>`.
    Unsatisfiable,
}

impl RangeVerdict {
    /// The `Content-Range` header value for this verdict, or `None` when the
    /// response carries no `Content-Range` (i.e. [`Self::Whole`]).
    #[must_use]
    pub fn content_range(&self, total: u64) -> Option<String> {
        match self {
            Self::Whole => None,
            Self::Satisfiable { start, end } => Some(format!("bytes {start}-{end}/{total}")),
            Self::Unsatisfiable => Some(format!("bytes */{total}")),
        }
    }
}

/// Parse a single-range `Range` header against a known total length.
#[must_use]
pub fn parse_range(header: Option<&str>, total: u64) -> RangeVerdict {
    let Some(raw) = header else {
        return RangeVerdict::Whole;
    };
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        // Any other unit ("items=0-5") is one we do not implement.
        return RangeVerdict::Whole;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        // Multi-range: answer in full rather than refuse. See the module doc.
        return RangeVerdict::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return RangeVerdict::Whole;
    };
    let (first, last) = (first.trim(), last.trim());

    // A zero-length resource can satisfy no range at all.
    if total == 0 {
        return RangeVerdict::Unsatisfiable;
    }

    if first.is_empty() {
        // Suffix form: `bytes=-N` means the LAST N bytes.
        let Ok(n) = last.parse::<u64>() else {
            return RangeVerdict::Whole;
        };
        if n == 0 {
            return RangeVerdict::Unsatisfiable;
        }
        let start = total.saturating_sub(n);
        return RangeVerdict::Satisfiable { start, end: total - 1 };
    }

    let Ok(start) = first.parse::<u64>() else {
        return RangeVerdict::Whole;
    };
    if start >= total {
        return RangeVerdict::Unsatisfiable;
    }
    if last.is_empty() {
        // Open-ended: `bytes=N-`.
        return RangeVerdict::Satisfiable { start, end: total - 1 };
    }
    let Ok(end) = last.parse::<u64>() else {
        return RangeVerdict::Whole;
    };
    if end < start {
        return RangeVerdict::Unsatisfiable;
    }
    RangeVerdict::Satisfiable {
        start,
        // A client may ask past the end; clamp rather than refuse.
        end: end.min(total - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOTAL: u64 = 1000;

    #[test]
    fn absent_header_is_whole() {
        assert_eq!(parse_range(None, TOTAL), RangeVerdict::Whole);
    }

    #[test]
    fn closed_range_is_inclusive() {
        assert_eq!(
            parse_range(Some("bytes=100-199"), TOTAL),
            RangeVerdict::Satisfiable { start: 100, end: 199 }
        );
    }

    #[test]
    fn open_ended_runs_to_the_last_byte() {
        assert_eq!(
            parse_range(Some("bytes=900-"), TOTAL),
            RangeVerdict::Satisfiable { start: 900, end: 999 }
        );
    }

    #[test]
    fn suffix_form_takes_the_last_n_bytes() {
        assert_eq!(
            parse_range(Some("bytes=-100"), TOTAL),
            RangeVerdict::Satisfiable { start: 900, end: 999 }
        );
    }

    #[test]
    fn a_suffix_longer_than_the_resource_clamps_to_the_whole_resource() {
        assert_eq!(
            parse_range(Some("bytes=-5000"), TOTAL),
            RangeVerdict::Satisfiable { start: 0, end: 999 }
        );
    }

    #[test]
    fn an_end_past_the_resource_clamps_rather_than_refusing() {
        assert_eq!(
            parse_range(Some("bytes=900-99999"), TOTAL),
            RangeVerdict::Satisfiable { start: 900, end: 999 }
        );
    }

    #[test]
    fn a_start_past_the_end_is_unsatisfiable() {
        assert_eq!(parse_range(Some("bytes=1000-"), TOTAL), RangeVerdict::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=5000-6000"), TOTAL), RangeVerdict::Unsatisfiable);
    }

    #[test]
    fn an_inverted_range_is_unsatisfiable() {
        assert_eq!(parse_range(Some("bytes=200-100"), TOTAL), RangeVerdict::Unsatisfiable);
    }

    #[test]
    fn a_zero_length_suffix_is_unsatisfiable() {
        assert_eq!(parse_range(Some("bytes=-0"), TOTAL), RangeVerdict::Unsatisfiable);
    }

    #[test]
    fn an_empty_resource_satisfies_nothing() {
        assert_eq!(parse_range(Some("bytes=0-0"), 0), RangeVerdict::Unsatisfiable);
    }

    /// Answering a multi-range request IN FULL is correct and deliberate.
    /// Returning 416 here would refuse a request we can satisfy.
    #[test]
    fn multi_range_falls_back_to_the_whole_resource() {
        assert_eq!(parse_range(Some("bytes=0-99,200-299"), TOTAL), RangeVerdict::Whole);
    }

    #[test]
    fn malformed_and_foreign_units_fall_back_to_the_whole_resource() {
        for h in ["bytes=", "bytes=abc-def", "items=0-5", "0-99", "bytes=--5"] {
            assert_eq!(parse_range(Some(h), TOTAL), RangeVerdict::Whole, "input: {h}");
        }
    }

    #[test]
    fn content_range_renders_the_wire_form() {
        assert_eq!(
            RangeVerdict::Satisfiable { start: 100, end: 199 }.content_range(TOTAL),
            Some("bytes 100-199/1000".to_string())
        );
        assert_eq!(
            RangeVerdict::Unsatisfiable.content_range(TOTAL),
            Some("bytes */1000".to_string())
        );
        assert_eq!(RangeVerdict::Whole.content_range(TOTAL), None);
    }
}
```

- [ ] **Step 2: Run and verify they FAIL**

```bash
cargo test -p alephcore --lib byte_range
```

Expected: FAIL — the module is not declared, so nothing compiles.

- [ ] **Step 3: Declare the module**

In `src/gateway/server/mod.rs`, add in alphabetical position among the existing
`mod` declarations:

```rust
pub mod byte_range;
```

- [ ] **Step 4: Run and verify they PASS**

```bash
cargo test -p alephcore --lib byte_range
```

Expected: `test result: ok. 13 passed`.

- [ ] **Step 5: Prove the tests bite**

Change `end: end.min(total - 1)` to `end` (dropping the clamp) and confirm
`an_end_past_the_resource_clamps_rather_than_refusing` fails. Restore.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/server/byte_range.rs src/gateway/server/mod.rs
git commit -m "gateway: add a shared single-range HTTP Range parser"
```

---

### Task 12: Range on the artifact route

**Files:**
- Modify: `src/gateway/server/artifact_route.rs` (constant, state, `serve_artifact`)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `crate::gateway::server::byte_range::{parse_range, RangeVerdict}` (Task 11).
- Produces: no new exports. Behaviour: successful reads carry
  `Accept-Ranges: bytes`; a satisfiable `Range` yields 206 with `Content-Range`;
  an unsatisfiable one yields 416 with `Content-Range: bytes */<total>`; both
  carry `ARTIFACT_DOCUMENT_CSP` when the record is an active document.

**Two things that are easy to get wrong:**
1. Range is applied **after** every existing gate — insecure-transport refusal,
   origin policy, rate limit, capability→session, segment shape check, store
   read, capability re-validation. A range must never be the reason a byte is
   reachable.
2. A 206 is still part of the document, so it needs the same CSP as the 200.
   So does a 416.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block. These reuse the existing `fixture()`,
`unique_session()`, `request()` and `body_bytes()` helpers; store a record large
enough to slice.

```rust
    /// Build a request carrying a Range header, addressed like `request()`.
    fn range_request(uri: &str, ip: [u8; 4], range: &str) -> Request<Body> {
        let mut req = Request::builder()
            .uri(uri)
            .header(header::RANGE, range)
            .body(Body::empty())
            .expect("request");
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((ip, 40000))));
        req
    }

    /// Seed a 1000-byte artifact and return (uri, bytes).
    async fn seeded_artifact(f: &Fixture, tag: &str) -> (String, Vec<u8>) {
        let session = unique_session(tag);
        let bytes: Vec<u8> = (0u32..1000).map(|i| (i % 251) as u8).collect();
        let record = f
            .store
            .write(&session, "probe.bin", "application/octet-stream", &bytes, ArtifactOrigin::Outbound)
            .await
            .expect("write artifact");
        let cap = ArtifactCapabilities::mint(&session);
        (
            format!("/artifact/{cap}/{}/{}", record.id, record.filename),
            bytes,
        )
    }

    #[tokio::test]
    async fn a_full_read_advertises_range_support() {
        let f = fixture();
        let (uri, _) = seeded_artifact(&f, "advertise").await;
        let resp = f.app.clone().oneshot(request(&uri, [127, 0, 0, 1])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes",
            "without Accept-Ranges a media element never offers a seek bar"
        );
    }

    #[tokio::test]
    async fn a_satisfiable_range_returns_exactly_that_slice() {
        let f = fixture();
        let (uri, bytes) = seeded_artifact(&f, "slice").await;
        let resp = f
            .app
            .clone()
            .oneshot(range_request(&uri, [127, 0, 0, 1], "bytes=100-199"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 100-199/1000"
        );
        let body = body_bytes(resp).await;
        assert_eq!(body.len(), 100);
        assert_eq!(body, bytes[100..200], "the slice must be the requested bytes");
    }

    #[tokio::test]
    async fn an_unsatisfiable_range_is_416_with_the_total() {
        let f = fixture();
        let (uri, _) = seeded_artifact(&f, "oob").await;
        let resp = f
            .app
            .clone()
            .oneshot(range_request(&uri, [127, 0, 0, 1], "bytes=999999999-"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(resp.headers().get(header::CONTENT_RANGE).unwrap(), "bytes */1000");
    }

    /// A 206 is still part of the document. Dropping the CSP on the partial
    /// responses would leave a hole exactly where the exporter's escaping is
    /// the only other line of defence.
    #[tokio::test]
    async fn partial_and_unsatisfiable_document_responses_keep_the_csp() {
        let f = fixture();
        let session = unique_session("csp-range");
        let html = b"<html><body>hi</body></html>".to_vec();
        let record = f
            .store
            .write(&session, "export.html", "text/html", &html, ArtifactOrigin::Export)
            .await
            .expect("write artifact");
        let cap = ArtifactCapabilities::mint(&session);
        let uri = format!("/artifact/{cap}/{}/{}", record.id, record.filename);

        let partial = f
            .app
            .clone()
            .oneshot(range_request(&uri, [127, 0, 0, 1], "bytes=0-4"))
            .await
            .unwrap();
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            partial.headers().get(header::CONTENT_SECURITY_POLICY).unwrap(),
            ARTIFACT_DOCUMENT_CSP
        );

        let refused = f
            .app
            .clone()
            .oneshot(range_request(&uri, [127, 0, 0, 1], "bytes=99999-"))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            refused.headers().get(header::CONTENT_SECURITY_POLICY).unwrap(),
            ARTIFACT_DOCUMENT_CSP
        );
    }

    /// A range must never be the reason a byte is reachable.
    #[tokio::test]
    async fn a_range_does_not_bypass_the_capability_gate() {
        let f = fixture();
        let (uri, _) = seeded_artifact(&f, "gate").await;
        let forged = uri.replacen(
            uri.split('/').nth(2).unwrap(),
            "0000000000000000000000000000000000000000000000000000000000000000",
            1,
        );
        let resp = f
            .app
            .clone()
            .oneshot(range_request(&forged, [127, 0, 0, 1], "bytes=0-9"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
```

If `ArtifactStore::write` or `ArtifactCapabilities::mint` have different
signatures in this crate, read the existing tests in the same module and match
them — do not invent an API.

- [ ] **Step 2: Run and verify they FAIL**

```bash
cargo test -p alephcore --lib artifact_route
```

Expected: the four range tests fail (200 instead of 206/416, no
`Accept-Ranges`); `a_range_does_not_bypass_the_capability_gate` passes already,
which is the point — it is a regression lock, not a new behaviour.

- [ ] **Step 3: Add the Range rate bucket**

Near `ARTIFACT_READS_PER_MINUTE`, add:

```rust
/// Per-minute byte reads allowed per remote IP for requests carrying a `Range`.
///
/// A seek-heavy media scrub issues far more than [`ARTIFACT_READS_PER_MINUTE`]
/// requests, and without a wider bucket remote (Tailnet) playback 429s while
/// loopback — which is exempt — works fine. 3000/min is about 50/s: comfortably
/// above any human scrubbing pattern, still orders of magnitude below what
/// makes bulk scraping worth writing.
///
/// The FIRST request for an artifact carries no `Range` and therefore still
/// draws from the narrow bucket, so **the number of distinct artifacts a caller
/// can start pulling per minute is unchanged** — which is the property the
/// limiter was built for. This only widens re-reads within an artifact the
/// caller was already allowed to open.
const ARTIFACT_RANGE_READS_PER_MINUTE: u32 = 3_000;
```

In `ArtifactRouteState::new`, extend the `RateLimitConfig` literal. The
`RpcRealtime` scope is unused by this private limiter and its own doc describes
high-frequency realtime frames — the same shape as a media scrub — so it carries
the range bucket rather than widening the shared `RateLimitScope` enum:

```rust
            rate_limiter: RateLimiter::new(RateLimitConfig {
                rpc_heavy: WindowConfig {
                    max_requests: ARTIFACT_READS_PER_MINUTE,
                    window_secs: 60,
                    lockout_secs: None,
                },
                // This limiter is private to the route, and `RpcRealtime` is
                // otherwise unused in it, so it carries the Range bucket
                // instead of widening the shared `RateLimitScope` enum for one
                // caller. Its own doc — high-frequency realtime frames — is the
                // right shape for a media scrub.
                rpc_realtime: WindowConfig {
                    max_requests: ARTIFACT_RANGE_READS_PER_MINUTE,
                    window_secs: 60,
                    lockout_secs: None,
                },
                ..RateLimitConfig::default()
            }),
```

- [ ] **Step 4: Wire the range into `serve_artifact`**

At the top of the file add:

```rust
use crate::gateway::server::byte_range::{parse_range, RangeVerdict};
```

In `serve_artifact`, step 3 (the rate limit) becomes scope-aware. Replace:

```rust
    let key = RateLimitKey::new(&client_ip.to_string(), RateLimitScope::RpcHeavy);
```

with:

```rust
    // Range re-reads within an already-opened artifact draw from the wider
    // bucket; the first, Range-less request still pays the narrow one.
    let has_range = headers.contains_key(header::RANGE);
    let scope = if has_range {
        RateLimitScope::RpcRealtime
    } else {
        RateLimitScope::RpcHeavy
    };
    let key = RateLimitKey::new(&client_ip.to_string(), scope);
```

Then replace step 8 (from `// 8. Explicit, correct Content-Type` to the end of
the function) with:

```rust
    // 8. Explicit, correct Content-Type from the record (`nosniff` is global).
    let content_type = HeaderValue::from_str(&record.mime_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let disposition = HeaderValue::from_str(&content_disposition(&record))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));

    // 9. Representation. This runs LAST, after every gate above: a range must
    //    never be the reason a byte is reachable.
    let total = bytes.len() as u64;
    let verdict = parse_range(
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        total,
    );

    let status = match verdict {
        RangeVerdict::Whole => StatusCode::OK,
        RangeVerdict::Satisfiable { .. } => StatusCode::PARTIAL_CONTENT,
        RangeVerdict::Unsatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
    };

    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        // Advertised on EVERY response, including the refusals: this is how a
        // media element learns it may seek at all.
        .header(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

    if let Some(cr) = verdict.content_range(total) {
        if let Ok(v) = HeaderValue::from_str(&cr) {
            response = response.header(header::CONTENT_RANGE, v);
        }
    }

    // A 206 and a 416 are still part of the document, so both need the policy
    // the 200 gets. Dropping it on the partial responses would leave a hole
    // exactly where the exporter's escaping is the only other defence.
    if is_active_document(&record.mime_type) {
        response = response.header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(ARTIFACT_DOCUMENT_CSP),
        );
    }

    let body = match verdict {
        RangeVerdict::Whole => Body::from(bytes),
        RangeVerdict::Satisfiable { start, end } => {
            // `parse_range` guarantees start <= end < total, so these casts and
            // the slice are in range.
            let (s, e) = (start as usize, end as usize);
            Body::from(bytes[s..=e].to_vec())
        }
        RangeVerdict::Unsatisfiable => Body::empty(),
    };

    response.body(body).unwrap_or_else(|_| not_found())
```

- [ ] **Step 5: Run and verify they PASS**

```bash
cargo test -p alephcore --lib artifact_route
```

Expected: all tests in the module pass.

- [ ] **Step 6: Prove the CSP test bites**

Delete the `if is_active_document(...)` block, confirm
`partial_and_unsatisfiable_document_responses_keep_the_csp` fails, restore it.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/server/artifact_route.rs
git commit -m "gateway: serve artifact byte ranges with 206/416 and a wider range bucket"
```

---

### Task 13: Range on the canvas-asset route

**Files:**
- Modify: `src/gateway/server/canvas_asset_route.rs`
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `crate::gateway::server::byte_range::{parse_range, RangeVerdict}` (Task 11).
- Produces: no new exports. Same behaviour as Task 12, with this route's own
  `Cache-Control: private, max-age=3600` preserved on every response.

- [ ] **Step 1: Write the failing tests**

Add to the module's `mod tests`, following the shape of its existing tests
(read them for the helper names — this route has its own fixture and its own
`header_of` helper):

```rust
    #[tokio::test]
    async fn a_full_read_advertises_range_support() {
        // Build the same fixture the existing success test uses, request the
        // asset with no Range header, and assert:
        //   status == 200
        //   header_of(&response, header::ACCEPT_RANGES) == Some("bytes")
        //   header_of(&response, header::CACHE_CONTROL) == Some(CACHE_CONTROL)
    }

    #[tokio::test]
    async fn a_satisfiable_range_returns_exactly_that_slice() {
        // Same fixture, with `Range: bytes=10-19` on the request. Assert:
        //   status == 206
        //   Content-Range == "bytes 10-19/<total>"
        //   body length == 10 and equals source[10..20]
        //   Cache-Control is still CACHE_CONTROL
    }

    #[tokio::test]
    async fn an_unsatisfiable_range_is_416_with_the_total() {
        // Same fixture, `Range: bytes=999999999-`. Assert:
        //   status == 416
        //   Content-Range == "bytes */<total>"
    }

    #[tokio::test]
    async fn an_svg_partial_response_keeps_the_document_csp() {
        // Store an image/svg+xml asset (the existing CSP test in this module
        // shows how), request `Range: bytes=0-4`, assert 206 AND that
        // Content-Security-Policy == ARTIFACT_DOCUMENT_CSP.
    }
```

Fill each body by copying the corresponding existing test in this module and
adding the Range header — the fixture, capability minting and assertion helpers
are already there. **Do not** copy Task 12's bodies verbatim: this route has a
different fixture, a different store, and an extra `Cache-Control` invariant.

- [ ] **Step 2: Run and verify they FAIL**

```bash
cargo test -p alephcore --lib canvas_asset_route
```

Expected: the four new tests fail on status and missing headers.

- [ ] **Step 3: Wire the range in**

Add the import:

```rust
use crate::gateway::server::byte_range::{parse_range, RangeVerdict};
```

Make the rate-limit key scope-aware exactly as in Task 12 Step 4 (this route
also uses `RateLimitScope::RpcHeavy` at line ~199). This route's limiter is also
private, so add an `ARTIFACT_RANGE_READS_PER_MINUTE`-equivalent constant here —
**name it `CANVAS_RANGE_READS_PER_MINUTE` and give it the same value and the
same reasoning in its doc**. It is a separate limiter guarding a separate
resource; sharing a constant across the two would couple them for no reason.

Then rewrite the response construction (currently at lines ~245-263) with the
same five moves as Task 12: compute `total`, `parse_range`, map the verdict to a
status, attach `Accept-Ranges` on every response plus `Content-Range` when the
verdict has one, keep the existing `Cache-Control` and the SVG CSP branch, and
slice the body.

- [ ] **Step 4: Run and verify they PASS**

```bash
cargo test -p alephcore --lib canvas_asset_route
```

Expected: all tests in the module pass, including the pre-existing
`Cache-Control` assertions.

- [ ] **Step 5: Confirm there is exactly one range parser**

```bash
grep -rn "bytes=" --include="*.rs" src/gateway/server/ | grep -v byte_range.rs | grep -v "mod tests"
```

Expected: no hits outside tests. A second parser here is the failure this shared
helper exists to prevent.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/server/canvas_asset_route.rs
git commit -m "gateway: serve canvas asset byte ranges through the shared parser"
```

---

### Task 14: The Linux decoder diagnosis

**Files:**
- Create: `src/diagnostics/checks/media_codecs.rs`
- Modify: `src/diagnostics/checks/mod.rs` (module + re-export)
- Modify: `src/diagnostics/mod.rs:70-83` (`default_registry`)

**Interfaces:**
- Consumes: `crate::diagnostics::{Finding, Severity, Posture, HealthCheck, check::DEFAULT_CHECK_TIMEOUT}`.
- Produces: `pub struct MediaCodecsCheck` with `pub fn new() -> Self`, `id()` =
  `"media/codecs"`, registered in `default_registry`.

**Three states, not two.** `gst-inspect-1.0` may itself be absent
(`gstreamer1.0-tools`), and that answer is "I don't know". It must not be read as
healthy (CLAUDE.md §8) and equally must not be read as broken.

**Why `tokio::process` and not `spawn_blocking`.** The engine wraps every check
in `tokio::time::timeout` (`src/diagnostics/mod.rs:237`). A `spawn_blocking`
future can be abandoned but its thread and its child process keep running;
`tokio::process::Command` is genuinely cancellable, which is what a deadline on
a registry rebuild needs.

- [ ] **Step 1: Write the failing test**

Create `src/diagnostics/checks/media_codecs.rs`:

```rust
//! `media/codecs` — can this machine actually decode what the Panel plays?
//!
//! Linux-only. WebKitGTK plays media through GStreamer, and MP3/AAC decoding
//! lives in `gstreamer1.0-plugins-{bad,ugly}`, which many distributions do not
//! install by default. When they are absent the Panel's TTS playback rejects
//! silently: the user hears nothing and no surface says which package is
//! missing.
//!
//! # Why GStreamer is asked directly
//!
//! Not by querying distro package names — those hold under neither Flatpak,
//! Snap, nor a source build. `gst-inspect-1.0 --exists <element>` asks the
//! registry that WebKitGTK itself will consult.
//!
//! # Three states, not two
//!
//! `gst-inspect-1.0` may itself be absent (`gstreamer1.0-tools`). That answer
//! is "I don't know", and it is reported as such: it must not be read as
//! healthy, and equally must not be read as broken.

use std::time::Duration;

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture, DEFAULT_CHECK_TIMEOUT};
use crate::diagnostics::finding::{Finding, Severity};

const CHECK_ID: &str = "media/codecs";

/// One decodable format and the GStreamer elements that can provide it.
///
/// Alternatives are OR-ed: `avdec_mp3` (libav) and `mpg123audiodec` both
/// decode MP3, and either is enough.
struct Format {
    label: &'static str,
    elements: &'static [&'static str],
    package_hint: &'static str,
}

const FORMATS: &[Format] = &[
    Format {
        label: "MP3",
        elements: &["mpg123audiodec", "avdec_mp3"],
        package_hint: "gstreamer1.0-plugins-ugly (or gstreamer1.0-libav)",
    },
    Format {
        label: "AAC",
        elements: &["avdec_aac", "faad"],
        package_hint: "gstreamer1.0-plugins-bad (or gstreamer1.0-libav)",
    },
    Format {
        label: "Opus",
        elements: &["opusdec"],
        package_hint: "gstreamer1.0-plugins-base",
    },
    Format {
        label: "VP8/VP9 (WebM)",
        elements: &["vp8dec", "vp9dec"],
        package_hint: "gstreamer1.0-plugins-good",
    },
];

/// What the probe learned.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CodecVerdict {
    /// Every format has at least one decoder.
    Ok,
    /// These formats have no decoder. Non-empty by construction.
    Missing(Vec<String>),
    /// The probe could not run. NOT healthy and NOT broken.
    Unknown(String),
}

/// Turn a verdict into findings. Pure, so the mapping is testable without a
/// GStreamer installation.
pub(crate) fn findings_for(verdict: &CodecVerdict) -> Vec<Finding> {
    match verdict {
        CodecVerdict::Ok => vec![Finding::ok(
            CHECK_ID,
            "Media decoders present",
            "GStreamer can decode every format the Panel plays.",
        )],
        CodecVerdict::Missing(formats) => vec![Finding::problem(
            CHECK_ID,
            Severity::Warning,
            "Missing media decoders",
            format!(
                "GStreamer has no decoder for: {}. Voice replies and media \
                 attachments in these formats will fail to play in the Panel, \
                 and the failure is silent at the WebKitGTK layer.",
                formats.join(", ")
            ),
        )
        .with_fix_hint(
            FORMATS
                .iter()
                .filter(|f| formats.iter().any(|m| m.starts_with(f.label)))
                .map(|f| format!("{}: install {}", f.label, f.package_hint))
                .collect::<Vec<_>>()
                .join("; "),
        )],
        CodecVerdict::Unknown(reason) => vec![Finding::problem(
            CHECK_ID,
            Severity::Info,
            "Media decoder status unknown",
            format!("Could not determine which media formats this system can decode: {reason}"),
        )
        .with_fix_hint("Install gstreamer1.0-tools to let `aleph doctor` answer this.")],
    }
}

/// Linux media-decoder availability.
pub struct MediaCodecsCheck;

impl MediaCodecsCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for MediaCodecsCheck {
    fn default() -> Self {
        Self::new()
    }
}

/// Ask GStreamer whether one element exists.
#[cfg(target_os = "linux")]
async fn element_exists(name: &str) -> Result<bool, std::io::Error> {
    // tokio::process, not spawn_blocking: the engine wraps this check in
    // tokio::time::timeout, and only a genuinely cancellable child honours it.
    // A cold `gst-inspect-1.0` rebuilds the registry and can take seconds.
    let status = tokio::process::Command::new("gst-inspect-1.0")
        .arg("--exists")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;
    Ok(status.success())
}

#[cfg(target_os = "linux")]
async fn probe() -> CodecVerdict {
    let mut missing = Vec::new();
    for format in FORMATS {
        let mut have = false;
        for element in format.elements {
            match element_exists(element).await {
                Ok(true) => {
                    have = true;
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    // The tool itself is unavailable — the answer for EVERY
                    // format is "unknown", not "missing". Reporting the first
                    // format as missing would be a confident wrong answer.
                    return CodecVerdict::Unknown(format!(
                        "gst-inspect-1.0 could not be run ({e}); install gstreamer1.0-tools"
                    ));
                }
            }
        }
        if !have {
            missing.push(format.label.to_string());
        }
    }
    if missing.is_empty() {
        CodecVerdict::Ok
    } else {
        CodecVerdict::Missing(missing)
    }
}

#[async_trait]
impl HealthCheck for MediaCodecsCheck {
    fn id(&self) -> &'static str {
        CHECK_ID
    }

    fn title(&self) -> &'static str {
        "Media decoders"
    }

    /// Inner bound: four formats x up to two `gst-inspect-1.0` invocations. A
    /// cold run rebuilds the GStreamer registry once (seconds); every later
    /// invocation reads the cache. The default 20s ceiling covers that with
    /// room, and the engine turns an overrun into a named Warning rather than
    /// a stall inside an agent turn.
    fn timeout(&self) -> Duration {
        DEFAULT_CHECK_TIMEOUT
    }

    #[cfg(target_os = "linux")]
    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        findings_for(&probe().await)
    }

    /// Windows (WebView2) and macOS (WKWebView) decode media through the OS,
    /// with no separate plugin set to be missing. Nothing to report.
    #[cfg(not(target_os = "linux"))]
    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_reports_a_single_info_finding() {
        let f = findings_for(&CodecVerdict::Ok);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Info);
    }

    #[test]
    fn missing_names_the_formats_and_the_packages() {
        let f = findings_for(&CodecVerdict::Missing(vec!["MP3".into(), "AAC".into()]));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Warning);
        assert!(f[0].detail.contains("MP3"));
        assert!(f[0].detail.contains("AAC"));
        let hint = f[0].fix_hint.as_deref().unwrap_or_default();
        assert!(hint.contains("gstreamer1.0-plugins-ugly"), "hint was: {hint}");
        assert!(hint.contains("gstreamer1.0-plugins-bad"), "hint was: {hint}");
    }

    /// The whole point of the third state: "I could not tell" must never be
    /// rendered as either health or breakage.
    #[test]
    fn unknown_is_neither_ok_nor_a_warning() {
        let f = findings_for(&CodecVerdict::Unknown("tool absent".into()));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Info);
        assert!(f[0].title.contains("unknown"), "title was: {}", f[0].title);
        assert!(
            f[0].detail.contains("Could not determine"),
            "an unknown verdict must SAY it is unknown, not imply health"
        );
    }
}
```

If `Finding::with_fix_hint` is not the builder's name, read
`src/diagnostics/finding.rs` and use the real one — `Finding::problem` there
sets `fix_hint: None`, so a builder exists.

- [ ] **Step 2: Run and verify they FAIL**

```bash
cargo test -p alephcore --lib media_codecs
```

Expected: FAIL — module not declared.

- [ ] **Step 3: Declare and register**

In `src/diagnostics/checks/mod.rs`, add in alphabetical position:

```rust
pub mod media_codecs;
```

and

```rust
pub use media_codecs::MediaCodecsCheck;
```

In `src/diagnostics/mod.rs`, add to the `checks` vector in `default_registry`,
after `BrowserRuntimeCheck`:

```rust
            Arc::new(checks::MediaCodecsCheck::new()),
```

- [ ] **Step 4: Run and verify they PASS**

```bash
cargo test -p alephcore --lib media_codecs
```

Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Verify it is inert on Windows**

```bash
cargo run --bin aleph-server -- doctor
```

Expected: the report completes and contains no `media/codecs` line — the
non-Linux `run` returns no findings. It must not error, and it must not add
latency.

- [ ] **Step 6: Prove the unknown test bites**

Change `Severity::Info` to `Severity::Warning` in the `Unknown` arm and confirm
`unknown_is_neither_ok_nor_a_warning` fails. Restore.

- [ ] **Step 7: Commit**

```bash
git add src/diagnostics/checks/media_codecs.rs src/diagnostics/checks/mod.rs src/diagnostics/mod.rs
git commit -m "diagnostics: report Linux media decoder availability as a three-state finding"
```

---

### Task 15: The TTS decoder receipt

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs:508+` (add a field)
- Modify: `interfaces/webchat/src/platform/wide/views/chat/voice_playback.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:1487+` (add a banner)
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs:1097`

**Interfaces:**
- Consumes: `crate::platform_host::{host, HostPlatform}` (Task 6).
- Produces: `ChatState.voice_notice: RwSignal<Option<String>>` and a
  `VoiceNoticeBanner` component rendered beside `SendErrorBanner`.
  `voice_playback::speak` gains a `chat: &ChatState` parameter.

**Why a new signal and not `send_error`.** `send_error` carries a
`ChatSendErrorCode` and describes a failed send. A missing decoder is a
different thing with a different remedy; folding it in would conflate two
conditions in one surface, and the code enum would have to grow a variant that
has nothing to do with sending.

**Why the predicate is narrow.** Only
`audio.error.code === MEDIA_ERR_SRC_NOT_SUPPORTED (4)` means "missing decoder".
`NotAllowedError` is the autoplay policy and has a completely different remedy;
telling that user to install a GStreamer package would be a confident wrong
answer.

- [ ] **Step 1: Add the state field**

In `ChatState` (`.../chat/state.rs`), add a field beside `send_error`:

```rust
    /// A non-fatal notice from the spoken layer — currently only "this system
    /// cannot decode the audio the core sent". Separate from [`Self::send_error`]
    /// on purpose: that one carries a `ChatSendErrorCode` and describes a failed
    /// send, and a decoder problem has a different remedy. Cleared by the user
    /// or by the next successful playback.
    pub voice_notice: RwSignal<Option<String>>,
```

Initialise it in the same constructor that initialises `send_error`, with
`RwSignal::new(None)`.

- [ ] **Step 2: Write the failing predicate test**

Add to `voice_playback.rs`:

```rust
/// `HTMLMediaElement.error.code` for "the resource is not supported".
///
/// This is the ONLY code that means "this system cannot decode these bytes".
/// A rejected `play()` promise is a different thing entirely — usually the
/// autoplay policy — and pointing that user at a GStreamer package would be a
/// confident wrong answer.
const MEDIA_ERR_SRC_NOT_SUPPORTED: u16 = 4;

/// Should this media error be reported to the user as a missing decoder?
///
/// Narrow by construction: only the not-supported code, and only on Linux,
/// where WebKitGTK's decoding depends on GStreamer plugin packages the
/// distribution may not have installed. On Windows (WebView2) and macOS
/// (WKWebView) the OS decodes and there is no package to name, so a
/// not-supported error there is a real bug worth a console warning, not a
/// user-facing install instruction.
fn is_missing_decoder(code: Option<u16>, platform: crate::platform_host::HostPlatform) -> bool {
    code == Some(MEDIA_ERR_SRC_NOT_SUPPORTED)
        && platform == crate::platform_host::HostPlatform::Linux
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_host::HostPlatform;

    #[test]
    fn only_the_not_supported_code_on_linux_is_a_decoder_problem() {
        assert!(is_missing_decoder(Some(4), HostPlatform::Linux));
    }

    #[test]
    fn other_media_errors_are_not_decoder_problems() {
        // 1 = ABORTED, 2 = NETWORK, 3 = DECODE (a corrupt stream, not a
        // missing decoder). None = the play() promise rejected with no media
        // error at all, which is usually the autoplay policy.
        for code in [Some(1), Some(2), Some(3), None] {
            assert!(
                !is_missing_decoder(code, HostPlatform::Linux),
                "code {code:?} must not be reported as a missing decoder"
            );
        }
    }

    #[test]
    fn other_platforms_decode_through_the_os_so_there_is_no_package_to_name() {
        assert!(!is_missing_decoder(Some(4), HostPlatform::Windows));
        assert!(!is_missing_decoder(Some(4), HostPlatform::MacOs));
    }
}
```

- [ ] **Step 3: Run and verify they FAIL**

```bash
cargo test -p aleph-panel --lib voice_playback
```

Expected: FAIL — `is_missing_decoder` is not defined until Step 2's code is in
place; if you pasted the whole block, the tests pass immediately, in which case
prove they bite by dropping the `&& platform == ...` clause and confirming
`other_platforms_decode_through_the_os_so_there_is_no_package_to_name` fails.

- [ ] **Step 4: Wire the pre-check and the receipt**

In `speak()`, take the chat state and add a `canPlayType` pre-check before
building the object URL:

```rust
pub fn speak(dash: &DashboardState, chat: &ChatState, text: String) {
    let dash = *dash;
    let chat = *chat;
```

After `mime` is resolved and before the object URL is built, insert:

```rust
        // Pre-check leg. An empty string from canPlayType is the engine saying
        // "definitely not"; "maybe"/"probably" are hedges we do NOT trust,
        // because whether WebKitGTK's canPlayType consults the GStreamer
        // registry is unverified. So a hedge falls through and plays, and the
        // error receipt in `play` is the second leg. Two legs, neither
        // load-bearing alone.
        if let Some(audio) = web_sys::HtmlAudioElement::new().ok() {
            if audio.can_play_type(mime).is_empty()
                && crate::platform_host::host() == crate::platform_host::HostPlatform::Linux
            {
                chat.voice_notice.set(Some(format!(
                    "This system cannot decode {mime}. Voice replies need GStreamer \
                     decoder plugins — run `aleph doctor` for the exact package."
                )));
                return;
            }
        }
```

In `play()`, add the `chat` parameter and replace the rejection handler's
`console::warn`-only body:

```rust
fn play(chat: ChatState, src: &str, revoke: bool) {
```

and inside the rejected branch:

```rust
            if let Err(e) = wasm_bindgen_futures::JsFuture::from(promise).await {
                web_sys::console::warn_1(&format!("voice playback rejected: {e:?}").into());
                let code = keep_for_error.error().map(|err| err.code());
                if is_missing_decoder(code, crate::platform_host::host()) {
                    chat.voice_notice.set(Some(
                        "This system cannot decode the voice reply. Install the GStreamer \
                         decoder plugins — run `aleph doctor` for the exact package."
                            .to_string(),
                    ));
                }
                if let Some(u) = rejected_url {
                    let _ = web_sys::Url::revoke_object_url(&u);
                }
            }
```

`keep_for_error` is a third clone of the `audio` element, taken alongside the
existing `keep`, so the error object is still reachable inside the async block.
Also clear the notice on the success path (in the `onended` closure) with
`chat.voice_notice.set(None)`.

Update the call site at `events.rs:1097`:

```rust
                        super::voice_playback::speak(&dash, &chat, text);
```

- [ ] **Step 5: Add the banner**

In `.../chat/composer/mod.rs`, beside `SendErrorBanner`, add:

```rust
/// Non-fatal notice from the spoken layer. Deliberately separate from
/// [`SendErrorBanner`]: that one reports a failed send with a
/// `ChatSendErrorCode`, this one reports that playback could not decode. Same
/// shape, different condition, different remedy.
#[component]
fn VoiceNoticeBanner() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    view! {
        <Show when=move || chat.voice_notice.get().is_some()>
            {move || {
                chat.voice_notice
                    .get()
                    .map(|msg| {
                        view! {
                            <div
                                class="mx-1 mb-1 px-3 py-2 rounded-lg border text-sm bg-warning-subtle border-warning/30 text-warning"
                                role="status"
                            >
                                {msg}
                            </div>
                        }
                    })
            }}
        </Show>
    }
}
```

Render `<VoiceNoticeBanner />` immediately after `<SendErrorBanner />` in the
composer's view tree.

- [ ] **Step 6: Run the panel test suite**

```bash
cargo test -p aleph-panel --lib
```

Expected: all tests pass, including the three new predicate tests.

- [ ] **Step 7: Verify the banner renders on Windows**

Start the server, open the Panel, and in the console set the signal indirectly
by forcing a decode failure: there is no console handle to `ChatState`, so
instead verify the banner's markup by temporarily initialising `voice_notice` to
`RwSignal::new(Some("test".into()))`, running `just wasm`, confirming the
warning bar renders below the composer, then reverting to `None` and rebuilding.

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/state.rs \
        interfaces/webchat/src/platform/wide/views/chat/voice_playback.rs \
        interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs \
        interfaces/webchat/src/platform/wide/views/chat/events.rs
git commit -m "panel: tell the user when this system cannot decode a voice reply"
```

---

### Task 16: The Linux / macOS QA script

**Files:**
- Create: `qa/webview_compat/run.sh`

**Interfaces:**
- Consumes: a running `aleph-server` on `127.0.0.1:18790` and a built
  `interfaces/webchat/dist/`.
- Produces: exit 0 when every assertion for the named platform passes.

**Every assertion is an effect assertion**, never "the command exited 0", and
**every assertion prints the value it actually read** — because the person
running this on Linux or macOS did not write it, and when it goes red they must
be able to tell a broken assertion from broken code without reading the source.

- [ ] **Step 1: Write the script**

Create `qa/webview_compat/run.sh`:

```bash
#!/usr/bin/env bash
# Real-machine assertions for the cross-platform WebView resource-control work.
#
#   Usage: qa/webview_compat/run.sh <linux|macos> [base-url]
#
# Windows is verified on the developer machine directly (see the plan, task 17);
# this script exists because Linux (WebKitGTK/GStreamer) and macOS (WKWebView)
# behaviours cannot be observed from there.
#
# READ THIS BEFORE FILING A FAILURE: only the Windows-side guards in this change
# were falsified by mutation. The assertions below are correct in SHAPE, but the
# first time one goes red the red may be the assertion rather than the code.
# Every assertion therefore prints the value it actually read.
set -uo pipefail

PLATFORM="${1:-}"
BASE="${2:-http://127.0.0.1:18790}"
DIST="interfaces/webchat/dist"

case "$PLATFORM" in
  linux|macos) ;;
  *) echo "usage: $0 <linux|macos> [base-url]" >&2; exit 2 ;;
esac

pass=0; fail=0; skip=0
ok()   { echo "  PASS  $1"; pass=$((pass+1)); }
bad()  { echo "  FAIL  $1"; echo "        observed: $2"; fail=$((fail+1)); }
skipit(){ echo "  SKIP  $1"; echo "        reason: $2"; skip=$((skip+1)); }

echo "== webview_compat ($PLATFORM) against $BASE =="

# ── br-negotiation ────────────────────────────────────────────────────────
hdr=$(curl -sS -o /tmp/wc_wasm.br -D - -H 'Accept-Encoding: br' \
      "$BASE/aleph_panel_bg.wasm" 2>/dev/null)
enc=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-encoding"{print $2}')
size=$(wc -c < /tmp/wc_wasm.br | tr -d ' ')
if [ "$enc" = "br" ]; then ok "br-negotiation: content-encoding"
else bad "br-negotiation: content-encoding" "content-encoding='$enc' (expected 'br')"; fi
if [ "$size" -gt 0 ] && [ "$size" -lt 4194304 ]; then ok "br-negotiation: body under 4 MiB"
else bad "br-negotiation: body under 4 MiB" "$size bytes"; fi
if command -v python3 >/dev/null 2>&1 && [ -f "$DIST/aleph_panel_bg.wasm" ]; then
  same=$(python3 - "$DIST/aleph_panel_bg.wasm" /tmp/wc_wasm.br <<'PY'
import brotli, hashlib, sys
src = open(sys.argv[1],'rb').read()
try:
    got = brotli.decompress(open(sys.argv[2],'rb').read())
except Exception as e:
    print("decompress-failed:%s" % e); raise SystemExit
print("same" if hashlib.sha256(src).digest()==hashlib.sha256(got).digest()
      else "sha-mismatch src=%s got=%s" % (hashlib.sha256(src).hexdigest()[:12],
                                           hashlib.sha256(got).hexdigest()[:12]))
PY
)
  if [ "$same" = "same" ]; then ok "br-negotiation: decompresses to the dist wasm"
  else bad "br-negotiation: decompresses to the dist wasm" "$same"; fi
else
  skipit "br-negotiation: sha comparison" "python3 with the 'brotli' module not available"
fi

# ── range-206 / range-416 ─────────────────────────────────────────────────
# ARTIFACT_URL must be a capability URL for a >=200-byte artifact. Mint one from
# a Panel session (open any artifact and copy its URL) and export it.
if [ -n "${ARTIFACT_URL:-}" ]; then
  hdr=$(curl -sS -o /tmp/wc_slice -D - -H 'Range: bytes=100-199' "$ARTIFACT_URL" 2>/dev/null)
  code=$(printf '%s' "$hdr" | head -1 | awk '{print $2}')
  cr=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-range"{print $2}')
  n=$(wc -c < /tmp/wc_slice | tr -d ' ')
  [ "$code" = "206" ] && ok "range-206: status" || bad "range-206: status" "HTTP $code"
  [ "$n" = "100" ]    && ok "range-206: exactly 100 bytes" || bad "range-206: exactly 100 bytes" "$n bytes"
  case "$cr" in bytes\ 100-199/*) ok "range-206: content-range" ;;
                *) bad "range-206: content-range" "'$cr'" ;; esac

  hdr=$(curl -sS -o /dev/null -D - -H 'Range: bytes=999999999-' "$ARTIFACT_URL" 2>/dev/null)
  code=$(printf '%s' "$hdr" | head -1 | awk '{print $2}')
  cr=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-range"{print $2}')
  [ "$code" = "416" ] && ok "range-416: status" || bad "range-416: status" "HTTP $code"
  case "$cr" in bytes\ \*/*) ok "range-416: content-range" ;;
                *) bad "range-416: content-range" "'$cr'" ;; esac
else
  skipit "range-206 / range-416" "set ARTIFACT_URL to a capability URL for an artifact of >=200 bytes"
fi

if [ "$PLATFORM" = "linux" ]; then
  # ── gst-codecs ──────────────────────────────────────────────────────────
  if command -v gst-inspect-1.0 >/dev/null 2>&1; then
    miss=""
    for e in mpg123audiodec avdec_mp3; do gst-inspect-1.0 --exists "$e" && { miss=""; break; } || miss="MP3"; done
    if [ -z "$miss" ]; then ok "gst-codecs: MP3 decoder present"
    else bad "gst-codecs: MP3 decoder present" "neither mpg123audiodec nor avdec_mp3 exists — install gstreamer1.0-plugins-ugly"; fi
    echo "        (now run \`aleph doctor\` and confirm media/codecs agrees with the line above)"
  else
    skipit "gst-codecs" "gst-inspect-1.0 absent (gstreamer1.0-tools) — the doctor check must report UNKNOWN, not missing; verify that"
  fi

  # ── flat-on-linux ───────────────────────────────────────────────────────
  echo "  MANUAL  flat-on-linux: open the Panel in the shell, then in the WebKit inspector run:"
  echo "          document.documentElement.dataset.flat"
  echo "          getComputedStyle(document.querySelector('.glass')).backdropFilter"
  echo "          expected: \"1\"  and  \"none\""

  # ── tts-playback (BOTH directions) ──────────────────────────────────────
  echo "  MANUAL  tts-playback: trigger a spoken reply, then assert ONE of:"
  echo "          success -> audio plays AND no warning bar under the composer"
  echo "          failure -> a warning bar appears naming the GStreamer plugins"
  echo "          A silent failure with NO bar is the defect this change exists to remove."
fi

if [ "$PLATFORM" = "macos" ]; then
  # ── min-system-version ──────────────────────────────────────────────────
  APP="${ALEPH_APP:-/Applications/Aleph.app}"
  if [ -f "$APP/Contents/Info.plist" ]; then
    v=$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist" 2>/dev/null)
    [ "$v" = "13.3" ] && ok "min-system-version" || bad "min-system-version" "LSMinimumSystemVersion='$v' (expected '13.3')"
  else
    skipit "min-system-version" "no app bundle at $APP — set ALEPH_APP"
  fi
  skipit "install-refusal below 13.3" "requires a machine running macOS < 13.3; NOT VERIFIED anywhere"

  echo "  MANUAL  wkwebview-baseline: in the Panel's inspector, all four must be true:"
  echo "          CSS.supports('color','oklch(0 0 0)')"
  echo "          CSS.supports('color','color-mix(in oklab, red, red)')"
  echo "          typeof CSS.registerProperty === 'function'"
  echo "          typeof WebAssembly === 'object'"
  echo "  MANUAL  tts-blob: trigger a spoken reply; it must play (blob object URL, not data:)"
  echo "  MANUAL  vibrancy: the window is still translucent and the material is visible"
fi

echo
echo "== $pass passed, $fail failed, $skip skipped =="
[ "$fail" -eq 0 ]
```

- [ ] **Step 2: Make it executable and syntax-check it**

```bash
chmod +x qa/webview_compat/run.sh
bash -n qa/webview_compat/run.sh
```

Expected: no output from `bash -n`.

- [ ] **Step 3: Smoke-run the platform-independent half on Windows**

```bash
cargo run --bin aleph-server &
bash qa/webview_compat/run.sh linux
```

Expected: the `br-negotiation` assertions pass (they are transport-level and
platform-independent), `range-*` skips without `ARTIFACT_URL`, and the Linux-only
sections skip or print their manual instructions. **The script must not crash**
on a platform it was not written for — that is what this smoke run proves.

- [ ] **Step 4: Commit**

```bash
git add qa/webview_compat/run.sh
git commit -m "qa: add Linux/macOS WebView compatibility effect assertions"
```

---

### Task 17: Rebuild dist, commit artifacts, and run the full Windows sweep

**Files:**
- Modify: `interfaces/webchat/dist/*` (rebuilt, including the new `.br` files)

**Interfaces:**
- Consumes: everything above.
- Produces: a committed, internally consistent `dist/`.

- [ ] **Step 1: Clean rebuild**

```bash
just wasm
```

Expected, in order: Tailwind, cargo wasm build, wasm-bindgen,
`✓ wasm-opt applied (feature set fenced)`, index.html written,
`✓ webview baseline consistent`, the precompress table, then the dist pair
check.

- [ ] **Step 2: Run every guard once more, from clean**

```bash
node scripts/check_webview_baseline.mjs
node scripts/check_panel_dist.mjs
```

Expected: both green.

- [ ] **Step 3: Run the minimum verification set**

```bash
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo clippy --all-targets
```

Then the actual runs for the touched areas:

```bash
cargo test -p alephcore --lib byte_range
cargo test -p alephcore --lib artifact_route
cargo test -p alephcore --lib canvas_asset_route
cargo test -p alephcore --lib control_plane::server
cargo test -p alephcore --lib media_codecs
cargo test -p aleph-panel --lib
```

- [ ] **Step 4: Build the panel in its SHIPPED form**

`cargo test -p aleph-panel --lib` compiles a *test* binary with `cfg(test)` on;
the shipped artifact is the non-test cdylib on `wasm32-unknown-unknown`, and
only this command compiles that:

```bash
cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release
```

Expected: no errors and no `unused_imports` warnings.

- [ ] **Step 5: Check the shell builds**

```bash
just _stage-shell-placeholders
cargo check -p aleph-desktop-shell
```

- [ ] **Step 6: End-to-end on the Windows real machine**

```bash
cargo run --bin aleph-server
```

Then verify, recording the observed value for each:

| Check | How | Expected |
|---|---|---|
| brotli on the wire | `curl -sS -o NUL -D - -H "Accept-Encoding: br" http://127.0.0.1:18790/aleph_panel_bg.wasm` | `content-encoding: br`, length near the Task 8 figure, **not** double-encoded |
| gzip fallback | same with `Accept-Encoding: gzip` | `content-encoding: gzip`, ~5.0 MB |
| no false 304 | request with `br` and note the ETag, then request with `gzip` sending that ETag as `If-None-Match` | 304 (the ETag describes the resource, and `Vary` tells the cache the rest) |
| Panel loads | open `http://127.0.0.1:18790` in Edge | renders normally, no console errors |
| fallback page | DevTools: `CSS.supports = () => false; location.reload()` | the readable fallback page, both capabilities listed |
| flat mode | DevTools: `document.documentElement.setAttribute('data-flat','1')` then read `getComputedStyle(document.querySelector('.glass')).backdropFilter` | `"none"` |
| platform marker | build and run the shell (`just shell-build`, install, launch), DevTools: `document.documentElement.dataset.platform` | `"windows"` |
| microphone | click the voice button in the shell | still prompts/records — `webview_perms` is unaffected |
| doctor | `aleph doctor` | completes, no `media/codecs` line on Windows |

- [ ] **Step 7: Commit the artifacts**

```bash
git add interfaces/webchat/dist
git commit -m "panel: rebuild dist with the baseline probe, flat-mode CSS and brotli siblings"
```

- [ ] **Step 8: Update the spec's measured figures**

In `docs/superpowers/specs/2026-08-21-tauri-webview-resource-control-design.md`
§5.1, replace the parenthetical "(Exact post-change numbers are filled in from
the Windows measurement.)" with the measured brotli size and the observed
`content-encoding` behaviour. Also record in §7.3 which assertions were actually
falsified by mutation.

```bash
git add docs/superpowers/specs/2026-08-21-tauri-webview-resource-control-design.md
git commit -m "docs: record the measured brotli figures in the resource-control spec"
```

---

## Self-Review

**1. Spec coverage.**

| Spec section | Task |
|---|---|
| §3.2 ordering hazard | T2 (probe resolves + writes), T6 (`host()` is a pure reader) |
| §3.3 baseline declaration, three consumers | T1 (JSON + edge A), T2 (edge B), T3 (edge C), T4 (edge D) |
| §4.1 install gate | T1 |
| §4.2 build gate + WASM feature fence | T1–T4, T5 |
| §4.3 runtime fallback page | T2, verified in T3 Step 5 |
| §5.1 build-time brotli | T8 (producer), T9 (guard), T10 (server) |
| §5.2 Range/206 + rate bucket | T11 (parser), T12 (artifact), T13 (canvas) |
| §6.1 flat mode + shell marker + `webviewInstallMode` | T6, T7, T1 (install mode) |
| §6.2 codec diagnosis + receipt | T14, T15 |
| §7.1 Windows sweep with mutation proofs | every task's RED step, consolidated in T17 |
| §7.2 QA script | T16 |
| §7.3 honest labelling | T16's header comment and its `SKIP` for macOS < 13.3 |
| §8 follow-ups FU-1…FU-4 | intentionally not implemented; recorded in the spec |

No spec requirement is unassigned.

**2. Placeholder scan.** Task 13 Step 1 gives comment-form test bodies rather
than literal code. That is deliberate and is not a placeholder in the prohibited
sense: it names the exact assertions and points at the sibling tests to copy,
because this route's fixture, store and `header_of` helper differ from Task 12's
and pasting Task 12's bodies would not compile. Every other step carries the
literal content.

**3. Type consistency.**
- `RangeVerdict::{Whole, Satisfiable{start,end}, Unsatisfiable}` and
  `parse_range(Option<&str>, u64)` are defined in T11 and used with those exact
  names in T12 and T13.
- `HostPlatform::{MacOs, Windows, Linux}` and `host()` are defined in T6 and
  used in T15 with those names.
- `MIN_BYTES` is exported by `precompress_dist.mjs` (T8) and imported by
  `check_panel_dist.mjs` (T9) — one threshold, one definition.
- `ChatState.voice_notice: RwSignal<Option<String>>` is added in T15 Step 1 and
  read in T15 Steps 4 and 5.
- `CodecVerdict::{Ok, Missing, Unknown}` and `findings_for` are defined and used
  within T14 only.
- The four guard edges are labelled A/B/C/D consistently in T1–T4 and in the
  script's header comment.
