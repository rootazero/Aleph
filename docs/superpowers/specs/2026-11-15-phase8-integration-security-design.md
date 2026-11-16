# Capability Phase8 Integration & Security Design / Phase8 集成与安全设计

**Status / 状态:** design-only; requires human approval before implementation
**Date / 日期:** 2026-11-15
**Track / 轨道:** Capability Phase8 — Integration & Security
**Input / 输入:** [Phase8 entry inventory](../runs/2026-11-15-phase8-entry-inventory.md)

## Decision boundary / 决策边界

本设计把 Phase8 的第一批工作限定为安全失败形状、跨异步边界的身份/代际证明、撤回后的真实清理，以及可复核的 Gateway/renderer/PTY 终态证据。它不扩展 Phase7 合同 A，不承诺模拟 transcript 持久化，不把 live notification 变成 durable replay，不增加第二 registry、ready map、session store、owner table 或 universal scheduler。

本设计目前不要求新的公共 wire 字段或新的 capability kind。若实现时发现现有错误返回无法表达一个已批准的失败合同，必须先修改本设计并重新评审；不能用宽泛的 `Unavailable` 或一个“调用已发生”计数代替真实终态证明。

## Current authorities / 当前权威

| Concern / 事实 | Current authority / 当前权威 | Live consumer / 实际消费者 |
|---|---|---|
| Discovery and descriptor revision | `src/capability/facade.rs::Zahir`, `src/capability/zahir_facade.rs::ZahirFacade`, source adapters | projection, Gateway capability callers, cache freshness hints |
| Tool source and registry revision | `src/tools/registry.rs::ToolHandlerRegistry`, adapted by `src/capability/backend.rs::ToolBackendAdapter` | `ZahirFacade::describe/resolve/subscribe/project` |
| Owner, generation, claim and irreversible scope state | `src/capability/ownership.rs::OwnershipTree` | lease validation, source reconciliation, projection host |
| Principal, visibility, lifetime and generation gates | `src/capability/policy.rs::PolicyEngine` | `resolve`, `validate_lease`, captured call admission |
| Provider instance and provider generation | `src/capability/provider.rs::ProviderHandle` and source-specific capture | provider/special/terminal validation |
| Source composition | `src/capability/zahir_facade.rs::ZahirFacade` | one facade; Tool, Provider, Memory Resource and special source composition |
| Live capability projection | `src/capability/projection_host.rs::ProjectionHost` | default run-loop consumer and attached consumers |
| Capability audit observations | `src/capability/events.rs::InMemoryEventStream` and `Zahir::subscribe_events` | trusted scoped event observers; live-only broadcast |
| UI attachment and ACK | `src/capability/ui_surface.rs::UiSurface`; Gateway `capability_ui.rs` | renderer display ACK and input queue |
| PTY ownership and operations | `src/gateway/handlers/pty.rs`, `src/capability/zahir_facade.rs::invoke_terminal_gateway`, `src/gateway/pty` | PTY RPC handlers plus `pty.screen`/`pty.exit` subscribers |

`CapabilityKind` names a closed descriptive axis. A kind appearing in the enum is not evidence that a corresponding backend is mounted. The design therefore records actual source composition and excludes all-backend completion claims.

## Trust boundaries and ingress map / 信任边界与入口

1. **Untrusted JSON-RPC → Gateway.** The Gateway parses typed protocol parameters and obtains authenticated connection/principal context from task-local caller identity. UI attach additionally checks session visibility through the existing `SessionStore`/`session_visible_to` path. PTY handlers reject empty or unknown session ids and derive ownership from the trusted call actor rather than wire data.
2. **Gateway → Core capability admission.** UI operations enter `UiSurface`/special dispatch and must retain the connection, surface id and mount generation. Terminal operations enter `require_owned_session`, then `ZahirFacade::invoke_terminal_gateway`, which resolves the exact capability and rechecks provider/source identity when a capture crossed an async boundary.
3. **Core source → projection.** `ProjectionHost` subscribes to the existing Tool registry and ownership notifications, then applies snapshots. It is a consumer, not an authority or replay store. Registry broadcasts are bounded live notifications; lag requires a fresh snapshot.
4. **Core → renderer/PTY transport.** UI display completion is controlled by matching `UiRenderAck`; PTY is a separate operator surface with operation replies and `pty.*` live output/exit events. No renderer ACK is implied by a PTY reply, and no PTY reply proves a UI render.

## Five failure classes / 五类失败形状

| Class | Current fail-closed mechanism | Required Phase8 observable case | Remaining work |
|---|---|---|---|
| Foreign owner or scope | `PolicyEngine::resolve` checks binding existence/revocation, `OwnerRef` equality, visibility, and disposed lifetime. UI `validate_attachment` checks connection/surface/generation. PTY `require_owned_session` checks manager ownership against trusted actor. | Positive same-owner/scope call succeeds. Foreign principal, workspace/session visibility, connection, surface id, mount generation and PTY owner each fail before the handler effect. The response identifies the domain refusal; no external effect or successful event is emitted. | Add/retain focused cross-face cases proving the exact ingress is rejected, including a malformed scope and a foreign owner after a valid attachment. |
| Stale lease, provider or connection | `Zahir::validate_lease` rechecks source-specific lease validity and required resources. `OwnershipTree` generation changes invalidate leases. Terminal `OwnedPtySession::provider_matches` checks provider instance + generation and the facade checks current source identity. UI mount generation is part of attachment validation. | Resolve a lease/capture, rotate or stop its owner/provider/source, then attempt use. The use fails closed and no old handler runs. A fresh resolve after a valid replacement is the only success path. | Add a controlled handoff test across the async boundary for special/terminal dispatch and a provider replacement test that observes no stale effect. |
| Queued cancellation | `UiSurface::display` retains a worker/handle, races cancellation with ACK timeout, clears pending state, and `close` releases the pending wait before joining. Special dispatch carries a cancellation token into the handler. | Cancel before send, after send before ACK, during handler admission, and while closing. The terminal state must settle: pending wait released, worker joined, no stale queue entry, and no post-cancel success event. If an external effect already started, its actual terminal result must be reported rather than relabeled as cancellation. | Prove cancellation/close races with notifications and join completion, not with a cancellation-request counter. Clarify the exact effect boundary for any external-effect operation before adding a stronger claim. |
| Reconnect or withdrawal | Capability events are live-only and can return `Lagged`; `ProjectionHost` subscribes before snapshot, invalidates on owner/source changes and resyncs from a fresh snapshot. UI mount generation rejects old connections. Source observations mark withdrawn bindings before flush. | Disconnect/reconnect after a mutation, provider withdrawal, registry lag and UI remount. The old connection/cursor/capture cannot invoke; the new consumer gets a fresh visible snapshot and can resolve only the current binding. | Add a bounded reconnect scenario that combines withdrawal with a pending consumer; preserve the distinction between notification loss and durable recovery. |
| Cleanup after disable | `UiSurface::close` clears input, resolves pending ACK with `Closed`, withdraws policy fact source and joins its worker. `ProjectionHost` closes consumers and joins owned workers. Ownership revoke/dispose removes or invalidates bindings. | Disable/close during idle, pending ACK, queued input, projection lag and terminal operation. Assert owned tasks/resources settle, old handles fail closed, and the original cleanup error is retained if join fails. | Add terminal manager/source shutdown evidence and a process-local no-poisoning test; do not infer resource settlement from `close()` returning or a counter increment alone. |

## Contract boundaries / 合同边界

### Canonical metadata versus simulated transcript

Contract A remains: a successful addressed-key real `chat.send`/`agent.run` response commits canonical metadata before success. Valid no-run, later queue/spend rejection, cancellation, timeout or caller disconnect may leave a row, and one storage latency is accepted.

The no-provider startup path is server-reachable: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1569` constructs `SimpleExecutionEngine`, `:1572` erases it as `ExecutionAdapter`, and `:1618` mounts it in fallback `GatewayContext`. `src/gateway/execution_engine/simple.rs` ignores installed SessionService user/assistant emit failures, so simulated transcript persistence remains a P2. Phase8 must not silently promote this to Contract A or turn metadata readiness into a durable streamed transcript guarantee.

### UI renderer ACK versus PTY replies

`UiSurface::display` sends a `UiDisplayMessage` and resolves only when `UiSurface::ack` receives the matching `UiRenderAck` from the owning connection, surface and mount generation with `rendered=true`. `submit_input` is a bounded queued input operation with the same attachment proof.

PTY is a distinct operator surface. `require_owned_session` captures session id, provider instance and provider generation from the active manager; the facade rechecks the capture and resolves the exact terminal capability. PTY operation replies and `pty.screen`/`pty.exit` notifications do not create an additional approved renderer ACK schema. This design does not add one.

## P2 disposition / P2 处置

| Existing P2 | Phase8 disposition |
|---|---|
| Simulated `SimpleExecutionEngine` ignores SessionService message emit failures | Preserve as recorded P2. Promote only with a specific transcript durability contract and real server terminal-state evidence. |
| Metadata file mutex is per store/process | Preserve. A cross-instance/cross-process contract is outside this bounded slice unless separately approved. |
| PTY error presentation | Preserve as presentation P2; do not change the PTY capability contract while proving ownership/cleanup. |
| Coverage parser | Preserve the corrected retained-LCOV method; do not quote the defective archived parser or introduce a new threshold. |

## Proposed interface decisions / 拟议接口决策

- **Public capability facade:** retain the current signatures `Zahir::describe`, `lookup_descriptor`, `lookup_descriptor_for_cache`, `descriptor_revision`, `resolve`, `validate_lease`, `activate`, `subscribe`, `subscribe_events`, and `project`. No new public method is required for the first Phase8 slice.
- **Ownership/policy:** retain `OwnershipTree` as the generation/claim authority and `PolicyEngine` as the fail-closed gate. No second owner map or readiness latch.
- **Events:** retain `Zahir::subscribe_events` as a trusted live-only observation. Event records are not authorization tokens and a `lease_id` remains audit correlation only. No durable replay claim is added.
- **UI/PTY:** retain the existing UI ACK wire and independent PTY operation/event wire. Any new error variant or wire field requires a separate reviewed contract with a terminal cleanup case.
- **NSH:** no native UI or NSH dependency change belongs in this Capability spec. NSH v0.5 compatibility, R2 transition and ADR status are covered by the separate NSH track.

## Acceptance cases / 验收案例

A reviewer may accept this design only if the implementation plan binds each case to a real source ingress and an observable terminal result:

1. foreign principal/scope/connection is refused before handler invocation;
2. stale owner generation, provider generation or UI mount generation refuses a previously captured handle;
3. cancellation at each queued/ack/close boundary releases waits and joins owned workers;
4. withdrawal plus reconnect produces fresh snapshot/resolution behavior and never treats a lagged broadcast as durable recovery;
5. disable/close proves resource settlement, stale-handle refusal and preserved first cleanup error;
6. Contract A remains unchanged and simulated transcript P2 remains explicitly recorded;
7. renderer ACK and PTY reply evidence remain separate;
8. no test claims all capability kinds, all providers, remote deployment, external-effect exactly-once, Safe Replay or universal scheduler completion.

## Out of scope / 不在范围

All-backend completion, remote deployment, same-port candidate routing, external-effect exactly-once, Safe Replay, universal durable scheduler, native NSH implementation, Tauri/WebChat/workspace-member removal, native Linux/Windows proof, and release/push remain outside this design.
