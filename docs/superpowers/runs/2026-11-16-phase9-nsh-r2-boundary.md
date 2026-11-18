# Phase9 NSH / R2 boundary decision / 边界决策材料

**Status / 状态:** user explicitly selected alternative A on 2026-11-16 in the structured architecture question: keep R2 and host the existing Panel in NSH v0.5 + WKWebView. This authorizes that host composition within Phase9; it does not amend either constitution entry or prove runtime/parity. / 用户已明确选择 A；R2 不变，运行与性能仍待验证。

## Current implementation / 当前实现

The inspected source has three concrete conflicts with the intended first-slice boundary:

- `desktop/nsh-shell/src/main.rs` creates the status transport with `spawn_connection_loop`, then opens a separate transport with `SessionManager::connect`. Its native component also owns message, artifact, queue, run and generation projections and renders Chat/Artifacts directly.
- `desktop/nsh-shell/src/session.rs::subscribe_events` promotes any `RunAccepted` session, forwards `ResponseChunk` without checking the active run, and handles completion without identifying the completing run. Existing connection tests do not prove isolation of this projection.
- `desktop/nsh-shell/src/runtime.rs::spawn_tokio_worker` returns a thread handle whose drop does not stop or join its work. The main component immediately leaves the status worker handle unowned.

这些是源码事实，尚不是 live 复现或修复证明。当前窗口不能凭编译通过被记为单 transport、安全重连、owned drain 或 R2 合规。

The binding R2 rule keeps complex business UI in Panel/Leptos/WASM. The earlier proposal `../specs/2026-11-15-nsh-r2-native-ui-decision.md` explicitly says a native business-UI transition needs a separate ADR. The Phase9 authorization preserves that boundary; it does not silently authorize an amendment. / Phase9 的执行授权不等于原生业务 UI 的修宪授权。

## Concrete alternatives / 具体选项

### A. Keep R2 and host the existing Panel / 保留 R2，NSH 托管既有 Panel

NSH owns its native window, host lifecycle and platform I/O. Its WebView hosts the production Panel at the canonical Chat `/` route and Artifacts; that Panel remains the only business UI implementation and owns its existing Gateway transport. The redundant native status/session transports and native Chat projection leave the production path. No second registry, session authority or artifact store is introduced.

Pinned upstream `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1` implements macOS `WKWebView` in `crates/nsh-platform-macos/src/native.rs` and exports Rust `web_view` / element-ref evaluation. This is source-backed API availability, not proof that Aleph hosting, events or cleanup work. A real prototype, controlled Gateway and native close evidence remain required.

**Cost / 代价:** this preserves the system WebView and its process/memory overhead. It does not achieve the historical pure-native business UI target. Debug parity must account for the WebView process boundary; a valid threshold miss remains NOT_ACCEPTED. This choice needs no R2 amendment, but changes the current NSH host composition and postpones native business-UI migration.

### B. Explicit native business-UI transition / 明确批准原生业务 UI 迁移

Adopt a separate ADR that names native NSH as the Chat/Artifacts presentation authority, retains Core/Server as canonical fact authority, and defines one shared transport/projection contract rather than maintaining parallel native and Panel reducers. The ADR must delimit coexistence with Tauri/WebChat, teardown ownership, event identity/epoch validation and rollback. Synchronize `AGENTS.md` and `CLAUDE.md` only after that explicit decision.

**Cost / 代价:** this changes R2 and expands the reviewed Phase9 implementation. A sound shared projection and migration cannot be replaced by patches to the current duplicate native state. The existing native sketch is not grandfathered into compliance.

### C. Keep the architecture gate open / 保留架构闸门

Complete independent test/dependency closure and bounded window/API diagnostics. Record native Chat/Artifacts live/parity as open pending an ADR. This is a partial Phase9 result, never full Phase9 PASS. / 可以先闭合独立工作，但不能伪称 C/D 已通过。

## Implementation result / 实施结果

The NSH entry creates one pinned NSH WebView hosting the existing Panel. It no longer mounts the old native Chat/Artifacts projection, status transport, or `SessionManager` transport. `shell_host.rs` validates the debug-only loopback endpoint and 32-byte lowercase-hex nonce. Four package-level input tests and the pinned v0.5 Window API test pass; actual native-window behavior is first recorded in [runtime attempt 14](2026-11-16-phase9-runtime-attempt14.json), with the final repaired Panel revalidated in [runtime16](2026-11-16-phase9-runtime-attempt16.json). / NSH 已通过包级测试及实际开窗闭环，旧原生业务投影与双连接不再挂载。

Both hosts share `qa/nsh_panel_driver.js`, an observer of the existing Panel DOM and WebSocket RPC/events. It submits through the production composer and never generates Gateway RPC results. Adverse-frame checks compare the actual rendered projection; the old-receipt test invokes the same production ingress with a stale receipt and a current positive control in an explicit `runtime-qa` build. The normal default bundle excludes that JS ingress. Observer-only filtering and closed-socket no-ops do not satisfy this gate. / 拒绝证据来自实际入口和 Panel 投影，不来自观察器自滤；默认产物不含测试注入入口。

Both native children in attempts 14 and 16 close normally with `owned_drained: true`; the Tauri debug lite path joins its owned startup thread. This does not claim ownership of detached daemons or every platform worker. The collector retains raw markers and 100ms direct-child samples, but WebKit XPC ownership is still incomplete. Consequently the debug performance gate remains OPEN and partial RSS/thread counts cannot become accepted parity data. / 运行时正常关闭与完整引擎性能归属是两个闸门；前者已取得证据，后者继续开放。

## Common boundaries / 共同边界

All choices retain Tauri/WebChat and workspace members, exclude detached-daemon lifecycle takeover and release-performance claims, preserve the one canonical registry/OwnershipTree, and require real runtime evidence. Alternative A is now selected; B and C were reviewed alternatives, not authorization for a native business-UI migration. / 用户选择 A，禁止事项继续有效；B/C 不构成原生业务 UI 迁移授权。
