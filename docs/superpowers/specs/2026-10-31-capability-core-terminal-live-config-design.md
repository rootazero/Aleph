# Terminal Async Live-Config Closure Design / Terminal 异步配置闭环设计

Date / 日期：2026-10-31
Status / 状态：Proposed next slice; planning only. Implementation begins when the user explicitly invokes the companion prompt/plan. / 下一阶段提案；本次只规划。用户在下一会话明确执行配套 prompt/plan 后开始实施。

## Intent and baseline / 意图与基线

Continue Capability Core toward lifecycle isolation and runtime composability by finishing the existing Terminal config path before adding another projection or mount. Baseline commits: `1505d06e5` (approval), `2372b2c22` (source-owned lifecycle), `075b508a8` (reference alignment). The prior run passed 194 focused tests and package/format checks; that is historical evidence, not validation of this proposal.

先补齐 Terminal 既有配置路径，让运行时启停真正可用，再推进其他 projection/mount。上述提交已完成 source-owned lifecycle，上一阶段 194 项专项测试通过；本设计尚未实施、未验证。当前检出已在 `main`，只有一个本地 worktree 和 `main` 分支，没有待合并或待删除的会话分支。

Existing [lifecycle design](2026-10-31-capability-core-terminal-lifecycle-design.md), [run](../runs/2026-10-31-terminal-lifecycle-run.md), source-owned contract and research master plans remain the architectural context. This slice closes a documented fallback allowed by the previous plan; it does not retrospectively declare that completed phase defective.

## Source facts / 源码事实

- `src/config/live_apply.rs::apply_live_sections` is synchronous. Its Terminal arm uses `try_manager`; an absent manager plus `enabled = true` cannot obtain a bus, and disable only polls `request_deactivate`. Missing context or a pending join correctly omits `policies.terminal` from `live_applied`.
- `src/config/patcher.rs::ConfigPatcher::{apply, rollback}` are already async; both call the central executor. The same production patcher serves `config.patch`, `self_config` update and rollback.
- `src/gateway/handlers/config.rs::handle_reload_with_subsystems` is async but currently calls live apply while holding the shared Config write guard.
- `src/bin/aleph-server/commands/start/mod.rs` already clones `server.event_bus()` and constructs the production patcher. `builder/handlers/config.rs::setup_config_watcher` already receives that same bus while registering explicit RPC reload.
- `PtyManager::activate` and `PtyManager::deactivate` already own the lifecycle, generation and retained driver. Flush/join/session cleanup do not acquire application Config locks. `ProviderHandle` stays pure.

以上入口可直接传递同一 event bus；不需要新增全局 bus 注册、不需要第二份 enabled 状态。自动文件 watcher 与显式 RPC reload 是不同路径，前者本阶段仍不承诺即时 Terminal reconcile。

## Goals and acceptance / 目标与验收

1. Disabled boot remains cold, including after wiring config contexts: no `manager()` initialization, bus attachment, flush driver or session.
2. A real explicit config write from disabled boot enables Terminal using the existing Gateway bus. `Ready` has one owned driver; a real spawned session publishes `pty.screen` to a subscriber of that very bus.
3. Explicit disable awaits existing source-owned `deactivate`; only a completed join and successful source cleanup include `policies.terminal` in `live_applied`. New admission is revoked first.
4. Re-enable after successful stop uses a larger provider generation. Old operations and frames remain rejected by existing guards.
5. Patch, rollback and explicit RPC reload share the executor and report observed outcomes. Dry-run and failed persistence cause no lifecycle effects.
6. Timeout, cancellation or genuine cleanup failure retains the manager's handle/evidence. Persisted configuration is not rolled back merely because hot apply failed; incomplete application is reported as non-live/Restart. Explicit `config.reload` retries application of the persisted intent through the same source. An identical patch remains a no-op and does not retry lifecycle work.
7. Missing bus context in offline/test construction fails closed on enable. Disabled plus absent manager is an already-satisfied no-op and must not initialize it.

验收断言必须覆盖真实 config→source→bus 的效果，不以调用次数或源码字符串作为唯一证据。

## Approaches / 方案取舍

| Approach / 方案 | Trade-off / 取舍 | Decision / 决策 |
|---|---|---|
| Async central live apply with an optional reference to the existing bus / 中央执行器异步化，传入既有 bus | Touches all current call sites and requires a lock audit, but keeps one executor and lets responses await evidence / 需要迁移调用点与审锁，可保持统一执行与真实结果 | Recommended for next session / 推荐 |
| Keep synchronous apply plus a detached cleanup/reconcile task / 同步入口加后台任务 | Needs new operation ownership/result reporting and risks reporting completion too early / 会引入额外操作寿命和结果语义 | Reject for this bounded slice / 本阶段不采用 |
| Terminal descriptor/read capture or Memory audit first / 先做 descriptor 或 Memory 审计 | Moves the broader Capability roadmap, but leaves a known runtime usability fallback; Terminal owner/lease mapping is not yet approved / 会先触及新契约而留下启停闭环 | Defer / 后置 |

Recommendation is the planner's proposal in response to the user's request to choose the next step; no new production architecture approval is claimed in this planning session.

## Contract and wiring / 契约与接线

Proposed exact executor signature in `src/config/live_apply.rs`:

```rust
pub async fn apply_live_sections(
    cfg: &Config,
    top_sections: &[&str],
    event_bus: Option<&Arc<GatewayEventBus>>,
) -> Vec<&'static str>
```

Use named imports for `Arc` and `GatewayEventBus`. Preserve the declaration table and exact-target `classify_verified` semantics. Non-Terminal arms retain their behavior; dedicated execution/search callers pass `None` because their requested targets do not include Terminal.

- Add `ConfigPatcher::with_event_bus(self, event_bus: Arc<GatewayEventBus>) -> Self`, storing only an optional clone of the existing reference. `new` remains usable offline. Construction must have no PTY effects. Production construction supplies the same bus obtained from `GatewayServer::event_bus()` before sharing the patcher with tools/RPC.
- Extend `handle_reload_with_subsystems` with `event_bus: Arc<GatewayEventBus>` and pass the existing bus at `setup_config_watcher` registration. Leave gateway-only `handle_reload` behavior intact.
- For Terminal enable, require the supplied bus context. If an existing manager is `Stopping` after a cancelled/timed-out disable, first await the same source-owned stop with the budget below; activate only after success. Then use existing `pty::attach_event_bus(bus.clone())`; do not initialize the manager before this enable branch. A different bus while already Ready remains an error under `PtyManager::activate`, never a silent bus replacement. Do not recover context from a new global or another Gateway instance.
- For disable, use `try_manager`. If absent, mark the target satisfied without allocating anything. If present, await its existing `deactivate` with a local **5-second async wait budget**. Success is live; error or elapsed budget is non-live and logs the source snapshot/residual. The timeout cancels only the caller's wait; it must not take/abort/drop the stored driver.

5 秒是本提案的局部等待预算，不新增用户配置项。它只能约束异步等待，不能抢占同步 PTY I/O 或 OS kill；不能宣称整个 RPC 有硬 5 秒上限。

## Config ordering and cancellation / 配置顺序与取消

Do not carry a Config write guard across Terminal join. For whole-config reload, persist/install under the existing write boundary, release it, then obtain a shared read guard and apply the current Config. Patcher apply/rollback likewise use the current read guard for live application. Hold that shared read guard until bounded apply completes, so a newer enabled intent cannot commit halfway through an older lifecycle application. This intentionally delays config writers during the wait; ordinary readers remain allowed. Do not substitute an unlocked cloned enabled value, a second enabled map or an epoch counter as a shortcut.

执行前复核锁链：Config read → source driver/admission/provider/inner；source driver、session cleanup、bus publication 与 join 完成路径不得反向获取 Config。无 Config write guard 跨 join。若当前源码不再满足此锁序，先报告具体 blocker，不能新增隐式调度器绕过。

After releasing a reload write guard another writer may win before the read guard is acquired; applying the latest shared Config is intentional. Responses must describe what the executor actually applied, not promise that an earlier intermediate value stayed current. Persisted-write success and runtime application remain distinct facts. Cancellation after stop begins leaves admission revoked and the driver owned by `PtyManager`. Recovery is an explicit `config.reload` of the persisted disabled intent: it re-enters the executor and can finish joining the retained driver. An identical `config.patch` or shared-patcher `self_config` update still exits through `diff.is_empty()` without live application, backup, file write or mtime change. Preserve that no-op response; it must not claim a retry or completed stop. Repeated reload may still report non-live on genuine cleanup failure. Do not add automatic background retries.

恢复入口明确为 `config.reload`，不修改相同 patch 的 no-op 语义。必须通过真实 reload handler 验证超时/取消后的 retained handle 最终完成 join；直接调用 manager 的重试测试不能代替这条配置入口证据。

## Authority, scope and exclusions / 权威、范围与排除

`Config::policies.terminal.enabled` remains sole intent; one manager-owned `ProviderHandle` remains state/generation/admission authority; `PtyManager` remains sole session/owner-retention/bus source; process journal remains durable PTY facts. An extra reference to the same bus is dependency wiring, not a new bus or runtime authority. Preserve `created_by`/`owner_admits`, source revision independence, cancellation-safe stored handle, real kill failures and successful-wait-only child exit evidence.

No Terminal descriptor/BackendLease/Zahir/MCP/ACP projection, Generic Gateway mount, Feishu, Memory, Sandbox, new CapabilityKind, owner migration, second registry/runtime table/store, subscribe, durable recovery, EffectClaim, Safe Replay, scheduler, native unload, process isolation, RSS claims or harness changes. The previous untested WebSocket visibility-filter timing remains disclosed; building a new WebSocket test apparatus is a separate task unless this change touches that boundary.

No new dependencies or protocol fields. No broad split of frozen startup files. Reference updates follow verified implementation; historical phase records remain historical.

## Builder choices / 留给实施者

Choose small fixture helpers and error log wording, while following existing Rust test guidance and serial global-PTY test keys. If deterministic timeout proof needs access to a retained test driver, extend the owning module's existing test arrangement instead of adding a production registry or a test-only runtime service. API aliases and a second sync executor are unnecessary. Exact limits, context ownership, outcome semantics and lock ordering above are fixed by this proposal.

## Planning check / 规划审查

A read-only builder check identified that an identical patch never enters live apply. This proposal resolves that gap by choosing explicit reload as recovery and preserving patch/self_config no-op semantics; the plan requires real handler evidence. The check found no additional blocker in the examined lock, context, Stopping-enable or cold-process arrangements. No production implementation or Cargo validation occurred during planning.

只读 builder check 指出了相同 patch 无法重试的缺口；本提案固定采用 explicit reload 恢复，并在实施任务中要求验证真实入口。本次未改生产代码，未运行 Cargo。

[Implementation plan / 实施计划](../plans/2026-10-31-capability-core-terminal-live-config-plan.md) · [Restart prompt / 重启提示](../prompts/2026-10-31-capability-core-terminal-live-config-prompt.md)
