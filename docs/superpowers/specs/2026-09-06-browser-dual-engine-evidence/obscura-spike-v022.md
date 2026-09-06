# obscura v0.2.2 re-measurement spike (THROWAWAY)

Binary: `/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/cfd2843f-43bd-4fe1-9d69-577277c06fcb/scratchpad/obscura-bin/obscura`
`obscura --version` -> **obscura 0.2.2**. Server: `obscura serve --port 9444 --allow-private-network` (PID 73636).
`Browser.getVersion` -> Chrome/145.0.0.0, V8 14.5.0.0, UA `Mozilla/5.0 (X11; Linux x86_64) ... Chrome/145.0.0.0 Safari/537.36`.
Host: darwin 27.0.0 aarch64, Node v24.14.1. Network goes through a fake-ip TUN proxy (github.com -> 198.18.x), so
**navigation timings are network-dominated — do not over-interpret them**. Baseline before start:
`ps aux | rg -c "cliDaemon|obscura serve|aleph-server"` = 3 lines, all from one pre-existing `aleph-server --daemon start`
(PID 63320, not started by me, not touched). No cliDaemon, no obscura at start.
Date: 2026-09-06. All raw JSON/PNG under `.../scratchpad/spike/`.

---

## M1 — Isolate latency (the #1 v0.2.1 blocker)

Command (per site):
```
node m1-isolate.mjs ws://127.0.0.1:9444/devtools/browser <URL> 3
```
Each run: fresh `Target.createTarget` -> `Target.attachToTarget{flatten:true}` -> `Page.enable`/`Runtime.enable` ->
`Emulation.setDeviceMetricsOverride 1280x800` -> `Page.navigate` -> on `Page.loadEventFired` immediately time
`Runtime.evaluate({expression:"1",returnByValue:true})` RTT; then after 8 s; then 5 more at 1 s spacing. Target closed after.

### Runtime.evaluate("1") RTT, milliseconds (obscura v0.2.2)

| site | at loadEventFired (min/med/max of 3 runs) | after 8 s | 5x @1s series (min/med/max of 15) | nav->load ms (min/med/max) |
|---|---|---|---|---|
| news.ycombinator.com | 1 / 1 / 1 | 1 / 1 / 2 | 0 / 1 / 4 | 1765 / 1879 / 2441 |
| github.com | 0 / **3** / 3 | 1 / 1 / 2 | 0 / 1 / 9 | 12373 / 14764 / 19446 |
| en.wikipedia.org/wiki/Rust_(programming_language) | 1 / 1 / 1 | **15574 / 17297 / 19086** | 0 / 1 / 6 | 5335 / 5462 / 5558 |

Raw: `spike/m1-{hn,github,wiki}.json`, per-run stderr in `spike/m1-*.err`.
Per-run github: load 19446/14764/12373 ms, atLoad 3/3/0 ms, after8s 2/1/1 ms.
Per-run wikipedia: atLoad 1/1/1 ms, **after8s 19086/15574/17297 ms** (all three runs).

**github.com is fixed**: v0.2.1 measured 16.8 s at load / 12.4 s after settle; v0.2.2 measures **3 ms / 1 ms**.
**But starvation is not gone — it moved.** Wikipedia stalls hard, reproducibly, in all 3 runs.

### M1b — mapping the stall window (wikipedia)
```
node m1b-window.mjs ws://127.0.0.1:9444/devtools/browser "https://en.wikipedia.org/wiki/Rust_(programming_language)"
```
`Runtime.evaluate("1")` every 500 ms for 45 s starting at `Page.loadEventFired`. `tSinceLoad:rttMs` —
```
0:19  521:25154  26176:1  26678:1  27180:1  27682:1  28184:1 ... 44763:1   (all remaining samples 1-4 ms)
```
One single block of **25,154 ms** starting 521 ms after load; everything before and after is 1-4 ms.
So the engine is still single-isolate and a post-load page script still monopolizes it — the difference vs v0.2.1
is *which* pages trip it and that the window is bounded and one-shot rather than a steady tax.
For an agent loop this is still fatal on the pages that trip it: **the first action after navigation waits ~15-25 s.**

Incidental: obscura enforces a hard navigation deadline — two `m1b` attempts died with
`Page.navigate: Network error: navigation exceeded 30000ms deadline` (network here is a TUN proxy; the deadline
is obscura's, not the harness's).

---

## M2 — JS-walker spatial snapshot (the engine-agnostic path)

Command:
```
node m2-walker.mjs ws://127.0.0.1:9444/devtools/browser obscura <3 urls>     # obscura 0.2.2
node m2-walker.mjs "$(cat chrome-ws.txt)" chrome <3 urls>                    # Chrome 152.0.7977.76 headless=new
```
Both: fresh target, 1280x800 device metrics, navigate, wait `Page.loadEventFired`, sleep 2500 ms, then ONE
`Runtime.evaluate({expression: <IIFE>, returnByValue:true})`. Walker = `spike/walker.js`: every element with
`getBoundingClientRect()` w>0&&h>0 and computed `visibility!=hidden && display!=none`, emitting
`{tag,id,role,name,rect:[x+scrollX,y+scrollY,w,h],interactive}` plus `{scrollX,scrollY,viewport,docHeight}`.

| site | engine | walker RTT ms | elements | interactive | JSON bytes | nav->load ms |
|---|---|---|---|---|---|---|
| news.ycombinator.com | obscura 0.2.2 | **142** | 703 | 191 | 64,179 | 1701 |
| news.ycombinator.com | Chrome 152 | **12** | 775 | 230 | 69,186 | 1056 |
| github.com | obscura 0.2.2 | **1,189** | 1123 | 174 | 112,003 | 18639 |
| github.com | Chrome 152 | **21** | 997 | 114 | 91,204 | 1263 |
| wikipedia Rust | obscura 0.2.2 | **29,043** | 5934 | 1342 | 584,261 | 4655 |
| wikipedia Rust | Chrome 152 | **71** | 7887 | 1766 | 761,747 | 1632 |

Raw: `spike/m2-{obscura,chrome}.json`, element dumps `spike/walk-<engine>-<slug>.json`.

- The wikipedia 29,043 ms is **the M1b stall hitting the walker directly** — the walker was issued 2.5 s after load,
  landing inside the 25 s block. This is exactly the path a spatial-snapshot design would use, so the stall is
  not an abstract engine property; it lands on the feature.
- Even off the stall, obscura's walker is **12x (HN) to 57x (github) slower than Chrome** for the same script.
- Element counts differ 703 vs 775 (HN), 1123 vs 997 (github), 5934 vs 7887 (wikipedia) — obscura's DOM/layout is
  not producing the same visible-element set as Chrome. github is the one where obscura reports *more* elements.

### M2 rect correctness spot-check
Same 5 HN nav links, page coords `[x,y,w,h]`, obscura vs Chrome:

| element | obscura 0.2.2 | Chrome 152 |
|---|---|---|
| "Hacker News" | [130, 11, 83, 15] | [130, 12, 98, 16] |
| "new" | [218, 11, 24, 15] | [233, 12, 27, 16] |
| "past" | [253, 11, 25, 15] | [276, 12, 29, 16] |
| "comments" | [289, 11, 61, 15] | [319, 12, 70, 16] |
| "login" | [1143, 11, 28, 15] | [1139, 12, 32, 16] |
| docHeight | 1173 | 1217 |

Verdict on correctness: **obscura's rects are plausible and self-consistent.** Viewed
`spike/shot-obscura-https_news_ycombinator_com_.png` (`Page.captureScreenshot` on the same target): the nav row sits
at y≈11-26 with "Hacker News" starting at x≈130 and "login" at the right edge — the walker's numbers land on the
pixels obscura actually painted. They do **not** match Chrome's numbers: obscura's text is ~15% narrower and 1 px
shorter per line (different font metrics), so x drifts cumulatively along a row (218 vs 233, 253 vs 276, 289 vs 319).
That is fine for act-on-what-you-read (coordinates are consumed by the same engine that produced them) and fatal for
any cached/cross-engine coordinate reuse.

---

## M3 — CDP-native geometry path (news.ycombinator.com)

Command: `node m3456.mjs <wsUrl> <engine> https://news.ycombinator.com/` (fresh target, 1280x800, 3 s settle after load).

| measure | obscura 0.2.2 | Chrome 152 |
|---|---|---|
| `DOM.getDocument{depth:-1,pierce:true}` ms | 25 | 13 |
| nodes returned (whole tree walked) | 1305 | 1243 |
| element nodes (nodeType 1) | 818 | 818 |
| `DOM.getBoxModel` x200 sequential, total ms | **56** | 50 |
| per call ms | 0.28 | 0.25 |
| failures out of 200 | **0** | 7 (`Could not compute box model.`) |
| `DOM.getContentQuads` on 3 nodes | works, 1 quad each, e.g. `[8,8,1272,8]` | works |
| `Page.getLayoutMetrics` | full Chrome shape incl. `cssLayoutViewport`/`cssContentSize`, contentSize 1280x1173 | full Chrome shape |

`DOM.getBoxModel` returns the Chrome-shaped model (`content,padding,border,margin,width,height`, 8-number quads) —
the v0.2.2 release note holds.

**The CDP-native geometry path is the faster path on obscura on a warm page.** getDocument + 200 box models =
**81 ms** on obscura vs the JS walker's 142 ms (HN) / 1189 ms (github) / 29 s (wikipedia).

> **CORRECTION (see M12).** The sentence that stood here claimed this path "runs in Rust and does not queue behind
> the JS isolate, so it is also immune to the M1 starvation stall." **That was an inference and it is wrong.**
> Measured directly in M12: during the post-load window on wikipedia, `DOM.getDocument`, `DOM.getBoxModel`,
> `Page.getLayoutMetrics` and even `Target.getTargets` all block for the **same ~25-28 s** as `Runtime.evaluate`,
> and in a later window the DOM calls block **19.4 s while `Runtime.evaluate` returns in 1 ms**. The CDP-native
> path is not immune; in one measured window it is the *worse* of the two. The 81 ms above is a warm-page number
> from HN, not a stall-proof number.

Cost: it gives geometry only —
no computed style, no innerText, no aria-label — so name/role/visibility must come from somewhere else.
Caveat on the 0 failures: Chrome refuses a box for 7 of the same 200 nodes; obscura answering for all 200 means it
is *not* rejecting non-rendered nodes, so a caller cannot use "getBoxModel failed" as a visibility signal on obscura.

---

## M4 — AX tree at v0.2.2 (news.ycombinator.com)

| measure | obscura 0.2.2 | Chrome 152 |
|---|---|---|
| `Accessibility.getFullAXTree` ms | 4 | 20 |
| nodes | 1305 | 1624 |
| `role=link` nodes | 229 | 229 |
| ...with non-empty `name.value` | **0** | **198** |
| `ignored` nodes | 0 | 8 |
| raw link node keys | `nodeId, ignored, role, parentId, properties, childIds, backendDOMNodeId` | `nodeId, ignored, role, chromeRole, name, properties, parentId, childIds, backendDOMNodeId` |
| `Accessibility.getPartialAXTree` (backendNodeId of a link) | returns `{}` — no `nodes` key at all | n/a (not run to failure) |

**Unchanged from v0.2.1: 0/229 named links.** obscura's AX nodes carry no `name` field whatsoever — this is not an
empty string, the key is absent. `getPartialAXTree` answers with an empty object rather than an error, which is a
fail-open shape: a caller that does `result.nodes ?? []` gets "no accessibility information" indistinguishable from
"this node has no relatives". Any snapshot generator on obscura must compute accessible names itself from content.

---

## M5 — `Runtime.evaluate` multi-statement

| expression | obscura 0.2.2 | prior v0.2.1 |
|---|---|---|
| `"1; 2"` | **`2`** (no error) | SyntaxError |
| `"var a=1;\nvar b=2;\na+b"` | **`3`** | (would have been SyntaxError) |
| `"(()=>{const a=1;const b=2;return a+b;})()"` | `3` | ok |
| `Runtime.callFunctionOn` with a multi-statement body (`const`/`let`/`for`) | `6` | ok |

**Fixed.** obscura 0.2.2 accepts multi-statement scripts. Chrome gives `2` for the same input. Aleph's
`browser_evaluate` no longer needs IIFE-wrapping for this driver.

---

## M6 — Per-connection target scoping

Connection A holds a target; connection B opens against the same `ws://127.0.0.1:9444/devtools/browser`.

| probe | obscura 0.2.2 | Chrome 152 |
|---|---|---|
| B: `Target.getTargets` | **`[]` (n=0)** | n=4 (2 pages + 2 browser_ui) |
| B: `Target.attachToTarget` with A's targetId | **error `Target not found`** | ok, sessionId `DE2C22AE...` |
| B: after `Target.setDiscoverTargets{discover:true}`, `getTargets` | **still `[]`** | n=4 |

**Unchanged from v0.2.1.** The v0.2.2 note "execution contexts owned by their session" did not change target
visibility across sockets. A second observer connection remains impossible: driver + live-view/human-takeover must
share one Aleph-owned socket.

---

## M7 — Memory (RSS)

Both engines restarted fresh immediately before the idle row. Sampling commands:
```
ps -axo pid,rss,command | rg "obscura serve --port 9444"           | awk '{s+=$2} END {print s}'
ps -axo pid,rss,command | rg -- "--user-data-dir=<spike>/chrome-udd" | awk '{s+=$2} END {print s}'
```
Targets held open by `node m7-mem.mjs`, sampled 2 s after each `Page.loadEventFired` + 3 s settle.

| state | obscura 0.2.2 RSS | Chrome 152 RSS (sum) | Chrome procs | ratio obscura:Chrome |
|---|---|---|---|---|
| idle (about:blank only) | **20,240 KB (20 MB)** | 1,380,544 KB (1,381 MB) | 10 | **1 : 68** |
| + github.com (1 target) | 363,808 KB (364 MB) | 1,435,504 KB (1,436 MB) | 8 | 1 : 3.9 |
| + HN (2 targets) | 286,272 KB (286 MB) | 1,580,176 KB (1,580 MB) | 9 | 1 : 5.5 |
| + wikipedia (3 targets) | 311,472 KB (311 MB) | 1,860,016 KB (1,860 MB) | 10 | **1 : 6.0** |
| all targets closed | 337,952 KB (338 MB) | 1,111,232 KB (1,111 MB) | 7 | 1 : 3.3 |

Caveats that matter for reading this table:
- Chrome's number is a **sum of per-process RSS, which double-counts shared pages** — it overstates Chrome. Take
  the ratio as an upper bound in obscura's favour.
- obscura is one process, so its number is exact.
- obscura's growth is **not per-target**: the first heavy page costs ~344 MB and targets 2 and 3 cost nothing
  measurable (it fell to 286 MB with 2 targets — that is GC, not accounting). Chrome adds ~145 MB and ~280 MB
  per extra target, as expected from process-per-site.
- **Neither engine returns to idle after closing targets.** obscura stayed at 338 MB vs its 20 MB idle: closing a
  target does not give the memory back. For a long-lived Aleph daemon that is the number that matters, not the
  20 MB cold start.

---

## M8 — playwright-cli drop-in against obscura (the least-churn integration path)

Fresh cwd with an empty `.playwright/` dir each time (note: playwright-cli actually writes its artefacts to
`.playwright-cli/`, not `.playwright/`). Endpoint confirmed via `curl http://127.0.0.1:9444/json/version` ->
`webSocketDebuggerUrl: ws://127.0.0.1:9444/devtools/browser`.
```
playwright-cli attach --cdp ws://127.0.0.1:9444/devtools/browser
```

| verb | result | evidence |
|---|---|---|
| `attach --cdp ...` | **works** | `### Browser 'default' opened with pid 95762.` exit 0 |
| `goto https://news.ycombinator.com/` | **works** | reports `Page Title: Hacker News`, writes a snapshot yml |
| `snapshot` | **works, and it is good** | 1073 lines / 54,394 bytes, **608 `[ref=eNN]`**, **223 of 224 links carry a name** ("Hacker News", "new", "past", "comments"), `/url:` per link, HN search box present as `textbox [ref=e612]` |
| `eval "() => document.title"` | **works** | `### Result "Hacker News"` |
| `fill "input[name=q]" "rustlang"` | **works** | follow-up eval returns `"rustlang"` |
| `screenshot` | **works** | writes `.playwright-cli/page-*.png` |
| `tab-new https://example.com` | **works** | new tab opened and became current |
| `click <ref>` (e.g. `e16` from a snapshot taken seconds earlier) | **FAILS** | `Error: Ref e16 not found in the current page snapshot. Try capturing new snapshot.` — reproduced with a freshly captured snapshot |
| `click "a[href='newest']"` (CSS selector) | **half-works then poisons the session** | reports `Error: TypeError: utilityScript.evaluate is not a function`, **but the navigation actually happened** (`tab-list` then shows tab 0 at `https://news.ycombinator.com/newest`) |
| everything after that click (`eval`, `screenshot`, `tab-list`, `click`, even after a fresh `goto`) | **FAILS permanently for that daemon** | `TypeError: utilityScript.evaluate is not a function` / `TypeError: Cannot read properties of undefined (reading 'evaluate')` |
| `close` | NOT MEASURED: session was already poisoned; killed the daemon instead |

**The shape of the failure.** On a fresh daemon the whole battery works — goto, snapshot, eval, fill, screenshot,
tab-new. The break has one trigger: **a click that causes a navigation.** After it, playwright's injected utility
script is gone from the new execution context and is never re-injected, so every JS-backed verb dies for the life
of that daemon. A fresh `goto` does not heal it; only a new `attach` does. Verified on two independent fresh
daemons (pids 95762, 97420) with the same sequence.

Two consequences that decide the integration question:
1. **The AX-name gap does not reach playwright.** Playwright computes accessible names in injected JS, not via
   `Accessibility.getFullAXTree`. So obscura's 0/229 named links (M4) becomes **223/224 named** through
   playwright-cli. The snapshot quality is genuinely good.
2. **But the agent loop is click -> observe -> click.** Breaking on the first navigating click is breaking on
   step one of every real task. As of v0.2.2 playwright-cli over CDP is **not** a usable drop-in for obscura.

Daemons killed after the run: 95762, 97420, 98394 (all started by me; the `.playwright-cli` namespace is
machine-global, so each was killed by pid, not by a blanket sweep).

---

## M9 — `obscura mcp` surface, and `fetch` / `scrape`

```
{initialize, notifications/initialized, tools/list} | obscura mcp     # stdio
```
`serverInfo: {name: "obscura-mcp", version: "0.2.2"}`, protocolVersion 2024-11-05, capabilities `{tools:{}}`.
**37 tools.** Full dump in `spike/mcp.out`. The ones that matter for the agent-facing question:

| tool | description (verbatim) |
|---|---|
| `browser_interactive_elements` | "List every clickable / typeable element on the current page with a stable ref ID and a brief description. Use this BEFORE clicking or filling so you can refer to elements by ref instead of guessing a CSS selector. **Refs look like 'e3' and stay valid until the next navigation.**" (props: `limit`) |
| `browser_snapshot` | "Get the current page content as text (title, URL, and readable body text)" (props: `max_chars`, default 4000) |
| `browser_markdown` | "Extract the current page as Markdown (headings, paragraphs, lists, links, code blocks). Use this instead of browser_snapshot when you want token-dense structured content rather than plain text." |
| `browser_links` | "List every anchor link on the current page as one JSON object per line: {text, href}" |
| `browser_extract` | "Extract a structured object from the page given a map of {field_name: css_selector}" incl. `sel@attr` and `field[]` list syntax |
| `browser_detect_forms` / `browser_fill_form` | form structure discovery; multi-field fill in one call with `type='text'\|'check'\|'uncheck'\|'select'` |
| `browser_storage_state` / `browser_set_storage_state` | export/restore cookies + localStorage + sessionStorage as JSON |
| `browser_screenshot`, `browser_pdf` | PNG of the CSS viewport; paginated raster PDF |

Rest: `browser_navigate, _click, _fill, _type, _press_key, _select_option, _evaluate, _wait_for, _wait_for_text,
_search, _count, _get_attribute, _scroll, _back, _forward, _reload, _network_requests, _console_messages,
_get_cookies, _set_cookie, _clear_cookies, _tab_new, _tab_list, _tab_switch, _tab_close, _close`.

**There is no spatial/geometry tool.** `browser_interactive_elements` gives refs and descriptions but the schema
carries no rectangles, so obscura's own agent API does not offer the coordinate snapshot the live-view design wants.
Note also the honest limitation in its own description: refs "stay valid until the next navigation" — the same
lifetime problem that broke playwright's refs in M8.

### `obscura fetch` / `obscura scrape`
`fetch --dump` accepts `html | text | links | markdown | original | assets | cookies`; `--file` for batch,
`--concurrency`. `scrape` takes multiple URLs, `-e/--eval`, `--format` (default `json`), `--concurrency 10`,
`--timeout 60`.

**But `fetch` could not reach any external URL on this machine:**
```
obscura fetch https://news.ycombinator.com
  -> Error: Failed to navigate ...: Network error: Network error: https://news.ycombinator.com/:
     error sending request for url (https://news.ycombinator.com/)      [exit 1, 0 bytes stdout]
obscura fetch --dump text https://example.com          -> same error
obscura fetch --allow-private-network --dump markdown http://127.0.0.1:18999/probe.html
  -> Page loaded: ... - "probe"   [works, emits markdown]
```
`obscura serve` navigates to those exact URLs successfully in the same minute, and there are no proxy env vars set.
So **`fetch`/`scrape` use a different HTTP stack than `serve`, and only the `serve` stack copes with this
machine's TUN proxy.** First 40 lines of external fetch output: NOT MEASURED — every external fetch returned
0 bytes on stdout. Local fetch markdown output is in `spike/` (probe.html is nearly empty, so it is not a
useful fidelity sample).

---

## M10 — Rendering fidelity (obscura 0.2.2, 1280x800, `Page.captureScreenshot` png)

```
node m10-shots.mjs ws://127.0.0.1:9444/devtools/browser obs <6 urls>
```
Fresh target per site, 1280x800 device metrics, wait `Page.loadEventFired`, sleep 4000 ms, capture. PNGs in
`spike/m10-obs-*.png`, page facts in `spike/m10-obs.jsonl`. Each viewed with the Read tool.

| site | nav ms | png bytes | verdict |
|---|---|---|---|
| github.com | 6870 | 78,772 | **recognizable-with-artifacts** — big improvement on v0.2.1 (no more double-painted "Puull requests"). Nav bar, hero type, and copy are correct. Defects: the two hero CTAs are painted **overlapping** ("Sign up for GitHub" on top of "Download GitHub Copilot app"), the hero media is missing and its alt text is painted as body copy, "Skip to content" bleeds at the top edge. |
| news.ycombinator.com | 1746 | 166,359 | **near-Chrome** — indistinguishable at this size. |
| en.wikipedia.org/wiki/Rust_(programming_language) | 3924 | 144,897 | **recognizable-with-artifacts, one serious layout bug** — the article body column is squeezed to roughly 100 px so prose wraps at one or two words per line, and the infobox overflows past the right edge of the viewport. Chrome renders the body full width. Table/flex width computation is wrong here. |
| react.dev | 3037 | 56,047 | **near-Chrome** — logo, type, buttons all correct. One defect: the "Learn" nav item and the search box's "⌘K" chip are painted on top of each other. |
| www.amazon.com | 967 | 251,236 | **recognizable-with-artifacts** — full nav, search, category cards, product images all render. Defects: a ~240 px blank band where the hero carousel should be, and two stray loading spinners overlapping a card. |
| x.com | 3927 | 74,663 | **near-Chrome** — the standard signed-out "Happening now" wall with all four sign-in buttons and the X mark, correctly laid out. This is what Chrome shows logged-out too, **not** a bot block. |

**No anti-bot or captcha page was hit on any of the six.** Therefore the `--stealth` re-run on port 9445 was
**NOT MEASURED: nothing was blocked, so there was nothing to compare against.**

Recurring defect class across three of six sites: **two elements painted on top of each other** (github CTAs,
react.dev nav, amazon spinners). Layout is close but overlap/stacking is not resolved the way Chrome resolves it.

### Bonus defect found while checking M10 page facts
obscura reports `document.body.innerText.length` = **3,477,253** on github.com; Chrome reports ~10 k for the same
page. Cause: **obscura's `innerText` includes `<style>` and `<script>` text.** The `<html>` element's innerText
starts `.turbo-progress-bar {\n  position: fixed;...` on obscura vs `Skip to content\nJoin GitHub Copilot Day...`
on Chrome. This directly poisons the M2 walker's `name` field, which falls back to `innerText.slice(0,80)`:
every container element on obscura gets a CSS-source name instead of its text. Any JS-walker snapshot design
must filter `style`/`script`/`noscript` itself rather than trusting `innerText`.

---

## Verdict

> Sections **M11** and **M12** were requested after this verdict was first written and appear **below** it.
> Both changed conclusions above: M12 retracted "the CDP-native path is immune to the stall" (corrected in place
> at M3 and in the bullets below), and M11 added that `DOMSnapshot.captureSnapshot` returns fabricated geometry.

- **Blocker 1 (isolate starvation): still stands, in a narrower form.** github.com is fixed — `Runtime.evaluate("1")`
  went from 16.8 s (v0.2.1) to **3 ms** at load and 1 ms after settle. But wikipedia stalls **15.6 / 17.3 / 19.1 s**
  at t+8 s in 3 of 3 runs, and a 500 ms-resolution sweep shows one contiguous **25,154 ms** block starting 521 ms
  after `loadEventFired`. `obscura 0.2.2`, `node m1-isolate.mjs ws://127.0.0.1:9444/devtools/browser <url> 3`.
- **Blocker 2 (AX names): unchanged. 0 of 229 `role=link` nodes carry a name** on HN; Chrome 152 gives 198/229.
  obscura's AX nodes have no `name` key at all, and `Accessibility.getPartialAXTree` answers `{}` rather than an
  error — a fail-open shape. `Accessibility.getFullAXTree` itself is fast (4 ms vs Chrome's 20 ms).
- **Blocker 3 (per-connection target scoping): unchanged.** A second socket sees `Target.getTargets` = `[]` and
  `Target.attachToTarget` -> `Target not found`, even after `setDiscoverTargets`. Chrome shows 4 targets and attaches
  fine. Driver + live-view must share one Aleph-owned connection. The v0.2.2 "contexts owned by their session" note
  did not touch this.
- **Fixed since v0.2.1:** multi-statement `Runtime.evaluate` (`"1; 2"` -> `2`, no IIFE needed), `DOM.getBoxModel`
  returns the Chrome-shaped model, and github-class pages no longer stall.
- **The JS-walker spatial snapshot is viable only off the stall, and its `name` field is wrong as specified.**
  RTT 142 ms (HN) / 1,189 ms (github) / 29,043 ms (wikipedia, stalled) vs Chrome's 12 / 21 / 71 ms. Rects are
  **correct against obscura's own rendering** (verified against a screenshot) but differ from Chrome by ~15% text
  width, so coordinates are only valid on the engine that produced them. `innerText` leaks CSS/JS source, so names
  need filtering obscura does not do.
- **~~Prefer the CDP-native geometry path to dodge the stall~~ — RETRACTED, see M12.** The geometry path is fast
  on a warm page (`DOM.getDocument{depth:-1,pierce:true}` 25 ms + 200x `DOM.getBoxModel` 56 ms = **81 ms** on HN,
  0 failures) but it is **not immune to the stall**: measured concurrently, `DOM.*`, `Page.getLayoutMetrics` and
  `Target.getTargets` block for the same 25-28 s as `Runtime.evaluate` after load, and block **19.4 s while
  `Runtime.evaluate` returns in 1 ms** on a cold-but-settled page. **There is no stall-proof path.** The geometry
  path also carries no name/role/visibility, and unlike Chrome it never refuses a box, so "getBoxModel failed"
  cannot be used as a visibility signal.
- **Geometry honesty (M11): `getBoundingClientRect` and `DOM.getBoxModel` are both correct and byte-identical on
  all 5 probed elements, and both match the screenshot pixels. `DOMSnapshot.captureSnapshot` is fabricated** — every
  bound is `[0, nodeIndex*18, 1280, 18]` (distinct widths across the first 200 entries: `{1280}`; distinct heights:
  `{18}`). It returns a Chrome-shaped `bounds` array containing no layout information, in 3 ms, with nothing in the
  response to mark it as fake. Any obscura integration must hard-avoid `DOMSnapshot`.
- **playwright-cli over CDP is NOT good enough to reuse Aleph's existing driver.** attach, goto, snapshot, eval,
  fill, screenshot, tab-new all work — and the snapshot is genuinely good (608 refs, **223/224 links named**,
  because playwright computes names in injected JS and so routes around blocker 2). But `click <ref>` never
  resolves (`Ref e16 not found in the current page snapshot` on a freshly captured snapshot), and `click <selector>`
  performs the navigation and then **permanently poisons the daemon**: every JS-backed verb afterwards returns
  `TypeError: utilityScript.evaluate is not a function`, not healed by a fresh `goto`. Reproduced on two fresh
  daemons. Since the agent loop is click -> observe -> click, **Aleph would need a native CDP client for obscura.**
- **Memory ratio obscura : Chrome = 1 : 68 idle (20 MB vs 1,381 MB) and 1 : 6.0 with 3 pages loaded
  (311 MB vs 1,860 MB).** Chrome's figure is a sum of per-process RSS and double-counts shared pages, so 1:6 is an
  upper bound in obscura's favour. Caveat: obscura does not return memory on target close (338 MB vs a 20 MB idle),
  and its cost is not per-target — the first heavy page buys the whole ~344 MB.
- **`obscura fetch`/`scrape` are on a different, weaker network stack than `serve`.** Every external fetch failed
  (`error sending request for url`) while `serve` navigated the same URLs in the same minute with no proxy env set.
  Local fetch works. Its own MCP surface is broad (37 tools, incl. `browser_interactive_elements`, `browser_markdown`,
  `browser_extract`, storage-state export) but carries **no geometry/rect tool**, so it does not supply the spatial
  snapshot the live-view design needs.
- **Top 3 reasons not to pick obscura as default:** (1) the starvation stall still exists, lands on the first
  action after navigation on real pages (25 s on wikipedia), and — per M12 — **hits every CDP domain including
  `DOM.*` and `Target.getTargets`, so no snapshot design can route around it**; (2) no working third-party driver,
  playwright breaks on the first navigating click, so the integration is a from-scratch native CDP client, not a
  swap; (3) rendering and DOM semantics are close-but-wrong in ways that silently corrupt a snapshot — overlapping
  paint on 3 of 6 sites, wikipedia's body column squeezed to ~100 px, an `innerText` that returns CSS source, and a
  `DOMSnapshot` that returns invented rectangles without saying so.

### Cleanup
Started and killed by me: obscura serve 73636 then 92250; headless Chrome 88001 then 92253; playwright-cli daemons
95762, 97420, 98394; `python3 -m http.server 18999` 74226. The pre-existing `aleph-server` PID 63320 was never touched.

---

## M11 — Real vs fabricated geometry: four sources for the same 5 elements

obscura 0.2.2, `obscura serve --port 9444 --allow-private-network` (PID 6107), news.ycombinator.com, 1280x800,
2.5 s settle after `Page.loadEventFired`.
```
node m11-geom.mjs ws://127.0.0.1:9444/devtools/browser obs3
```
Sources: (a) `Runtime.evaluate` -> `getBoundingClientRect()`; (b) `DOM.querySelector` then `DOM.getBoxModel`,
`content` quad reduced to `[x, y, w, h]`; (c) `DOMSnapshot.captureSnapshot({computedStyles:[]})`, element located by
`backendNodeId` -> index in `documents[0].nodes.backendNodeId` -> entry in `documents[0].layout.nodeIndex` ->
`layout.bounds`; (d) pixel position read off `Page.captureScreenshot` PNG (`spike/m11-obs3-shot.png`) with the Read tool.
Raw: `spike/m11-obs3.json`, full snapshot `spike/m11-obs3-domsnapshot.json`.

| element (rendered text) | (a) getBoundingClientRect | (b) DOM.getBoxModel | (c) DOMSnapshot bounds | (d) screenshot |
|---|---|---|---|---|
| "Hacker News" (`a[href="news"]`) | [130, 11, 83, 15] | [130, 11, 83, 15] | **[0, 486, 1280, 18]** | bold text at x≈130-212, y≈11-25 |
| first story "Cloud in a Bottle: making self-hosting accessible to everyone" | [136, 44, 359, 15] | [136, 44, 359, 15] | **[0, 1224, 1280, 18]** | link at x≈136-495, y≈44-59 |
| "160 comments" (`span.subline > a:last-of-type`) | [304, 61, 61, 10] | [304, 61, 61, 10] | **[0, 1710, 1280, 18]** | text at x≈304-365, y≈61-71 |
| "login" | [1143, 11, 28, 15] | [1143, 11, 28, 15] | **[0, 918, 1280, 18]** | text at right edge, x≈1143-1171, y≈11-26 |
| footer search `input[name=q]` | [589, 1144, 153, 21] | [589, 1144, 153, 21] | **[0, 23454, 1280, 18]** | NOT MEASURED: y=1144 is below the 800 px viewport and `captureScreenshot` captures the viewport only |

**Which agree with the screenshot: (a) and (b), exactly. (c) is fabricated.**

- **(a) and (b) are byte-identical on all five elements** — not merely close. `DOM.getBoxModel` returns
  sub-pixel quads (e.g. `[1142.836, 11.121, 1170.999, ...]` for login) that round to exactly the
  `getBoundingClientRect` integers. Both land on the painted pixels in the screenshot. Neither returned the
  documented constant fallback quad `[8,8,108,8,108,28,8,28]` for any of the five.
- **(c) `DOMSnapshot.captureSnapshot` is a synthesized vertical stack, exactly as the source survey said.**
  Proof independent of the five elements: across the first 200 layout entries the set of distinct widths is
  `{1280}` and the set of distinct heights is `{18}`, and the first ten bounds are
  `[0,0,1280,18], [0,18,1280,18], [0,36,1280,18], [0,54,1280,18], ...` — y is simply `nodeIndex * 18`. Confirm on the
  five: 486 = 27x18, 918 = 51x18, 1224 = 68x18, 1710 = 95x18, 23454 = 1303x18, and each equals the element's own
  node index times 18. The call is also suspiciously cheap (3 ms for 1305 layout nodes) because it computes nothing.
  It is not "approximate" or "stale" — it carries **no layout information at all** while presenting a Chrome-shaped
  `bounds` array. A consumer cannot tell from the response that it is fake.

So the JS `getBoundingClientRect` path **is** honest on obscura, and `DOM.getBoxModel` is honest too and agrees with
it. Only `DOMSnapshot` lies. This upgrades M3's recommendation: the CDP-native geometry path is not just faster,
its numbers are verified correct against both the JS path and the pixels — provided the geometry comes from
`DOM.getBoxModel` and **never** from `DOMSnapshot.captureSnapshot`.

Caveat on the fallback quad: I did not observe `[8,8,108,8,108,28,8,28]` in this run, so I cannot say from
measurement how often `DOM.getBoxModel` degrades to it. Related and relevant: in M3, obscura returned a box for
**all 200** nodes where Chrome refused 7, so obscura does not signal failure by erroring. If the constant quad is
the failure mode, a caller must detect it by value, not by an error. **NOT MEASURED: the frequency and trigger of
the constant fallback quad.**

Not a defect, recorded because I initially misread it as one: `td.subtext a:last-of-type` resolves to the age link
("8 hours ago"), not the comments link. That is correct CSS — the age anchor is the only anchor inside `span.age`,
so it is the first document-order match for `a:last-of-type`. obscura's selector engine is right; my first selector
was wrong. Verified by enumerating the row's anchors: `["zplizzi", "8 hours ago", "hide", "160 comments"]`.

---

## M12 — Does `DOM.*` wait during the stall? (correcting the M3 inference)

obscura 0.2.2, PID 6107, `https://en.wikipedia.org/wiki/Rust_(programming_language)`, 1280x800.
```
node m12-lock.mjs   ws://127.0.0.1:9444/devtools/browser   # ticks from loadEventFired
node m12d-settled.mjs ws://127.0.0.1:9444/devtools/browser # same five calls, 45 s after load
```
`<body>` nodeId resolved once before the stall (nodeId 69, resolved in 77-78 ms). Each tick fires **all five calls
concurrently** via `Promise.all` and records each RTT independently.

**Why the first attempt was invalid, and what it would have told me.** My first version issued the five calls
*sequentially*. It produced `evaluate 25407, getDocument 5, getBoxModel 8, getLayoutMetrics 23657, getTargets 0` —
which reads exactly like "DOM.* is immune". It is an artefact: `Runtime.evaluate` went first, absorbed the whole
block, and the DOM calls then ran in the clear. **A sequential probe cannot answer this question**, because
whichever call is first pays for all the others. Raw in `spike/m12.err`.

### Concurrent ticks, RTT in ms

| tSinceLoad ms | Runtime.evaluate | DOM.getDocument | DOM.getBoxModel | Page.getLayoutMetrics | Target.getTargets |
|---|---|---|---|---|---|
| 77 (run A) | **24,884** | **24,884** | **24,892** | **24,892** | **24,892** |
| 25,470 (run A) | 1 | **23,372** | **23,372** | **23,372** | **23,372** |
| 79 (run B) | **28,079** | **28,079** | **28,088** | **28,088** | **28,088** |
| 28,668 (run B) | 1 | **25,942** | **25,942** | **25,942** | **25,942** |
| 45,012 (run C, first touch after 45 s of connection idle) | 1 | **19,352** | **19,352** | **19,352** | **19,352** |
| 64,865 (run C) | 1 | 7 | 7 | 7 | 7 |
| 65,374 (run C) | 1 | 7 | 7 | 7 | 7 |
| 65,883 (run C) | 2 | 8 | 8 | 8 | 8 |
| 66,393 (run C) | 1 | 7 | 7 | 7 | 7 |

Raw: `spike/m12b.err` (run A), `spike/m12c.err` (run B), `spike/m12d.err` (run C), `spike/m12-lock.json`.

**Confirmed: the M3 sentence was wrong, and I have corrected it in place at M3.** `DOM.*` is not immune.

What the numbers actually say:
- **In the post-load window every method blocks together, for the same duration, and releases together** — 24,884 /
  24,892 in run A, 28,079 / 28,088 in run B. The spread across five different domains is 8 ms out of ~25,000 ms.
  They are waiting on one shared thing that is released once, not queueing serially behind each other (serial
  queueing would give staggered completion times, not identical ones).
- **`Target.getTargets` stalls too.** If it is on a lock-free allowlist in the source, that allowlist is not
  protecting it in practice at v0.2.2. Whatever the mechanism, there is no method in this set that stays responsive.
- **The asymmetry runs the *other* way from the M3 guess.** At t=25.5 s, t=28.7 s and t=45.0 s, `Runtime.evaluate`
  returns in **1 ms** while the four DOM/layout/target calls block for **19-26 s** in the very same tick. So on a
  settled-but-cold page the JS path is the responsive one and the CDP-native geometry path is the one that hangs.
- **The stall is not one bounded window after load.** Run C sat idle for 45 s and its first touch still cost
  19.4 s. Only after that did everything drop to 1-8 ms and stay there across four consecutive ticks.

Consequence for the design question: **neither geometry path is stall-proof on obscura, so "pick the CDP path to
dodge the stall" is not an available move.** The 81 ms figure in M3 is a warm-page HN number. On a large page the
first geometry call after any quiet period can cost ~19 s, and during the post-load window everything costs ~25 s.

Mechanism NOT MEASURED: I did not instrument obscura, so I cannot say from this data whether the shared thing is
`ctx.v8_lock`, a layout/style recalculation, or connection-level head-of-line blocking. The 1 ms `Runtime.evaluate`
sitting beside a 19 s `DOM.getDocument` in the same tick is evidence *against* a pure single-v8-lock story and
*against* pure head-of-line blocking on the socket, but distinguishing the remaining candidates needs the source
or a build with tracing.

### M12b — the dense 500 ms series (supersedes the thin table above)

The table above has only two rows per run because each tick *awaited* its five calls, and a stalled tick eats
25 s of a 40 s window. Re-run on a **fixed 500 ms schedule that does not await the previous tick**, so the sampling
window is real: 80 ticks x 5 calls = **400 samples, 0 errors, 0 unfinished**.
```
node m12e-dense.mjs ws://127.0.0.1:9444/devtools/browser     # obscura 0.2.2, PID 15625
```
`<body>` resolved to nodeId 69 (after `loadEventFired`; the pre-load attempt loses the race on this page — the
document is not queryable until load, so "resolve before load" is NOT ACHIEVABLE here). Page identity asserted
before sampling (`location.href` contains wikipedia.org) — an earlier run silently navigated nowhere and measured
about:blank at 1-3 ms, so the script now retries navigation and aborts on the wrong page.

RTT ms, one row per 500 ms tick (abridged; full 80 rows in `spike/m12e2.err`, raw in `spike/m12e-dense.json`):

| tSinceLoad | evaluate | getDocument | getBoxModel | layoutMetrics | getTargets |
|---|---|---|---|---|---|
| 0 | 25,727 | 25,750 | 25,749 | 25,749 | 25,750 |
| 501 | 25,248 | 25,248 | 25,248 | 25,248 | 25,248 |
| 1,001 | 24,749 | 24,748 | 24,748 | 24,748 | 24,748 |
| 5,002 | 20,748 | 20,748 | 20,748 | 20,748 | 20,748 |
| 10,001 | 15,769 | 15,769 | 15,769 | 15,769 | 15,769 |
| 15,000 | 10,790 | 10,790 | 10,790 | 10,790 | 10,790 |
| 20,001 | 5,789 | 5,789 | 5,789 | 5,789 | 5,789 |
| 25,001 | 806 | 806 | 806 | 806 | 806 |
| 25,500 | 306 | 306 | 306 | 306 | 306 |
| **26,000** | **15** | **23,420** | **23,420** | **23,420** | **23,420** |
| 26,500 | 43,981 | 43,981 | 43,981 | 43,981 | 43,981 |
| 30,000 | 40,481 | 40,481 | 40,481 | 40,481 | 40,481 |
| 35,001 | 35,499 | 35,499 | 35,499 | 35,499 | 35,499 |
| 39,500 | 31,000 | 31,000 | 31,000 | 31,000 | 31,000 |

**The RTT column is misleading on its own; the completion instants are the finding.** All 400 calls finish at just
three moments:

| completion instant (ms since load) | calls finishing there |
|---|---|
| 25,727 - 25,807 | 260 |
| 49,420 | 4 |
| 70,481 - 70,500 | 135 |

Every call issued between t=0 and t=25,500 completes at **t≈25,750 regardless of when it was issued** — which is
why RTT counts down linearly (25,727 / 25,248 / 24,749 / ... / 306). That is not per-call cost, it is **one global
barrier releasing everything at once**. The same again for the second barrier at t≈70,481.

And the release is not method-specific. Per method, the number of calls finishing at each barrier is essentially
identical across all five:

| method | @25.7-25.8 s | @49.4 s | @70.5 s |
|---|---|---|---|
| Runtime.evaluate | 51 + 1 at 26,015 | 0 | 27 |
| DOM.getDocument | 52 | 1 | 27 |
| DOM.getBoxModel | 52 | 1 | 27 |
| Page.getLayoutMetrics | 52 | 1 | 27 |
| Target.getTargets | 52 | 1 | 27 |

**Answer: all five stall, together, for the same duration — including `Target.getTargets`.** Two barriers on this
page: ~25.8 s starting at load, and a second ~44 s barrier starting at t≈26.5 s. The only sample that escaped is a
single `Runtime.evaluate` issued at t=26,000, in the 250 ms gap between the two barriers, which returned in 15 ms
while its four tick-mates were caught by the second barrier and waited 23.4 s.

This is consistent with the source survey's `ctx.v8_lock` at dispatch.rs:653-717 being taken by every `DOM.*`
method, and shows the lock's reach is wider still, since `Page.getLayoutMetrics` and `Target.getTargets` are gated
too. I did not instrument the binary, so **NOT MEASURED: which lock it actually is.** What is measured is the
behaviour: on obscura 0.2.2 there is no CDP method in this set that stays responsive while a page script runs.
