# NSH Panel S1-A — verification / 共享分发验证回执

Scope: [S1-A plan](../plans/2026-11-22-nsh-panel-s1a-shared-dispatch.md), local `nsh-panel-refactor` only. Baseline `71cbacc43f698abe61504180242000129aa13cda`; verified code `fc940578ba3991c92f20360a4e1478d30a0db4bd`. / 本回执只证明共享分发和 Client socket 所有权，不证明新传输已上线。

## Verified paths / 已测通路

- The real `/ws` router rejects a first request other than `connect`; `connect` followed by `health` reaches the existing middleware. [The common RPC executor](../../../src/gateway/server/connection/rpc.rs) reuses lane and TTL idempotency state; existing connection owners still handle UI ACK, off-loop scheduling, subscriptions and cleanup. / 旧入口仍走原握手和清理所有者。
- [The bound-peer fixtures](../../../src/gateway/server/connection/rpc/tests/bound_tests.rs) exercise the same executor against a live connection table. They verify actual session visibility, live admin demotion, refusal after a device/identity change before handler execution or cached/Waiting results, and cleanup of only the disposed peer's subscription and reverse RPC. The [selector fixture](../../../src/gateway/server/connection/rpc/tests.rs) checks project switching without replaying old success. No production listener constructs a pre-bound peer. / 预绑定主体仅限测试；不替代真实准入。
- [The Client socket tests](../../../shared/client/src/connection.rs) hold the old peer open across reconnect and verify prompt disconnection, independent request tables, stable event receivers, rejection of stale delivery, and no automatic resend. Cancelling reconnect or close releases the socket while the Client stays alive. / 请求不会跨 socket 重发，事件不会从旧 socket 混入新连接。

## Execution / 实测

Local macOS arm64, Rust/Cargo 1.99.1, libprotoc 36.2. Cargo commands ran serially with `qa/browser_live/check_memory.py` before each invocation. The final Gateway commands used separate temporary `ALEPH_HOME` directories. / 本机隔离验证，非跨平台回执。

| Command / 命令 | Result / 结果 |
|---|---|
| `cargo test --locked -p alephcore --lib gateway::server::` | 234 passed, 0 failed, 22097 filtered |
| `cargo test --locked -p alephcore --test gateway_ui_ingress` | 1 passed, 0 failed |
| `cargo test --locked -p aleph-client` | 64 passed, 0 failed |
| `cargo check --locked -p aleph-cli -p aleph-tui` | Passed |
| `cargo clippy --locked -p aleph-client --all-targets -- -D warnings` | Passed on the identical Client code at `d05972574`; the next commit changed only the Gateway fixture |
| `rustfmt --check` on changed Rust files; `git diff --check` | Passed |

The legacy Router baseline passed before extraction; the Client baseline passed 57 tests. RED runs failed on the intended assertions: ordinary executor stub (3), bound legacy stub (3, with 4 parity cases passing), request-state stub (3), then the real reconnect disconnection and writer-ownership cases. Two additional real loopback cases timed out waiting for termination on cancellation before `d05972574` and passed after it. / RED 均为编译后的断言失败，不是编译失败。

One pre-receipt run without an explicit `ALEPH_HOME` had 233 passes and one failure: `mcp_route_is_mounted_only_when_a_face_and_auth_handles_are_set` failed with `snapshot store init: disk I/O error`. The unchanged fixture opens the default SQLite snapshot store; other fixtures can temporarily change the process home. With temporary `HOME` and `ALEPH_HOME`, the same binary passed the MCP case alone, three parallel Gateway runs and one serial run. The original I/O failure was not reproduced, and its exact cause was not established. The final locked runs above passed without changing MCP or SQLite code. A separate 60-second diagnostic command timed out during recompilation before running the test. Gateway builds retained the existing compact-unwind linker warning. / 保留原失败与重跑记录；未将 I/O 错误定性为已证实的竞争。

## Independent review / 独立审阅

GPT Sol reviewed `71cbacc43..69212a40d` and reported an Important writer leak on cancelled reconnect/close and a Minor fixture concern. `d05972574` fixes socket retirement; `fc940578b` uses one connection table with three distinct IDs for producer, waiter and cached reader. The focused independent follow-up on `69212a40d..fc940578b` confirmed both findings closed, with no blocker. / 两项发现均已关闭；只读审阅不代替运行结果。

## Limits / 未覆盖

A graceful close still attempts a Close frame and has no independent timeout for a stalled peer. Cancellation releases the local socket but need not deliver a Close frame. No new UDS, Named Pipe, `/v1/ws`, credential exchange, production first-`hello` gate, durable idempotency, TSX, or snapshot/replay was mounted. This focused run did not execute the full workspace matrix or real native/browser QA. No installed service changes, default cutover, merge, push or release. / 不代签真实传输、跨平台、浏览器或旧入口退场验收。
