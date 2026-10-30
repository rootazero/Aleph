# Panel 社区 snapshot transport validation / Community Panel Snapshot Transport Validation

日期 / Date: 2026-10-30
实现分支 / Implementation branch: `panel-community-transport`
基线 / Baseline: `main` at `40525896a` (the historical discovery baseline `ba3c64c1d` is an ancestor)

## 结论 / Result

本阶段已证明 Core canonical `PanelUiCapabilitySnapshot` 可以通过现有、已认证的 JSON-RPC/WebSocket control plane 到达 webchat discovery state。实现停在 discovery transport boundary：社区 declaration 没有进入 route selection、`PanelShell`、`PanelLoader`、asset fetch 或 module import。

This phase proves delivery of the Core canonical `PanelUiCapabilitySnapshot` into webchat discovery state through the existing authenticated JSON-RPC/WebSocket control plane. The implementation stops at the discovery transport boundary: community declarations are not connected to route selection, `PanelShell`, `PanelLoader`, asset fetching, or module import.

## Transport ownership / Transport owner

- Endpoint: the existing authenticated WebSocket endpoint `/ws`, mounted by `src/gateway/server/mod.rs::GatewayServer::build_router()` through `handler::ws_upgrade_handler`.
- Request path: the existing `src/gateway/server/connection/dispatch.rs` JSON-RPC dispatch path and `MiddlewareChain`; no HTTP route or second WebSocket endpoint was added.
- Method: `panel.capabilities.snapshot`, owned as the shared constant `PANEL_UI_CAPABILITY_SNAPSHOT_METHOD` in `shared/protocol/src/panel.rs`.
- Handler: `src/gateway/handlers/panel_capabilities.rs`; `HandlerRegistry::new()` registers it, `src/gateway/method_census.rs` classifies it as open, and `src/gateway/lane.rs` classifies it as `Lane::Query`.
- Core source: `get_extension_manager()` obtains the existing process-global `ExtensionManager`. The handler calls `ExtensionManager::panel_ui_capability_snapshot()` exactly once per response and performs no Panel derivation, trust decision, registry lookup, or revision computation.
- Wire shape: the handler serializes the shared `PanelUiCapabilitySnapshot` directly. It does not introduce a server DTO or duplicate declaration fields.

## Response and failure semantics / 响应与失败语义

A successful response uses the existing `JsonRpcResponse::success` envelope. `Ok(empty snapshot)` is a successful value with its canonical revision and an empty declaration list; it is not a transport error. A non-empty snapshot is returned with the same shared shape, including the complete replacement declaration set and snapshot revision.

The existing `get_extension_manager()` unavailable response is returned unchanged. Serialization failure uses the existing `JsonRpcResponse::error` envelope and `INTERNAL_ERROR`, with a method-specific diagnostic. Webchat keeps transport failures and decode failures as `PanelDiscoveryStatus::Error { message }`; neither failure path is converted into an empty snapshot. Explicit disconnect becomes `Disconnected` and retains the last known snapshot for inspection.

成功响应使用既有 `JsonRpcResponse::success` envelope。`Ok(empty snapshot)` 是带 canonical revision 和空 declaration 列表的成功值，不是 transport error。非空 snapshot 仍使用同一个共享 shape，并携带完整 replacement declaration set 与 snapshot revision。

已有 `get_extension_manager()` 的 unavailable response 原样返回。序列化失败使用既有 `JsonRpcResponse::error` envelope 与 `INTERNAL_ERROR`，并包含 method-specific diagnostic。webchat 将 transport failure 与 decode failure 保留为 `PanelDiscoveryStatus::Error { message }`；两者都不会被转换成 empty snapshot。显式 disconnect 进入 `Disconnected`，同时保留最后一个已知 snapshot 供状态检查。

## Reconnect, epoch, and revision semantics / 重连、epoch 与 revision

`DashboardState` already owns the connection and its monotonic `connection_epoch`. After an authorized handshake, including reconnect, the existing RPC path starts a complete `panel.capabilities.snapshot` query. The discovery model is feature-gated under `panel-modular` and remains separate from `PanelProjection`, `PanelShell`, and `PanelLoader`.

`PanelDiscoveryState` applies the following fences:

- A result from an older connection epoch is `StaleEpoch` and cannot write discovery state.
- A lower revision, or a duplicate revision in the same epoch, is `StaleRevision` and cannot replace the accepted snapshot.
- A same-revision complete snapshot is accepted after a new connection epoch so reconnect can rebase state even when no revision changed.
- Each accepted snapshot replaces the complete declaration set, including replacement with an empty snapshot for retraction.
- Malformed payloads fail decode before state replacement; the last good snapshot remains intact.
- A missed advisory update is repaired by reconnecting and querying the complete snapshot again.

`DashboardState` 已经拥有单调递增的 connection `epoch`。每次授权 handshake（包括 reconnect）之后，既有 RPC 路径都会发起完整的 `panel.capabilities.snapshot` query。discovery model 在 `panel-modular` 下 feature-gated，并且与 `PanelProjection`、`PanelShell`、`PanelLoader` 分离。

`PanelDiscoveryState` 执行以下 fencing：旧 connection epoch 的结果返回 `StaleEpoch`，不能写入 discovery state；较低 revision 或同 epoch 重复 revision 返回 `StaleRevision`，不能覆盖已接受 snapshot；新 epoch 可以接受同 revision 的完整 snapshot，使 reconnect 即使 revision 没变也能重新建立状态；每个 accepted snapshot 都替换完整 declaration set，包括用 empty snapshot 表达 retraction；malformed payload 在 state replacement 之前 decode 失败，最后一个正确 snapshot 保持不变；丢失 advisory update 时，reconnect 的完整 query 可以修复状态。

## Notification decision / Notification 裁定

No live snapshot notification was added. The existing EventBus/SubscriptionManager path is a lossy advisory broadcast with lag/drop behavior and authorization filtering. It is not a durable complete-snapshot recovery source, so adding a second watcher, poller, broadcaster, or revision authority would violate the ownership boundary. Production discovery refreshes on connect, reconnect, and explicit refresh. Live push remains deferred to a separate transport decision.

没有新增 live snapshot notification。现有 EventBus/SubscriptionManager 是可能 lag/drop 的 advisory broadcast，并带有 authorization filtering；它不是 durable complete-snapshot recovery source。因此没有增加第二个 watcher、poller、broadcaster 或 revision authority。production discovery 在 connect、reconnect 与 explicit refresh 时刷新。live push 延后到独立的 transport decision。

## Validation evidence / 验证证据

| Check | Result |
| --- | --- |
| `cargo test -p aleph-protocol panel --lib` | 9 passed, 0 failed |
| `cargo test --no-default-features --features panel-modular --lib panel` from `interfaces/webchat` | 81 passed, 0 failed |
| Focused `panel::discovery` suite | 10 passed, 0 failed |
| `cargo check --no-default-features --features panel-modular` | passed |
| `cargo clippy --no-default-features --features panel-modular --lib -- -D warnings` | passed |
| `cargo check -p alephcore --tests` | passed |
| `node --test tests/panel_loader.test.mjs` from `interfaces/webchat` | 12 passed, 1 skipped, 0 failed |
| Targeted rustfmt and `git diff --check` | passed |
| Full `cargo fmt --check` | blocked by unrelated pre-existing formatting diffs outside this branch; no unrelated files changed |
| `cargo test -p alephcore panel_capabilities --lib` | reached test linking, then Windows LLVM OOM: `rustc-LLVM ERROR: out of memory`, `Allocation failed`, exit `0xc0000409, STATUS_STACK_BUFFER_OVERRUN`; no assertion ran |
| Generated Panel glue/artifact tests | not run: `interfaces/webchat/.panel-tabs/build_id` is absent |
| Historical local Panel artifact build | blocked at `bun run --bun tailwindcss` because the local `tailwindcss` script is missing |
| Real Gateway/live smoke test | BLOCKED: no authorized disposable live Gateway target; deterministic fixtures are not live parity evidence |

## Deferred boundaries / 延后边界

This phase does not publish assets or map manifest entries to public files. It does not add production browser routes, community `PanelShell` mounts, route selection, `PanelLoader` imports, asset fetches, install/update/uninstall lifecycle, marketplace or signature services, background refresh, default-release changes, external WASM, `ZahirFacade` runtime mounting, `src/harness/` changes, PTY/Gateway business logic changes, or live Gateway parity evidence.

本阶段不发布 asset，也不把 manifest entry 映射到 public files；不增加 production browser route、community `PanelShell` mount、route selection、`PanelLoader` import、asset fetch、install/update/uninstall lifecycle、marketplace 或 signature service、background refresh、default release 变更、external WASM、`ZahirFacade` runtime mount、`src/harness/` 变更、PTY/Gateway business logic 变更，也不声称 live Gateway parity。

The implementation commits are `956b330d8 test(panel): define snapshot transport contract`, `a0195570f feat(panel): expose canonical discovery snapshot`, `5dff02ffc feat(panel): consume discovery snapshots over control plane`, and `52b12b04e test(panel): cover discovery reconnect fencing`. No merge or push was performed.
