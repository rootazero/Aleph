# Chrome two-CDP-connection spike (throwaway; 2026-09-05)

Question: can a SECOND CDP connection screencast + inject input into the page that
`playwright-cli` 0.1.8 is driving, and which side must launch Chrome for that to work.

Machine: macOS (darwin 27.0.0), Node v24.14.1, Python 3.14.2,
Google Chrome 152 at `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`,
`playwright-cli` 0.1.8 at `~/.local/share/fnm/node-versions/v24.14.1/installation/bin/playwright-cli`.

Contrast with the sibling obscura spike (`SPIKE-FINDINGS.md`): obscura scopes targets
per CDP connection, so a second connection could not attach at all. This file asks the
same question of real Chrome.

## STEP 0 — probe page served

```
cd $S && (python3 -m http.server 18999 --bind 127.0.0.1 > http.log 2>&1 &)
curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:18999/probe.html
```
=> `200`. Server pid seen via `pgrep -fl "http.server 18999"`.

`probe.html` has: a red 300x200 `#box` that counts its own clicks and toggles colour,
a CSS spinner (continuous repaint), a `#clock` div rewritten every 100 ms, an `#name`
text input, and 3000 px of scrollable gradient.

## STEP 1 — Aleph-launches-Chrome: two CDP connections on one page

```
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new \
  --remote-debugging-port=0 --user-data-dir=$S/chrome-udd \
  --no-first-run --no-default-browser-check about:blank > $S/chrome.log 2>&1 &
cat $S/chrome-udd/DevToolsActivePort   # 58363 \n /devtools/browser/ac5f508a-...
node probe-chrome.mjs "ws://127.0.0.1:58363/devtools/browser/ac5f508a-..." http://127.0.0.1:18999
```

`probe-chrome.mjs` ran unmodified. Connection A stands in for playwright-cli (drives),
connection B is the human-observer (screencast + takeover). Both are separate WebSockets
to the SAME browser endpoint, both `Target.attachToTarget{flatten:true}` on the SAME page
target `996D80D4…`.

| key | value |
|---|---|
| `version` | `Chrome/152.0.7977.64` |
| `A_session` | targetId `996D80D414BED20854B8376D75B67AB2`, sessionId `AC83180F…` |
| `B_getTargets` | 7 targets visible to B, **including** A's page (`type:page`, `url:about:blank`, `attached:true`) plus service_worker / browser_ui / background_page |
| `B_session` | sessionId `EDF5913CE6730FA95F275A3C5C936636` (different session, same target) |
| `B_startScreencast` | `ok` |
| `A_nav_probe` | 1018 ms |
| `B_frames_during_A_nav` | 59 frames, avg 10336 B |
| `B_frames_idle_animation_3s` | 180 frames (= 60 fps), avg 10441 B |
| `A_click_to_B_frame_ms_x5` | `[0, 2, 7, 9, 13]` (see caveat) |
| `box_count_after_A_clicks` | `"5"` — A's 5 clicks all landed |
| `B_click` | `ok` |
| `box_count_after_B_click` | `"6"` — **B's click landed on the page A drives** |
| `B_insertText` | `ok` |
| `value_seen_by_A` | `"中文 hello"` — **B's `Input.insertText` (CJK) readable by A** |
| `scrollY_after_B_wheel` | `600` — B's `mouseWheel` scrolled the page |
| `A_nav_hn` | `https://news.ycombinator.com/` in 2729 ms |
| `B_saw_frameNavigated` | `["https://news.ycombinator.com/"]` — B observes A's navigations |
| `B_frames_across_nav` | 17 frames, avg 16547 B — screencast survives A's navigation, no restart needed |
| `B_screencast_alive_after_nav` | `true` |
| `A_eval_rtt_ms_with_B_streaming` | `2` ms |
| `chrome_ax` | 1624 nodes, 229 links, **198 named**, sample `["FAQ","Lists","API"]` |
| `A_alive_after_B_detach` | `"Hacker News"` — A unaffected by B's `Target.detachFromTarget` + socket close |

No errors anywhere in the run. Raw: `probe-chrome.out`, `results-chrome.json`.

### Latency caveat and a clean re-measure

`probe.html` animates continuously (CSS spinner + a 100 ms clock interval), so frames arrive
at 60 fps whether or not anything was clicked. `A_click_to_B_frame_ms_x5 = [0,2,7,9,13]` is
therefore "time to the next frame of a free-running stream", not click→visible-change.

`probe-latency.mjs` (written this step) quiesces the page first
(`for(let i=1;i<9999;i++)clearInterval(i)` + remove the spinner) so the screencast becomes
event-driven, then measures again:

| key | value |
|---|---|
| `frames_idle_3s_quiesced` | **0** — screencast is strictly damage-driven, idle cost is zero |
| `A_click_to_B_frame_ms_quiesced` | `[16, 15, 14, 11, 14]` ms |
| `B_click_to_B_frame_ms_quiesced` | `[19, 13, 17, 14, 14]` ms |
| `frames_3s_animated` (spinner restored) | 180 (60 fps) |
| `A_eval_rtt_ms_x5_while_B_streams` | `[0, 0, 0, 0, 0]` ms |

So: ~11–19 ms click→frame either direction, and A's command RTT is unmeasurably small
(0 ms) while B streams 60 fps. **No measurable interference between the two connections.**

Bandwidth: 10.3–10.4 KB/frame at 1280x800 jpeg q60 on the probe page, 16.5 KB on Hacker News.
At the 60 fps ceiling that is ~620 KB/s–1 MB/s; on a static page it is 0.

### Chrome memory, one tab open (on news.ycombinator.com)

`ps -o rss=` over the 8 live `chrome-udd` processes:

```
browser        268 MB
gpu-process    144 MB
utility        102 MB
utility         81 MB
renderer       178 MB
renderer       148 MB
renderer       152 MB
renderer       102 MB
------------------------
total         1175 MB
```

(An earlier sum over 9 processes read 1304 MB.) macOS RSS double-counts shared framework
pages across Chrome's processes, so treat this as an upper bound, not working set. It is
still a real number to plan around: a resident Chrome is ~1 GB of RSS in `ps`, not ~200 MB.
Note four renderers for one visible tab — headless Chrome 152 still starts component
extensions (`chrome-extension://nkeimhogjdpnpccoofpli…`, `nmmhkkegccagdldgiimed…`) and
`browser_ui` targets (`chrome://omnibox-popup.top-chrome/`).

Killed with `pkill -f chrome-udd`.

## STEP 2 — playwright-cli launches Chrome (what Aleph does today)

### The CLI surface

`playwright-cli --help`: sessions are selected with the global `-s=<session>` prefix
(`playwright-cli -s=spike2 open …`). `--config <path>` is per-command, defaults to
`.playwright/cli.config.json`. Relevant verbs: `open [url]`, `attach [name]`,
`close`, `goto`, `click <target>`, `snapshot [element]`, `eval`, `tab-list`,
`tab-new/close/select`, `screenshot`, `resize`, `mousemove/mousedown/mouseup/mousewheel`,
`press/keydown/keyup`, and session-wide `list` / `close-all` / `kill-all`.

`playwright-cli attach --help` is the important one:

```
Options:
  --cdp        connect to an existing browser via cdp endpoint url.
  --endpoint   playwright browser server endpoint to attach to.
  --extension  connect to browser extension, optionally specify browser name
  --session    session name (defaults to bound browser name or "default")
```

So the CDP-attach path is a first-class flag, not a config hack.

### It does NOT refuse a user-supplied --remote-debugging-port

```
cat > $S/pw-args.json <<'JSON'
{"browser":{"launchOptions":{"headless":true,"args":["--remote-debugging-port=0"]}}}
JSON
playwright-cli -s=spike2 open --config $S/pw-args.json http://127.0.0.1:18999/probe.html
```

Result — **no refusal**, verbatim:

```
### Browser `spike2` opened with pid 44142.
### Ran Playwright code
```js
await page.goto('http://127.0.0.1:18999/probe.html');
```
### Page
- Page URL: http://127.0.0.1:18999/probe.html
- Page Title: probe
```

The believed rejection ("Playwright manages remote debugging connection itself") **did not
occur** on playwright-cli 0.1.8 / playwright-core v1.60.0-alpha-2026-04-14.

Chrome's argv (`ps -ww -o command= -p 44143`), relevant tail:

```
--remote-debugging-port=0 --remote-debugging-port=58419
--user-data-dir=/var/folders/…/T/playwright_chromiumdev_profile-EbGYZy
--remote-debugging-pipe --no-startup-window
```

Both our `=0` and Playwright's own `=58419` are present; Chrome takes the last occurrence,
so **Playwright's port wins and ours is ignored — you cannot pin the port this way.**

### The bigger finding: the port is there even WITHOUT our config

```
playwright-cli -s=spike3 open http://127.0.0.1:18999/probe.html   # no --config at all
ps -ww -o command= -p <chrome pid> | tr ' ' '\n' | grep remote-debugging
  --remote-debugging-port=58447
  --remote-debugging-pipe
curl -s http://127.0.0.1:58447/json/version   # 200, returns webSocketDebuggerUrl
```

playwright-cli **already** launches Chrome with a random free `--remote-debugging-port`
(in addition to the pipe it actually drives over). Aleph needs no launch-config change today.

### Discovery of that port is the weak link

- The port is random per launch.
- `DevToolsActivePort` is **absent** from Playwright's profile dir
  (`/var/folders/…/playwright_chromiumdev_profile-*/DevToolsActivePort` → *No such file or
  directory*), unlike the Aleph-launched Chrome in STEP 1 where it was written.
- `playwright-cli list` prints session name, status, browser-type, user-data-dir
  (`<in-memory>`) and an "attach" hint — **but no CDP endpoint**.

So the only discovery route is scraping `ps` argv of the Chrome child, which is racy
(the port isn't listening the instant the process appears) and platform-specific.

### Two external CDP connections on Playwright's page

```
WS=$(curl -s http://127.0.0.1:58419/json/version | jq -r .webSocketDebuggerUrl)
node probe-chrome.mjs "$WS" http://127.0.0.1:18999      # -> probe-pwchrome.out
```

`/json/list` before the run showed exactly one `page` (`.../probe.html`, title `probe`)
plus two `browser_ui` omnibox targets.

| key | value |
|---|---|
| `version` | `Chrome/152.0.7977.76` |
| `A_session` | targetId `3921C485…` (Playwright's own page) |
| `B_session` | `908F55B6…` — a **second** WS session on the same target |
| `B_startScreencast` | `ok` |
| `B_frames_during_A_nav` | 55 frames, avg 10414 B |
| `B_frames_idle_animation_3s` | 181 frames (60 fps) |
| `A_click_to_B_frame_ms_x5` | `[26, 0, …]` (animated page — see STEP 1 caveat) |
| `box_count_after_A_clicks` | `"5"` |
| `box_count_after_B_click` | `"6"` |
| `value_seen_by_A` | `"中文 hello"` |
| `scrollY_after_B_wheel` | `600` |
| `B_saw_frameNavigated` | `["https://news.ycombinator.com/"]` |
| `B_frames_across_nav` | 2 frames, avg 89141 B |
| `A_eval_rtt_ms_with_B_streaming` | `1` ms |
| `A_alive_after_B_detach` | `"Hacker News"` |

That is **three concurrent CDP clients on one page**: playwright-cli over the pipe,
plus my A and B over the WebSocket port. Everything worked; nobody was evicted.

### playwright-cli survived and stayed in sync

After the probe navigated the page to Hacker News behind Playwright's back:

```
playwright-cli -s=spike2 tab-list
### Result
- 0: (current) [Hacker News](https://news.ycombinator.com/)

playwright-cli -s=spike2 goto http://127.0.0.1:18999/probe.html   # worked, snapshot written
```

Playwright tracked the externally-driven navigation correctly and kept driving.

`playwright-cli -s=spike2 close` → `Browser 'spike2' closed`, and `pgrep -f "Google Chrome"`
returned 0 processes: **closing the session kills the browser it launched.**

## STEP 3 — Aleph launches Chrome, playwright-cli attaches over CDP

Fresh Chrome, same launch line as STEP 1 but `--user-data-dir=$S/chrome-udd2`.
`DevToolsActivePort` → port `58467`, path `/devtools/browser/b813aa0f-…`.
Targets before anything attached: **6 total, 1 page (`about:blank`)**.

### The config is accepted

```
cat > $S/pw-cdp.json <<JSON
{"browser":{"cdpEndpoint":"http://127.0.0.1:58467"}}
JSON
playwright-cli -s=spike4 open --config $S/pw-cdp.json
### Browser `spike4` opened with pid 48144.
### Ran Playwright code
```js
await page.goto('about:blank');
```
```

The **http form worked**; the ws fallback was not needed for the config. No new Chrome was
spawned (`pgrep -f playwright_chromiumdev_profile` → 0), and `pgrep -f chrome-udd2` still
showed the Aleph-launched one.

**It reused the existing page, it did not create one**: pages before = 1, pages after = 1
(total targets 6→4, the two `chrome-extension://…` `background_page` event pages having
idled out on their own — unrelated to Playwright). Confirmed independently below: the
observer attached to the pre-existing page id `619CFFDB…` and then watched the CLI drive
*that* page.

⚠️ `open` with no url issues `page.goto('about:blank')` on the reused page, i.e. it
**clobbers whatever the page was showing**. `attach --cdp` (below) does not.

### Observer streams while the CLI drives — window 1 (30 s)

`observe.mjs` (written this step, reusing the `Cdp` class from `probe-chrome.mjs`) attaches
flatten to the FIRST page target, enables Page, starts a jpeg q60 1280x800 screencast,
acks every frame, and samples target list + url once a second.

```
node observe.mjs "ws://127.0.0.1:58467/devtools/browser/b813aa0f-…" 30 &
playwright-cli -s=spike4 tab-list
playwright-cli -s=spike4 goto http://127.0.0.1:18999/probe.html
playwright-cli -s=spike4 snapshot
```

| key | value |
|---|---|
| `observedTargetId` | `619CFFDB8C655A6276A2515CD6CC4809` |
| `startScreencast` | `ok` |
| `frames` / `kb` over 30 s | **1619 frames / 10043 KB** |
| `navs` | `[{t: 3086, url: "http://127.0.0.1:18999/probe.html"}]` — the CLI's `goto`, seen by the observer |
| `detached` | `[]` |
| targets/pages start→end | 4→4, 1→1 |

Timeline: 1 frame in the first 3 s (static `about:blank`), then a steady 60 fps
(60–61 frames/s) for the remaining 26 s once the animated probe page loaded.
`tab-list` at t+3 reported `0: (current) [](about:blank)` — the same page the observer
was attached to.

`snapshot` ref syntax:

```yaml
- generic [active] [ref=e1]:
  - generic [ref=e2]: "0"
  - generic [ref=e4]: "--"
  - textbox "name" [ref=e5]
```

so refs are bare `eN` and `click <target>` takes the ref without brackets.

### Observer streams while the CLI clicks and navigates cross-origin — window 2 (25 s)

```
node observe.mjs "<wsUrl>" 25 &
playwright-cli -s=spike4 click "e2"
playwright-cli -s=spike4 --raw eval "() => document.getElementById('box').textContent"
playwright-cli -s=spike4 goto https://example.com
playwright-cli -s=spike4 tab-list
```

- `click "e2"` → `await page.getByText('0', { exact: true }).click();`
- `eval` → `"1"` — the click landed on the red box.
- `goto https://example.com` → `### Page URL: https://example.com/`
- `tab-list` → `0: (current) [Example Domain](https://example.com/)`

Observer: **541 frames / 3261 KB**, `navs = [{t: 9535, url: "https://example.com/"}]`,
`detached = []`, targets/pages 4→4 and 1→1. Timeline shows 60 fps on the probe page
through s4, frames continuing through the navigation (s5–s9: 301→514), then **0 frames**
for s10–s25 because example.com is static.

Two things worth flagging: the screencast **survived a cross-origin navigation**
(127.0.0.1 → example.com, which swaps renderer process) with the same session and no
`Target.detachedFromTarget`; and the idle cost on the static page is exactly zero frames.

### Lifecycle: Aleph owns the browser

```
pgrep -f chrome-udd2 | wc -l     # 9 before
playwright-cli -s=spike4 close   # "Browser 'spike4' closed"
pgrep -f chrome-udd2 | wc -l     # 9 after — UNCHANGED
curl -s http://127.0.0.1:58467/json/version   # still 200
curl -s http://127.0.0.1:58467/json/list      # total 4, pages 1, page still https://example.com/
playwright-cli list                            # spike4 is gone from the list
```

Under `cdpEndpoint`, `close` disconnects the client and leaves the browser, the page, and
the page's state untouched. Contrast STEP 2, where `playwright-cli -s=spike2 close` left
`pgrep -f "Google Chrome"` at **0**.

### Re-attach works, and two CLI sessions can share one Chrome

```
playwright-cli -s=spike5 attach --cdp "http://127.0.0.1:58467"
### Browser `spike5` opened with pid 50940.
### Page URL: https://example.com/     ### Page Title: Example Domain

playwright-cli -s=spike6 attach --cdp "ws://127.0.0.1:58467/devtools/browser/b813aa0f-…"
### Browser `spike6` opened with pid 50949.
### Page URL: https://example.com/     ### Page Title: Example Domain
```

Both the **http and ws forms** of `--cdp` are accepted. Re-attach found the page exactly
where it was left, and unlike `open`, `attach` did **not** navigate it. Two independent
playwright-cli sessions were attached to the same browser at the same time without
either being evicted.

## STEP 4 — cleanup

```
playwright-cli kill-all
  Killed daemon process 50949
  Killed daemon process 50940
  Killed 2 daemon processes.
pkill -f chrome-udd; pkill -f chrome-udd2; pkill -f playwright_chromiumdev_profile
pkill -f "http.server 18999"; pkill -f cliDaemon
```

Confirmed with `pgrep`, all zero: `chrome-udd` 0, `chrome-udd2` 0,
`playwright_chromiumdev_profile` 0, `Google Chrome` 0, `http.server 18999` 0, `cliDaemon` 0.

`playwright-cli list` shows only `alephprobe5` (status closed), which pre-dated this spike.
Nothing under `/Volumes/TBU4/Workspace/Aleph` was read, written, or built.

---

## Verdict

### (a) Does a second CDP connection get frames and inject input on a page another connection drives?

**Yes, unambiguously, and with no measurable interference.** Real Chrome 152 permits many
concurrent CDP clients per browser and many `flatten:true` sessions per page target. Both a
second and a third client attached to the page a first client was driving; every client saw
the full target list including the driven page, and none was evicted. The observer connection
started a screencast (`Page.startScreencast` → `ok`) and received frames throughout the other
connection's navigations, clicks and a cross-origin page swap. Input injected by the observer
landed and was read back by the driver: a synthetic click bumped the page's own counter from
`"5"` to `"6"`, `Input.insertText` put `"中文 hello"` into the input element, and a
`mouseWheel` moved `window.scrollY` to `600`. Latency, measured on a deliberately quiesced
page so the stream is damage-driven rather than free-running at 60 fps, was **11–16 ms**
driver-click→observer-frame and **13–19 ms** observer-click→observer-frame. Interference is
effectively nil: the driver's `Runtime.evaluate` round-trip measured **0 ms** five times over
while the observer pulled 60 fps, and `2 ms` and `1 ms` in the two full probe runs. Cost is
**10.3–10.4 KB per frame** at 1280x800 jpeg q60 (16.5 KB on Hacker News, 89 KB on a
mid-navigation frame), so roughly **620 KB/s–1 MB/s at the 60 fps ceiling and exactly zero on
a static page** — the screencast is strictly damage-driven, which measured as literally 0
frames in 3 s on a quiesced page and 0 frames for 16 consecutive seconds on example.com. The
stream survived same-origin *and* cross-origin navigation (which swaps renderer process) with
the same session id and no `Target.detachedFromTarget`, and the driver was unaffected when
the observer detached and closed its socket (`A_alive_after_B_detach` → `"Hacker News"`).
This is the exact opposite of the sibling obscura result, where targets are scoped per CDP
connection and a second connection could not attach at all.

### (b) Can playwright-cli-launched Chrome expose a debug port, or must Aleph launch Chrome?

**It already does, today, with no config change — but the port is not discoverable, which is
what forces the other direction.** The believed refusal never happened: passing
`{"browser":{"launchOptions":{"args":["--remote-debugging-port=0"]}}}` produced a normal
`### Browser 'spike2' opened` and no complaint. More to the point, launching with **no config
at all** still produced `--remote-debugging-port=58447` in Chrome's argv next to Playwright's
own `--remote-debugging-pipe`, and that port answered `/json/version` with a live
`webSocketDebuggerUrl`. Three problems make it unusable as a contract. The port is random per
launch. Your own switch cannot pin it: both `--remote-debugging-port=0` (ours) and
`=58419` (Playwright's) appear in argv and Chrome takes the last, so Playwright's always
wins. And nothing publishes the value — `DevToolsActivePort` is **absent** from Playwright's
profile directory (present in the Aleph-launched case), and `playwright-cli list` prints
session name, status, browser-type and user-data-dir but no endpoint. The only discovery
route left is scraping `ps` argv of the Chrome child, which is racy and platform-specific.
So: not *must*, but *should*. **Aleph launching Chrome and handing playwright-cli a
`cdpEndpoint` is the arrangement to build on** — Aleph then knows the port because it chose
it, gets a `DevToolsActivePort` file, and controls the user-data-dir. Both the `http://` and
`ws://…/devtools/browser/<id>` forms of `--cdp` were accepted.

### (c) Who owns Chrome's lifecycle in the cdpEndpoint arrangement?

**Whoever launched it, and under cdpEndpoint that is Aleph, completely.** With playwright-cli
launching, `close` took `pgrep -f "Google Chrome"` to 0 — the CLI owns and destroys the
browser. With `cdpEndpoint`, `playwright-cli -s=spike4 close` printed
`Browser 'spike4' closed` and left `pgrep -f chrome-udd2` at 9 processes, unchanged; the
endpoint kept serving, and the page was still sitting on `https://example.com/` with its
state intact. The session simply vanished from `playwright-cli list`. Re-attaching afterwards
with `attach --cdp` found that same page, same URL, same title. So `close` degrades to
"disconnect", Aleph decides when Chrome dies, and a crashed or restarted playwright-cli
costs nothing but a reconnect. That is the right ownership split for a live-view surface that
must outlive individual automation runs.

### (d) Surprising things

The capability we were going to ask for is already shipping and merely hidden: playwright-cli
opens a debug port on every launch and tells nobody. Duplicate `--remote-debugging-port`
switches resolve last-wins in Chrome's favour of Playwright, so `launchOptions.args` is not a
lever. `open --config <cdpEndpoint>` issues `page.goto('about:blank')` on the page it reuses,
silently clobbering whatever was displayed — **use `attach --cdp`, not `open`, when handing
over an existing browser**; `attach` left the page untouched. Two playwright-cli sessions
attached to one Chrome simultaneously without conflict, and playwright-cli's `tab-list`
stayed correct after an *external* CDP connection navigated its page to Hacker News behind
its back, then kept driving normally — Playwright tracks out-of-band navigation rather than
desynchronising. Chrome 152's `Accessibility.getFullAXTree` computes accessible names
(**198 of 229 links named**, e.g. `["FAQ","Lists","API"]`), where the obscura spike measured
0 of 229 and would have needed a name-from-content implementation. And headless Chrome 152
is not cheap: 8–9 processes and **~1175 MB summed RSS** for one tab, with four renderers,
component extensions and `chrome://omnibox-popup.top-chrome/` `browser_ui` targets running
despite `--headless=new` (macOS `ps` RSS double-counts shared framework pages, so treat this
as an upper bound, but the process count is real).

## What I could NOT verify

- **Headed Chrome.** Every run used `--headless=new`. Screencast behaviour in a headed
  window, and a real human's physical mouse/keyboard interleaving with CDP-injected input,
  are untested. This matters for "take control" — trusted vs untrusted event handling and
  focus can differ.
- **Platforms.** macOS only. No Windows or Linux.
- **Simultaneous input contention.** The two connections never dispatched input at the same
  instant; I alternated. Whether interleaved `Input.dispatchMouseEvent` from two sessions can
  produce a torn drag or a lost mouseup is unknown.
- **Port-discovery timing.** I read Playwright's port from `ps` after the fact. I never
  measured how long after spawn the port becomes connectable, so the race window for the
  scrape-argv approach is unquantified.
- **Version stability.** `playwright-core v1.60.0-alpha-2026-04-14`. Whether the unprompted
  `--remote-debugging-port` survives future versions is unknown — it looks incidental, not
  contractual, which is a reason not to depend on it.
- **Multi-tab.** One page target throughout. Tab create/close by one connection as observed
  by the other, and how the observer should follow tab switches, were not exercised.
- **Frame content.** I counted frames and bytes; I never decoded a JPEG to confirm the pixels
  showed the expected state. The 11–19 ms is CDP event latency, not glass-to-glass, and
  excludes encode/transport to a browser-based viewer.
- **Long-run stability and backpressure.** Longest continuous stream was 30 s with prompt
  acking. Behaviour over minutes, or when the viewer acks slowly, is unmeasured.
- **Security.** The debug port binds 127.0.0.1 but is unauthenticated; any local process can
  drive the browser. Not assessed against Aleph's trust model.
- **Flag interactions.** `--headed`, `--persistent`, `--profile` combined with `cdpEndpoint`
  were not tried, nor `attach --extension` / `--endpoint`.
- **Aleph itself.** No repo file was read, changed, or compiled; nothing here is integrated
  or proven against the real `src/browser/` code path.
