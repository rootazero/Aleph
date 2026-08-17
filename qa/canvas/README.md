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

## Item 8: the member role

Loopback is **always operator**, so member visibility cannot be tested on
`127.0.0.1` at all. The recipe (proven in the workspace-unarchive round; full
notes in the project memory):

* set `[gateway] host = "0.0.0.0"` in the scratch config and restart;
* connect from the machine's **LAN IP** (not loopback) over the self-signed
  TLS the gateway mints — trust it in-app (TOFU);
* authenticate as a member (member user + device pairing), then assert: the
  operator's unlinked canvas is absent from the member's library and its id
  answers not-found, while a project-room canvas is visible and editable
  from both sides — and the `canvas.updated` broadcast reaches both.

This scenario deliberately boots loopback-only; the LAN bind is a config
edit away, kept out of the default run so an unattended QA box never opens
a port by accident.

## Where the automated half lives

The wire-level assertions that do NOT need a browser are already automated
in `tests/canvas_wire.rs` (`cargo test -p alephcore --features test-helpers
--test canvas_wire`): contract key-set equality over the real handlers, the
AI-template tool-name resolution guard, and owner/member/stranger event
visibility over a typed bus subscription.
