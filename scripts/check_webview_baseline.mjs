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
      if (new RegExp(`\\b${name.replace(/\./g, '\\.')}\\b`).test(src)) found.add(name);
    }
    for (const d of declared) {
      if (!found.has(d)) fail('B', `${PROBE} does not probe declared capability ${JSON.stringify(d)}`);
    }
    for (const f of found) {
      if (!declared.has(f)) fail('B', `${PROBE} probes ${JSON.stringify(f)}, which is not declared in ${BASELINE} — add it to the declaration or drop the probe`);
    }
    // Version numbers: the fallback page shows the minimum system versions
    // (e.g., "macOS 13.3+"), which must match the declaration. Check them in
    // the "Minimum:" line that the user sees.
    const macOsCheck = `macOS ${baseline.macos_min}+`;
    const webKitGtkCheck = `WebKitGTK ${baseline.webkitgtk_min}+`;
    if (!src.includes(macOsCheck)) {
      fail('B', `${PROBE} fallback text does not contain ${JSON.stringify(macOsCheck)} — the version number in the user-facing "Minimum:" line must match ${BASELINE}`);
    }
    if (!src.includes(webKitGtkCheck)) {
      fail('B', `${PROBE} fallback text does not contain ${JSON.stringify(webKitGtkCheck)} — the version number in the user-facing "Minimum:" line must match ${BASELINE}`);
    }
  }
}

// ── Edge C: dist/index.html carries the probe VERBATIM ────────────────────
// Same class as the js/wasm pairing guard in check_panel_dist.mjs: catches a
// one-sided rebuild where the probe changed but dist did not, or vice versa.
{
  const PROBE = 'interfaces/webchat/baseline-probe.js';
  const INDEX = 'interfaces/webchat/dist/index.html';
  let probe;
  try {
    probe = readFileSync(PROBE, 'utf8');
  } catch (e) {
    fail('C', `cannot read ${PROBE}: ${e.message}`);
    probe = null;
  }
  let index;
  try {
    index = readFileSync(INDEX, 'utf8');
  } catch (e) {
    fail('C', `cannot read ${INDEX}: ${e.message} — run \`just wasm\``);
    index = null;
  }
  if (probe !== null && index !== null) {
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
    // A plain substring match (`css.includes(fn + '(')`) is a false-negative
    // trap: `lch(` is a suffix of `oklch(`, so a probe named `lch(...)` would
    // read as "still used" purely because a DIFFERENT, longer function is
    // present. D1's whole job is catching a probe that stopped being
    // load-bearing, so its match must be anchored: the character immediately
    // before the function name must not be an identifier character
    // (`[A-Za-z0-9_-]`), and start-of-string counts as a boundary. Implemented
    // as a negative lookbehind rather than scanning for a preceding
    // non-identifier char by hand, so it reads the same as the rule it
    // enforces.
    const escapeRegExp = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const isLoadBearing = (fn) => new RegExp(`(?<![A-Za-z0-9_-])${escapeRegExp(fn)}\\(`).test(css);

    // D1 and D2's extraction regex (below) deliberately disagree about a
    // leading hyphen, and that is not a bug to unify. They answer different
    // questions, and each must fail in the opposite direction to be safe:
    // D2 has to over-report — a vendor-prefixed function it failed to see
    // (e.g. -webkit-foo() reported as nothing at all) would break the one
    // property that makes the census worth trusting, that its silence means
    // something. D1 has to never silently pass — a probe whose only
    // remaining occurrence is prefixed (e.g. -webkit-oklch() but not a
    // standalone oklch()) must NOT read as "still load-bearing", because a
    // vendor prefix is not the same guarantee as the standard function the
    // probe declares. So D1 stays conservative and treats `-` as part of an
    // identifier: a prefixed-only occurrence produces a spurious red here,
    // not a false green. If a future edit ever makes these two regexes
    // identical, that is a sign one of them lost its failure-direction
    // guarantee, not a cleanup.

    // D1 (reverse, honest): every declared CSS probe must still be exercised by
    // the built stylesheet. This catches a probe list rotting into a stale
    // licence — a capability we still gate on that the CSS stopped using.
    for (const [, value] of baseline.css_probes) {
      const fn = value.slice(0, value.indexOf('('));
      if (!fn || !isLoadBearing(fn)) {
        fail('D', `${CSS_PATH} no longer uses ${fn}(), but ${BASELINE} still gates on it — drop the probe or find out why the CSS changed`);
      }
    }

    // D2 (forward, over-reporting): every CSS function name in the built
    // stylesheet must be on the reviewed list. A Tailwind upgrade that emits a
    // new function goes RED and a human decides whether the baseline moves.
    // False positives are the intended failure direction: a new name is cheap
    // to review, a silently-shipped capability cliff is not.
    //
    // Two lists, not one, because they answer different questions and mixing
    // them would let the second question go unasked:
    //   IN_FLOOR         — actually supported at Safari 16.4 / WebKitGTK 2.42.
    //   DEGRADES_UNUSED  — NOT supported at the floor, accepted anyway
    //                      because the construct's failure mode when
    //                      unsupported is "this rule is dropped, the feature
    //                      is silently absent" rather than "the stylesheet is
    //                      poisoned" (e.g. an invalid oklch() inside a
    //                      custom property invalidates that property at
    //                      computed-value time and can collapse an entire
    //                      palette to initial/inherit — an unknown
    //                      pseudo-element invalidates only its own rule).
    //                      Every entry here must say what actually happens
    //                      when it's unsupported, and where the JS
    //                      feature-detect (if any) lives — this file can
    //                      only scan CSS, so it cannot verify that detect;
    //                      the comment is a human-checked claim, not an
    //                      assertion this script enforces.
    const IN_FLOOR = new Set([
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
      // basic shapes (clip-path) / grid — Baseline widely available since
      // 2017-2020, Safari 10.1+; nowhere near the 16.4 floor
      'circle', 'inset', 'minmax', 'repeat',
      // selectors / at-rule conditions
      'not', 'is', 'where', 'has', 'nth-child', 'nth-last-child', 'nth-of-type',
      'selector', 'supports', 'lang', 'dir', 'host', 'slotted',
      // animation
      'cubic-bezier', 'steps', 'attr', 'counter', 'format', 'local',
    ]);
    const DEGRADES_UNUSED = new Set([
      // ::view-transition-old(root) / ::view-transition-new(root) — View
      // Transitions API, requires Safari 18; the floor is 16.4. Failure mode:
      // an unrecognized pseudo-element invalidates only the rule it appears
      // in, so on an unsupported browser this theme-switch reveal animation
      // is simply absent — no other rule or custom property is affected.
      // Also feature-detected before use, independently of this CSS, at
      // interfaces/webchat/src/components/theme_toggle.rs:53
      // (`document.startViewTransition` read via `Reflect.get`, only called
      // if present).
      'view-transition-old', 'view-transition-new',
    ]);
    const REVIEWED = new Set([...IN_FLOOR, ...DEGRADES_UNUSED]);
    const seen = new Set();
    // The captured name allows an optional leading `-` so a vendor-prefixed
    // function (-webkit-foo(), -moz-foo()) is captured as its own name
    // instead of structurally invisible: without it, every scan position
    // inside "-webkit-foo(" is preceded by a hyphen, which `[^\w-]` rejects
    // as a boundary — including the leading one — so the match can never
    // start. This census's entire value is that its silence can be trusted
    // (never false-negative); a class of function it cannot see by
    // construction contradicts that, even with zero occurrences today.
    for (const m of css.matchAll(/(?:^|[^\w-])(-?[a-zA-Z][\w-]*)\(/g)) {
      seen.add(m[1]);
    }
    const novel = [...seen].filter((n) => !REVIEWED.has(n)).sort();
    if (novel.length) {
      fail('D', `${CSS_PATH} uses CSS function(s) this census has not reviewed: ${novel.join(', ')}.\n` +
        `      This is an OVER-REPORTING census: it goes red on anything new, by design.\n` +
        `      For each name, first: is it supported at Safari ${baseline.safari_min} / WebKitGTK ${baseline.webkitgtk_min}?\n` +
        `        Yes -> add it to IN_FLOOR.\n` +
        `        No  -> then answer the question that actually decides this: on an in-floor\n` +
        `               browser, what happens when this is unsupported? Does the rule simply\n` +
        `               get dropped (safe — e.g. an unrecognized selector or pseudo-element\n` +
        `               invalidates only itself), or does it get invalidated somewhere that\n` +
        `               poisons OTHER declarations too (e.g. an unparseable value inside a\n` +
        `               custom property can collapse everything that reads that property)?\n` +
        `               Drops safely -> add it to DEGRADES_UNUSED, with a comment stating the\n` +
        `               failure mode and where the JS feature-detect (if any) lives.\n` +
        `               Poisons anything else -> this is a real floor violation: rework the\n` +
        `               CSS/JS, or move the floor in ${BASELINE}.\n` +
        `      Do not add it to REVIEWED — that Set is derived from IN_FLOOR + DEGRADES_UNUSED\n` +
        `      and is not meant to be edited directly.`);
    }
  }
}

if (problems.length) {
  console.error(problems.join('\n'));
  console.error(`\n${problems.length} baseline violation(s). The declaration is ${BASELINE}; fix the consumer, not the declaration, unless you are deliberately moving the floor.`);
  process.exit(1);
}
console.log('✓ webview baseline consistent');
