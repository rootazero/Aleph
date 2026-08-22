# Tauri Cross-Platform WebView Resource Control — Design Spec

**Date:** 2026-08-21
**Scope:** `desktop/shell/` · `src/gateway/control_plane/` · `src/gateway/server/` · `src/diagnostics/checks/` · `interfaces/webchat/` · `justfile`
**Status:** Approved (方案 A + B 嵌合)
**Source material:** `Windows-macOS-linux开发Tauri时的资源控制.md` (user-supplied, 2026-08-21)

---

## 0. 中文摘要 (Executive summary)

这份 spec 把一篇跨平台 Tauri 资源控制的经验文档，逐条对照 Aleph 现状核查之后，落成五项工程改动。

**文档的三个主要章节对 Aleph 结构上不适用**：Aleph 的 Tauri shell 只打包一个 `splash/`，Panel 从 `http://127.0.0.1:18790` 的真实 loopback HTTP 服务加载——也就是文档反复推荐的"终极兜底方案"本来就是 Aleph 的出厂形态。因此 `asset://` / `tauri://` 自定义协议的 CORS 问题在本仓不存在，"重 Rust 轻 Web"就是 R1/R2/R4 红线，"网络请求下沉 Rust"已经是全部 Panel 出口的现状。

**真实缺口有五条**，按代价排序：Panel 的 WebView 支持下限从未被声明或强制（旧 macOS 上静默变成不可读页面）· 21.98 MB 的 WASM 每次冷加载被现场压缩两次 · 两条字节路由不支持 Range/206 · Linux 缺 GStreamer 插件时 TTS 静默失败且无诊断 · 毛玻璃在 Linux 上没有降级路径。

设计的统一判据：**每条事实落在唯一知道它的那一层，不新增第二个真源**。

---

## 1. Background

### 1.1 What the source document recommends

The document covers four themes across Windows (WebView2/Blink), macOS
(WKWebView), and Linux (WebKitGTK):

1. Media codec divergence — Linux WebKitGTK lacks proprietary codecs without
   GStreamer plugins.
2. CSS/layout divergence — scrollbars, `backdrop-filter` cost, font smoothing,
   modern-selector availability.
3. Web API divergence — context menus, drag & drop, storage quotas,
   `getUserMedia` permission declarations.
4. Runtime version consistency — WebView2 is evergreen; WKWebView is pinned to
   the macOS version; WebKitGTK is pinned to the distro.

Its architectural conclusion — "heavy Rust, light Web", and when custom
protocols get in the way, embed a real localhost HTTP server — is the strongest
part of the document.

### 1.2 What Aleph already satisfies

Verified by reading the code, not assumed:

| Document recommendation | Aleph state | Evidence |
|---|---|---|
| Embed a real localhost HTTP server instead of a custom protocol | **Already the shipping shape.** `frontendDist` is `./splash`; the Panel loads from `http://127.0.0.1:18790` | `desktop/shell/tauri.conf.json` `build.frontendDist`; `capabilities/default.json` `remote.urls` |
| Heavy Rust, light Web | Redlines R1/R2/R4 | `CLAUDE.md` |
| Push network requests down to Rust | Panel has exactly one `fetch` call site, same-origin; CSP is `connect-src 'self' ws: wss:` | `interfaces/webchat/src/context.rs:620`; `src/security/headers.rs:21` |
| Linux blank-webview workaround | `WEBKIT_DISABLE_DMABUF_RENDERER=1` set before WebKit init | `desktop/shell/src/main.rs:158-174` |
| macOS `getUserMedia` permission declaration | `NSMicrophoneUsageDescription` present; Windows WebView2 grant handler installed | `desktop/shell/Info.plist:8`; `desktop/shell/src/webview_perms.rs` |
| Static asset caching | gzip + content-hash ETag + 304 revalidation | `src/gateway/control_plane/server.rs` |

**Consequence:** three of the document's four chapters (macOS custom-protocol
CORS, Linux custom-protocol CORS, Windows custom-protocol CORS) describe a
problem class Aleph structurally does not have. This spec does not add
`convertFileSrc`, `register_uri_scheme_protocol`, or any custom scheme.

### 1.3 The five real gaps

**G1 — The Panel's real WebView floor is undeclared, unenforced, and fails
silently.**
`interfaces/webchat/dist/tailwind.css` contains **328 unguarded `oklch()`
custom-property definitions**. On an engine that cannot parse `oklch()`,
`var(--color-surface)` becomes invalid at computed-value time and the entire
palette collapses to `initial`/`inherit` — an unreadable page, no error. Tailwind
v4.2's own floor is Safari 16.4 / Chrome 111 (≈ macOS 13.3+, WebKitGTK ≈2.42+).
The repo declares no `minimumSystemVersion`, has no browserslist, no CI baseline
check, and no runtime probe.

Scope note verified during survey: CI builds against `libwebkit2gtk-4.1`, whose
availability already floors Linux at Ubuntu 22.04 / Debian 12 — both currently
ship WebKitGTK 2.48, which supports all three features. **The real exposure is
almost entirely macOS.**

Sibling risk: `justfile`'s wasm recipe pins no WASM target features. Rust 1.82+
enables the WASM 2.0 set by default on `wasm32-unknown-unknown`. That set is
inside the Safari 16.4 baseline today, so nothing is broken now — but a future
toolchain bump could silently enable a feature outside it, and the symptom is
the same white screen.

**G2 — 21.98 MB WASM, compressed twice on every cold load.**
`dist/aleph_panel_bg.wasm` is 21,980,519 bytes (the comment in `server.rs`
claiming "~15.5 MB → ~3.7 MB gzipped" is stale; measured `gzip -9` = 5,020,809
bytes). `rust-embed` has the `compression` feature (zstd in-binary),
`tower-http` has only `compression-gzip`. So every ETag miss runs: zstd-decompress
22 MB → gzip-compress 22 MB → send 5.02 MB.

**G3 — Neither byte route supports Range/206.**
Repo-wide there is no `ACCEPT_RANGES` or `PARTIAL_CONTENT`. `/artifact/...` and
`/canvas-asset/...` return whole bodies. The document names this specifically for
Linux: WebKitGTK plays media through GStreamer, and without Range the seek bar
does not work, audio does not buffer, and large files can fail outright.

**G4 — Linux TTS playback can be entirely silent with no diagnosis.**
`voice_playback.rs` defaults to `audio/mpeg` (MP3). WebKitGTK's MP3/AAC decoding
depends on `gstreamer1.0-plugins-{bad,ugly}`. When they are absent, `play()`
rejects and the only trace is a `console::warn`. The user sees "voice is broken"
and nothing tells them which package is missing.

**G5 — `backdrop-filter` has no Linux degradation path.**
18 occurrences in the built CSS. The document names it as the top Linux
performance hazard on machines without hardware acceleration.

### 1.4 Two first-pass claims that were WRONG — recorded so they are not re-derived

1. ~~"`-webkit-font-smoothing` appears only once, so macOS text renders heavy."~~
   **Retracted.** That one occurrence is on `body`
   (`interfaces/webchat/styles/tailwind.css:211`), paired with
   `-moz-osx-font-smoothing: grayscale`. It is global and correct. The claim was
   derived from an occurrence count without reading the context.
2. "`backdrop-filter` needs a new Linux degradation path" **overstated the cost.**
   A complete ~70-line degradation block already exists under
   `@media (prefers-reduced-transparency: reduce)`
   (`styles/tailwind.css:1078-1145`). It both zeroes the `--glass-*` tokens and
   sets `backdrop-filter: none !important` on the concrete classes. G5 needs a
   second *trigger*, not a second *block*.

---

## 2. Decisions taken (with the reasoning that is load-bearing)

| # | Decision | Alternatives rejected, and why |
|---|---|---|
| D1 | **Support floor = macOS 13.3+ / Safari 16.4 / WebKitGTK 2.42+.** Make the contract Tailwind v4 already implies explicit and enforced. | Dropping to macOS 12.3 would require generating an sRGB fallback layer for 328 tokens. Dropping to macOS 11 would mean abandoning `oklch`/`color-mix`/`@property` — a rebuild of the design-token system. |
| D2 | **Build-time precompression**, `.br` siblings committed to git. | Adding `compression-br` to tower-http is a one-line change but makes the runtime compress 22 MB with brotli — slower first byte than today's gzip. Trading latency for bandwidth is the wrong direction here. |
| D3 | **Platform fact is declared by the host** (shell fills `data-platform` on all three), capability probing used only where it is accurate (the CSS baseline). | Pure capability probing is structurally blind to G4/G5: `CSS.supports('backdrop-filter: blur(1px)')` answers `true` on WebKitGTK (the problem is "supported but slow"), and `canPlayType` reflects engine claims, not installed decoders. Server-side adjudication (`std::env::consts::OS`) would confidently give the *wrong* answer for a remote Tailnet client and violates R6. |
| D4 | **`wasm-opt` becomes required**, not an optional shrink step. | It now carries a correctness gate (the WASM feature fence). A gate that silently skips when a tool is missing is a gate that exists only on the author's machine. Cost accepted: contributors without binaryen cannot run `just wasm`; the recipe must say so loudly. |
| D5 | **Range requests get their own wider rate-limit bucket.** | Byte-based limiting is semantically better but requires changing `RateLimiter` itself, spilling into another subsystem. Doing nothing means remote Tailnet media playback 429s. |
| D6 | **Linux degrades glass unconditionally; no opt-out toggle.** | `Material` has three variants (Luxe/Liquid/Aurora) and all three are glass, so the existing knob cannot express "solid". Adding a fourth variant or a separate toggle is a real product surface (icon, copy, i18n, persistence key) that this round does not buy. Reversible later — nothing here locks it out. |

### 2.1 Explicit non-goals

These are decisions, not omissions. Each is listed so a later reader does not
read the gap as a bug:

- **No custom URI scheme work.** No `convertFileSrc`, no
  `register_uri_scheme_protocol`, no `asset://`. Aleph does not use them.
- **No WASM size audit.** D2 covers transport, not payload. Reducing the 22 MB
  itself (locale inlining, `web-sys` feature surface, name section) is a separate
  investigation with uncertain yield.
- **No WebView2 fixed-runtime bundling.** It would add roughly 180 MB to the
  Windows installer. Only `webviewInstallMode` is declared.
- **No scrollbar changes.** The document names cross-platform scrollbar
  divergence, and Aleph has 9 `::-webkit-scrollbar` rules plus 2
  `scrollbar-width`. There is no observed symptom; changing them would be an
  evidence-free adjustment.
- **No TTS format negotiation.** The deeper fix for G4 is for the core to prefer
  Opus/WebM (covered by `gstreamer1.0-plugins-good`, present on virtually every
  distro) over MP3. That depends on the TTS provider capability matrix and is its
  own spec. **Recorded as follow-up FU-1.**
- **`src/harness/` is untouched.** Zero LOC delta; the R10 ratchet is unaffected.

---

## 3. Architecture — one throat per fact

The unifying criterion: every fact lands in the single layer that actually knows
it, and no fact gains a second source of truth.

| Fact | Who actually knows it | Throat (single source) | Consumers |
|---|---|---|---|
| What platform / WebView am I on | shell (knows `target_os` at compile time); browser case has only the UA | `baseline-probe.js` resolves it once and **writes `data-platform` if absent**; `platform_host.rs::host()` only *reads* the attribute (see §3.2) | G5 flat trigger, G4 copy |
| Is this WebView new enough | only the running engine | `interfaces/webchat/baseline-probe.js`, inlined into the generated `index.html` **before** the module script | G1 fallback page, G5 flat trigger |
| What floor do we promise | build-time declaration | `interfaces/webchat/webview-baseline.json` (see §3.3) | installer, CI, runtime probe |
| Are these bytes precompressed | build time (`just wasm`) | `dist/*.br` siblings + the pairing assertion in `check_panel_dist.mjs` | `control_plane/server.rs` negotiation |
| Can these bytes be fetched in parts | shared by both byte routes | `src/gateway/server/byte_range.rs` | `/artifact`, `/canvas-asset` |
| What decoders does this machine have | only the Linux runtime | `src/diagnostics/checks/media_codecs.rs` | `aleph doctor`, TTS failure receipt |

### 3.1 Redline placement

- **shell**: only the three-platform completion of `SHELL_MARKER_JS` — a string
  constant, zero logic. R2/R10 clean.
- **Panel**: `platform_host.rs` plus CSS degradation — UI rendering, the correct
  side of R2.
- **gateway**: Accept-Encoding negotiation and the Range helper — pure I/O.
  R4 clean.
- **No new runtime dependency.** Brotli encoding happens at build time via node's
  built-in `zlib.brotliCompressSync` (`just wasm` already runs node to resolve
  cargo metadata). The Range helper is hand-written; `tower-http`'s `fs` path is
  not used.

### 3.2 Ordering hazard: `data-platform` is NOT reliably set when the probe runs

`SHELL_MARKER_JS` is registered as an `initialization_script`, which runs before
page scripts — but **only for same-origin pages**. `desktop/shell/src/main.rs`
says so in its own comment, which is why the shell *also* re-asserts the marker
from `on_page_load` for the remote case. `on_page_load` fires at
`PageLoadEvent::Finished` — **after** the inline probe. So in a panel-only shell
pointed at a remote Gateway, and in any plain browser, `data-platform` does not
exist at probe time.

This is the "A 之后紧跟 B 时算数的是 B" shape, and reading it wrong would make the
G5 Linux trigger silently dead in exactly the deployments that are hardest to
notice.

**Resolution — the probe owns the resolution, everything else reads it:**

1. `baseline-probe.js` resolves the platform itself: use `data-platform` if the
   shell already set it; otherwise derive it from the UA; then **write the
   attribute** so there is exactly one resolved value on the document.
2. `platform_host.rs::host()` is a pure *reader* of `data-platform`. It contains
   no UA fallback — one implementation of the fallback, in the probe, in JS.
3. The shell's existing `on_page_load` re-assert stays and remains idempotent: it
   writes the same value the probe already derived.

The UA fallback only needs to distinguish three buckets; it is not a general
UA parser, and its ambiguous case resolves to `linux`, which is the **safe**
direction (flat rendering is a degradation, never a hazard).

### 3.3 The baseline declaration and its three derived consumers

`interfaces/webchat/webview-baseline.json` is the one declaration:

```json
{
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

Three consumers derive from it, and **one guard covers all three edges**
(§4.2): the two `tauri.conf.json` files' `minimumSystemVersion`,
`baseline-probe.js`'s probe list, and `scripts/check_webview_baseline.mjs`.

The JSON is deliberately not code-generated into the probe. Generation would
remove drift but adds a build step and a generated tracked file; a single guard
that fails by name is cheaper and fails just as loudly. If that guard ever grows
a third edge, revisit.

---

## 4. G1 — The compatibility baseline, in three gates

### 4.1 Gate 1 — install time (zero code, hardest)

Both `desktop/shell/tauri.conf.json` and `tauri.lite.conf.json` gain:

```json
"bundle": { "macOS": { "minimumSystemVersion": "13.3" } }
```

macOS refuses the install itself; the user sees the OS's own message. Windows and
Linux are unaffected (`minimumSystemVersion` is macOS-only).

### 4.2 Gate 2 — build time

**A source-level guard** (a Rust test that reads the files) asserts four things
are consistent, all against `webview-baseline.json` (§3.3):

1. Both `tauri.conf.json` files' `minimumSystemVersion` equal `macos_min`.
2. `baseline-probe.js`'s probe list equals `css_probes` + `js_probes` — **set
   equality in both directions**, so neither a new probe nor a removed one can
   drift.
3. The inline probe in `interfaces/webchat/dist/index.html` is byte-identical to
   `interfaces/webchat/baseline-probe.js` — catching a one-sided `just wasm`
   rebuild, the same class the existing js/wasm pairing guard exists for.
4. **Every probe entry is actually exercised by `dist/tailwind.css`.**

Item 4 is a *reverse* assertion, and the asymmetry must be stated in the guard's
doc comment. The forward assertion — "every modern capability in the CSS must be
covered by a probe" — **cannot be made honest**: a scanner only recognizes the
patterns it was taught, which is exactly the "enumeration only covers the world as
it was on the day it was written" failure named in `CLAUDE.md` §0. The reverse
assertion catches the failure that *can* be caught: a probe list rotting into a
stale licence.

The forward half is covered by a deliberately **over-reporting** approximation:
extract every CSS function name appearing in `dist/tailwind.css`, diff against a
known set, and **fail on any new name**, handing the judgement to a human. Its
failure direction is false-positive, never false-negative.

**WASM feature fence.** `just wasm` passes an explicit allow-list to `wasm-opt`:

```
--enable-bulk-memory --enable-sign-ext --enable-nontrapping-float-to-int
--enable-mutable-globals --enable-multivalue --enable-reference-types
```

(no `--enable-simd`). If a toolchain bump enables a feature outside the set,
wasm-opt fails validation. Per D4, `wasm-opt` becomes required: the recipe must
**error loudly** when binaryen is absent, naming the install command and stating
that this step is now a correctness gate, not a size optimization.

### 4.3 Gate 3 — runtime fallback page

`baseline-probe.js` runs as a **synchronous inline `<script>`** placed before the
`<script type="module">` that boots the WASM (module scripts are deferred, so
ordering is guaranteed). It does three things, in this order:

1. **Resolve and write `data-platform`** (§3.2) — must come first, because the
   flat-mode decision below depends on it.
2. **Compute `data-flat`** from `prefers-reduced-transparency` OR `linux` (§6.1),
   and register the `matchMedia` listener.
3. **Run the baseline probes**, whose list is the one in `webview-baseline.json`:
   - `CSS.supports('color', 'oklch(0 0 0)')`
   - `CSS.supports('color', 'color-mix(in oklab, red, red)')`
   - `typeof CSS.registerProperty === 'function'`
   - `typeof WebAssembly === 'object'`

On probe failure it sets `data-webview-unsupported="1"` on `<html>` and renders a
page. Steps 1 and 2 run **unconditionally**, before and independently of the probe
verdict — a supported browser still needs its platform and flat attributes.

**The trap:** the fallback page must not use `tailwind.css` — that file is
precisely what has failed. It carries its own inline styles using hex colors and
basic layout only.

The page states three things: which capability was actually missing (measured,
not guessed), the concrete threshold (macOS 13.3+ / WebKitGTK 2.42+), and the
**ways out** — the CLI, the TUI, and the phone Panel all still work. Not
dead-ending the user is R5.

The same script also computes the flat-mode attribute used by G5 (see §6.1).

---

## 5. G2 + G3 — the transport layer

### 5.1 G2 — build-time precompression

**Hard constraint discovered during survey, and it shapes the whole design:**
`interfaces/webchat/dist/` is a **git-tracked build output**. The release workflow
embeds it verbatim and **no job owns a WASM toolchain** — `check_panel_dist.mjs`'s
header says so literally, and that guard exists because of the v26.6.22 js-only
rebuild. Therefore the `.br` files must also be committed, and a `.br` out of sync
with its `.wasm` would **serve wrong bytes** — worse than the problem it fixes.

**Producer** (`just wasm`, new step after wasm-opt): for every file in `dist/`
larger than 4 KiB whose brotli output is actually smaller, write an `X.br`
sibling. The criterion is size plus measured benefit, **not an extension
allow-list**. Parameters: `quality = 11`, `lgwin = 24` (the standard-brotli
maximum, not the Large-Window extension), `SIZE_HINT` set to the original size.
Round-trip verify immediately after writing. Cost: roughly 1–2 minutes for the
22 MB wasm, paid once per `just wasm`.

**Guard** (`check_panel_dist.mjs`, **both directions**):

- every `X.br` must decompress byte-identically to `X`;
- every dist file over the threshold must **have** an `X.br`.

Writing only one direction is a recurrence of §0's "count the writers when you
converge them".

**Server** (`control_plane/server.rs::serve_static_or_index`):

`CompressionLayer` **is not removed.** tower-http's compression layer passes
through any response that already carries a `Content-Encoding`. So the route only
needs to set `Content-Encoding: br` when it serves the precompressed sibling;
runtime compression then does not happen, files without a `.br` keep their gzip
path, and a client that does not send `Accept-Encoding: br` (bare curl) is
unaffected. **This behaviour is asserted with a real request on Windows, not
taken from documentation.**

**The one correctness trap in this section:** the ETag must always be derived from
the **identity** representation, with `Vary: Accept-Encoding` on the response. An
ETag that follows the served representation gives a client that switched
`Accept-Encoding` a false 304 — it receives brotli bytes believing they are
identity.

Expected effect: an ETag miss goes from
*zstd-decompress 22 MB + gzip-compress 22 MB → send 5.02 MB*
to *zstd-decompress ~3.4 MB → send ~3.4 MB*, with compression CPU at zero.

**Measured on Windows, 2026-08-22**, from the `just wasm` precompression pass
(build-time brotli, quality 11, 4 KiB floor):

| Asset | identity | `.br` | saved |
|---|---|---|---|
| `aleph_panel_bg.wasm` | 21,914,484 | 3,360,760 | −84.7% |
| `tailwind.css` | 145,347 | 19,476 | −86.6% |
| `aleph_panel.js` | 110,252 | 13,373 | −87.9% |
| `index.html` | 6,925 | 2,411 | −65.2% |
| **total** | **22,177,008** | **3,396,020** | **−84.7%** |

All four assets are over the floor, so none is skipped. The wasm figure lands
where the estimate put it. Observed `content-encoding` behaviour is recorded
in §7.3 rather than here, because it is a wire observation, not a build one.

### 5.2 G3 — Range / 206

New `src/gateway/server/byte_range.rs`, called by **both** byte routes — not
written twice.

- `parse_range(header, total)` returns three states: no Range header /
  `Satisfiable { start, end }` / `Unsatisfiable`.
- **Single range only** (`bytes=a-b`, `bytes=a-`, `bytes=-n`).
  `multipart/byteranges` is **deliberately not implemented**: browser media
  elements and GStreamer issue single ranges, and that complexity buys nothing.
  This is a recorded decision, not an omission.
- `Unsatisfiable` → 416 with `Content-Range: bytes */<total>`.
- **Every** successful response carries `Accept-Ranges: bytes` — that is how the
  client decides whether to offer a seek bar at all.

Two things that are easy to get wrong:

1. Range must be applied **after every existing gate** (rate limit, origin policy,
   capability validation, session resolution). It is a representation concern and
   must not bypass authorization.
2. `/artifact` serves HTML/SVG under `ARTIFACT_DOCUMENT_CSP`. **The 206 and 416
   responses must carry the same CSP** — a 206 is still part of that document.

**Rate limiting (D5).** `ARTIFACT_READS_PER_MINUTE = 240` counts requests. A
seek-heavy playback issues far more than 240. Loopback is exempt, so the desktop
App is unaffected, but remote Tailnet playback would 429. Requests carrying a
`Range` header draw from a separate bucket, `ARTIFACT_RANGE_READS_PER_MINUTE =
3000` (≈50/s — comfortably above any human scrubbing pattern, still four orders of
magnitude below what a scraper needs to be worth writing). The first, Range-less
request still draws from the existing 240/min bucket, so **the number of distinct
artifacts a caller can start pulling per minute is unchanged** — which is the
property the limiter was built for. Ranges only widen re-reads *within* an
artifact the caller was already allowed to open.

---

## 6. G4 + G5 — platform-differentiated degradation and diagnosis

### 6.1 G5 — reuse the existing degradation path

**The trap to avoid:** copying the ~70-line block for Linux creates a second
source of truth. And CSS cannot OR an `@media` condition with an attribute
selector.

Zeroing the tokens alone is **not sufficient**: `backdrop-filter: blur(0px)` still
creates a compositing layer and still costs on WebKitGTK — which is the exact
thing being fixed. The `backdrop-filter: none` half is required, so the rule block
cannot be avoided.

**Design:** change that block's condition from
`@media (prefers-reduced-transparency: reduce)` to a single attribute selector
`html[data-flat="1"]`. The attribute is computed by the inline probe script G1
already adds, from two inputs:

```
matchMedia('(prefers-reduced-transparency: reduce)').matches
  ||  resolvedPlatform === 'linux'
```

where `resolvedPlatform` is the value the same script resolved and wrote in step 1
(§3.2) — **not** a second read of `data-platform`, which may not exist yet. Plus a
`matchMedia` change listener so a mid-session OS setting change still applies.
**One rule block, one attribute, two inputs.**

Accessibility risk, stated plainly: degradation now depends on JS. The risk is
bounded — the probe is a synchronous inline script running before the WASM boots,
so if it does not run the Panel does not load at all. There is no partial state
where the page renders but the degradation is missing.

**shell change:** `SHELL_MARKER_JS` currently sets `data-platform` **only on
macOS** (`desktop/shell/src/main.rs:88-95`; the non-macOS arm sets only
`data-shell`). Complete it for all three platforms. This asymmetry is itself a
pre-existing instance of §0's "count how many faces this capability has".

**Windows:** both conf files declare
`bundle.windows.webviewInstallMode: { "type": "downloadBootstrapper", "silent": true }`
— making today's implicit default explicit and silent.

### 6.2 G4 — Linux decoders: diagnosis plus receipt

**Diagnosis (primary):** `src/diagnostics/checks/media_codecs.rs`, Linux-only.
It asks **GStreamer itself** (`gst-inspect-1.0 --exists <element>`) rather than
querying distro package names — package names hold under neither Flatpak, Snap,
nor a source build. Elements probed: `mpg123audiodec`/`avdec_mp3` (MP3),
`avdec_aac` (AAC), `vp8dec`/`vp9dec`/`opusdec` (WebM/Opus).

**Three states, not two:** `Ok` / `Missing(list)` / **`Unknown(reason)`**.
`gst-inspect-1.0` may itself be absent (`gstreamer1.0-tools`), and that answer is
"I don't know" — which must not be read as healthy (§8) and equally must not be
read as broken.

**A deadline is mandatory.** `gst-inspect-1.0` rebuilds the GStreamer registry on
a cold run and can take seconds, and `doctor` runs inside an agent turn — a check
without a deadline disguises silence as health. Timeout folds to a **named**
Warning.

**Receipt (secondary):** `voice_playback.rs::play()` currently only
`console::warn`s on rejection. Add a user-visible message through the Panel's
existing error surface (not a new one), behind a **narrow** predicate: only
`audio.error.code === MEDIA_ERR_SRC_NOT_SUPPORTED (4)` means "missing decoder".
`NotAllowedError` (autoplay policy) is a different thing and must not be
conflated.

A `canPlayType()` pre-check is added as a *second leg*, not a foundation: an
empty-string answer prompts immediately; `"maybe"`/`"probably"` still attempts
playback and falls through to the receipt on failure. Both legs exist because
**"does WebKitGTK's `canPlayType` consult the GStreamer registry" is an
unverified assumption** and is not used as load-bearing.

---

## 7. Testing and three-platform delivery

Verification constraint accepted from the user: **Windows is the only machine
available here.** Linux and macOS receive runnable scripts with documented
assertions, to be executed by the user on those platforms.

### 7.1 Windows — run locally, and each guard is broken once to prove it goes RED

| # | Verification | Mutation used to prove RED |
|---|---|---|
| 1 | `byte_range.rs` unit tests (platform-independent) | flip the `Unsatisfiable` arm to `Satisfiable` |
| 2 | Both routes: 206 / 416 / `Content-Range` / `Accept-Ranges` / **206 also carries CSP** / Range applied after the auth gates | move range application ahead of capability resolution |
| 3 | br negotiation hit · gzip fallback when no `.br` · **ETag does not emit a false 304 across `Accept-Encoding`** · `Vary` present | derive the ETag from the served representation |
| 4 | `check_panel_dist.mjs` br pairing, both directions | flip one byte inside a `.br` |
| 5 | The three G1 source-level guards | change one conf's `minimumSystemVersion` |
| 6 | wasm-opt feature fence | rebuild with `-C target-feature=+simd128` and assert wasm-opt errors |
| 7 | WebView2 on real hardware: fallback page · `content-encoding: br` · mp4 seek issues 206 · microphone still works | override `CSS.supports` from devtools |

### 7.2 `qa/webview_compat/run.sh <linux|macos>` — for the user to run

Every assertion is an **effect assertion**, never "the command exited 0". Each one
prints the value it actually read, so a red result is immediately attributable to
the assertion or to the code.

| Assertion | What it checks |
|---|---|
| `br-negotiation` | response header is `br` **and** body < 4 MiB **and** the brotli-decompressed sha256 equals the dist `.wasm` |
| `range-206` | `Range: bytes=100-199` → 206 + correct `Content-Range` + **exactly 100 bytes** + content equals the corresponding slice of the original |
| `range-416` | out-of-range → 416 + `Content-Range: bytes */<total>` |
| `gst-codecs` (Linux) | the doctor check returns one of the three states, and the script distinguishes `Unknown` (your box lacks `gstreamer1.0-tools`) from a genuine `Missing` |
| `flat-on-linux` | `documentElement.dataset.flat === "1"` **and** `getComputedStyle('.glass').backdropFilter === "none"` |
| `tts-playback` | **both directions**: success path asserts `audio.error === null && currentTime > 0`; failure path asserts the user-visible receipt actually appeared. Asserting only the success path means a box missing the codec goes red without telling you whether the receipt worked. |
| `min-system-version` (macOS) | `.app/Contents/Info.plist` `LSMinimumSystemVersion == 13.3` (no old machine needed) |
| `wkwebview-baseline` (macOS) | injected JS asserts all four probes pass |
| `tts-blob` (macOS) | blob object-URL playback succeeds (the WKWebView `data:`-URL issue) |
| `vibrancy` (macOS) | window remains transparent and the material is still applied |

### 7.3 Two things labelled honestly

1. **Only the Windows guards go through "break it, watch it go red".** The
   Linux/macOS assertions are guaranteed correct in shape (each names a file and
   line, each has a breakable point), but the first time one goes red, **the red
   may be the assertion rather than the code** — which is why each prints its
   observed value.
2. **One assertion is necessarily SKIPped**: install refusal and the fallback page
   below macOS 13.3 need an old machine. The script marks it
   `SKIP: requires macOS < 13.3` and this spec records it as **not verified on
   real hardware** rather than pretending coverage.
3. **Only the Windows arm of the shell's platform marker has ever been
   compiled.** The macOS and Linux `cfg` arms of `SHELL_MARKER_JS` are
   unverified. The QA script's header says so, so that a Linux user seeing no
   `data-platform` attribute knows which half is untested rather than assuming
   a broken install.

#### What was actually falsified, by mutation

Each guard below was broken, observed red, restored, and observed green. The
list is exhaustive for guards this work added — anything not named here was
not falsified.

| Guard | Mutation | Observed |
|---|---|---|
| brotli sibling is served | serve the identity file | red |
| `br;q=0` refusal honoured | accept any token containing `br` | red |
| dist pair check, direction 1 | truncate / garbage the `.br` | red, "not valid brotli" |
| dist pair check, direction 1b | recompress different content | red, "is STALE" |
| dist pair check, direction 2 | delete the `.br` | red |
| Range status mapping | force `OK` for `Satisfiable` | red |
| `is_bulk_read` | revert to exact coverage | red ×4, across three modules |
| capability gate precedes Range | disable the gate's early returns | red, 206 where 404 required |
| codec verdict tags | tag the unknown verdict `codecs-ok` | red |
| decoder predicate, platform half | drop the Linux clause | red |
| decoder predicate, code half | move the code from 4 to 3 | red |
| flat-mode census | drop `.aleph-todo-wrap`'s rule | red, names todo_panel.rs |
| flat-mode census | drop `.glass` from its rule | red, names tailwind.css |

Two mutations did **not** go red, and both were the mutation's fault rather
than the guard's — recorded because a silent non-red is otherwise
indistinguishable from a blind guard:

- `printf '\x00' >> tailwind.css.br` (the plan's original text for direction
  1). Appending to a brotli stream is inert: the decoder stops at the final
  block, output is byte-identical, and no guard can fire. Replaced with the
  two mutations above.
- Removing `.aleph-composer` from the flat block. That selector has a flat
  rule but sets no backdrop filter anywhere, so there was nothing for the
  census to miss.

---

## 8. Follow-ups recorded, not done

- **FU-1 — TTS format negotiation.** Have the core prefer Opus/WebM over MP3 when
  the client reports it cannot decode MP3. Depends on the TTS provider capability
  matrix; its own spec.
- **FU-2 — WASM payload audit.** The 22 MB uncompressed figure itself. Multiplies
  with G2 rather than replacing it.
- **FU-3 — Byte-based rate limiting.** D5 takes the cheap bucket instead; the
  semantically correct fix changes `RateLimiter`.

  Review sharpened this into something with a measurable residual. The shipped
  predicate (`RangeVerdict::is_bulk_read`) charges the narrow bucket when a
  ranged request returns **more than half** the resource, which closes the
  `Range: bytes=1-` bypass — a fixed string, needing no knowledge of the size,
  that returns a complete usable copy while an exact-coverage test waves it
  through. What it does not close: a caller who splits each resource across
  two requests stays under the threshold on both, so bulk reading is bounded
  at roughly **half the wide bucket's rate**, not at the narrow bucket's.
  Lowering the threshold buys less than it costs, because real playback pulls
  large chunks. Only byte-budget accounting — this follow-up — makes the
  narrow bucket mean what its name says.
- **FU-4 — A `Material::Solid` variant or a transparency toggle.** D6 degrades
  Linux unconditionally; giving the user a way back is a real product surface.

---

## 9. Files touched

| Path | Change |
|---|---|
| `desktop/shell/tauri.conf.json` | `minimumSystemVersion`, `webviewInstallMode` |
| `desktop/shell/tauri.lite.conf.json` | same two |
| `desktop/shell/src/main.rs` | `SHELL_MARKER_JS` completed for all three platforms |
| `desktop/shell/src/` (new test module) | the four-way baseline consistency guard (§4.2) |
| `interfaces/webchat/webview-baseline.json` | **new** — the single baseline declaration (§3.3) |
| `interfaces/webchat/baseline-probe.js` | **new** — platform resolution + `data-flat` + baseline probes, in that order (§4.3) |
| `interfaces/webchat/src/platform_host.rs` | **new** — `host()`, a pure reader of `data-platform` with no UA fallback (§3.2), alongside the existing `is_native_shell()` shape |
| `interfaces/webchat/styles/tailwind.css` | degradation block re-keyed from `@media` to `html[data-flat="1"]` |
| `interfaces/webchat/src/platform/wide/views/chat/voice_playback.rs` | narrow-predicate receipt + `canPlayType` pre-check |
| `interfaces/webchat/dist/*` | rebuilt, plus committed `.br` siblings |
| `justfile` | brotli step; wasm-opt required + feature allow-list; probe inlined into the generated `index.html` |
| `scripts/check_panel_dist.mjs` | bidirectional `.br` pairing assertions |
| `scripts/check_webview_baseline.mjs` | **new** — over-reporting CSS function-name diff |
| `src/gateway/control_plane/server.rs` | Accept-Encoding negotiation, identity-derived ETag, `Vary` |
| `src/gateway/server/byte_range.rs` | **new** — shared Range helper |
| `src/gateway/server/artifact_route.rs` | Range wiring after the gates; CSP on 206/416; Range bucket |
| `src/gateway/server/canvas_asset_route.rs` | same |
| `src/diagnostics/checks/media_codecs.rs` | **new** — Linux GStreamer probe, three-state, deadlined |
| `qa/webview_compat/run.sh` | **new** — Linux/macOS effect assertions |

`src/harness/` — **zero delta.**
