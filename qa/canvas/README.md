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
  it prints a ready-to-open member URL at the LAN IP;
* open that URL in a browser, click through the self-signed-cert
  interstitial (TOFU), and assert (all held on 2026-08-17): the member's
  library shows ONLY the room canvas; the private canvas id over the
  member's wire answers `-32009 not found` **byte-shaped like a truly
  nonexistent id** (no-oracle); the room canvas is editable from both sides
  and `canvas.updated` reaches both live, no reload.

This scenario deliberately boots loopback-only; the LAN bind is a config
edit away, kept out of the default run so an unattended QA box never opens
a port by accident.

## Where the automated half lives

The wire-level assertions that do NOT need a browser are already automated
in `tests/canvas_wire.rs` (`cargo test -p alephcore --features test-helpers
--test canvas_wire`): contract key-set equality over the real handlers, the
AI-template tool-name resolution guard, and owner/member/stranger event
visibility over a typed bus subscription.
