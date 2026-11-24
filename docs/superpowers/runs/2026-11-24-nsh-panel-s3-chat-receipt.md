# S3 chat vertical slice receipt / S3 会话纵切真机回执

## Scope / 范围

S3 exit criteria from the [roadmap](../plans/2026-11-22-nsh-panel-migration-roadmap.md) and the
[S3 design](../specs/2026-11-24-nsh-panel-s3-chat-slice-design.md) §7: sessions, chat and cancel on a real
Native surface and its Web fallback, with real Core results, CJK input, streaming and snapshot refetch (not
replay). This receipt covers [plan](../plans/2026-11-24-nsh-panel-s3-chat-slice.md) Task 7 on macOS arm64 only.

本回执在 macOS arm64 上验证 S3 退出条件：同一份 TSX Chat 在桌面原生表面（Desktop IPC）与 Web 回退
（Web v1）上完成会话、发送、流式回复与停止，结果来自真实 Core；中文输入法回车不误发；断线后靠快照重取
恢复，不做重放。Linux / Windows 不在本回执内。

## Build under test / 被测构建

- Branch `nsh-panel-s3-chat` at `c56b01837` (Task 6 `1280a3647` plus the device fix below) plus the QA
  driver, evidence and S2 driver update committed with this receipt. `main` untouched at `b2c00e624`.
- `interfaces/panel` git tree `e283127929393949efd99635fd0f366eb2624c64`.
- `just panel-tsx-bundle`: `bundle.js` sha256 `d2d94efbd4b3c2192255179ff56215e46726dd49be971f4b2fbfa1338f6d9c3a`
  (27,673 bytes), `capabilities.json` sha256 `facfd1b500b68e2863a8f35104abafcebf5e32dbca44b36a161dd42a04c4c3f2`
  (unchanged from S2).
- `just panel-tsx-web`: `Aleph Panel/index.html` sha256 `dd60b8d456bda7582e6a7121f4185dee284ea88c6cc8a4be13174c389c9922cb`.
- Debug binaries: `aleph-server` sha256 `7abfa66d…f389f`, `aleph-nsh-shell --features tsx-panel` sha256
  `742ba37c…4fb2e` (full hashes in the [JSON](2026-11-24-nsh-panel-s3-chat-receipt.json)). The shell is
  136,025,904 bytes, the same size as before the fix; no dependency was added. Release size not measured.
- macOS 27.2.1 arm64, Rust/Cargo 1.99.1, Node 26.13.0, Playwright Chromium 145.0.7632.6, `nsh 0.6.0 (5541637)`.
- Selected input source during the run: Apple Shuangpin (`com.apple.inputmethod.SCIM.Shuangpin`).

## Real-machine run / 真机运行

Driver [`qa/nsh_s3_chat/run.py`](../../../qa/nsh_s3_chat/README.md) reuses the S2 fixture: one isolated
Gateway (temporary `HOME`/`ALEPH_HOME`, random port, self-signed TLS, `--desktop-ipc <root>/gw-N.sock`), no
provider, so Core runs the simulated engine. No installed service, port 18790 or user home was touched.
Evidence: [`report.json`](../../../qa/nsh_s3_chat/evidence/report.json),
[`web-report.json`](../../../qa/nsh_s3_chat/evidence/web-report.json).

### Desktop (embedded TSX surface over Desktop IPC) / 桌面

| phase | action | result |
|---|---|---|
| `connected` | first launch on `gw-1.sock` | default layout order Overview, Sessions, Chat, Capabilities ([png](../../../qa/nsh_s3_chat/evidence/desktop-connected.png)) |
| `chat-sent`, first Return | stored `active` = Chat; `osascript` types `S3desktophello`, then Return | under Shuangpin the Return commits the composition; text stays in the composer, nothing sent ([png](../../../qa/nsh_s3_chat/evidence/desktop-first-return.png)) |
| `chat-sent`, second Return | Return again | `chat.send` over Desktop IPC; Core creates `agent:main:main:s1`; the streamed reply is replaced by history: "I received your message: \"S3desktophello\"…" ([png](../../../qa/nsh_s3_chat/evidence/desktop-chat-sent.png)) |

The web phase checks that `S3desktophello` reached Core exactly once (`desktop_message_copies: 1`), so the
first Return did not send.

### Web (same TSX over Web v1 in Chrome) / Web

| step | result |
|---|---|
| `paired_live_events_granted` | ticket pairing; `system.hello` asked for `live_events` and got exactly `["live_events"]`; same `subject` as the desktop layout key |
| `cross_surface_history` | Sessions lists "Open chat S3desktophello"; Chat shows the desktop message and reply from `chat.history` ([png](../../../qa/nsh_s3_chat/evidence/web-history.png)) |
| `ime_return_does_not_send` | CDP `Input.imeSetComposition("nihao")` + Enter: no `chat.send` frame in 1.5 s, composition still marked ([png](../../../qa/nsh_s3_chat/evidence/web-ime-composing.png)); `Input.insertText("你好")` commits |
| `send_streams_and_completes` | Enter sends one `chat.send` with a v4 `idempotency_key`; the socket received `stream.run_accepted` ×2, `stream.response_chunk` ×4, `stream.run_complete` ×1 and no other pushed topic; reply then comes from history ([png](../../../qa/nsh_s3_chat/evidence/web-streamed.png)) |
| `stop_probe` | Stop pressed while the run was live: `chat.abort` sent, run ended with `stream.run_error`; the transcript keeps the user message without a reply |
| `gateway_restart_repair_restores_history` | Gateway restarted on a new socket: "Disconnected from Aleph" ([png](../../../qa/nsh_s3_chat/evidence/web-disconnected.png)); Reconnect fails authentication (cookie binding was in the old process, design §7a B2) and shows pairing; after pairing again the full transcript returns from disk ([png](../../../qa/nsh_s3_chat/evidence/web-after-restart.png)) |

The two console errors in `web-report.json` are the expected 401 upgrades from that Reconnect.

## Found on device and fixed / 真机发现并修复

- **No Web Crypto in the desktop runtime.** The first desktop run showed "crypto is not defined" and sent
  nothing. `Chat.tsx` called `crypto.randomUUID()`. `nsh test` and browsers provide `crypto`, but the shell's
  embedded QuickJS is built with `nsh-platform-macos/js` only. Enabling `nsh-platform-macos/crypto` worked but
  added 16 crates (`rsa 0.10.0-rc.19`, `p256`/`p384`/`p521`, `simd-json`, …) for one key. Decision: the Panel
  does not use the `crypto` global. `newIdempotencyKey()` builds a v4-format UUID from `Math.random()`, the
  same code on every runtime. An idempotency key is namespaced by principal and method and only needs to be
  unique while the Gateway caches it; it does not need to be unguessable. Send parameters are now built before
  the optimistic message, so a local failure keeps the draft. Commit `c56b01837`.
- **Harness lesson:** the headless `nsh test` runtime is not the desktop runtime. A global that a test sees can
  be missing on device; only this real-machine run catches that.

## Automated checks on the final tree / 自动化检查

| suite | result |
|---|---|
| `just panel-tsx-test` (`nsh test`, `nsh check`, `host.test.mjs`) | 22 passed; check clean; 11 passed |
| `cargo test -p aleph-protocol` (incl. `v1_frames` golden vectors) | 439 passed |
| `cargo test -p aleph-client` | 69 passed |
| `cargo test -p aleph-nsh-shell --features tsx-panel` | 35 passed |
| `cargo test --lib` gateway server, system_hello, agent, chat | 343 passed, 1 ignored (the Chrome Web v1 test, run separately and passing in Task 4) |
| S2 replay `qa/nsh_s2_shell/run.py` with the same binaries | desktop and web PASS after updating its default-order expectations for the Chat tab |

Known failures outside this branch (also fail on `main`, files untouched):
`gateway::handlers::browser_viewer::tests::issuer_rpc_owned_issue_and_revoke_enforce_identity_without_oracle`,
`gateway::handlers::events::tests::the_cli_carve_out_delivers_everything_an_unsubscribed_socket_did_but_reasoning`.
Intermittent: `gateway::server::tests::mcp_route_is_mounted_only_when_a_face_and_auth_handles_are_set`
("snapshot store init: disk I/O error").

## Observations, not fixed / 观察到但未修改

- Every simulated run publishes `stream.run_accepted` twice: once from `AgentRunManager::start_run`
  (`src/gateway/handlers/agent.rs:380`) and once from the simulated engine
  (`src/gateway/execution_engine/simple.rs:130`). Chat keys on `run_id`, so it is harmless here. It predates
  this branch.
- With CDP's synthetic Enter during a composition the textarea gains a newline (committed value `你好\n`;
  `chat.send` carries the trimmed `你好`). A real input method consumes that Return, as the desktop Shuangpin run
  shows. Not verified with a real IME in Chrome.

## Not verified / 未验证

- Linux and Windows (Desktop IPC named pipe, WebView fallback).
- A real input method in Chrome. The desktop phase used Shuangpin with ASCII letters, not a Chinese candidate.
- Stop on the desktop surface. Stop was exercised on Web only; the simulated run is too short to press it from
  `osascript`.
- `connection.warning` / `connection.stale` on device. Unit and headless tests cover them; the device run
  never overflowed.
- Real providers, long transcripts beyond `limit: 200`, very long lists, release build size.
- Replay after a gap. By design there is none (S4); this slice only refetches the snapshot.
