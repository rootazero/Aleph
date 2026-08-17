# `qa/canvas/` — whiteboard canvas real-machine QA

Boots a real `aleph-server` in a throwaway root and prints the nine-item
manual checklist from the implementation plan (Task 20). Unlike its siblings,
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

## Item 10: the left-column gallery (added 2026-08-17, verified same day)

The library moved out of the main area into `ModeSidebar`, so the assertions
that used to be "the list page renders" are now about a list that coexists
with the open board. Drive it in one browser tab. All of the below held on 2026-08-17 against a
live server; the two worth knowing how to drive are noted inline.

* **Cold load** — open `/canvas` on a fresh reload with the socket still
  connecting: the list must read *Loading…*, and only after `canvas.list`
  answers may it read *No canvases yet*. Seeing "no canvases" first is the
  bug this state exists to prevent, and it is only observable on a real
  socket — the unit tests hold `rows_loaded` but not the timing.
* **Open + highlight** — click a row: the board opens in the main area, the
  row takes the active tile styling, and the row list stays put (no
  navigation away from the list, which was the whole point).
* **Rename, surface 1** — hover a row, click the pencil, type, Enter: the row
  title changes, and so does the header of the open board if it is that
  canvas. Refuse cases: an empty title and a 300-character title must both
  keep the input open with a red reason, and must NOT reach the wire.
* **Rename, surface 2** — click the title in the board header, type, Enter:
  same effect, and the sidebar row follows. Escape on either surface reverts
  without a request.
* **Rename while drawing** — with a canvas open, apply shapes to it
  continuously (its row's shape count and timestamp change on every batch)
  while a rename input is open on that same row: the caret must survive. This
  is the regression the id-keyed `For` plus leaf memos exist for; the old
  `(id, revision)` key rebuilt the row — and the input — on every stroke.
  **Drive both halves from inside one page evaluation**: open a second
  `WebSocket` to `/ws` from the page itself and apply from there. Two reasons —
  a second browser tab or an out-of-process client makes the window lose
  focus, and a window blur *commits the rename* (correct behaviour, but it
  ends the test); and the assertion has to interleave with the applies.
  Assert all four: the row's meta line advances (1→2→3… 个形状, so the test is
  not vacuous), `document.contains(rowNode)` stays true (the node was not
  remounted), the input is still `document.activeElement`, and
  `selectionStart` is where you parked it. Held for 5 consecutive applies.
* **Search** — type into the filter: matching rows remain (title *and* id
  match), a query matching nothing reads *No canvases match* (not *No
  canvases yet* — different sentence, different state), and clearing the box
  restores the full list in the server's most-recently-updated-first order.
* **Delete** — the trash icon arms an inline confirm inside the row;
  confirming removes the row, and if that canvas was open the board falls
  back to the welcome pane rather than leaving an editor bound to a document
  the server no longer has.

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
