# Survey: browser-use/desktop (+ vendored Browser Harness JS)

Clone: `/Volumes/TBU4/Github/desktop` (read-only, not modified). Repo:
`browser-use-desktop` (package.json name), productName "Browser Use", MIT
license, `github.com/browser-use/desktop`.

**Browser Harness availability**: NOT a separate npm dependency — it is
**vendored source**, sitting at
`app/src/main/hl/stock/browser-harness-js/` (confirmed via `rg -n
"browser-harness" package.json app/package.json app/src shared` — no
`node_modules/@browser-use/*` or `node_modules/browser-harness` entry
exists; it's copied into the repo, not installed). This is the actual
"Browser Harness" the task asked about. Its own doc identifies the
upstream org as `browser-use/browser-harness-js`.

## 0. Headline architectural finding (read this before the rest)

`browser-use/desktop` is **not** the classic browser-use Python agent
(no `buildDomTree`, no `highlightIndex`, no clickable-element JSON
schema exists anywhere in this repo — confirmed by exhaustive `rg` for
`buildDomTree|DOMTree|clickable|highlightIndex|isTopElement|
SerializedDOM|EnhancedDOM|isVisible\(|paint order|set-of-marks` across
the whole tree; the only hits are the *codegen'd CDP method* named
`captureScreenshot` and unrelated UI/markdown text). Desktop is a
**host app that hands a live CDP session to a general-purpose coding
agent** (Claude Code, Codex, or "browsercode") running in a terminal
harness, and lets *that* agent write ad-hoc JavaScript against the raw
DOM/Runtime/Input CDP domains itself, call by call. There is no fixed
"DOM → structured-JSON-for-the-model" serialization step anywhere in
this codebase — page understanding is delegated entirely to the coding
agent's own judgment (via `Runtime.evaluate` snippets it writes) plus
ad-hoc screenshots it takes and looks at with vision. This is a
materially different design point than browser-use's Python library and
should be treated as such by the architect — see closing section.

## 1. Layout

- `app/src/main/` — Electron **main process**: window/tray/pill/popup
  chrome, IPC handlers, identity/auth, chrome-cookie-import, the coding
  "harness" launcher (`hl/`), session/browser-pool management
  (`sessions/`), channel adapters (`channels/`, e.g. WhatsApp).
- `app/src/main/hl/` — "Harness Loop"(?) subsystem: spawns/manages the
  coding-agent CLI (Claude Code / Codex / a custom "browsercode" engine)
  as a child process per session, wires env vars (`BU_TARGET_ID`,
  `BU_CDP_PORT`, `BU_OUTPUTS_DIR`, etc.), and stages the vendored
  `stock/` runtime (browser-harness-js CLI + skill docs) into each
  session's working dir.
  - `hl/engines/{claude-code,codex,browsercode}/adapter.ts` — one
    adapter per supported coding-agent CLI; each injects the
    "Use the `browser-harness-js` CLI…" instruction into that agent's
    system/tool prompt (e.g. `hl/engines/claude-code/adapter.ts:129`).
  - `hl/engines/cliSpawn.ts`, `runEngine.ts`, `registry.ts`,
    `installer.ts`, `pathEnrich.ts` — process spawn plumbing, CLI
    binary discovery/install, PATH injection so the vendored
    `browser-harness-js` binary is reachable from inside the spawned
    agent's shell.
  - `hl/harness.ts` — stages the **read-only** `stock/` tree
    (`AGENTS.md`, `browser-harness-js/`, `interaction-skills/`,
    `domain-skills/`) into each session dir every launch (files are
    "overwritten on app launch" per `stock/AGENTS.md:119`); computes
    `browserHarnessJsDir()` (harness.ts:64); vendored via
    `import.meta.glob('./stock/browser-harness-js/**/*', …)`
    (harness.ts:43).
  - `hl/cdp.ts` — main-process-side CDP helper (separate from the
    agent-facing SDK in `stock/browser-harness-js/sdk/`).
  - `hl/context.ts`, `hl/engine.ts` — small per-session context/engine
    glue (77 and 14 lines respectively).
  - `hl/pricing.ts`, `hl/streamToTerm.ts` — token/cost accounting and
    piping the agent CLI's stdout to the in-app terminal (xterm) view.
- `app/src/main/hl/stock/browser-harness-js/` — the **vendored Browser
  Harness JS runtime** itself (the reference tool the task named):
  - `sdk/generated.ts` (~655 KB) — codegen'd CDP SDK: every method from
    `browser_protocol.json` + `js_protocol.json` gets a typed wrapper
    class (56 domains, 652 methods total, per its own SKILL.md).
  - `sdk/repl.ts` — a `Bun.serve` HTTP server on `127.0.0.1:9876`
    holding one persistent `Session`; the CLI just POSTs JS snippets to
    it and prints the raw result.
  - `sdk/session.ts` — the `Session` class: transport (WS), `connect()`
    auto-detection, `use(targetId)` target routing, event
    subscribe/`waitFor`.
  - `sdk/gen.ts` — the codegen script that turns the two upstream
    protocol JSONs into `generated.ts`.
  - `SKILL.md` — the actual operating manual injected/available to the
    coding agent (see §3 below for the page-representation implications).
- `app/domain-skills/`, `app/interaction-skills/` — markdown "recipe"
  libraries (see §3/§5) pulled from a sibling repo `browser-use/
  harnessless` (domain skills) and `browser-use/browser-harness-js`
  (interaction skills), staged read-only into each session.
- `app/src/preload/` — thin Electron preload bridges per window
  (`shell.ts`, `popup.ts`, `pill.ts`, `onboarding.ts`, `logs.ts`) —
  contextBridge exposure only, matches Electron's standard main/renderer
  isolation split.
- `app/src/renderer/` — React UI (chat hub, onboarding, pill/popup
  windows); not yet explored in depth (out of scope — task is about the
  agent/DOM pipeline, not the chat UI chrome).
- `app/src/shared/` — types/schemas shared between main and renderer
  (`session-schemas.ts`, `types.ts`, `attachments.ts`, `hotkeys.ts`).
- `shared/schemas/` (repo-root, outside `app/`) — cross-cutting schema
  definitions shared with `docker/`/CI tooling, not yet inspected.
- **Electron main vs renderer split**: standard — all CDP driving,
  process spawning, session DB, and coding-agent lifecycle live in
  `app/src/main/`; the renderer is chat/UI only and talks to main via
  IPC (`app/src/main/channels/ipc.ts`, `consentIpc.ts`, `themeIpc.ts`,
  `telemetryIpc.ts`, `rendererLogIpc.ts`). No DOM-serialization or agent
  logic in the renderer.
- **Where the agent loop actually lives**: it does **not** live in this
  Electron app at all in the classic sense — the "agent loop" is
  whichever external coding-agent CLI (Claude Code / Codex / browsercode)
  is spawned as a child process per session; Desktop's job is orchestration
  (spawn, env wiring, terminal streaming, session persistence) not
  reasoning. This is the single biggest structural difference from a
  typical browser-use-style agent loop and is why there's no DOM-serializer
  in this repo — that reasoning is entirely delegated to whatever coding
  agent is plugged in.

## 2. Browser lifecycle

**The "browser" being driven is not a separate launched process — it is
Electron's own embedded Chromium.** There is no `puppeteer.launch()` /
`playwright.chromium.launch()` anywhere. Each session's "browser view" is
an Electron `WebContentsView` created by `BrowserPool.create()`
(`app/src/main/sessions/BrowserPool.ts:134`), sandboxed
(`contextIsolation: true, nodeIntegration: false, sandbox: true`), sized
1280×800 by default, and composited as a child view inside the app's own
window (`contentView.addChildView`). Viewport sizing is a single
`setZoomFactor` knob (no `enableDeviceEmulation`) so the page always sees
a fixed ~900 CSS-px-tall viewport regardless of the pane's actual pixel
height (`BrowserPool.ts:16-22, 579-590` — comment explains they tried
device emulation + zoom together and got asymmetric gutters).

- **CDP endpoint discovery — the whole app IS the CDP target.** Before
  `app.whenReady()`, main calls
  `app.commandLine.appendSwitch('remote-debugging-port', String(resolvedCdp.port))`
  (`app/src/main/index.ts:186`) — i.e. Electron's underlying Chromium
  exposes a normal CDP HTTP/WS endpoint for the *entire app*, port chosen
  by `resolveCdpPort(argv)` (`startup/cli.ts`, precedence: CLI flag >
  other source, documented at `cli.ts:76-119`). After ready, main verifies
  the port is really *its own* Chromium and not some other process that
  happened to already own it, by fetching `/json/version` and diffing the
  User-Agent against the app's own spoofed identity
  (`verifyCdpOwnership`, called at `index.ts:448`) — logs a loud
  `main.cdp.verifyFailed` if e.g. the user's real Chrome squatted the port.
- **Per-session target resolution.** When a coding-agent CLI is spawned
  for a session, main briefly attaches Electron's `webContents.debugger`
  to that session's `WebContentsView`, calls `Target.getTargetInfo` to
  read its CDP `targetId`, then detaches
  (`resolveTargetIdForWebContents`, `hl/engines/runEngine.ts:30-46`). That
  `targetId` + the app-wide CDP port are handed to the spawned agent as
  `BU_TARGET_ID` / `BU_CDP_PORT` env vars (set per-adapter, e.g.
  `hl/engines/claude-code/adapter.ts:172-173`). The agent's own
  `browser-harness-js` then does its **own independent** WebSocket
  connection to that same port/target — main's attach was just a
  read-only probe, it detaches before the agent ever connects, so there's
  no dueling-debugger-session conflict.
- **"Ports your cookies into a fresh Chromium" — this is a one-shot,
  separate, throwaway Chromium, unrelated to the driven browser.**
  `chrome-import/cookies.ts` implements the whole flow: copy the user's
  *real* Chrome/Brave/Edge/etc. profile directory to a temp dir (skipping
  `Service Worker`, `IndexedDB`, `Local Storage`, `GPUCache`, lock files,
  etc. — `SKIP_DIRS`/`SKIP_FILES`, `cookies.ts:25-32`), spawn that copy
  **headless** with `--headless=new --remote-debugging-port=<free port>
  --user-data-dir=<tempdir>` (`launchChromiumHeadless`, `cookies.ts:269`),
  pull cookies via `Storage.getCookies` over a raw CDP WebSocket
  (`getCookiesViaCdp`, `cookies.ts:315`), kill the headless process and
  delete the temp dir, then merge into Electron's own
  `session.defaultSession` cookie jar via `electronSession.cookies.set`
  (`importChromeProfileCookies`, `cookies.ts:360`). Re-imports are
  "conservative re-syncs": every domain present in the new export gets its
  *existing* Electron-jar cookies wiped first, so stale/rotated cookies
  don't linger (`cookies.ts:390-447`). `chrome-import/profiles.ts` is a
  from-scratch, no-dependency Chromium-family locator: hardcoded
  per-OS/per-browser executable + `User Data` dir candidate lists for 15
  browser "definitions" (Chrome, Canary, Brave, Edge, Chromium, Arc,
  Opera, Vivaldi, Yandex, Iridium, Comet, Helium, Dia, Sidekick, Thorium,
  SigmaOS, Wavebox, Ghost Browser, Blisk — `browserDefinitions()`,
  `profiles.ts:79-368`), falls back to PATH lookup, and only counts a
  directory as a real "profile" if it has a readable `Cookies` or
  `Network/Cookies` file (`hasReadableCookieStore`, `profiles.ts:512`).
  **All sessions share one cookie jar** — `session.defaultSession` is
  process-global; `BrowserPool.create()` never sets a `partition` on the
  `WebContentsView`'s webPreferences, so there is no Playwright-style
  per-agent incognito browser context. Multiple concurrent agent sessions
  in Desktop are separate tabs with separate CDP targets, but **one
  shared identity/cookie/localStorage space** — worth flagging explicitly
  since it's the opposite of what a multi-tenant or multi-agent-isolation
  design usually wants.
- **Identity spoofing.** `sessions/browserIdentity.ts` fabricates a
  **Firefox** User-Agent + `Accept-Language` + strips all
  `sec-ch-ua*` Client-Hint headers (`USER_AGENT_CLIENT_HINT_HEADERS`,
  `browserIdentity.ts:13-25`) and applies it both to
  `session.defaultSession.setUserAgent` and every outgoing request via
  `webRequest.onBeforeSendHeaders` (`index.ts:283-298`,
  `registerBrowserIdentityHeaders`). So the driven browser is Chromium
  under the hood but presents as Firefox on the wire — a deliberate
  anti-bot-detection stance (Chromium + real CDP debugger attached is one
  of the most common headless/automation fingerprints; masking the UA and
  dropping Client Hints removes one detection signal, though the CDP
  `Runtime.enable` side channel and other automation tells are not
  addressed here).
- **Crash / gone handling.** `BrowserPool` wires `webContents` events
  `destroyed` and `render-process-gone` to a `notifyGone` callback
  (`BrowserPool.ts:322-332`); main forwards that to the renderer as
  `sessions:browser-gone` IPC and calls
  `sessionManager.markBrowserEnded(sessionId)` (`index.ts:228-236`) so an
  idle session whose view died gets promoted straight to a terminal state
  instead of hanging in "Browser starting…". There is no auto-relaunch of
  a crashed `WebContentsView` — the session simply ends; the user has to
  start a new one. Separately, a generic **inactivity** ("stuck") timer
  (`STUCK_TIMEOUT_MS = 30_000`, `sessions/SessionManager.ts:17`) flips a
  `running` session to `stuck` if no output event arrives for 30s
  (`resetStuckTimer`, `SessionManager.ts:766-779`) — this is the *only*
  "is something wrong" signal in the whole system, and it's engine-output
  silence, not a captcha/DOM-specific check (see §7).
- **Idle throttling / lifecycle, not crash-related but adjacent:**
  `BrowserPool` also drives Chromium's own `Page.setWebLifecycleState`
  (`'active'`/`'frozen'`) via a second short-lived debugger attach
  (`setLifecycleState`, `BrowserPool.ts:544-573`) plus per-state
  `webContents.setFrameRate` (60fps active / 4fps throttled / 1fps frozen,
  `BrowserPool.ts:10-12`) to cut CPU/GPU cost for detached or idle
  sessions, with a configurable delay
  (`BU_IDLE_BROWSER_FREEZE_DELAY_MS`, default 15s, `BrowserPool.ts:39-45`).

## 3. Page representation for the model

**There is no serialized-DOM data structure at all** — confirmed by
exhaustive `rg` across the whole repo for every schema name the task
asked about (`buildDomTree`, `DOMTree`, `clickable`, `highlightIndex`,
`isTopElement`, `SerializedDOM`, `EnhancedDOM`, "paint order",
"set-of-marks"): zero real hits. The design instead is: **the coding
agent looks at screenshots and writes its own ad-hoc `Runtime.evaluate`
JS per step**, guided by markdown "interaction skill" recipes bundled
into its working directory (`app/src/main/hl/stock/interaction-skills/`,
18 files, staged read-only every launch). Concretely, per the task's
specific questions:

- **Primary channel = vision, not structure.**
  `interaction-skills/screenshots.md` states outright: `session.Page.
  captureScreenshot` is "your default discovery and verification tool";
  "a screenshot answers 'is the thing I need visible and where?' faster
  than a DOM walk." The repo-root legacy doc (`app/interaction-skills/
  screenshots.md:3`, the older `helpers.js`-era version) is even more
  blunt: "Screenshots are the primary way to understand the current page
  state. Use them before and after every meaningful action."
- **Bounding boxes**: computed on-demand, never pre-serialized. Two
  mechanisms, both ad hoc:
  1. `Runtime.evaluate` calling `getBoundingClientRect()` inside a
     hand-written selector query, returned as
     `{x, y, width, height}` in **CSS pixels, relative to the current
     viewport** (`interaction-skills/dropdowns.md`, `shadow-dom.md`,
     `scrolling.md` all use this pattern). For elements inside iframes,
     the skill explicitly documents that `getBoundingClientRect()`
     returns **iframe-local** coordinates and must be manually offset by
     the iframe's own rect to get page coordinates
     (`interaction-skills/iframes.md`, "Frame-local vs page
     coordinates" section).
  2. `DOM.getBoxModel({nodeId})` → `model.border` (8 numbers, 4 corners)
     for one already-resolved CDP node, used mainly to clip a screenshot
     to one element (`interaction-skills/screenshots.md`, "Element
     screenshots via DOM.getBoxModel").
  `captureScreenshot` output is always in **device pixels**; the skill
  explicitly calls out the CSS-px/device-px conversion trap when
  eyeballing coordinates from a screenshot vs. clicking in CSS-px space
  (`interaction-skills/viewport.md`, "Traps").
- **Occlusion / "is this the top element"**: no `isTopElement`-style
  check exists. The docs instead push the agent toward **compositor-level
  coordinate clicks** (`Input.dispatchMouseEvent`) precisely *because*
  they bypass the question of DOM occlusion, shadow roots, and
  iframe/OOPIF boundaries entirely — "Compositor-level clicks don't care
  about shadow roots. If you can see it in a screenshot,
  `Input.dispatchMouseEvent` can click it" (`shadow-dom.md`), and the same
  claim is repeated near-verbatim in `iframes.md` and
  `cross-origin-iframes.md`. When occlusion actually needs to be reasoned
  about, that reasoning happens in the model's own vision on the
  screenshot — there is no `elementFromPoint`-based determinism layer.
- **Visibility**: no dedicated `isVisible()` helper; the pattern in the
  skills is ad hoc `getComputedStyle(el)` checks written per-query (e.g.
  `overflowY === 'auto' && el.scrollHeight > el.clientHeight` to find the
  active scroll container in `scrolling.md`), plus the meta-rule "always
  verify with a screenshot" repeated across nearly every skill file.
- **Iframes**: same-origin → walk `.contentDocument` recursively, with an
  explicit caveat that a same-origin frame can *become* cross-origin
  mid-session (e.g. an OAuth redirect) and the agent must re-check
  (`interaction-skills/iframes.md`). Cross-origin → Chrome's real OOPIF
  model: `Target.getTargets()` lists the iframe as its own CDP target,
  `session.use(iframe.targetId)` re-routes subsequent Page/DOM/Runtime
  calls to it, and the skill explicitly warns the OOPIF target can be
  **lazily created** (e.g. Stripe's card iframe mounts only after the
  outer input is focused) and **destroyed on parent navigation**
  (`interaction-skills/cross-origin-iframes.md`). No unified iframe tree
  is ever built — each iframe is discovered and addressed independently,
  per step.
- **Shadow DOM**: `DOM.querySelector`'s `pierceShadow`/`>>>` combinator,
  or a hand-rolled recursive JS walk over `.shadowRoot` (both shown
  verbatim in `interaction-skills/shadow-dom.md`); closed shadow roots
  are explicitly called out as unreachable from JS, with coordinate
  clicking as the documented fallback.
- **Text trimming / size budget**: there is no fixed "the page's text is
  truncated to N characters" policy anywhere — because there's no
  whole-page serialization step to truncate. Truncation only happens
  locally, where the agent's own `Runtime.evaluate` snippet chooses to
  `.slice()`/`.textContent` a specific element it already selected.
- **Accessibility tree**: not used anywhere in this repo (no `Accessibility.*`
  CDP domain calls found in any interaction-skill or engine adapter file).
  Purely DOM + Runtime + compositor-level Input, plus vision on
  screenshots.
- **Set-of-marks / drawn overlays**: none. No highlight boxes, index
  numbers, or any visual annotation is ever drawn onto the page or
  screenshot before it's shown to the model — the screenshot the model
  sees is the literal, unmodified page pixels.

## 4. Element addressing

**There is no persistent index → node mapping, and thus no "stale index"
failure mode to handle** — because there's no index. Every interaction is
freshly computed, every step:

- The closest thing to "addressing" is a CDP `nodeId` from
  `DOM.querySelector`/`getDocument`, but the skills only ever use it
  transiently within a single snippet (e.g. immediately feeding it to
  `DOM.getBoxModel` for a screenshot clip) — never stored across steps.
  This sidesteps CDP's own well-known `nodeId` invalidation-on-DOM-mutation
  problem by simply never caching a `nodeId` past one call.
  `options-block.md` documents the actual cross-step addressing strategy
  used for anything that needs to survive a user round-trip (a human
  picking from an `options` card grid): each option carries the site's
  **own stable id** (SKU/ASIN/listing-id) picked by the agent itself, and
  the agent is told to "re-locate the tile in the live page" using that id
  plus the URL it already has, *by re-querying the DOM from scratch* —
  not by any node handle surviving the round-trip.
  `interaction-skills/cross-origin-iframes.md` explicitly warns that "a
  cached `iframe.targetId` from before a navigation is dead" — the one
  place a persisted identifier is discussed, and the guidance is "don't
  trust it, don't cache it."
  The repeated cross-file refrain ("re-measure after opening", "always
  verify with a screenshot", "layout shifts invalidate cached coords" in
  `scrolling.md`) is this project's entire answer to staleness: never
  cache positions or handles across an action boundary; always
  re-`querySelector`/re-screenshot immediately before acting.

## 5. Actions

**There is no bounded, structured action vocabulary (no fixed
`click(index)` / `type(index, text)` tool schema) — the "tools" are the
entire ~652-method CDP surface** (`sdk/generated.ts`, 56 domains,
codegen'd from `browser_protocol.json` + `js_protocol.json`), invoked as
arbitrary JS via a bash CLI. Concretely:

- **How it's exposed to the agent**: `browser-harness-js '<JS>'` (or
  heredoc for multi-statement) is a plain shell command any of the coding
  agents' native "run a bash command" tool can call — so from the coding
  agent's own tool-schema point of view, browser automation is not a
  distinct tool at all, just more bash. The three engine adapters
  (`hl/engines/{claude-code,codex,browsercode}/adapter.ts`) each inject
  the same instruction text into the wrapped system/user prompt: "Use the
  `browser-harness-js` CLI for browser actions. Start with
  `browser-harness-js 'await connectToAssignedTarget()'`" (e.g.
  `claude-code/adapter.ts:129`), and for the `browsercode` (OpenCode/
  `bcode`) engine specifically, its own native `browser_execute` tool is
  hard-disabled in config (`tools: { browser_execute: false }`,
  `browsercode/adapter.ts:144`) precisely so the agent has no competing
  built-in browser tool and must go through `browser-harness-js`.
- **Canonical low-level primitives**, all raw CDP, all documented with
  copy-paste snippets in the interaction-skills:
  - Click/type: `Input.dispatchMouseEvent` (`mousePressed`/`mouseReleased`
    pairs) and `Input.insertText` / `Input.dispatchKeyEvent` — deliberately
    **coordinate-based and compositor-level**, not DOM-node-based, so the
    same two calls work through iframes, OOPIFs, and shadow DOM without
    special-casing any of them (repeated in `iframes.md`, `shadow-dom.md`,
    `cross-origin-iframes.md`, `dropdowns.md`).
  - Navigate: `Page.navigate({url})`, waited on via
    `session.waitFor('Page.loadEventFired', predicate, timeoutMs)` — a
    generic CDP-event-wait helper on `Session`, not a `waitUntil:
    'networkidle'` Playwright-style option. No built-in "network idle"
    heuristic exists; the skills instead teach waiting for a *specific*
    `Network.responseReceived` matching a URL substring when the agent
    needs to know a particular request finished (`cross-origin-iframes.md`,
    "Listening to events from an OOPIF").
  - Scroll: three explicit fallback tiers documented in order of
    reliability in `interaction-skills/scrolling.md` — (1)
    `Input.dispatchMouseEvent({type:'mouseWheel'}, x, y, deltaX, deltaY)`
    at a point (closest to a real user, required for virtualized lists
    like `react-window`); (2) `element.scrollIntoView({block:'center',
    behavior:'instant'})` via `Runtime.evaluate`; (3) setting
    `el.scrollTop` directly for custom `overflow:auto` containers that
    don't respond to wheel events. "Scroll into view" is thus present but
    explicitly demoted to tier 2, not the default.
  - Screenshot: `Page.captureScreenshot` (`format`, `quality`,
    `captureBeyondViewport`, `clip`) — see §3.
  - Dropdowns: branches by DOM shape rather than one universal action —
    native `<select>` gets `.value =` + a synthetic `change` event (never
    simulated clicks, since a real click opens an OS-native menu CDP can't
    close); custom overlay menus get click-trigger → re-measure → click-
    by-text-match; searchable comboboxes get click-to-focus →
    `Input.insertText` → arrow-key + Enter via `Input.dispatchKeyEvent`
    (documented library-specific gotchas for Radix/MUI Autocomplete).
  - Uploads/downloads/drag-and-drop/print-to-PDF/tabs/network-request
    inspection/cookies/viewport-resize each get their own interaction-skill
    file with the matching CDP domain (`Page.setDownloadBehavior`,
    `Input.dispatchDragEvent` / synthetic HTML5 DnD events,
    `Page.printToPDF`, `Target.*`, `Network.*`, `Network.getCookies` /
    `Network.setCookie`, `Emulation.setDeviceMetricsOverride`) — not
    individually transcribed here for space, but all follow the same
    "raw CDP call + copy-paste snippet + a Traps section" format.
- **Verification loop, not a fixed step contract**: `stock/AGENTS.md`
  ("Verification Loop" section) instructs: screenshot for visual state,
  `Runtime.evaluate` for page state, `session.waitFor` for protocol
  events — "Verify after every meaningful browser action." This is
  prose guidance to the agent, not an enforced harness-level check.

## 6. Multi-agent / "team of browser agents"

- **Per-session isolation**: each session = one `WebContentsView` (own
  CDP target, own uploads/`outputs/<sessionId>/` dirs, own coding-agent
  child process) but **shared Electron `session.defaultSession`** (see §2
  — no per-agent cookie/storage partition). Concurrency is capped by
  `BrowserPool`'s `maxConcurrent` (default 10,
  `BrowserPool.ts:9`); beyond that, new sessions queue
  (`this.queue`) until a slot frees (`drainQueue`,
  `BrowserPool.ts:970-981`).
- **Session persistence**: `SessionManager` backs every session with a
  row in a `better-sqlite3` `sessions.db`
  (`app/src/main/index.ts:209`), with status transitions
  (`running`/`stuck`/`paused`/`stopped`/etc.), auth-mode snapshot, and
  full output-event log, so sessions survive app restarts and can be
  resumed (`resumeSession`, referenced from `ChannelRouter.ts`).
- **Live view / screencast**: **not** CDP's native `Page.startScreencast`
  streaming API — `SessionScreencast` (`app/src/main/sessions/
  SessionScreencast.ts`) instead does **polled** `Page.captureScreenshot`
  over a short-lived `webContents.debugger` attach, JPEG quality 55, once
  per second by default (`DEFAULT_OPTIONS`, `SessionScreencast.ts:15-19`),
  with a 2.5s per-capture timeout (`CAPTURE_TIMEOUT_MS`) and an
  owner-token model so multiple UI surfaces (e.g. a session list thumbnail
  vs. the main hub view) can each request/release the same preview stream
  without stepping on each other (`start()`/owner-transfer logic,
  `SessionScreencast.ts:78-102`). Practically this means "live view" in
  Desktop is closer to a slow filmstrip than a real screencast — a
  deliberate simplicity/cost tradeoff (no continuous frame-diffing
  transport) worth noting since it's a much cheaper thing to build than a
  true CDP screencast pipeline, at the cost of ~1s latency and no
  frame-drop backpressure signalling beyond the timeout.
- **"Team of agents"**: Desktop's model is **N independent sessions**, each
  with its own tab/agent-process, coordinated only by the shared
  `SessionManager`/`BrowserPool`/one Electron cookie jar — there is no
  cross-session shared task graph, hand-off protocol, or supervisor/worker
  hierarchy in this codebase. "Multi-agent" here means "the user can have
  several independent browsing tasks open at once," not orchestrated
  agent teams.

## 7. Human takeover / captcha / login

- **Takeover mechanism, not detection**: a persistent, always-present
  **overlay `WebContentsView`** sits stacked on top of every browser view
  while its session is running (`takeoverOverlay.ts`) — a transparent,
  pulsing cyan-glow border with an "Automating" chip, revealing a "Stop
  and take over" button on hover. Because it's a sibling view stacked
  above the browser view in Electron's `contentView` z-order, **it
  physically intercepts all mouse/keyboard input** while present — "Input
  blocking is implicit: mouse events route to the topmost view at the
  cursor" (`takeoverOverlay.ts:9-19`, design-rationale comment). Clicking
  "Stop and take over" fires `ipcRenderer.invoke('takeover:stop',
  sessionId)` (`takeoverOverlay.ts:230-233`), which (per the file's own
  header comment) cancels the running agent session; once the overlay is
  hidden (`hide()`, `takeoverOverlay.ts:316-328`, called from
  `browserPool.setOnGone`) the browser view underneath receives input
  directly again — no CDP `Input.setIgnoreInputEvents` toggling needed,
  purely view-stacking.
  - There is no separate "give control back to the agent" UI captured
    here beyond starting a new session — takeover looks like a one-way
    door (stop-and-hand-off), not a pause/resume-agent toggle.
- **Captcha/login-wall detection is *not* a deterministic check anywhere
  in the codebase** (confirmed: no `captcha` string in any `.ts` file,
  only in markdown skill *prose*, e.g. `interaction-skills/
  cross-origin-iframes.md` mentioning recaptcha as an OOPIF example, and
  various domain-skills warning "this site may show a captcha if you go
  too fast"). Instead: (1) the harness's own `stock/AGENTS.md`
  ("Verification Loop" section) tells the agent to take a screenshot and
  judge for itself — "You're stuck on a captcha, login wall, or page
  state you can't resolve, and showing it helps the user see what you
  see" — i.e. captcha recognition is delegated entirely to the model's
  vision, not code; and (2) the app-level fallback is the generic 30s
  output-inactivity "stuck" timer (`SessionManager.ts:17`, see §2), which
  surfaces as a WhatsApp "Needs input — Check the hub" notification via
  `ChannelRouter` (`ChannelRouter.ts` — `session-updated` handler
  filtering `status === 'stuck'`). Both signals are generic ("something
  is blocking progress"), never captcha-specific.

## 8. Channels — WhatsApp (`@BU`)

One paragraph as requested: `channels/WhatsAppAdapter.ts` wraps
`@whiskeysockets/baileys` (an unofficial, reverse-engineered WhatsApp Web
multi-device client library — not the official Business API), persisting
auth state to `<userData>/whatsapp-auth/` via Baileys'
`useMultiFileAuthState`, with QR-code pairing surfaced through an
`onQr` callback and auto-reconnect with exponential backoff+jitter on
disconnect (`BACKOFF` constants, `WhatsAppAdapter.ts:21-26`). Inbound
messages flow through `ChannelRouter.handleInbound()`
(`channels/ChannelRouter.ts:81`): if the message is a WhatsApp *reply* to
a message the router previously sent (tracked in an in-memory
`sentMessageToSession` map keyed by WhatsApp message id), it resumes that
existing session with the reply text as new input
(`sessionManager.resumeSession`); otherwise it spawns a brand-new session
via `sessionManager.createSession(msg.text, {originChannel: 'whatsapp',
originConversationId})` and starts it immediately
(`createNewSession`, `ChannelRouter.ts:107-137`) — so a fresh inbound
WhatsApp message always becomes a new agent session (with its own fresh
browser tab), one-to-one, with no channel-side queuing or session-picker
step. `SessionManager` then emits `session-completed` /
`session-error` / `session-updated(status: stuck)` events that
`ChannelRouter`'s constructor subscribes to and turns into outbound
WhatsApp replies threaded back to the original conversation
(`sendAndTrack`, `ChannelRouter.ts:139-152`), closing the loop entirely
inside the main process — no renderer/UI involvement is required for a
channel-originated task to run end-to-end.

## What Aleph should borrow (design only, not code)

All of `browser-use/desktop` is MIT-licensed (`LICENSE`), so borrowing
patterns is uncontroversial license-wise; these are design ideas, called
out by exact file, not code to port:

1. **Don't build a bespoke DOM→JSON serializer at all — consider letting
   the agent drive raw CDP via a bash-callable REPL instead.**
   (`app/src/main/hl/stock/browser-harness-js/sdk/repl.ts` +
   `SKILL.md`). For an "Agent Browser" layer meant to be engine-agnostic
   and used by *arbitrary* coding agents (not just Aleph's own loop), a
   thin persistent-session CDP RPC bridge is dramatically less
   maintenance than a hand-tuned DOM-scoring/highlighting pipeline, and
   sidesteps the entire "stale index" class of bugs by never having
   indices.
2. **Coordinate-based, compositor-level input as the default action
   primitive** (`interaction-skills/shadow-dom.md`, `iframes.md`,
   `cross-origin-iframes.md`) — `Input.dispatchMouseEvent` at page
   coordinates uniformly solves shadow-DOM, same-origin iframe, and OOPIF
   targeting without three different code paths. Worth adopting as
   Aleph's default click primitive regardless of what serialization
   layer sits on top.
3. **Verify by screenshot diff, not DOM diff, after every action**
   (`stock/AGENTS.md`, "Verification Loop") — "the DOM can lie about
   state; pixels cannot" (`interaction-skills/screenshots.md`). Cheap,
   engine-agnostic verification heuristic.
4. **Human-takeover as view-stacking, not an input-blocking flag**
   (`takeoverOverlay.ts:1-19` design comment) — an overlay surface
   physically above the browser view gets free input interception from
   the compositor's own hit-testing; no CDP `setIgnoreInputEvents`
   bookkeeping, no race between "is the agent still sending input."
5. **Generic inactivity timeout as the sole "agent is stuck" signal,
   surfaced identically regardless of cause** (`SessionManager.ts:17,
   766-779`) — captcha, login wall, or genuine confusion all look the
   same from the harness's point of view, and delegating "what kind of
   stuck is this" to the model's own vision (rather than building N
   per-cause detectors) is a defensible simplicity call in R7/P8's spirit
   (LLM does the judgment, harness only measures silence).
6. **Cookie import as a disposable, fully out-of-process headless
   Chromium — never touch the user's real profile live**
   (`chrome-import/cookies.ts:193-267`, `copyProfileToTemp` +
   `launchChromiumHeadless`) — copy-then-launch-then-discard avoids any
   file-lock contention with the user's actual running browser and
   avoids ever attaching CDP to a profile the user is using interactively.
7. **From-scratch, dependency-free Chromium-family discovery across 19
   browser variants** (`chrome-import/profiles.ts:79-368`,
   `browserDefinitions()`) — a good reference table if Aleph ever needs
   "find any Chromium-based browser on this machine" without pulling in
   Playwright's browser-fetching machinery.
8. **CDP-ownership verification after binding a debug port**
   (`index.ts:448-463`, `verifyCdpOwnership`) — cheap guard against
   silently attaching to the wrong Chromium process if another one
   already held the chosen port; worth replicating anywhere Aleph binds a
   well-known/predictable debug port.

## What NOT to copy, and why

- **Shared cookie jar across all concurrent sessions**
  (`session.defaultSession` used everywhere, no per-`WebContentsView`
  `partition`) — fine for a single-user desktop app where "the browser"
  is conceptually one identity, wrong for Aleph if multiple agents/teams
  ever need isolated browsing identities concurrently. This is a
  one-user-desktop-app assumption baked deep into `BrowserPool` and
  `chrome-import`; don't inherit it uncritically.
- **UA/Client-Hint spoofing as Firefox while running Chromium**
  (`browserIdentity.ts`) — a legitimately fragile anti-detection tactic:
  it fixes one fingerprinting signal (UA/Client-Hints) while leaving
  Chromium-specific JS-visible tells (CDP `Runtime.enable` side effects,
  `navigator.webdriver`, Chromium-only APIs) unaddressed; a half-measure
  that can make behavior *more* suspicious to sophisticated bot detection
  (UA says Firefox, `window.chrome` and V8-specific behavior say
  Chromium) rather than less. If Aleph needs anti-detection, treat it as
  its own researched subsystem, not a header-rewrite bolt-on.
  License note: MIT, so no legal blocker — this is a technical/ethical
  judgment call, not a licensing one.
- **Polled screenshot "screencast" instead of CDP's real
  `Page.startScreencast`** (`SessionScreencast.ts`) — acceptable for a
  1fps preview thumbnail, but if Aleph wants an actually-responsive live
  view (e.g. for real-time human collaboration on a browsing task), the
  native streaming API avoids the poll-interval latency and repeated
  full-frame JPEG re-encoding cost this design pays every second per
  active session.
- **No per-cause blocked-page detection** — fine as a v1 (delegates to
  model vision, per R7-style philosophy), but note it also means Desktop
  has zero telemetry on *why* a session is stuck (captcha vs. login vs.
  rate-limit vs. genuine model confusion all produce the identical
  `stuck` signal) — if Aleph wants better observability/analytics on
  failure modes, this generic timeout alone won't provide it.
- **`browser-harness-js` is vendored/copy-pasted into the app tree**
  rather than an installed dependency (confirmed: no `node_modules`
  package for it) — reasonable for an Electron app bundling a CLI it
  ships to a sandboxed working directory, but means there is no version
  pin/lockfile discipline on this component from `desktop`'s own
  dependency graph; a fork would need to track updates manually against
  the upstream `browser-use/browser-harness-js` repo.

## Availability note

**Browser Harness JS source WAS available locally** — fully vendored at
`app/src/main/hl/stock/browser-harness-js/` inside the `desktop` clone
itself (not a separate `node_modules` package, not a sibling repo
checkout). No `browser-harness` npm package or standalone repo checkout
was found anywhere on this machine outside of this vendored copy, so
this survey's account of Browser Harness JS is entirely from that
vendored source (its `SKILL.md`, `sdk/session.ts` signatures referenced
via grep, and the `sdk/generated.ts` codegen description) — `sdk/repl.ts`
and `sdk/session.ts` were read for signatures/architecture but not
reproduced in full here for space; `sdk/generated.ts` (~655KB codegen
output) was not read in full, only grepped, per its own advice ("only
loaded if you read it").

