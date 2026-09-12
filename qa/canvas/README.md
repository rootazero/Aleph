# `qa/canvas/` — whiteboard canvas real-machine QA

Boots a real `aleph-server` in a throwaway root and prints the ten-item
manual checklist (items 1–9 from the implementation plan's Task 20, item 10
added for the gallery and rewritten for the right-pane body). Unlike its siblings,
this fixture **boots and waits** rather than driving scenarios itself: every
item below is about live Panel behaviour — broadcast latency between two
tabs, optimistic-lock conflict recovery, fullscreen presentation — so the
driving hand is a browser (plus chrome-devtools-mcp when you want the
assertions machine-checked). Each item already carries its effect assertion.

## What the fixture guarantees

* **Isolated `HOME` *and* `ALEPH_HOME`** — the server can neither read nor
  write the operator's real `~/.aleph`. (`ALEPH_HOME` alone is not enough:
  some libraries consult `HOME`; see `qa/README.md`.)
* **Mock provider, no vault** — `qa/busy_input/patch_config.py` rewrites the
  generated config to exactly one provider whose `api_key` is **inline in the
  config file** (`ProviderConfig.api_key` is `skip_serializing` but still
  deserializes), pointed at `qa/busy_input/mock_anthropic.py`. The run costs
  nothing and reaches no network.
* **Build before the `HOME` redirect** — cargo's registry lives under the
  real `HOME`; building after the redirect silently re-downloads the world.
* **Idempotent** — every invocation mints a fresh `mktemp` scratch root and
  removes it on exit (`KEEP=1` keeps it); ports are env-overridable
  (`GATEWAY_PORT`, `MOCK_PORT`) so runs never collide.

## Prerequisites

* A Panel build on disk: `just wasm` (debug servers read
  `interfaces/webchat/dist/` from disk — an empty dist serves a blank page
  and every item "fails" for the wrong reason; the script refuses to start
  without it).
* `python3` on PATH (mock provider + config patcher).

## Run

```bash
./qa/canvas/run.sh          # boot, print checklist, wait until Ctrl-C
KEEP=1 ./qa/canvas/run.sh   # keep the scratch dir for post-mortem
```

Then open `http://127.0.0.1:18798` (or your `GATEWAY_PORT`) and work the
checklist. Loopback connections are always operator — no credentials needed
for items 1–7 and 9.

**Where the canvas is (since 2026-09-12):** there is no `/canvas` route and no
sidebar entry any more. Stay on the chat route, open the workspace pane with
the `LayoutToggle` at the chat surface's top-right, and pick the **Canvas** tab
in the pane's header — it is one of the pane's bodies (`WorkspaceBody::Canvas`),
next to Artifacts (single agent) or Deliverables / Tasks (team). The library
is the picker popover on the canvas header strip. `/canvas` in the address bar
lands on the chat route.

## The checklist (plan Task 20, verbatim — 每条带效果断言)

> 真机清单（chrome-devtools-mcp 执行，**每条带效果断言**）:

1. 建画布→画矩形/便签/画笔→刷新页面→内容还在（持久化）
2. 双标签页：A 画一笔 B 实时出现；B 移动形状 A 实时跟随（广播）
3. A/B 同时拖同一形状→一端收冲突→自动重拉不丢另一端改动（乐观锁）
4. 对话里让模型 `canvas(action='create')` + `insert_html`→Panel 实时弹出新画布内容（工具面+事件面）
5. AI 图片框全流程（mock provider 返回固定 data URL 图）→框被图替换
6. 标注重生成：标注→提交→模型插新图于原图旁
7. Slides：三帧组 deck→播放→翻页→Esc
8. member 角色（0.0.0.0 + 自签 TLS + 局域网 IP，配方见 memory）看不到 operator 的私有画布；房间画布双方可见可编辑
9. PNG 导出落文件且可打开
10. 右栏体 + picker + 自动弹开 + resizer + 绘图基础设施（断言全文见下方「Item 10」）

## Item 3's residual: driving the REAL conflict window

Two MCP-driven tabs can never lose the optimistic-lock race on loopback —
frame propagation is <100 ms, so serial driving always reconciles before the
next send and item 3 only ever verifies convergence, not the conflict arm.
`latency_proxy.py` manufactures the window on the genuine wire by delaying
**upstream traffic only** (tab A's sends arrive late; broadcasts still reach
A instantly — which simultaneously pins that an in-flight batch is not
rebased by a broadcast arriving after send):

```bash
python3 qa/canvas/latency_proxy.py 18799 18798 2500   # proxy → gateway
# tab A: http://127.0.0.1:18799   (through the proxy — sluggish by design)
# tab B: http://127.0.0.1:18798   (direct)
```

Open the same canvas in both, edit a shape in A, then **within the delay
window** move the same shape in B. B lands first; A's in-flight
`canvas.apply` arrives stale; the proxy prints `CONFLICT FRAME SEEN` when
the `REVISION_CONFLICT` refusal crosses the downstream half (positive proof
the arm fired), and the effect assertions are: A recovers without a reload,
both edits survive in both tabs, and doc.json holds both (revision advanced
past both commits). No config change needed — the `/ws` origin policy allows
any loopback origin regardless of port. Verified 2026-08-17: see spec §8.

The oracle scans the **stream**, not each TCP chunk. It used to test
`marker in chunk`, so a 6-byte needle split across a 64 KiB read boundary
was missed — rare enough never to show up in a run, and quietly unsound the
whole time. `ConflictScanner` now carries `len(marker) - 1` bytes across
reads: every occurrence is found, none twice. `python3
qa/canvas/latency_proxy.py --self-test` drives the boundary case (and four
others) directly, so the claim is falsifiable rather than asserted.
The remaining honest caveat is narrower than the old one: the oracle reads
plaintext, so it is blind to a downstream frame that was compressed
(`permessage-deflate`) or otherwise re-encoded — if the line never appears,
cross-check doc.json revisions before concluding no conflict occurred.

## The request-log oracle (always on)

`run.sh` wires `mock_anthropic.py`'s 5th argument unconditionally:
`$QA_ROOT/request_log.jsonl` receives every request body the mock saw, one
JSON object per line. The `tool_result` blocks inside are the only ground
truth for "did the model's canvas call actually commit" — the one anomaly
this fixture ever produced (an in-run `insert_image` that never committed,
spec §8) was unattributable precisely because this log was off. When
restarting only the mock (items 4–6 recipe below), keep the argument.

## Items 4–6: making the mock "drive" canvas tools

`mock_anthropic.py` emits **one fixed tool call per tool turn** (its
`tool_spec` argument). That is enough for the effect assertions — "the frame
is replaced by the image", "a new canvas pops up live" — because the
assertion is about the tool face + event face, not about model reasoning.
The live ids are only known after you create the canvas/frame in the Panel,
so the loop is:

1. In the Panel, create the canvas (and for item 5 an AI image frame), note
   the ids (`canvas.list` over RPC, or the frame panel).
2. Write a spec file, e.g. item 5 (a fixed 1×1-px data-URL image):

   ```json
   {"name": "canvas",
    "input": {"action": "insert_image",
              "canvas_id": "<cv-…>", "frame_id": "<frame-id>",
              "location": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="}}
   ```

3. Restart **only the mock** with the spec (the server keeps running, state
   survives — `run.sh` prints the exact command with the live PID/port).
4. Press Generate (item 5) / send the chat message (items 4/6) and watch the
   canvas update live.

For item 4 the spec is `{"action": "insert_html", "canvas_id": …,
"html": "<h1>hello</h1>"}`; for item 6 an `insert_image` with `x/y/w/h`
instead of `frame_id`. A real provider also works — edit the scratch config's
provider section by hand — but then the run dials out, which is exactly what
the mock recipe exists to avoid.

## Item 8: the member role (verified 2026-08-17)

Loopback is **always operator** — presenting a member bootstrap ticket over
`127.0.0.1` still answers `role: "operator"` (verified on a live wire; that
is the trust model, not a bug) — so member visibility can only be tested
from the machine's LAN IP. The seeding half is now executable,
`member_seed.py`; the recipe:

* set `[gateway] host = "0.0.0.0"` **and** `[gateway.tls] enabled = true`
  in the scratch config (the plaintext gate refuses a bare LAN bind) and
  restart the server;
* `python3 qa/canvas/member_seed.py <port> --tls` over loopback — creates
  the member user, a project room with the member on the roster, an
  operator-private canvas, a room canvas, and a one-time bootstrap ticket;
  it prints a ready-to-open member URL at the LAN IP. Every step is
  find-or-create: the server has no natural key for any of them
  (`display_name`, project name and canvas title are all presentation
  labels with no uniqueness constraint — correctly, since nothing resolves a
  principal by name), so the first version left a duplicate "QA Member" and,
  worse, a duplicate canvas pair behind whenever a run died halfway and was
  retried — which silently breaks the operator control group's *counting*
  assertion below. The reused ids are listed in the script's output;
* open that URL in a browser, click through the self-signed-cert
  interstitial (TOFU), and assert (all held on 2026-08-17): the member's
  library shows ONLY the room canvas; the private canvas id over the
  member's wire answers `-32009 not found` **byte-shaped like a truly
  nonexistent id** (no-oracle); the room canvas is editable from both sides
  and `canvas.updated` reaches both live, no reload.

This scenario deliberately boots loopback-only; the LAN bind is a config
edit away, kept out of the default run so an unattended QA box never opens
a port by accident.

## Item 10: the right-pane body, picker, auto-reveal, resizer and the drawing infrastructure (rewritten 2026-09-12)

The canvas is a **body of the chat's workspace pane** (`WorkspaceBody::Canvas`,
`interfaces/webchat/src/state/layout.rs`; mechanism in CANVAS.md §6). The
gallery is a picker popover on the canvas header strip and the same list is
inlined in the welcome pane. Everything below is driven in one browser tab on
the chat route unless a step says otherwise.

**Status: the browser half has NOT been run.** The 2026-09-12 round was built
on a Linux box with no wasm toolchain; every assertion below is backed only by
the native unit tests named in FEATURE_LOCATOR §6.10 (block "画布入右栏"). The
gallery assertions marked *(held 2026-08-17)* passed on the left-column host
and are host-independent by construction — they still need one run on the
picker host. Run the list, then replace this paragraph with the date.

Several of the new wire fields have **no human editor input**: `bend`,
`head_start` / `head_end`, `reveal`, `timeline` and the `path` shape are
written by the `canvas` tool or raw JSON-RPC only (CANVAS.md §6). Drive them
the way the "rename while drawing" recipe does: open a second `WebSocket` to
`/ws` **from inside the page** and send `canvas.apply` from there (an
out-of-process client blurs the window, which commits any open rename and
ends that sub-test). `base_revision` is the `revision` the page's own
`canvas.get` last returned; a conflict answers `-32031` and is your cue that
the page applied something in between.

### Pane and picker

* **Reachability** — on the chat route, with the pane collapsed, the
  `LayoutToggle` opens it on the last body; the header tab strip lists
  Artifacts + Canvas (single agent) or Deliverables + Tasks + Canvas (team).
  `/canvas` typed into the address bar lands on the chat route, not a 404 and
  not a stale canvas page. Switching a team conversation to a single-agent one
  while on Tasks shows Artifacts (fallback), and switching back shows Tasks
  again (the stored body is not overwritten).
* **Cold load** *(held 2026-08-17 on the sidebar host)* — reload with the
  pane open on Canvas and nothing open: the welcome pane's inline list reads
  *Loading…* first and *No canvases yet* only after `canvas.list` answers.
  Only a real socket shows the transition; the unit tests hold `rows_loaded`
  but not the timing.
* **Picker** — the header trigger reads *Library* with nothing open and the
  open canvas's title otherwise. Click it: the popover lists rows; clicking a
  row opens that canvas in the body **and closes the popover**; clicking the
  backdrop closes it without opening anything. The filter box, an armed
  delete and an open rename are all dropped when the popover closes (they are
  component-local by design).
* **Rename, surface 1** *(held)* — in the popover, hover a row, pencil, type,
  Enter: the row title changes and so does the header title if that canvas is
  open. Refuse cases: an empty title and a 300-character title keep the input
  open with a red reason and must NOT reach the wire (hijack
  `WebSocket.prototype.send` and count `canvas.apply`: zero).
* **Rename, surface 2** *(held)* — click the header title, type, Enter: same
  effect, and the popover row follows on next open. Escape on either surface
  reverts without a request.
* **Rename while drawing** *(held)* — with a canvas open and its row's rename
  input open in the popover, apply five batches from the in-page second
  socket. Assert all four: the row's meta line advances (so the test is not
  vacuous), `document.contains(rowNode)` stays true, the input is still
  `document.activeElement`, `selectionStart` is where you parked it.
* **Search / delete** *(held)* — title and id both match; a query matching
  nothing reads *No canvases match* (a different sentence from *No canvases
  yet*); clearing restores the server's most-recently-updated-first order.
  Deleting the open canvas drops the body back to the welcome pane.

### Keep-alive body

* Open a canvas, draw three shapes, pan and zoom somewhere recognisable, select
  one shape. Switch the tab strip to Artifacts, then back to Canvas. Assert:
  the camera is where you left it (read the world `<g>`'s CSS transform before
  and after), the selection is intact, and Ctrl/Cmd+Z still undoes the third
  shape — the body was hidden with `style:display`, never unmounted. Hijack
  `WebSocket.prototype.send` across the switch: **zero** `canvas.list` and
  zero `subscribe` frames (the three liveness wires did not re-run).
* Collapse the pane with the `LayoutToggle`, then reopen: same three
  assertions.

### Auto-reveal (the agent-driven half — D0)

Use the items 4–6 mock recipe: a tool spec that makes the mock call
`canvas(action='create', ...)` on every tool turn.

* **Reveals** — with the pane collapsed, send a chat message. When the tool
  call completes the pane opens (`aside.aleph-workspace-pane` loses
  `workspace-collapsed`) on the Canvas tab with the new canvas open — both
  `open_canvas` and `body` moved, not just one (the "effect reached" check).
* **Does not re-open an open canvas** — with that canvas open and the pane
  open, send again with a spec that names that `canvas_id` in an `apply`.
  Assert the editor's `<svg>` node identity survives (`document.contains`)
  and the camera did not reset; the new shape arrives through the
  `canvas.updated` reconciler, not through a refetch (zero `canvas.get` on
  the hijacked socket).
* **Already-open pane only switches the body** — put the pane on Artifacts
  (open), send again: the body switches to Canvas and
  `localStorage['aleph.panel.layout_mode']` is not rewritten (watch
  `Storage.prototype.setItem`).
* **Muted for the rest of the run after the user collapses** — start a run
  whose mock makes several canvas calls (one call per tool turn with a
  multi-turn plan), collapse the pane with the `LayoutToggle` after the first
  reveal: later canvas results in the **same** run do not reopen it. The
  next run's first canvas result reopens it (the mute is per run).
* **Replay never reveals** — collapse the pane, switch to another
  conversation and back (or reload): the history contains canvas tool
  results and the pane stays collapsed. Wired at the live `agent_trace` arm
  only, never in `apply_trace_event`.
* **Foreground only** — with two conversations, have the mock run in
  conversation A while you are reading conversation B: B's pane does not pop
  open.
* **Transcript reachability** — a `canvas` tool row in the transcript shows
  *Open in canvas*; clicking it opens the pane on that canvas (same path as
  the auto-reveal). A `list` row or a refused call shows no such button.

### Resizer

* Drag the pane's left edge: the pane, the chat surface's right padding and
  the `LayoutToggle` all follow the same edge (three readers of
  `--aleph-workspace-w`, published on `<html>`'s inline style —
  `document.documentElement.style.getPropertyValue('--aleph-workspace-w')`
  reads a px value during the drag).
* **Persists on release, not during** — watch `Storage.prototype.setItem`:
  no write while the pointer moves, one write of
  `aleph.panel.workspace_w` on pointer-up. Reload: the width is back.
* **Clamps** — drag past 80% of the viewport: the pane stops at 80%; drag
  narrower than 280px: it stops at 280. Both from `clamp_width`.
* **Double-click resets** — the inline property is **removed** (not set to
  40%), the key is gone from `localStorage`, and the pane is back at the
  CSS default.
* **Capture** — start a drag on the 6px handle and move the pointer far
  outside it (over the chat, outside the window): the drag continues until
  release (`setPointerCapture`). This is the one resizer assertion no unit
  test can hold.

### Drawing infrastructure

* **Style panel is the writer** — pick red, fill on, size large, stroke
  dashed; draw a rect, a note and an arrow. `canvas.get(detail=full)` (or the
  outgoing `canvas.apply`) shows every one of them carrying
  `style.color="red"`, `fill=true`, `size="large"`, `stroke="dashed"` —
  nothing is born `ShapeStyle::default()` any more. Type `#336699` into the
  color input: accepted; the swatch row shows nothing selected (a hex is not
  a slot). Every value the color input emits passes `check_color`, so no
  apply is ever refused for color.
* **Four new forms** — diamond, triangle, hexagon, pill each have a tool
  button; the rect has square corners now (no `rx`).
* **Sketch is identical across tabs** — set stroke to sketch, draw a rect.
  In a second tab open the same canvas and read that shape's `<path d>`:
  byte-identical to the first tab's (seeded from the shape id). Move the
  shape in tab A: the wobble does not re-roll (only the position changes).
* **Bent arrow renders as an arc** — from the in-page socket, apply an arrow
  with `bend: 40`, `head_start: "dot"`, `head_end: "bar"`. The shaft is a
  `<path>` of cubic segments curving off the chord; a dot at the start and a
  bar at the end. Apply `bend: -40`: mirrored across the chord. Export PNG
  and animated SVG of it: the exported shaft is the same curve (shared
  `arrow_geom`).
* **Path shape** — apply `{type:"path", d:"M0 0 L80 40 Q120 0 160 40 Z",
  closed:true}` with a filled style: renders at its `x,y` as a filled
  outline; `d:"M0 0 A 10 10 0 0 1 20 20"` is refused (`A` is outside the
  subset) and nothing lands (revision unchanged).
* **Play draws on** — apply two shapes with `reveal: {start_ms: 0,
  duration_ms: 1000}` and `reveal: {start_ms: 1000, duration_ms: 1000, mode:
  "fade"}`. The *Play* button appears only now (hidden while nothing has a
  reveal). Press it: the first outline draws on over ~⅔ s then its fill
  fades in; the second fades in from 1 s; after ~2 s the surface returns to
  static rendering. Press Play again: both restart from t=0 (the epoch
  bump). Pause freezes in place. In presentation mode (a deck of frames) Play
  replays the current frame only and changing slides stops it.
* **Animated SVG export** — select the two revealed shapes, *Export animated
  SVG*: the downloaded file opens in a browser and plays on open; the same
  export with no revealed shape in the selection is byte-identical to the
  plain SVG the PNG path serialises.
* **Snapping** — draw two rects; move one so its left edge comes within ~8
  screen px of the other's left edge: it snaps and a vertical guide line
  spans both boxes; the guide is 1px at any zoom (`vector-effect`). Release:
  the guide disappears and the committed position is the snapped one (read
  the outgoing `canvas.apply`). Hold Alt/Option while dragging: no snap, no
  guide. Resize handles, ink and arrow drags never snap.

### What the browser cannot reach: `title_gate_probe.py`

`check_title` refuses three things, and a browser can only produce two of
them. `<input type="text">` runs the DOM's value-sanitization algorithm, which
**strips CR and LF outright** — a person typing into the rename box can never
submit a newline, so the control-character arm is unreachable from the Panel
by construction. (Discovered the honest way: the browser pass "failed" that
case by renaming a canvas to `onetwo`, which is exactly what a sanitized
single-line input should do.)

That arm exists for the other two writers — the `canvas` tool and any raw
JSON-RPC client — and `python3 qa/canvas/title_gate_probe.py [port] [--tls]`
is what exercises it, on a live wire, against both writers. It also pins the
property that makes the gate worth having: a refused `SetDocMeta` leaves the
document *and its revision* untouched, so nothing half-lands and a rejected
batch costs nobody a revision. Twelve assertions; run it after any change to
the gate or to `ops_shape`.

Verified 2026-08-17: 12/12 PASS.

## Where the automated half lives

The wire-level assertions that do NOT need a browser are already automated
in `tests/canvas_wire.rs` (`cargo test -p alephcore --features test-helpers
--test canvas_wire`): contract key-set equality over the real handlers, the
AI-template tool-name resolution guard, and owner/member/stranger event
visibility over a typed bus subscription.
