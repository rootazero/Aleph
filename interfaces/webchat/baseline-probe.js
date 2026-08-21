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
  if (document.body) {
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
  }
})();
