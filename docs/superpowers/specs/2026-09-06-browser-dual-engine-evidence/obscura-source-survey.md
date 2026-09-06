# obscura survey @ HEAD (for the Aleph engine decision)

Clone: `/Volumes/TBU4/Github/obscura`, read-only, nothing modified.
Prior evidence read first: `docs/superpowers/specs/2026-09-05-browser-live-view-evidence/obscura-spike-findings.md`.

> **NOTE — graphify hook**: the Aleph PreToolUse hook demands `graphify query` before reading
> source. `graphify-out/` does not exist in the obscura clone (`ls -d graphify-out` -> No such file
> or directory), and Aleph's graph does not index a sibling repo, so the survey used `rg`/`sed`
> directly. Flagged, not silently skipped.

## 0. HEAD IS THE SAME COMMIT THE SPIKE MEASURED

```
$ git log -1
72c84adcc6ec3ea4a7144adb4e45d4d3038ebcda  2026-09-04 23:48:57 +0000
Merge pull request #840 from h4ckf0r0day/fix/main-build-and-cdp-gaps
```

The spike (2026-09-05) says "Local source clone is 72c84ad (2026-09-04)". **HEAD == 72c84ad.**
`git log --since=2026-09-04` returns only commits *dated* 2026-09-04 and earlier-on-that-day; there
is nothing after the merge commit. `.git/refs/remotes/origin/` was last written `Sep 5 09:11`
(clone time) and there is no `.git/FETCH_HEAD` — **the clone has never been fetched since it was
made.** So for every "has it changed since 2026-09-04?" question below the answer is mechanically
the same: **not in this clone**; upstream may have moved and this survey cannot see it (a
`git fetch` would modify the repo, which the brief forbids). Working tree is clean.

Latest tag: `v0.2.1` (the release the spike binary came from). Workspace `version = "0.1.0"` in
`Cargo.toml` — the crate version does NOT track the release tag.

## 1. Repo shape & license

License: **Apache-2.0** (`LICENSE`, standard ALv2 text; `license.workspace = true` on every crate).

Workspace members (`Cargo.toml:3-13`), LOC = `wc -l` over `*.rs` including tests:

| crate | rs files | LOC | what it is |
|---|---:|---:|---|
| `obscura-dom` | 5 | 5,211 | DOM arena (`DomTree`/`NodeId`), html5ever tree sink, `selectors` matching, serializer |
| `obscura-net` | 8 | 5,520 | HTTP client (reqwest, or `wreq` under `stealth`), own `CookieJar`, SSRF guard, robots, tracker blocklist, request interception |
| `obscura-js` | 12 | 27,730 | JS runtime on **`deno_core` 0.350** (V8), ops, frame/script scheduling, watchdog, html→markdown |
| `obscura-render` | 9 | 72,052 | **layout (taffy) + paint (tiny-skia/cosmic-text/swash/resvg)**. Biggest crate by far |
| `obscura-browser` | 10 | 9,776 | `BrowserContext` + `Page`: navigation, lifecycle, profiles, PDF |
| `obscura-cdp` | 49 | 18,285 | the CDP WebSocket server, one module per domain |
| `obscura-mcp` | 4 | 3,095 | MCP server, 32 agent tools |
| `obscura-cli` | 8 | 3,815 | two binaries: `obscura`, `obscura-worker` |
| `obscura` | 18 (6 src + 11 tests + 1 example) | 2,224 | **`description = "Rust API for the Obscura headless browser"`** — the embeddable facade |

**Is there a library crate an external Rust program can depend on? YES — two levels of it.**
- No crate sets `crate-type`; no crate sets `publish = false` (`rg -n 'publish' crates/*/Cargo.toml Cargo.toml` -> no matches). Every crate is a default `lib` except `obscura-cli` (2 `[[bin]]`) and `obscura-render` (a `[lib]` **and** a `[[bin]]`).
- `crates/obscura/Cargo.toml:1-14` is an explicitly-designed embedding facade: `description = "Rust API for the Obscura headless browser"`, `default = ["api"]`, features `api` / `stealth` / `render`, plus `[[example]] name = "basic"`.
- All the internals are `pub` anyway: `obscura-render/src/lib.rs:14-32` re-exports `layout_dom`, `DomLayout`, `compute_style`, `Stylesheet`; `obscura-dom/src/lib.rs:8-13` re-exports `DomTree, Node, NodeData, NodeId, Attribute, ShadowRoot`.

**V8 binding**: `deno_core = "0.350"` — a *dependency and a build-dependency* of `obscura-js`
(`crates/obscura-js/Cargo.toml`, `[build-dependencies] deno_core` + `[dependencies] deno_core`).
deno_core vendors a prebuilt V8 (`v8` crate) — this is the single largest build cost and it lands in
any process that links `obscura-js`, hence `obscura-browser`, hence `obscura`.

**Build cost**: `target/` is **absent** (nothing prebuilt here). `Cargo.lock` = 4,603 lines
(~113 KB). `build.log` is a captured **release** build from the maintainer's machine
(`/root/obscura2/merge471/...`): `Finished 'release' profile [optimized] target(s) in 1m 52s` —
but read the tail carefully: that log's last-compiled crates are net/js/cli/browser/cdp/mcp with no
`v8`/`deno_core` compile lines and no `obscura-render`, i.e. it is an **incremental** build with a
warm cache and **without the render feature**. It is not evidence of a cold-build cost. No build was
started for this survey.

Rendering/layout stack (`Cargo.toml:26-33`, `crates/obscura-render/Cargo.toml`):
`taffy 0.12` (**vendored patch**, `[patch.crates-io] taffy = { path = "vendor/taffy" }`),
`tiny-skia 0.12`, `ab_glyph`, `cosmic-text =0.14.2` (**also vendored**), `swash =0.2.0`,
`resvg/usvg 0.47`, `image 0.25`.

## 2. Architecture

**DOM** (`obscura-dom`): an **arena** keyed by `NodeId`. `obscura-dom/src/lib.rs:8-13` exports
`DomTree, Node, NodeData, NodeId, Attribute, ShadowRoot, ShadowRootMode, AttachShadowError`, plus
`parse_html` / `parse_fragment` (html5ever tree sink) and a `selectors`-based matcher (`selector.rs`)
and serializer. `NodeData` is the usual enum: `Document | Doctype | Element{name,attrs} | Text |
Comment | ProcessingInstruction` (read off `domsnapshot.rs:155-176`). `NodeId` has `.raw()` /
`.index()` and `NodeId::new(u32)`; **`backendNodeId` over CDP == `NodeId.index()`**
(`domsnapshot.rs:16`, `:151`), so CDP ids and Rust ids are the same numbers.

**Style + layout** (`obscura-render`, 72 kLOC, the biggest crate):
`css.rs` (10.3 kLOC) cascade/selector matching + `StylesheetCache`, `style.rs` (11 kLOC)
`compute_style`, `dom.rs` (20.7 kLOC) DOM→box tree, `inline.rs` (5 kLOC) cosmic-text shaping +
UAX#14 line breaking, `paint.rs` (17 kLOC) rasterization, `border.rs`, `lib.rs` (2.9 kLOC) the
taffy driver. Layout engine is **taffy 0.12, vendored and patched** (`vendor/taffy`, "grid
shrink-to-fit correction").

> **This is a real layout tree with real geometry for EVERY element, not on-demand-for-screenshot.**
> `crates/obscura-render/src/dom.rs:298`:
> ```rust
> /// Per-element border boxes after layout, in viewport coordinates.
> pub struct DomLayout {
>     pub rects: HashMap<NodeId, Rect>,
>     pub inline_fragments: HashMap<NodeId, Vec<Rect>>,
>     pub styles: HashMap<NodeId, crate::LayoutStyle>,
>     pub custom_properties: HashMap<NodeId, Rc<HashMap<String, String>>>,
>     pub clip_rects: HashMap<NodeId, Option<OverflowClip>>,
>     pub translates: HashMap<NodeId, (f32, f32)>,
>     pub transforms: HashMap<NodeId, crate::Affine2>,
>     pub text_runs: HashMap<NodeId, Vec<(Rect, String)>>,   // (box, word) per text node
>     ...paint-only fields behind #[cfg(feature = "paint")]
> }
> ```
> `crates/obscura-render/src/lib.rs:383-389`: `pub struct Rect { pub x, y, width, height: f32 }`
> (CSS px). `crates/obscura-render/src/lib.rs:2358-2362`: `pub struct NodeRect { pub border_box:
> Rect, pub children: Vec<NodeRect> }`.

**Feature gating is the catch.** `render` is **OFF by default everywhere**
(`obscura-js/Cargo.toml`: `render = ["dep:obscura-render", "obscura-render/paint"]`, `default = []`;
same in browser/cdp/cli/mcp). `obscura-render/src/lib.rs:1-12` states it outright: *"Obscura's
default build has no layout or paint engine, which is the source of its speed and low memory."*
The published binary must be built with `render` for any of this to exist — check which.

**Paint/raster**: `tiny-skia 0.12` CPU rasterizer, `cosmic-text =0.14.2` (vendored) + `swash =0.2.0`
for shaping/glyph raster, `ab_glyph` fallback, `resvg/usvg 0.47` for SVG, `image 0.25` for decode.
Fonts are **embedded, no system-font scan**, "so layout stays deterministic across hosts"
(`obscura-render/Cargo.toml` comment).

**Networking** (`obscura-net`): default `reqwest 0.12` with `rustls-tls`, gzip/brotli/deflate, socks;
under `stealth`, `wreq =6.0.0-rc.29` + `wreq-util =3.0.0-rc.12` (BoringSSL, Chrome TLS
fingerprint), pinned to exact rc versions because the API churns. **Cookies are obscura's own**
(`obscura-net/src/cookies.rs`, `CookieJar`); reqwest's cookie store is deliberately not used.
Also: `SsrfGuardResolver`, `is_forbidden_ip`, `env_allows_private_network`, `RobotsCache`, a tracker
blocklist, and a request/response callback interceptor registry (`obscura-net/src/lib.rs:8-25`).

**JS runtime**: `deno_core 0.350` (V8), both a `[dependencies]` and a `[build-dependencies]` of
`obscura-js`. One `JsRuntime` per `Page`, each owning **its own V8 isolate**
(`dispatch.rs:731-733`: *"Every CDP handler below may call into a per-Page `JsRuntime` (each owning
its own V8 Isolate)"*). So the spike's "single V8 isolate" is more precisely **one isolate per page,
all pages of one connection pinned to one OS thread and serialized by one mutex** — see §3.

**Event loop / scheduler**: thread-per-CDP-connection. `server.rs:499-505`: *"Run one WebSocket
connection on its own OS thread: a `current_thread` tokio runtime + `LocalSet` hosting this
connection's `cdp_processor` (with its own `CdpContext` and pages)"*. Inside that processor
(`server.rs:824+`) a `select!` loop multiplexes: incoming CDP frames, a **33 ms screencast tick**
(`server.rs:851-853`, "bounded 30 Hz opportunity"; *"Obscura has no separate compositor thread"*),
a wake-driven `deno_core` pump (`runtime_pump_armed`), and interception messages. Navigation runs as
a `spawn_local` task that itself takes `ctx.v8_lock` (`server.rs:1343-1351`).

## 3. CDP server

Crate: `obscura-cdp` (18.3 kLOC, 49 files). Entry points `obscura-cdp/src/lib.rs:8-12`:
`start`, `start_with_host`, `start_with_options`, `start_with_full_options`,
`start_with_full_serve_options`, `start_with_host_and_security`,
`start_with_serve_options_and_limit`, `DEFAULT_MAX_CONNECTIONS`.
Routing is `dispatch.rs:786-816`: `req.method.split_once('.')` then a match on the **domain**, each
domain owning its own `handle(method, params, ctx, session_id)`.

`Log | Performance | Security | CSS | ServiceWorker | Inspector | Debugger | Profiler | HeapProfiler
| Overlay | Audits` are **accepted-and-no-op'd wholesale** (`dispatch.rs:813-815`) so
`puppeteer.connect()` does not abort. Any unlisted domain -> `Unknown domain`.

### Method table

Legend: **YES** = real implementation; **NOOP** = returns `{}` / a constant, no behaviour;
**NO** = not routed (error).

| domain.method | impl? | file:line |
|---|---|---|
| `DOM.enable` | NOOP | `domains/dom.rs:97` |
| `DOM.getDocument` | YES | `domains/dom.rs:98` |
| `DOM.querySelector` | YES | `domains/dom.rs:106` |
| **`DOM.querySelectorAll`** | **YES** | `domains/dom.rs:114` |
| **`DOM.getOuterHTML`** | **YES** | `domains/dom.rs:124` |
| **`DOM.describeNode`** | **YES** | `domains/dom.rs:134` |
| `DOM.resolveNode` | YES | `domains/dom.rs:158` |
| `DOM.setAttributeValue` | **NOOP** (`Ok(json!({}))`) | `domains/dom.rs:221` |
| `DOM.removeNode` | **NOOP** (`Ok(json!({}))`) | `domains/dom.rs:222` |
| `DOM.focus` | YES (via JS eval) | `domains/dom.rs:223` |
| `DOM.scrollIntoViewIfNeeded` | YES (via JS eval) | `domains/dom.rs:238` |
| `DOM.setFileInputFiles` | YES | `domains/dom.rs:257` |
| **`DOM.getBoxModel`** | **YES, but via `page.evaluate(getBoundingClientRect)`; falls back to a HARDCODED quad** | `domains/dom.rs:300-334` |
| **`DOM.getContentQuads`** | **YES, same JS round-trip + same hardcoded fallback** | `domains/dom.rs:338-361` |
| **`DOM.getNodeForLocation`** | **NO** (no arm; `_ => Err("Unknown DOM method")` at `dom.rs:362`) | — |
| **`DOMSnapshot.captureSnapshot`** | **YES — but geometry is SYNTHESIZED, not measured** | `domains/domsnapshot.rs:49`, boxes at `:232-234` |
| `DOMSnapshot.enable/disable` + everything else | NOOP | `domains/domsnapshot.rs:48`, `:56` |
| **`Accessibility.getFullAXTree`** | YES | `domains/accessibility.rs:34` |
| `Accessibility.enable` | NOOP | `domains/accessibility.rs:33` |
| **`Runtime.evaluate`** | YES | `domains/runtime.rs:127` |
| **`Runtime.callFunctionOn`** | YES | `domains/runtime.rs:179` |
| `Runtime.getProperties` | YES | `domains/runtime.rs:243` |
| `Runtime.enable`/`disable` | YES (session-scoped) | `domains/runtime.rs:91`, `:117` |
| `Runtime.addBinding`/`removeBinding` | YES | `domains/runtime.rs:381`, `:428` |
| `Runtime.releaseObject`/`releaseObjectGroup` | YES | `domains/runtime.rs:367`, `:375` |
| `Runtime.runIfWaitingForDebugger` | NOOP | `domains/runtime.rs:447` |
| `Runtime.getExceptionDetails` | **NOOP — always `{"exceptionDetails": null}`** | `domains/runtime.rs:448` |
| `Runtime.discardConsoleEntries` | NOOP | `domains/runtime.rs:449` |
| **`Page.navigate`** | YES | `domains/page.rs:1285` |
| `Page.reload` | YES | `domains/page.rs:1292` |
| `Page.enable` | YES (replays initial load events, #833) | `domains/page.rs:1244` |
| **`Page.loadEventFired`** (event) | YES, emitted | `domains/page.rs` (event name literal) |
| `Page.domContentEventFired`, `Page.lifecycleEvent`, `Page.frameNavigated`, `Page.frameStoppedLoading`, `Page.frameAttached`, `Page.frameDetached` | YES, emitted | `domains/page.rs`, `dispatch.rs` |
| `Page.getFrameTree` | YES | `domains/page.rs:1302` |
| `Page.createIsolatedWorld` | YES | `domains/page.rs:1313` |
| `Page.addScriptToEvaluateOnNewDocument` / `remove…` | YES | `domains/page.rs:1364`, `:1374` |
| **`Page.getLayoutMetrics`** | **YES** | `domains/page.rs:1386` |
| `Page.getNavigationHistory` / `navigateToHistoryEntry` / `resetNavigationHistory` | YES | `domains/page.rs:1444`, `:1474`, `:1527` |
| `Page.printToPDF` | YES (render feature) | `domains/page.rs:1534` -> `domains/pdf.rs` |
| **`Page.startScreencast` / `stopScreencast` / `screencastFrameAck`** | YES | `domains/page.rs:1535`, `:1569`, `:1580` |
| **`Page.captureScreenshot`** | YES (render feature) | `domains/page.rs:1598` |
| `Page.captureSnapshot` (MHTML) | YES | `domains/page.rs:1725` |
| `Page.setLifecycleEventsEnabled` / `setInterceptFileChooserDialog` / `setDownloadBehavior` | NOOP | `domains/page.rs:1363`, `:1382`, `:1385` |
| **`Page.javascriptDialogOpening` / `handleJavaScriptDialog`** | **NO** (`rg` finds neither string anywhere in the crate) | — |
| **`Input.dispatchMouseEvent`** | YES | `domains/input.rs:113` |
| **`Input.insertText`** | **YES (new since v0.2.1, commit `ce9714f` 2026-09-04)** | `domains/input.rs:329` |
| `Input.dispatchKeyEvent` (`keyDown`/`rawKeyDown`/`keyUp`/`char`) | YES | `domains/input.rs:336-412` |
| `Input.dispatchTouchEvent` | **NOOP** (`Ok(json!({}))`) | `domains/input.rs:413` |
| `Input.setIgnoreInputEvents` | NOOP | `domains/input.rs:414` |
| `Input.dispatchDragEvent` | **NO** | — |
| **`Network.setCookies` / `setCookie` / `getCookies` / `getAllCookies` / `deleteCookies` / `clearBrowserCookies`** | YES | `domains/network.rs:65-96` |
| `Network.getResponseBody` | YES | `domains/network.rs:120` |
| `Network.setExtraHTTPHeaders` / `setUserAgentOverride` / `setBlockedURLs` | YES | `domains/network.rs:45`, `:58`, `:99` |
| `Network.enable` / `setCacheDisabled` / `setRequestInterception` | **NOOP** | `domains/network.rs:34`, `:97`, `:98` |
| `Network.requestWillBeSent` / `responseReceived` / `loadingFinished` (events) | YES, emitted | `domains/page.rs` |
| **`Fetch.enable`** + `requestPaused` interception | **YES** | `domains/fetch.rs:63`; pause/resume plumbing in `server.rs` `process_with_interception` |
| `Fetch.continueRequest` / `fulfillRequest` / `failRequest` | YES | `domains/fetch.rs:110`, `:135`, `:173` |
| `Fetch.takeResponseBodyAsStream` (+ `IO.read`/`IO.close`) | YES | `domains/fetch.rs:191`, `domains/io.rs:142`, `:176` |
| `Fetch.getResponseBody` | **NOOP — always returns `{"body": "", "base64Encoded": false}`** | `domains/fetch.rs:190` |
| **`Emulation.setDeviceMetricsOverride`** | **YES** | `domains/emulation.rs:68` |
| `Emulation.clearDeviceMetricsOverride` / `setDefaultBackgroundColorOverride` | YES | `domains/emulation.rs:109`, `:116` |
| `Emulation.setTouchEmulationEnabled` | NOOP | `domains/emulation.rs:126` |
| **`Target.createTarget`** | YES | `domains/target.rs:65` |
| **`Target.attachToTarget`** | YES | `domains/target.rs:174` |
| **`Target.getTargets`** | YES (**per-connection scope — see blocker (a)**) | `domains/target.rs:47` |
| `Target.setDiscoverTargets` / `getTargetInfo` / `closeTarget` / `detachFromTarget` / `attachToBrowserTarget` | YES | `domains/target.rs:14`, `:295`, `:213`, `:243`, `:147` |
| `Target.createBrowserContext` / `disposeBrowserContext` / `getBrowserContexts` | YES | `domains/target.rs:262`, `:266`, `:257` |
| `Target.setAutoAttach` / `activateTarget` | NOOP | `domains/target.rs:240`, `:256` |
| `Target.sendMessageToTarget` | YES (unwraps + recurses) | `dispatch.rs:725` |
| **`Browser.getVersion`** | YES (constant) | `domains/browser.rs:5` |
| `Browser.close` | YES | `domains/browser.rs:12` |
| `Browser.getWindowForTarget` / `getWindowBounds` | **constant** | `domains/browser.rs:15`, `:26` |
| `Browser.setWindowBounds` / `setDownloadBehavior` / `grantPermissions` / `resetPermissions` | NOOP | `domains/browser.rs:33`, `:25`, `:39` |
| `Storage.getCookies` / `setCookies` / `clearCookies` / `deleteCookies` | YES | `domains/storage.rs:31-47` |
| **`LP.getMarkdown`** (non-standard, obscura-only) | YES | `domains/lp.rs:13` |

**Is `Runtime.evaluate` still expression-only?** Effectively yes, with a wrapper.
`obscura-js/src/runtime.rs:3338+` `fn wrap_expression(expression: &str)` sniffs
`trimmed.starts_with("var ") || "let " || …` to decide `is_multi_statement`. That is a **prefix
heuristic, not a parser**: the spike's `"1; 2"` starts with neither keyword, so it still goes down
the expression path and still raises `SyntaxError`. Aleph's free-form `browser_evaluate` scripts
still need IIFE wrapping on this driver. `Runtime.callFunctionOn` (arrow functions) is unaffected.

### The three blockers at HEAD

**(a) Targets scoped per CDP connection — STILL STANDS, and it is load-bearing.**
`dispatch.rs:51-52`: `pub struct CdpContext { pub pages: Vec<Page>, pub sessions: HashMap<String,
String>, … }`. `server.rs:824-829`: `async fn cdp_processor(...) { let mut ctx =
CdpContext::new_with_shared_context(default_context); … }` — a fresh `CdpContext`, hence a fresh
empty `pages`, **per connection**. The doc comment at `server.rs:817-823` says why:
*"Each connection runs its own processor (with its own `CdpContext` and pages) on its own OS thread,
so every page's V8 isolate is confined to a single thread. This removes the #430 abort by
construction: V8's `heap->isolate() == Isolate::TryGetCurrent()` invariant is per-thread."*
So a second connection seeing no targets is not an oversight — it is the mechanism that keeps V8
from aborting. Fixing it upstream means moving pages off the connection thread, i.e. cross-thread V8
plumbing. Last change to `server.rs`: `33f4830` (2026-09-04), to `target.rs`: `7673591`
(2026-09-01). **Nothing since.**

**(b) V8 serialization — STILL STANDS, and is now explicitly a lock, not just an isolate.**
`dispatch.rs:125`: `pub v8_lock: Arc<tokio::sync::Mutex<()>>` with the comment
*"Serializes V8 work within THIS connection … keeps a connection's own nav task and command dispatch
from interleaving two of its pages' isolates on that one thread. It is deliberately per-connection,
not a process-wide lock, so connections run in parallel (measured ~2x at concurrency 2, ~3x at 4)."*
`dispatch.rs:748-754`: **every** method except an explicit `is_v8_free_method` allowlist takes
that lock before running. The allowlist (`dispatch.rs:653-717`) is broader than the spike implied —
it covers all of `Target.*`, all of `Browser.*`, `Page.{enable,disable,getFrameTree,
setLifecycleEventsEnabled,add/removeScriptToEvaluateOnNewDocument,getNavigationHistory,
resetNavigationHistory,captureSnapshot,stopScreencast,screencastFrameAck,createIsolatedWorld,
setDownloadBehavior,setInterceptFileChooserDialog}`, `Runtime.{enable,disable,
runIfWaitingForDebugger,getExceptionDetails,discardConsoleEntries}`, `Network.{enable,disable}`, all
the cookie methods, `Fetch.{continueRequest,fulfillRequest,failRequest,getResponseBody}`,
`IO.{read,close}`, and `Storage.*`. **What is NOT on it is exactly what a live view and a spatial
snapshot need**: `Runtime.evaluate`, `Runtime.callFunctionOn`, every `DOM.*` method, every `Input.*`
method, `Page.navigate`, `Page.captureScreenshot`, and `Page.startScreencast`. So screencast *frames*
are behind the lock even though the ack that paces them is not. Mitigation
added since v0.2.1: a **per-command V8 watchdog**, `dispatch.rs:766-780`, default budget
`OBSCURA_CDP_COMMAND_TIMEOUT_MS = 60_000` ms, which **terminates the isolate** rather than queueing
forever. That bounds the stall at 60 s by default; it does not remove it, and the recovery is a
killed isolate. Note the direction: a 60 s default is 4x the 16.8 s the spike measured, so the
watchdog would not have fired on github.com.

**(c) `Accessibility.getFullAXTree` has no name-from-content — STILL STANDS, unchanged.**
`domains/accessibility.rs:273-345` `fn compute_name`: `aria-label` -> `aria-labelledby` ->
`alt` -> `title` -> `placeholder` -> (text nodes only) their own trimmed text -> `None`.
There is **no** "if the role supports name-from-content, concatenate descendant text" step, so
`<a href=…>Foo</a>` yields no `name` and the string only exists on the child text node.
`accessibility.rs:131-133` also hardcodes `"ignored": false` for every node (no AX pruning at all).
`git log --since=2026-09-04 -- crates/obscura-cdp/src/domains/accessibility.rs` -> **empty**; last
touch `d17e3dc`, 2026-08-26. A snapshot generator must still compute name-from-content itself.

### Two geometry landmines over CDP (new findings, not in the spike)

1. **`DOMSnapshot.captureSnapshot` returns fabricated boxes.** The module doc
   (`domains/domsnapshot.rs:1-16`) is explicit: *"Obscura has no layout/paint engine, so there is no
   real geometry to report. We synthesize it: every node gets a distinct, on-screen, non-icon-sized
   box (a simple vertical stack) … the coordinates are not real."* The code
   (`domsnapshot.rs:231-236`) is literally `let y = (i as f64) * 18.0; bounds.push(json!([0.0, y,
   1280.0, 18.0]))`, plus a constant style vector claiming `visibility:visible`, `opacity:1`,
   `position:static`, `background-color: rgba(0,0,0,0)` for **every** node
   (`domsnapshot.rs:200-214`). `rg 'render|layout_dom|DomLayout' domains/domsnapshot.rs` finds
   nothing — it does **not** consult the render layer even in a render build. Any agent that reads
   these bounds (browser-use does) is reading a stack of 1280x18 lies with correct-looking shape.
2. **`DOM.getBoxModel` / `DOM.getContentQuads` invent a rect on failure.** `domains/dom.rs:322-327`
   and `:352-357`: when the JS round-trip returns anything unparseable, both return the constant
   quad `[8,8,108,8,108,28,8,28]`, `width:100, height:20` — as a successful response, indistinguishable
   from a measurement. And the measurement path itself is a `page.evaluate(...)` string, so it is on
   the wrong side of blocker (b).

## 4. Direct Rust access to DOM + layout — THE KEY SECTION

**Short answer: yes, the pieces are all `pub` and the join is about 30 lines of glue you write —
but there is no single existing call that returns "tag + attrs + text + rect" per element, and the
retained layout the screenshot path builds is NOT reachable from outside.**

### What is public today

1. **The DOM, without touching V8.** `crates/obscura-browser/src/page.rs:3566`:
   ```rust
   pub fn with_dom<R>(&self, f: impl FnOnce(&DomTree) -> R) -> Option<R> {
       if let Some(js) = &self.js { return js.with_dom(f); }
       self.dom.as_ref().map(f)
   }
   ```
   and `crates/obscura-js/src/runtime.rs:3311`:
   ```rust
   pub fn with_dom<R>(&self, f: impl FnOnce(&DomTree) -> R) -> Option<R> {
       let state = self.state.borrow();          // a RefCell borrow, NOT a V8 lock
       state.dom.as_ref().map(f)
   }
   ```
   Also `Page::dom(&self) -> Option<&DomTree>` (`page.rs:3976`, the non-JS path only) and
   `JsRuntime::dom_ref(&self) -> Option<Ref<'_, Option<DomTree>>>` (`runtime.rs:3329`).
   The DOM is behind a `RefCell`, not behind the isolate. **Reading it enters no V8 scope.**

2. **Layout, as a free function over that DOM.** `crates/obscura-render/src/dom.rs:3207`:
   ```rust
   pub fn layout_dom(tree: &DomTree, viewport: (f32, f32)) -> DomLayout;
   pub fn layout_dom_with_images(tree: &DomTree, viewport: (f32,f32),
                                 intrinsic: &HashMap<NodeId,(f32,f32)>) -> DomLayout;   // :3216
   pub fn layout_dom_with_resources(tree, viewport, intrinsic, fonts: &[Vec<u8>]) -> DomLayout; // :3226
   ```
   `DomLayout` (quoted in full in §2) gives you `rects: HashMap<NodeId, Rect>`,
   `styles: HashMap<NodeId, LayoutStyle>` (that is the computed style — display, visibility,
   opacity, position, transform_ops…), `text_runs: HashMap<NodeId, Vec<(Rect, String)>>` (the actual
   laid-out words with their boxes), `clip_rects`, `translates`, `transforms`, `inline_fragments`,
   `custom_properties`. All fields are `pub`.
   External `<link rel=stylesheet>` CSS is **materialized into the DOM** as
   `<style data-obscura-external-stylesheets>` by `materialize_linked_stylesheet_script`
   (`page.rs:978-1008`), so a `layout_dom` over the live `DomTree` sees the real cascade — you do
   not have to re-fetch CSS.

3. **Everything an element node carries.** `obscura-dom` exports `Node`, `NodeData::Element{name,
   attrs}`, `Attribute`, plus `DomTree::children/descendants/ancestors/get_node/with_node/
   text_content/get_element_by_id/query_selector` (used exactly this way at
   `domains/domsnapshot.rs:139-176` and `domains/accessibility.rs:47-120`).

**So a Rust caller CAN walk the DOM tree + layout tree in one pass after load.** The shape is:

```rust
// mode (B), no CDP, no V8:
let spatial = page.with_dom(|dom| {
    let laid = obscura_render::layout_dom(dom, (1280.0, 800.0));   // one layout pass
    let mut out = Vec::new();
    for nid in std::iter::once(dom.document()).chain(dom.descendants(dom.document())) {
        let Some(node) = dom.get_node(nid) else { continue };
        let rect  = laid.rects.get(&nid);                  // Option<&Rect> — None == no box
        let style = laid.styles.get(&nid);                 // computed display/visibility/opacity
        let text  = laid.text_runs.get(&nid);              // Vec<(Rect, String)> per word
        // node.data -> tag + attrs; dom.text_content(nid) -> text
        out.push(/* your JSON */);
    }
    out
});
```
`rects.get(&nid) == None` is the honest "this element generates no box" answer — `display:none`,
detached, etc. — which is exactly what `DOMSnapshot.captureSnapshot` fakes over CDP.

### What is NOT public, and what it costs

The screenshot path does **not** call `layout_dom` fresh each time. It builds and retains a
`PreparedRender` (`crates/obscura-render/src/paint.rs:830-845`) held in the JS runtime's
`SharedState.prepared_render`, invalidated on navigation/DOM mutation
(`runtime.rs:1051`, `:1079`, `:1283`, `:1393`, `:1419`). `PreparedRender` has the accessors you want
and they are **`pub`** (`paint.rs:986` `pub fn layout(&self) -> &crate::DomLayout`, `:991`
`content_size`, `:995` `viewport_fixed_nodes`, `:999` `sticky_layout`, plus
`viewport_rect_with_scroll`, `client_size`, `viewport_client_rects_with_scroll`, `element_scroll_metrics`
used at `ops.rs:5829-5876`).

**But nothing hands a `&PreparedRender` out of `obscura-js`.** `rg 'pub fn .*prepared' runtime.rs`
yields only derived scalars: `prepared_content_size()`, `prepared_has_active_css_animations()`,
`screenshot_prepared*()`. `Page` mirrors exactly those (`page.rs:3885-3931`). So mode (B) has two
variants:

- **(B1) zero upstream change**: call `obscura_render::layout_dom(dom, viewport)` yourself inside
  `page.with_dom(...)`. Correct and available today. Cost: a **second, independent layout pass**
  that does not reuse the retained cascade — you pay full layout per snapshot, and you can drift
  from what the screenshot shows if the retained sample advanced.
- **(B2) ~3 lines upstream**: add `pub fn with_prepared_render<R>(&self, f: impl FnOnce(&PreparedRender) -> R) -> Option<R>`
  to `JsRuntime` and forward it from `Page`. That reuses the exact layout the screenshot was painted
  from, so the JSON tree and the pixels agree by construction. This is a genuinely small upstream PR
  against a repo that merges community fixes daily (36 merges in the last 24 h of history).

### Where the screenshot path gets its geometry

`Page::screenshot_with_animation_sample` (`page.rs:3782-3806`) -> `js.screenshot_prepared_with_surface_color`
-> `obscura_render::screenshot_prepared_*` over the retained `PreparedRender`, i.e. **the same
`DomLayout`**. `obscura_render::screenshot_png(tree: &DomTree, viewport, base_url) -> Option<Vec<u8>>`
(`paint.rs:6287`) is the stateless one-shot. Confirmed: pixels and `DomLayout` rects come from one
source of truth, so a spatial tree built from `DomLayout` is consistent with the screenshot.

### Does obscura already emit an agent-oriented page representation?

Yes — three, none of them spatial:

| thing | where | shape | geometry? |
|---|---|---|---|
| `LP.getMarkdown` (non-standard CDP domain) | `crates/obscura-cdp/src/domains/lp.rs:13` | HTML -> Markdown, JS-side (`obscura-js/src/markdown.rs`, `HTML_TO_MARKDOWN_JS`) | no |
| MCP `browser_snapshot` | `crates/obscura-mcp/src/lib.rs:952` `fn tool_snapshot`; described as *"Get the current page content as text (title, URL, and readable body text)"* (`:333`), capped by `DEFAULT_TEXT_LIMIT = 4000` (`lib.rs:26`) | text dump | no |
| MCP `browser_interactive_elements` | `crates/obscura-mcp/src/lib.rs:1264` + `rebuild_interactive_refs` `:1307` | tags every interactable with `data-obscura-ref="eN"` **via `page.evaluate`**, then lists `ref / tag[type] / label / name / role` as plain lines | **no** — no x/y/w/h at all |

`rg -il 'aria.?snapshot|readability'` finds no aria-snapshot and no readability extractor.
The MCP tool set is 36 tools (`browser_navigate/click/fill/type/press_key/scroll/select_option/
screenshot/pdf/markdown/extract/links/count/search/detect_forms/fill_form/get_attribute/
wait_for/wait_for_text/evaluate/console_messages/network_requests/cookies/storage_state/
tab_new/tab_list/tab_switch/tab_close/back/forward/reload/close/snapshot/interactive_elements`).
Note `#![recursion_limit = "512"]` at `obscura-mcp/src/lib.rs:1-4` because the `tools/list` JSON is
that big.

**Nothing in the repo produces a "spatial state JSON tree". Aleph would be writing that itself —
which is the good news, because the raw material (`DomLayout`) is better than what CDP exposes.**

## 5. Embedding vs process

### (A) Spawn the binary, speak CDP over WebSocket

CLI (`crates/obscura-cli/src/main.rs:10-198`) — note the flags are **obscura's own, not Chromium's**:

```
obscura serve --port 9222 --host 127.0.0.1 --workers 1 \
              --max-connections <DEFAULT_MAX_CONNECTIONS> \
              --allow-file-access --storage-dir <DIR> --quiet
```
Global flags (`main.rs:17-58`): `-v/--verbose`, `--proxy <url>` (http/https/socks5, with auth),
`--stealth`, `--obey-robots`, `--user-agent`, `--storage-dir`, `--allow-private-network`,
`--v8-flags "<raw V8 flags>"`. Subcommands: `serve`, `fetch`, `scrape`, `mcp`.

- **No `--remote-debugging-port`** (it is `--port`, default 9222) and **no `--headless`** (it is
  always headless).
- **No `--user-data-dir`**; the equivalent is `--storage-dir <DIR>`
  (`obscura-browser/src/context.rs:26,41`: *"When `storage_dir` is set, cookies are automatically
  loaded from…"*). Cookie persistence is real, and cookie deltas from each connection thread are
  merged back into the persistence template on thread exit (`server.rs:571`).
- **Endpoint advertisement**: an HTTP control plane on the same port, `server.rs:736-740` —
  `/json/version`, `/json` / `/json/list`, `/json/protocol`. `server.rs:791` returns
  `"webSocketDebuggerUrl": "ws://127.0.0.1:{port}/devtools/browser"` and `:800`
  `"ws://127.0.0.1:{port}/devtools/page/page-1"`. **There is no `DevToolsActivePort` file.** The CLI
  also prints `CDP server: ws://127.0.0.1:{port}/devtools/browser` on stdout at `main.rs:226-236`.
  Note the URLs are **hardcoded to `127.0.0.1`** regardless of `--host` — a `--host 0.0.0.0` server
  advertises a loopback URL.
- Bind default is loopback; `--allow-file-access` is off by default so a CDP client cannot read
  local files; `--allow-private-network` is off (the spike's `127.0.0.1` block).
- Docker image exists (`Dockerfile`), and the README says official archives and the Docker image
  include rendering.

### (B) Link the crates into Aleph (or an Aleph-owned sidecar crate)

**What Aleph would depend on.** Minimum for DOM + layout: `obscura-browser` (which pulls
`obscura-dom`, `obscura-net`, `obscura-js`) with `features = ["render"]`, plus `obscura-render`
directly if you call `layout_dom` yourself. The friendly facade `obscura` re-exports
`Browser`/`Page`/`Element`/`CookieStore` but **not** `with_dom`
(`crates/obscura/src/page.rs:26-207`: `goto, url, evaluate, frame_urls, evaluate_in_frame, content,
query_selector, wait_for_selector, settle, add_preload_script, enable_interception, on_request,
on_response, off_request, off_response` and `Element::{text, attribute, click}`) — so for the spatial
tree you would depend on **`obscura-browser` directly**, not `obscura`.

**V8 build implications.** `deno_core 0.350` -> `v8 137.3.0` from crates.io (`Cargo.lock:3787`).
That is `rusty_v8`, whose default is a **prebuilt static V8 download** (source builds are opt-in).
`docs/Build-from-source.md:4-7`: *"~5 GB free disk space (V8 compiles from source on first build) …
First build takes about 5 minutes. Incremental builds are seconds."* CI budgets 75-90 minutes for
the heavy jobs (`.github/workflows/ci.yml:118`, `:203`) with a `Swatinem/rust-cache`.
`docs/Use-as-a-Rust-library.md` warns *"The first build compiles V8 from source, so it is slow and
needs the same build tools as Build from source"* and requires cmake/clang/libclang.
468 packages in `Cargo.lock`. Also note `obscura-js` embeds a **696 KB `bootstrap.js`**
(`crates/obscura-js/js/bootstrap.js`) — the entire Web API shim layer, not counted in the Rust LOC.

**Binary size** — from the prior spike, not re-measured here: v0.2.1 aarch64-macos `obscura` = 94 MB,
`obscura-worker` = 86 MB. Linking obscura into `aleph-server` puts a V8 of that order into Aleph's
own binary. This collides head-on with Aleph redline R3 ("核心轻量化"), which the 2026-09-01 ruling
about "跑别人 agent 的运行时" softens but does not delete — the "why can't this be a Skill/MCP"
answer still has to be given per lump.

**Is the public surface stable?** No promise anywhere. `rg -i 'semver|stab' AGENTS.md CONTRIBUTING.md`
finds only `CONTRIBUTING.md:216` ("do not regress performance or stability", about perf).
Workspace `version = "0.1.0"` while releases are tagged `v0.2.1` — the crate versions do not move
with releases at all, so a git dependency has **no version signal**: pin a commit SHA.
`AGENTS.md:94` is the only statement about library use:
*"**obscura** — embeddable Rust library API (git dependency; builds V8 locally, not on crates.io).
Public request-interception API on `Page`: `add_preload_script`, `enable_interception` …"* — it
scopes the "public API" to interception, and says nothing about DOM or layout being stable.
`docs/Use-as-a-Rust-library.md` documents only the facade crate's ~15 methods.
**Conclusion: `DomLayout`, `layout_dom`, `Page::with_dom` are `pub` but undocumented and
uncontracted.** They can change in any merge. Pin a SHA and own a compile-time smoke test.

**One `Page` is not `Send`.** `obscura-js`'s runtime state is `Rc<RefCell<…>>` (`runtime.rs:3311`,
`:3329`) and the whole CDP server is built around thread-per-connection with `LocalSet`
(`server.rs:499-505`). Whatever Aleph does in mode (B) must pin each page to one thread and reach it
by channel — the same shape obscura's own server uses. This is not a blocker, it is a design
constraint you inherit.

## 6. Rendering fidelity + stealth

**`render-repros/` is a Chromium *parity harness*, not a bug list.** 64 `.html` fixtures + a runner.
`render-repros/run.sh` renders every fixture in obscura (`obscura fetch file://… --screenshot`) and
in real Chromium (`capture_chromium.py`) side by side, then `check.py` asserts against
`checks.json`, which contains **explicit per-fixture geometry**, e.g.
`{"name": "nearest positioned ancestor", "color": "#087f5b", "x": 85, "y": 75, "width": 80,
"height": 60}`. Coverage spans absolute/fixed containing blocks, floats/BFC, flex, grid, tables,
inline/replaced flow, transforms, z-index, RTL, sticky, line-height/font metrics, `long-full-page-
capture`, PDF print media. `spot-sites.txt` is a real-world set: example.com, HN, sqlite.org,
blog.rust-lang.org, docs.python.org, web.dev, jvns.ca, Wikipedia.
**This is the strongest single piece of evidence that the layout geometry is trustworthy** — the
project pins box coordinates against Chromium in CI.

**No known-incompatible-sites list and no "fall back to Chromium" recommendation** anywhere.
`rg -i 'not supported|limitation|fall back|fallback|unsupported'` over `README.md` returns the
banner *"Native rendering is here. No Chromium required."* (`README.md:19`) and one honest paragraph
(`README.md:288-293`): *"It remains an evolving independent engine: long-tail CSS, some Web APIs,
media playback, compositor effects, and platform font rasterization may differ from Chromium."*
The spike's own measurement stands as the counterweight: HN and Wikipedia near-Chrome, **GitHub
visibly wrong** (double-painted nav labels, phantom tooltip, empty commit column, README not
rendered).

**"Stealthy" concretely** (`docs/Configure-stealth-and-proxies.md`, `README.md:390-400`,
`skills/obscura/SKILL.md`):
- **TLS**: `wreq` + BoringSSL emulating `Profile::Chrome145` — ClientHello, ALPN, cipher order.
- **UA/identity**: `obscura-net/src/wreq_client.rs:61-73` — `STEALTH_USER_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) … Chrome/145.0.0.0 Safari/537.36"`,
  `STEALTH_NAVIGATOR_PLATFORM = "Win32"`, `STEALTH_UA_PLATFORM = "Windows"`, with an explicit
  comment that navigator must agree with the wire or the mismatch is itself a bot signal.
- **`navigator.webdriver`**: `crates/obscura-js/js/bootstrap.js:6921`
  `defGetter('webdriver', function() { return false; });` on the prototype (`:6813`).
- Per-session fingerprint randomization (GPU, screen, canvas, audio, battery),
  `navigator.userAgentData` high-entropy values, `event.isTrusted = true`, masked patched natives,
  tracker-domain blocklist, bundled webpki roots.
- **Non-stealth builds leak**: `Browser.getVersion` is a **constant** (`domains/browser.rs:5-11`)
  reporting `Chrome/145.0.0.0` with a **Linux X11** UA — matching the spike, and inconsistent with
  the stealth build's Windows identity.

**Captcha**: explicitly out of scope. `docs/Configure-stealth-and-proxies.md` — *"What stealth does
not handle: Cloudflare interactive challenges. Datadome and Akamai bot manager active challenges.
CAPTCHAs. IP-based rate limiting (use proxies)."*

## 7. Screencast / screenshot

- **Requires the `render` feature**; without it `Page.startScreencast/stopScreencast/
  screencastFrameAck/captureScreenshot` all return `Err("… requires a build with the render
  feature")` (`domains/page.rs:1565-1596`).
- **Params honoured** (`domains/page.rs:579-619` `parse_screencast_state`): `format` (`png` |
  `jpeg`, default **png**; anything else is a hard error), `quality` (0-100, out-of-range silently
  falls back to the default), `maxWidth`, `maxHeight` (positive ints, else ignored), `everyNthFrame`
  (must be > 0). Screenshot format additionally accepts `webp` (`domains/page.rs:209-211`).
- **fps behaviour is activity-driven, not a fixed rate.** `startScreencast` returns a non-standard
  ack `{"obscuraFrameSource": "activity-driven", "obscuraAutonomousFrames": true}`
  (`domains/page.rs:1560-1563`) and logs `"started activity-driven screencast"`. The pump is a 33 ms
  `tokio::time::interval` with `MissedTickBehavior::Skip` on the connection's own LocalSet
  (`server.rs:851-853`), described as *"Obscura has no separate compositor thread yet, so active
  screencasts get a bounded 30 Hz opportunity"*. Frames are also queued after any command that could
  change the frame (`dispatch.rs:818-826`, `command_can_change_screencast_frame`).
  This matches the spike exactly: **~31 fps under motion, 0 frames on a static page.**
- Backpressure is real: `frames_in_flight` decremented by `Page.screencastFrameAck`
  (`domains/page.rs:1583-1594`), ignoring acks from a replaced stream.

## 8. Skills / docs for agents

- **`skills/obscura/SKILL.md`** — a single Claude-style skill file. Its own description covers
  "JavaScript page loading, stealth browsing, anti-fingerprinting, tracker blocking, screenshots and
  visual comparison, CDP automation with Puppeteer or Playwright, screencasting, PDF export, MCP
  browser interaction, and web extraction". Contains the exact build lines
  (`cargo build --release -p obscura-cli --bins --features render[,stealth]`, with
  `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2`).
- **MCP server**: `obscura mcp` (stdio) or `obscura mcp --http --host --port` (`main.rs:181-198`),
  36 tools, documented in `docs/Use-the-MCP-server.md`.
- **Playwright / Puppeteer drop-in**: `docs/Use-with-Playwright.md`, `docs/Use-with-Puppeteer.md`,
  `docs/Connect-Puppeteer-or-Playwright.md`. The pattern is `chromium.connectOverCDP(...)`
  (`README.md:353-356`) — connect, not launch.
- **`docs/Watch-agent-sessions-live.md` + `tools/live-view.mjs`** — directly relevant to Aleph's live
  view. The official recipe **polls `Page.captureScreenshot` twice a second on the same connection**
  and forwards JPEG over Server-Sent Events. It notably does **not** use `Page.startScreencast`, and
  does **not** open a second CDP connection — consistent with blocker (a) being a known shape.
- Other docs: `Architecture-overview`, `Adding-a-CDP-method-or-Web-API`, `Extract-data`,
  `Markdown-extraction`, `Intercept-and-modify-requests`, `Persist-cookies-and-storage`,
  `Run-in-production-at-scale`, `Environment-variables`, `CLI-reference`, `Testing-and-debugging`.

## 9. Release artifacts

`.github/workflows/release.yml`. **Five platforms**: `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`
(`release.yml:15-42`). **Four variants per platform** (`release.yml:79-127`, packaged at `:164-182`):

| archive | build |
|---|---|
| `obscura-<plat>.tar.gz` (**the default download**) | `--features render` |
| `obscura-<plat>-stealth.tar.gz` | `--features render,stealth` |
| `obscura-<plat>-no-render.tar.gz` | `--no-default-features` |
| `obscura-<plat>-no-render-stealth.tar.gz` | `--no-default-features --features stealth` |

Each archive holds two binaries: `obscura` and `obscura-worker`. **So the default release asset DOES
include the layout+paint engine** — the README confirms: *"Official release archives and the Docker
image include the rendering engine"* (`README.md:272-273`). A `no-render` archive would silently give
you the fake-geometry world of §3.
Sizes are not in the repo; the prior spike measured v0.2.1 aarch64-macos at **94 MB + 86 MB**.

**Version at HEAD**: latest tag `v0.2.1`; workspace `version = "0.1.0"`. The CLI's version string is
`env!("OBSCURA_BUILD_VERSION")`, computed in `crates/obscura-cli/build.rs` as
`OBSCURA_VERSION` -> the GitHub tag ref -> `CARGO_PKG_VERSION`. **So a locally built binary reports
`0.1.0` while a released one reports the tag.** Don't use `obscura --version` to identify a source
build.

**Install scripts**: there is **no install.sh**. `scripts/` holds only CI helpers
(`scripts/ci/pr_policy.py`, `compare_obstacle.py`, `perf_smoke.py`). `README.md:130-145` documents
plain `curl -LO …/releases/latest/download/obscura-<plat>.tar.gz && tar xzf`. `tools/` holds only
`live-view.mjs`. Docker via the root `Dockerfile`.

### One documentation inconsistency worth knowing

`docs/Build-from-source.md:100` says *"the engine owns a single V8 isolate per process"*, while
`crates/obscura-cdp/src/dispatch.rs:731-733` says *"a per-Page `JsRuntime` (each owning its own V8
Isolate)"* and commit `6f4e6b5` (2026-09-04) is literally *"fix(js): support independent isolate
teardown"* with `crates/obscura/tests/concurrent_isolate_teardown.rs`. **The code is authoritative:
one isolate per page, all pages of one connection pinned to one OS thread and serialized by one
mutex.** The doc line is stale. This matters because "one isolate per process" would make blocker (b)
unfixable, and it isn't — the real constraint is the per-connection thread + `v8_lock`.

---

## Verdict for the architect

1. **HEAD is the same commit the spike measured** (`72c84ad`, 2026-09-04) and the clone has never
   been fetched (`.git/refs/remotes/origin/HEAD` written at clone time, no `FETCH_HEAD`). Every
   "changed since?" answer below is therefore *unchanged in this clone*; upstream merges ~30 PRs a
   day, so re-fetch before acting on anything time-sensitive.
2. **Blocker (a), targets scoped per connection: STILL STANDS, and it is structural.**
   `CdpContext { pages: Vec<Page> }` is constructed fresh per connection thread
   (`dispatch.rs:51-52`, `server.rs:824-829`) precisely so V8's per-thread isolate invariant holds
   (`server.rs:817-823`). This is not a bug someone will fix; undoing it means cross-thread V8.
   **Plan on exactly one Aleph-owned CDP connection serving both tools and the viewer.**
3. **Blocker (b), V8 serialization: STILL STANDS, now with a 60 s guillotine.** Every method
   outside the `is_v8_free_method` allowlist takes `ctx.v8_lock` (`dispatch.rs:653-717`, `:748-754`).
   The allowlist is broad (all `Target.*` / `Browser.*`, `Page.enable/getFrameTree/stopScreencast/
   screencastFrameAck`, cookies, `Fetch.*`, `IO.*`) but **excludes precisely what a viewer and a
   spatial snapshot need**: `Runtime.evaluate`, `Runtime.callFunctionOn`, every `DOM.*`, every
   `Input.*`, `Page.navigate`, `Page.captureScreenshot`, `Page.startScreencast`. So screencast frames
   stall with the page even though the ack pacing them does not. A per-command watchdog terminates
   the isolate at `OBSCURA_CDP_COMMAND_TIMEOUT_MS` (default 60 000 ms, `dispatch.rs:766-780`) — the
   spike's 16.8 s stall would not trip that default. Set it low yourself, and expect the recovery to
   be a **killed isolate**, not a queued command.
4. **Blocker (c), no name-from-content in the AX tree: STILL STANDS, unchanged since 2026-08-26.**
   `accessibility.rs:273-345` stops at aria-label / aria-labelledby / alt / title / placeholder, and
   `ignored` is hardcoded `false` (`:131-133`). Any snapshot generator computes names itself.
5. **New, and worse than the three: two CDP geometry paths return fabricated numbers.**
   `DOMSnapshot.captureSnapshot` synthesizes a 1280x18 vertical stack for every node and constant
   "visible/opaque/static" styles (`domsnapshot.rs:1-16`, `:200-236`) and never consults the render
   layer. `DOM.getBoxModel`/`getContentQuads` fall back to a constant quad
   `[8,8,108,8,108,28,8,28]`, `100x20`, on any failure (`dom.rs:322-327`, `:352-357`) — returned as
   success. **A wrong rect reads like a fact.** If Aleph builds spatial state over CDP, it inherits
   both.
6. **Mode (B), direct-Rust DOM + layout: YES, with ~30 lines of glue you write, no upstream change.**
   The exact calls:
   - `obscura_browser::Page::with_dom<R>(&self, f: impl FnOnce(&DomTree) -> R) -> Option<R>`
     (`page.rs:3566`) — a `RefCell` borrow, **enters no V8 scope**.
   - `obscura_render::layout_dom(tree: &DomTree, viewport: (f32, f32)) -> DomLayout`
     (`render/dom.rs:3207`; `_with_images` `:3216`, `_with_resources` `:3226`).
   - Read `DomLayout.rects: HashMap<NodeId, Rect>` (`Rect {x,y,width,height}: f32`,
     `render/lib.rs:383`), `.styles: HashMap<NodeId, LayoutStyle>`,
     `.text_runs: HashMap<NodeId, Vec<(Rect, String)>>`, `.clip_rects`, `.transforms`.
   - Tag/attrs/text from `obscura_dom::{Node, NodeData::Element{name, attrs}, DomTree::children /
     descendants / text_content}`.
   - `NodeId.index()` **is** the CDP `backendNodeId` (`domsnapshot.rs:16`), so a Rust-built tree and
     CDP commands address the same nodes.
   - `rects.get(&nid) == None` is the honest "generates no box" — the very thing CDP fakes.
   Requires the `render` feature (which the default release archive already has) and external CSS is
   already materialized into the DOM (`page.rs:978-1008`), so no CSS re-fetch.
7. **Mode (B) variant worth the 3-line upstream PR.** `PreparedRender` — the retained layout the
   screenshot is painted from — exposes `pub fn layout(&self) -> &DomLayout` (`paint.rs:986`) plus
   `content_size`, `viewport_fixed_nodes`, `viewport_rect_with_scroll`, `client_size`. But **nothing
   hands it out of `obscura-js`**; `Page` exposes only derived scalars (`page.rs:3885-3931`). Adding
   `pub fn with_prepared_render<R>(&self, f: impl FnOnce(&PreparedRender) -> R) -> Option<R>` makes
   the JSON tree and the pixels agree by construction and avoids a second full layout pass. Without
   it, (B1) still works and is correct, just recomputed.
8. **The honest minimum CDP surface Aleph could rely on** (all verified implemented, one connection):
   `Target.createTarget / attachToTarget / getTargets`, `Page.enable / navigate / reload /
   getFrameTree / getLayoutMetrics / captureScreenshot / startScreencast / stopScreencast /
   screencastFrameAck` + the `loadEventFired` / `lifecycleEvent` / `frameNavigated` events,
   `Runtime.evaluate` (IIFE-wrap free-form scripts) / `callFunctionOn` / `addBinding`,
   `DOM.getDocument / querySelector / querySelectorAll / getOuterHTML / describeNode`,
   `Input.dispatchMouseEvent / dispatchKeyEvent / insertText`,
   `Network.{get,set,delete}Cookies` + `Storage.*Cookies`, `Fetch.enable / continueRequest /
   fulfillRequest / failRequest`, `Emulation.setDeviceMetricsOverride`, `Browser.getVersion`.
   **Do not rely on**: `DOMSnapshot.captureSnapshot` (fake), `DOM.getBoxModel` /`getContentQuads`
   (V8-bound + fabricated fallback), `DOM.getNodeForLocation` (absent), `Page.javascriptDialogOpening`
   / `handleJavaScriptDialog` (absent — **no dialog handling at all**), `Input.dispatchTouchEvent`
   and drag (no-op / absent), `Fetch.getResponseBody` (always empty string),
   `Runtime.getExceptionDetails` (always null), `DOM.setAttributeValue` / `removeNode` (no-ops that
   report success).
9. **License and packaging are clean.** Apache-2.0 throughout, no `publish = false` anywhere, every
   crate is a normal `lib`, 5 platforms x 4 feature variants published per tag, plus a Dockerfile.
   The *default* release archive includes render; a `no-render` archive would put you in the
   fabricated-geometry world without warning.
10. **Risk 1 — the API you would depend on is `pub` but uncontracted.** `AGENTS.md:94` scopes the
    "public library API" to interception; `docs/Use-as-a-Rust-library.md` documents ~15 facade
    methods and never mentions DOM or layout. Workspace version is frozen at `0.1.0` while releases
    tag `v0.2.1`, so a git dependency carries no version signal. Pin a commit SHA, and own a
    compile-guard test that fails loudly when `DomLayout`'s fields or `with_dom`'s signature move.
11. **Risk 2 — weight, and it lands on redline R3.** `deno_core 0.350` + `v8 137.3.0`, 468 locked
    packages, a 696 KB embedded `bootstrap.js`, ~5 GB disk and ~5 min for a first build, and a
    ~94 MB binary in the prior measurement. Mode (B) puts all of that inside `aleph-server` unless
    you isolate it in an Aleph-owned sidecar process — which I would recommend, because it also
    solves the `!Send` thread-pinning constraint (`Page` holds `Rc<RefCell<…>>`, `runtime.rs:3311`).
12. **Risk 3 — rendering fidelity is good where it is tested and unknown where it is not.**
    `render-repros/` pins box coordinates against real Chromium across 64 fixtures with explicit
    x/y/w/h assertions (`checks.json`), which is genuinely strong evidence for layout. But there is
    **no incompatible-sites list and no fall-back-to-Chromium guidance anywhere in the docs**, and
    the spike already measured GitHub rendering visibly wrong. The escape hatch has to be Aleph's own
    decision procedure; obscura will not tell you when it is wrong. Corollary: keep the
    engine-agnostic interface honest about *which* engine produced a given spatial tree, and never
    let a rect reach the model without a provenance that says which engine measured it.

### Things I looked for and did not find

- `DOM.getNodeForLocation` — `rg '"getNodeForLocation"'` and the `dom.rs` match arms: **not found**.
- `Page.javascriptDialogOpening` / `Page.handleJavaScriptDialog` — `rg` across `crates/obscura-cdp`:
  **not found** (confirms the spike).
- `Input.dispatchDragEvent` — **not found**.
- An aria-snapshot / readability / "interactive elements with geometry" representation —
  `rg -il 'aria.?snapshot|readability'` over `docs/ skills/ crates/*/src/`: **not found**.
  The nearest thing, MCP `browser_interactive_elements` (`obscura-mcp/src/lib.rs:1264`), emits
  ref/tag/role/label lines with **no coordinates**, computed by `page.evaluate`.
- Any `pub` accessor returning `&PreparedRender` or `&DomLayout` from `obscura-js` / `obscura-browser`
  — `rg 'pub fn .*prepared' crates/obscura-js/src/runtime.rs`: **not found** (only derived scalars).
- Binary sizes at HEAD — `target/` is absent and no build was run; sizes quoted are the prior
  spike's v0.2.1 measurement.
- Whether upstream has moved past `72c84ad` — a `git fetch` would modify the clone, which the brief
  forbids. **Unknown.**
