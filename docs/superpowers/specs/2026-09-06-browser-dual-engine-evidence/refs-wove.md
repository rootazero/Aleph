# wove browser integration scan

Repo: /Volumes/TBU4/Github/wove (read-only reference; Go + Electron coding agent on Wave Terminal fork)
License: Apache 2.0 (top-level LICENSE). Note: NOTICE / MODIFICATIONS.md present — check for attribution requirements if anything is ever ported.

**Bottom line up front: wove's browser integration is NOT thin. It's a real CDP-driven "computer use for the web" layer**, not just a screenshot tool: SoM (set-of-marks) visual grounding, native CDP mouse input for iframes/reCAPTCHA, a full JS-driven DOM action toolkit (click/type/press-key), console-log capture, and text-only degradation for non-vision models. Worth studying in depth.

## 1. Where the browser lives

It's an Electron `<webview>` tag embedded in the app window (`frontend/app/view/webview/webview.tsx:1237` `WebView` component), **not** a separate Chromium process over external CDP and not Playwright/Puppeteer. Electron's `<webview>` is itself backed by an out-of-process `WebContents` guest, and the Go backend reaches that `WebContents` two ways:
- `wc.executeJavaScript(...)` for DOM reads/writes (`emain/emain-web.ts:82,154`, `emain/emain-wsh.ts:217,221,337`)
- `wc.debugger.attach("1.3")` + `wc.debugger.sendCommand(...)` — Electron's **built-in CDP debugger API** on that same WebContents, for `DOMSnapshot.captureSnapshot`, `Page.captureScreenshot`, and `Input.dispatchMouseEvent` (`emain/emain-wsh.ts:273,280-293,346,355,368`)

So CDP is used, but it's CDP-over-Electron's-own-debugger-attach, not an external `--remote-debugging-port` Chromium instance. No Playwright/Puppeteer dependency anywhere in the browser path (only appears in `frontend/preview/mock/preview-electron-api.ts` as an unrelated mock).

**Yes — the agent reuses Wave Terminal's existing "web" block type**, the same one a human uses to browse manually. `GetWebOpenToolDefinition` (`pkg/aiusechat/tools_web.go:654-722`) creates a block with `waveobj.MetaKey_View: "web"` — identical to a user opening a browser tab by hand. This is the architectural crux of section 5 below: **agent and human share the literal same webview instance**, there is no separate "headless agent browser."

## 2. The agent's browser tools

All defined in `pkg/aiusechat/tools_web.go` and `pkg/aiusechat/tools_webcapture.go` (Go), registered into the tool list in `pkg/aiusechat/tools.go:230` etc. Each `ToolTextCallback`/`ToolAnyCallback` runs Go-side, and does its real work by making an RPC call over the `wshrpc` bus with `Route: wshutil.ElectronRoute`, which is handled TypeScript-side in the Electron **main process** by `ElectronWshClientType` (`emain/emain-wsh.ts:224`).

| Tool | Args | Purpose | Go impl | TS impl |
|---|---|---|---|---|
| `web_open` | `url` | create a new "web" block/widget, registers it in the calling chat's `OwnedWidgetSet` | `tools_web.go:654` | (block creation only, no webview RPC) |
| `web_navigate` | `widget_id, url` | change URL of existing widget via wstore metadata update | `tools_web.go:54` | n/a (webview watches block meta) |
| `web_read_text` | `widget_id, selector` | reload page, return `innerText` for all matches, briefly highlights matched elements | `tools_web.go:217` → `webReadContent` | `handle_webselector` (`emain-wsh.ts:229`) → `webGetSelector` (`emain-web.ts:46`) |
| `web_read_html` | `widget_id, selector` | same but `innerHTML` | `tools_web.go:236` | same path |
| `web_seo_audit` | `widget_id` | runs a canned JS snippet (`seoAuditJS`, `tools_web.go:255`) extracting title/meta/OG/Twitter/JSON-LD/headings/alt-text/link counts | `tools_web.go:379` | same path, `execjs` opt |
| `web_exec_js` | `widget_id, code` | arbitrary JS, function-body semantics, **no reload** (preserves state) | `tools_web.go:329` → `webExecJsOnWidget` | same `handle_webselector`, `execjs` opt |
| `web_click` | `widget_id, selector` | JS `el.click()`, with special-case: if the element is/contains an `<a href>`, sets `window.location.href` directly (webview's synthetic click doesn't reliably navigate) | `tools_web.go:455` | via `webExecJsOnWidget` |
| `web_mouse_click` | `widget_id, selector? , x?, y?` | **native OS-level mouse click via CDP** `Input.dispatchMouseEvent` — for iframes/reCAPTCHA/anything JS `.click()` can't reach | `tools_web.go:589` → `webMouseClickOnWidget`/`webMouseClickXYOnWidget` | `handle_webselector` `mouseclick` branch, CDP attach (`emain-wsh.ts:243-301`) |
| `web_type_input` | `widget_id, selector, text, clear` | sets value via native property setter (to defeat React/Vue controlled-input shadowing) + dispatches `input`/`change`/`InputEvent` | `tools_web.go:724` | via `webExecJsOnWidget` |
| `web_press_key` | `widget_id, key, selector?` | dispatches `keydown`/`keypress`/`keyup` KeyboardEvents, special-cases Enter→form submit | `tools_web.go:825` | via `webExecJsOnWidget` |
| `web_get_console` | `widget_id, level` | reads `window.__woveConsoleLogs` (populated by `CONSOLE_CAPTURE_JS` injected on load, `webview.tsx:270,1494`) | `tools_web.go:924` | via `webExecJsOnWidget` |
| `web_inspect_vue` | `widget_id, selector?` | walks `__vueParentComponent` chain + Inertia.js `[data-page]` JSON — framework-specific debugging aid | `tools_web.go:1038` | via `webExecJsOnWidget` |
| `web_capture` | `widget_id` | **the SoM tool** — CDP `DOMSnapshot.captureSnapshot` + `Page.captureScreenshot`, see §3 | `tools_webcapture.go:73` | `handle_webcapture` (`emain-wsh.ts:323-395`) |

**RPC path, precisely**: Go tool callback → `wshclient.WebSelectorCommand`/`WebCaptureCommand` (generated client, `pkg/wshrpc/wshclient/wshclient.go`) → `wshrpc`/`wshutil` bus (a JSON-RPC-like multiplexed transport wove inherits from Wave Terminal, running over a local socket) with `Route: wshutil.ElectronRoute` → routed to the long-lived `ElectronWshClientType` instance registered in the Electron **main** process → main process resolves which `WebContents` backs the target `<webview>` via a *second*, lightweight IPC round-trip: it emits `"webcontentsid-from-blockid"` on the active tab's `webContents`, the renderer (which owns the `<webview>` DOM node) replies over a one-shot `ipcMain.once` channel with the guest's numeric `webContentsId`, and main resolves it via `webContents.fromId(id)` (`emain/emain-web.ts:7-25`). From there main drives that `WebContents` directly (`executeJavaScript` / `debugger.sendCommand`) — no further hop through the renderer is needed because Electron's main process can command any `WebContents` (including a `<webview>` guest) once it has its id.

## 3. Page representation for the model

Two distinct representations, chosen per-tool:

**a) Text tools** (`web_read_text`/`web_read_html`/`web_seo_audit`/`web_exec_js`): plain string(s) via `executeJavaScript`, no structure beyond what the JS returns. Truncated at 15,000 chars (`tools_web.go:185`) or 50,000 for `web_exec_js` (`tools_web.go:448`).

**b) `web_capture` (the SoM tool)** — this is the interesting one, output shape from `WebCaptureRtnData` (`pkg/wshrpc/wshrpctypes.go:610-637`) built by `runWebCapture` (`tools_webcapture.go:20-68`):
```
Viewport: 1280x800, scroll: 240/3400
Elements (37):
[0] [Click] a.nav-link "Home" at (24,12)
[1] [Type] input[name="q"] at (140,12)
[2] [Click] button.btn-primary "Search" at (620,12)
...
```
plus (when the model has `AICapabilityImages`) a JPEG data-URI screenshot with the same numbered markers burned into the image (SoM = Set-of-Marks, red numbered badges, see `injectSomMarkers`, `emain-wsh.ts:201-218`).

The element list comes from a genuine **accessibility/layout tree**, not naive DOM text extraction: it's built by calling CDP's `DOMSnapshot.captureSnapshot` (with `computedStyles: ["display","visibility"]`, `includeDOMRects: true`) then `processCdpSnapshot` (`emain-wsh.ts:84-199`) which:
- flattens CDP's string-interned node/layout arrays across **all frames** (`docIdx` loop → cross-iframe, unlike plain `document.querySelectorAll`)
- keeps only element nodes with a nonzero layout box, filters out non-meaningful tags (`HTML/HEAD/SCRIPT/STYLE/...`)
- flags interactivity via a hardcoded tag set (`A,BUTTON,INPUT,SELECT,TEXTAREA,DETAILS,SUMMARY`) **or** ARIA role set **or** `contenteditable`
- keeps non-interactive elements only if they carry visible text or are `IMG`
- caps at `MAX_ELEMENTS = 200`, sorted interactive-first then top-to-bottom/left-to-right, each element re-numbered by final position

**Bounding-box / spatial data**: yes, full `[x, y, w, h]` in page coordinates per element (`WebCaptureElement.bbox`, `wshrpctypes.go:616-625`), each accompanied by a synthesized one-line CSS selector (`buildCssSelector`, `emain-wsh.ts:71-82`) built from id → name/type → up to 2 classes.

**Size control**: the screenshot is downscaled so width ≤512px (`scale = Math.min(512/viewport.width, 1)`, `emain-wsh.ts:367`), JPEG quality 30 — explicit comment: "LLM needs layout, not text" (`emain-wsh.ts:366`). The element list is capped at 200 and text per element at 80 chars (`emain-wsh.ts:17,146`). Non-vision models (e.g. MiniMax, detected via `capabilities` not containing `AICapabilityImages`) get element-list-only, no screenshot at all (`tools_webcapture.go:73-125`) — a deliberate text/vision fork at the tool-definition level, not a runtime capability probe.

## 4. Element addressing

Two independent addressing schemes, not unified:
- **CSS selector** (the default/preferred path for `web_click`/`web_type_input`/`web_press_key`/`web_read_*`): either the model writes its own selector, or it copies one verbatim from a prior `web_capture`'s `sel`/`desc` field (selectors are synthesized server-side from id/name/type/class, `buildCssSelector`, `emain-wsh.ts:71-82` — deliberately simple/robust, not a full unique-path CSS generator).
- **Numeric SoM index + x,y coordinates**: `web_capture` assigns each element a small integer `idx` for the model to *refer to in reasoning*, but there's **no tool that accepts that index directly** — the model must fall back to the element's `sel` (CSS selector) or its `(x,y)` from the description to act on it. `web_mouse_click` is the only tool that accepts raw `x,y` (for elements CSS can't reach, e.g. inside iframes/canvas/reCAPTCHA), dispatched as native CDP mouse events, not a JS click (`tools_web.go:552-587`, `emain-wsh.ts:243-301`).

## 5. Human-in-the-loop

**No lock, no mutex, no "agent is driving" mode indicator, no takeover UI.** The agent and the human operate the exact same `<webview>` DOM element in the exact same block (§1) — there's a single shared browsing context, full stop. Two mitigations exist, both weak/partial:
- **Transient visual highlight**: `webGetSelector`'s `highlight` option (set by `web_read_text`/`web_read_html`/`web_seo_audit`, not by click/type/exec_js) injects a 2-second "AI Reading…" badge + indigo outline + `scrollIntoView` on matched elements (`emain-web.ts:100-147`). This is a **read-only** signal — it doesn't fire for `web_click`, `web_type_input`, `web_press_key`, or raw `web_exec_js`, so most of the agent's actual interference with a page a human might be looking at is invisible.
- **CDP debugger-attach mutual exclusion**: `wc.debugger.attach("1.3")` throws if DevTools (or anything else) already has a debugger attached, surfaced as `"Cannot capture/click: another debugger is attached (DevTools open?)"` (`emain-wsh.ts:274-278,347-350`). This is an accidental byproduct of Electron's single-debugger-per-WebContents constraint, not a designed human/agent contention protocol — and it only fires for `web_capture`/`web_mouse_click` (the two CDP-attach tools), not for the JS-exec-based tools which race silently.
- Nothing stops a human from navigating/typing/scrolling the same webview mid-agent-turn, and nothing stops the agent's next tool call from acting on whatever the human just did to the page (the "reload before read" behavior in `webReadContent`, `tools_web.go:152-163`, actually makes this worse — every read silently blows away human-in-progress state like unsubmitted form input or scroll position).

## 6. Sub-task isolation

`run_sub_task` (`pkg/aiusechat/tools_subtask.go:47-220`) spawns an isolated AI conversation (own `chatId`, own system prompt, depth-limited to 2 via `subTaskMaxDepth`) that gets its **own** `uctypes.OwnedWidgetSet` (`subOwnedWidgets := uctypes.NewOwnedWidgetSet()`, line 152) — so ownership/cleanup bookkeeping is per-subtask. But browsers are **not** physically isolated: the code deliberately reuses the **parent's tab** for any widget the sub-task opens ("Always use the parent tab for widget creation/interaction so browsers and terminals opened by the subtask are visible to the user in the active tab", comment at `tools_subtask.go:172-176`) by wiring a fresh `TabStateGenerator` closure that captures the parent's `tabId` but the sub-task's own `OwnedWidgets`. Net effect: a sub-task's `web_open` creates a new widget/tab-content next to the parent's, gets tracked for auto-cleanup under the sub-task's own set, but there is **no separate browser instance or context per sub-task** — they're all still individual `<webview>` tags a human could equally interact with, per §5's caveats.

## 7. Provider abstraction

`UseChatBackend` interface (`pkg/aiusechat/usechat-backend.go:20`) is the seam: each provider (Claude/GPT/Gemini/Ollama/MiniMax/etc.) implements `RunChatStep`, converting the common `uctypes.ToolDefinition` (plain `map[string]any` JSON-Schema, exactly what's shown in §2's table) into that provider's native function-calling wire format. Tool *behavior* (the `ToolTextCallback`/`ToolAnyCallback`/`ToolImageTextCallback` Go closures) is defined once and shared; only the schema *serialization* and response *parsing* are backend-specific — the same pattern Aleph would want for its own provider fan-out.

## 8. `bench/`

No browser-task benchmarking. `bench/` (`wove_agent.py`, `lab-*.sh`, `compare-*.py`) runs **terminal-bench@2.0** via `harbor run --dataset terminal-bench@2.0` (`bench/wove_agent.py:5`) — a Docker/Harbor-driven coding-agent benchmark (shell tasks: install runtimes, run tests, etc.), with cross-task error-memory experiments and endpoint/plan comparison tooling. `metadata.yaml` registers the agent for the (presumably terminal-bench leaderboard) harness. Nothing in `bench/` exercises `web_capture`/`web_click`/etc.

## What Aleph should borrow (design only, not code)

- **SoM (set-of-marks) capture as the primary grounding primitive**: pair a downscaled/low-quality screenshot (`emain/emain-wsh.ts:366-378`, 512px/JPEG-q30 — explicitly "layout not text") with a numbered, CDP-`DOMSnapshot`-derived element list carrying `bbox` + synthesized CSS selector (`emain/emain-wsh.ts:84-199`, `pkg/wshrpc/wshrpctypes.go:616-625`). This is a much stronger grounding signal than screenshot-alone or DOM-text-alone, and it degrades cleanly to text-only for non-vision models by construction (`pkg/aiusechat/tools_webcapture.go:73-125`) — worth mirroring for Aleph's own multi-model roster (R7/R8: this is exactly "赋能 LLM" infrastructure, not a rules engine).
- **Cross-frame flattening at the CDP layer**: `processCdpSnapshot`'s per-document loop (`emain-wsh.ts:88` `for docIdx...`) walks *all* frames in one snapshot, giving iframe-aware element addressing for free — something plain `document.querySelectorAll` from injected JS can never see across origins.
- **Native input as an escape hatch, not the default**: JS `el.click()`/property-set is the default (cheap, fast) path; CDP `Input.dispatchMouseEvent` is reserved for the "JS can't reach it" case (cross-origin iframes, reCAPTCHA) and is explicitly framed that way in the tool description (`pkg/aiusechat/tools_web.go:593` `"Native mouse click (CDP). Works with iframes, reCAPTCHA..."`). Two-tier click tooling (`web_click` vs `web_mouse_click`) is a good shape to copy.
- **Ownership-scoped widget lifecycle**: `uctypes.OwnedWidgetSet` (`pkg/aiusechat/uctypes/uctypes.go:14-23`) ties every agent-opened browser/terminal to the chat (or sub-task) that opened it, enabling `close_widget` to refuse closing anything the agent didn't create (`pkg/aiusechat/tools_close.go:24`) and enabling auto-cleanup on chat end (`CleanupOwnedWidgets`, `tools_close.go:101`). Directly reusable idea for any Aleph resource an agent can spawn.
- **Sub-task-scoped ownership, shared physical surface** (`pkg/aiusechat/tools_subtask.go:152-176`): give each sub-task its own bookkeeping set even when the underlying widget/tab is intentionally shared with the parent for visibility — a clean way to reconcile "isolated accounting" with "human/parent should still see what's happening," if Aleph wants sub-agents' browser actions visible rather than fully hidden.
- **Provider-agnostic tool definition struct** (`uctypes.ToolDefinition` + `UseChatBackend.RunChatStep`, `pkg/aiusechat/usechat-backend.go:20`): one Go struct carries name/schema/description/callback; each backend only translates schema→wire-format and parses the response. Matches the shape Aleph already wants for its own provider fan-out (R7/`src/providers/`).

## What NOT to copy and why

- **The shared-webview human/agent model (§5) is a real defect, not a pattern** — no lock, no visible "agent is driving" state outside a 2s read-only highlight, and a reload-before-read that silently destroys human in-progress page state (`pkg/aiusechat/tools_web.go:152-163`). Aleph's obscura/Chromium dual-engine plan (per [[project-browser-dual-engine-pivot]]) already intends a CDP-unified interface; if Aleph ever lets a human co-drive the same tab an agent controls, it should design an explicit contention/handoff protocol from day one rather than falling into wove's implicit-sharing trap — this is precisely the kind of "two ends complete, no line between them" gap judged in CLAUDE.md's engineering-criteria list (a visible ownership signal exists for reads, doesn't exist for writes — an asymmetry, not an oversight to shrug off).
- **CDP-via-Electron-debugger-attach as the only capture mechanism** — it's mutually exclusive with DevTools being open (`emain-wsh.ts:274-278`), which is a real usability trap for a *desktop app* where a developer debugging their own page is a common scenario; an external-CDP-over-devtools-port architecture (which Aleph's obscura/Chromium plan already favors) doesn't have this specific conflict, though it introduces others (port management, multi-tab attach).
- **Hardcoded interactive-tag/role sets in `processCdpSnapshot`** (`emain-wsh.ts:15-16`, `INTERACTIVE_TAGS`/`INTERACTIVE_ROLES`) — a classic enumerate-the-world guard (CLAUDE.md 判据 §5/§3): custom components (`div role="button"` via unusual patterns, web-component shadow-DOM interactive elements) silently fall through to "not interactive," and it's not derived from any authoritative source (no fallback to computed ARIA role or `tabindex`-based heuristics). If Aleph builds an equivalent SoM extractor, derive interactivity from a broader signal set (computed accessibility role via `Accessibility.getFullAXTree`, not just tag/role/contenteditable).
- **No browser-task eval coverage** (§8) — wove's own bench suite never exercises any of these tools, so none of this is regression-tested upstream; treat everything above as "read the code, don't assume it's battle-tested by wove's own CI."

## License / provenance note

Apache-2.0 (top-level `LICENSE`). Repo also carries a `NOTICE` and a `MODIFICATIONS.md` (this is a fork of Wave Terminal, and Wave Terminal's own upstream license terms likely flow through — check both files before porting any literal code, not just design ideas). Nothing here proposes copying code; all borrow-items above are architecture/design patterns only, expressed in Aleph's own Rust core per R1/R3.

