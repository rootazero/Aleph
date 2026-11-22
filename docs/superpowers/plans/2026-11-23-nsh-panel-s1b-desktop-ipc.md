# NSH Panel S1-B Desktop IPC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans task-by-task, with test-driven-development and a final independent review. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在独立 Gateway 与 NSH 宿主之间接通显式启用的有界 Desktop IPC，并用实际 Core 数据证明 `system.hello` → `health` / `sessions.list`。 / Connect the standalone Gateway and NSH host over opt-in, bounded local IPC.

**Architecture:** Server owns endpoint and connection lifetime. OS-authenticated clients are mapped only to that instance's active Core owner; actual RPCs pass through the shared S1-A executor, live bound peer and cleanup. Shared Client keeps one socket epoch and request table per connection; no automatic substitution of `/ws`.

**Tech Stack:** Rust 1.99.1, Tokio, Unix credentials, Windows Named Pipes, NSH `=0.6.0`.

**Spec:** [Customizable Panel design](../specs/2026-11-22-nsh-panel-modular-refactor-design.md) §5/7–9; [migration roadmap](2026-11-22-nsh-panel-migration-roadmap.md); [S1-A evidence](../runs/2026-11-23-nsh-panel-s1a-shared-dispatch-verification.md).

## Global Constraints / 全局约束

- Work only on `nsh-panel-refactor`, baseline `3713f06f3ec571a1668b34ff3f38492b2f8c7a5a`. No merge, service registration, installed/default changes, release or push.
- Existing `/ws` and legacy `connect`, trust/target/credential state, Panel renderer, supervisor acquisition and Tauri retention gates stay unchanged. No `/v1/ws`, web credential exchange, push events, snapshot/replay, write RPCs or generated UI in S1-B.
- Opt-in `aleph-server --desktop-ipc <endpoint>`; opt-in `aleph-nsh-shell --desktop-ipc-status <endpoint>` is an actual local host command, with no supervisor launch, target change or network fallback. It must return failure on an unsupported or unreachable endpoint and report real Core health on success. It does not change the WebView data plane.
- OS access proof is process-account identity only, not code identity. Same OS user on an explicitly owned endpoint maps to the active Core `OWNER_USER_ID`, with role read from `SecurityStore` on admission and before dispatch. Do not create an account, accept claimed identities, synthesize verified paired-device identity, grant another principal, or use the legacy loopback shortcut. Without active owner/store or before readiness, refuse.
- One length-prefixed UTF-8 JSON message, 4-byte big-endian header, 8,388,608 bytes maximum, 8 concurrent requests per connection, JSON nesting cap 64, array/object length cap 4096, member-name or string cap 65,536 bytes. Use stable connection ID, handshake 5 seconds, bounded frame writes, no token fields or advertised unsupported features. Platform evidence is separate; disabled/unavailable is not a pass. Every `cargo` call runs `python qa/browser_live/check_memory.py` first; `--locked`, serial `CARGO_BUILD_JOBS=2`. No production/default port probes.

## Review Focus / 审阅重点

1. Occupied or replaced endpoint → never unlink a live/unowned path (Task 2/4).
2. Same OS account but absent/deactivated owner, wrong peer, or claimed principal → no handler / no elevation (Task 2/4).
3. Fragmented/cancelled reading or an oversized header → reject without desynchronization, oversized allocation or leaking a connection slot (Tasks 1/2).
4. Delayed handler plus another reply, duplicate active ID and EOF → independent correlation, no duplicate execution, and owned cleanup (Tasks 1/2).
5. Requested local endpoint down, unsupported OS, or wrong launch combination → no TCP fallback, credential writes or supervisor activity (Task 3).

## Task 1: Shared native frame I/O and Client adapter / 共享原生帧与客户端

**Files:** `shared/client/src/{lib,connection}.rs`, `shared/client/src/local_ipc.rs`, `shared/client/Cargo.toml` if needed for existing Tokio `io-util` and native features. Consume `shared/protocol/src/framing.rs` and `connection/requests.rs::RequestState`; no new registry or protocol dependency.

**Interfaces:** `local_ipc::{read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<String>>, write_frame(writer: &mut (impl AsyncWrite + Unpin), text: &str) -> io::Result<()>, parse_payload(text: &str) -> io::Result<serde_json::Value>}` (async for I/O). `AlephClient::connect_local(endpoint: &Path, hello: &HelloParams) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)>` uses the same socket/request state, first `system.hello`, checks major/limits/features and does not synthesize a WebSocket URL. Existing `reconnect` does not reconnect a local socket as WS; retire and open a new local client explicitly.

- [ ] Add tests `frames_split_and_coalesce`, `frames_refuse_invalid_before_body`, `json_shape_limits`, `local_client_uses_hello_and_correlates_out_of_order` using actual stream pairs and an ephemeral UDS. Assert separate responses by ID, malformed/zero/oversize/partial EOF, no legacy `connect`, close fails unanswered work, same-user peer check. Host caller uses only common `call` and `close`.
- [ ] Run gated `cargo test --locked -p aleph-client` against the test seam; capture expected RED rather than treating a compiler error as behavioral evidence.
- [ ] Implement frame I/O from S0 `decode_header` / `encode_header` without a Tokio dependency in Protocol. Read header exactly before allocation and apply JSON shape limits. Adapt existing socket read/write halves; parse local frames separately from unchanged WS decoding, reuse the request table, event delivery fence and stop ownership. Unix connector checks private parent/socket and kernel peer UID against effective UID.
- [ ] Run gated `cargo test --locked -p aleph-client`, `cargo clippy --locked -p aleph-client --all-targets -- -D warnings` and `cargo check --locked -p aleph-cli -p aleph-tui`. Commit shared adapter; no listener yet.

## Task 2: Server-owned Unix endpoint and Core dispatch / Unix 端点

**Files:** `src/gateway/server/{mod.rs,connection/{mod,dispatch,upgrade}.rs}`, new `src/gateway/server/connection/local_ipc.rs`, `src/bin/aleph-server/{cli.rs,commands/start/mod.rs}`. Reuse `handlers/system_hello.rs`, `RpcExecutor::{prepare,execute}`, the existing `connections` table and `cleanup::run_cleanup`. Share the existing `GatewaySharedState` with legacy router and IPC, including the same middleware instance.

**Interfaces:** `GatewayServer::run_with_desktop_ipc(&self, shutdown: oneshot::Receiver<()>, endpoint: Option<&Path>) -> Result<(), GatewayError>`; keep `run_until_shutdown` as no-IPC call. `local_ipc::bind(endpoint: &Path)` owns listener and socket entry; `local_ipc::serve(listener, state: Arc<GatewaySharedState>, stop: CancellationToken)` owns accepted tasks. `DispatchPeer::bound` is production only when constructed from authenticated, server-stamped state. IPC `ChannelClass::Desktop`, no network origin bucket, no loopback authorization. Use owner checks around actual execution, not just accept.

- [ ] Add mounted `desktop_ipc_uses_gateway_and_releases_its_connection` real UDS regression (with normal legacy server router also enabled on an ephemeral port), a real no-auth/first/wrong hello path, an actual Core owner-visible `sessions.list` result that excludes a second fixture user, and a slow instrumented read RPC proving out-of-order responses/duplicate-active-ID denial and disconnect cleanup. Check same-user endpoint ownership, strict permissions, no unlink of occupied or replaced paths, no sockets after server shutdown. Filter prefix `gateway::server::connection::local_ipc::tests`.
- [ ] Run gated test against a stub-mounted or missing endpoint and record intended assertions before implementation. Retain pre-change legacy smoke results from S1-A.
- [ ] Bind only an absolute path with an existing owner-only real parent (0700) and no preexisting entry. Accept only kernel-attested same effective UID. Set socket mode 0600 before use; compare inode/device before removing the owned entry on stop, never auto-delete stale/active entries. Limit IPC to 8 accepted sockets (including pending handshakes) and insert a ready row under the existing Gateway global cap; guard each live connection's decoded requests with a 16 MiB aggregate wire-byte budget; use a single connection UUID, exact first hello, return actual frame/inflight limits, and refuse unmounted methods. Serialize bounded writes, keep one independent reader (or a pinned read future that is not restarted on worker completion), and bound the dispatch jobs; terminate on unexpected frame or duplicate active ID without running it. Drain accepted tasks and run existing cleanup on EOF, error and shutdown; no implicit session stop.
- [ ] Verify gated Gateway server lib and `gateway_ui_ingress`, server CLI check and format. Commit Unix endpoint with opt-in flags; leave unsupported platforms fail-closed.

## Task 3: Consume IPC in the actual NSH host / 宿主消费

**Files:** `desktop/nsh-shell/src/main.rs` (early explicit status branch, before settings/supervisor/credential access); `desktop/nsh-shell/src/ipc_status.rs` if helper needs independent tests. Do not activate disconnected prototype `connection.rs` / `supervisor.rs` or move current WebView/offline flow.

- [ ] Test argument combinations: exactly one endpoint and no remote/QA options; standalone `--help` reports distinction; missing endpoint produces nonzero exit and no connection to any WS target. Use a prestarted isolated Gateway and actual `aleph-nsh-shell --desktop-ipc-status <endpoint>` artifact to assert its stderr/stdout contains only sanitized diagnostics or canonical Health data. No fake IPC server for the positive host smoke.
- [ ] Wire `AlephClient::connect_local` using fixed host hello metadata, real `call("health", ...)`, explicit close; use bounded connection/call/close, no target mutation or settings persistence. This is a status command, not a new UI renderer.
- [ ] Run gated `cargo test --locked -p aleph-nsh-shell` and actual host smoke on supported local platform, record command, binary revisions and endpoint scope. Commit host wiring.

## Task 4: Named Pipe, platform receipts and final review / Windows 与证据

- [ ] Windows counterpart of Task 2: Windows-only Named Pipe listener, `reject_remote_clients`, first-instance refusal, DACL scoped to expected user SID, peer SID independently matched from pipe client's token after bounded initial I/O and before hello dispatch. Restore impersonation synchronously on every return, never hold across `.await`. Verify client-side server account ownership and fail closed on absent/inconclusive proof. Use the same framed loop, owner resolver, shared cleanup and Client request state; no fallback. Count this platform only after native tests (occupied name/replacement, DACL/SID, deny nonmatching/remote, first frame, concurrency, process close) pass; if no suitable native runner, explicitly mark unavailable and retain the opt-in refusal rather than mounting unverified security code.
- [ ] Verify genuine macOS, Linux and Windows receipts separately. If a runner is unavailable, publish exact unrun commands and acceptance checks. Re-run locked regression suites on final candidate; independently review baseline-to-code diff with focused ownership and auth checks. No promotion to S1-C/full S1-B acceptance on missing evidence.
- [ ] Update `docs/reference/{FEATURE_LOCATOR,DESKTOP_SHELL}.md` with exact executable surfaces and platform status; publish receipt in `docs/superpowers/runs/`. Verify links and diff, commit locally.
