# Next Session Prompt: Terminal Source-Owned Lifecycle Boundary
# 下一会话 Prompt：Terminal source-owned lifecycle boundary

You are continuing Aleph Capability Core work in `/Volumes/TBU4/Workspace/Aleph`.

你将在 `/Volumes/TBU4/Workspace/Aleph` 继续 Aleph Capability Core 工作。

## Read first / 首先阅读

1. `AGENTS.md`
2. `docs/reference/REDLINES.md`
3. `docs/reference/ARCHITECTURE.md`
4. `docs/reference/GATEWAY.md`
5. `docs/reference/FEATURE_LOCATOR.md`
6. `docs/superpowers/specs/2026-10-30-capability-core-source-owned-contract-design.md`
7. `docs/superpowers/specs/2026-10-31-capability-core-terminal-lifecycle-design.md`
8. `docs/superpowers/plans/2026-10-31-capability-core-terminal-lifecycle-plan.md`
9. `src/capability/provider.rs`
10. `src/gateway/pty/{mod,manager,session}.rs`
11. `src/gateway/server/mod.rs`
12. `src/gateway/handlers/pty.rs`
13. `src/config/types/policies/terminal.rs`
14. `src/config/live_apply.rs`

## Mission / 任务

Implement only the approved Terminal source-owned lifecycle boundary over the existing `PtyManager`.

只实现已批准的 Terminal source-owned lifecycle boundary，复用现有 `PtyManager`。

The target behavior is:

目标行为：

- disabled boot does not initialize the PTY manager runtime, attach the event bus, create a flush task, or create a PTY session;
- enabled activation owns exactly one flush driver and publishes `Ready` only with driver evidence;
- stop revokes new admission, closes sessions, joins the driver, and preserves residual evidence on failure;
- re-enable uses a new provider generation and old generation operations fail closed;
- existing `PtyManager` session map, owner retention, journal, and `created_by` filtering remain the only Terminal session authorities.

## Settled architecture / 已确定架构

- `Config::policies.terminal.enabled` is the only enabled-state authority.
- One `ProviderHandle` owned by `PtyManager` is the provider state/generation/admission authority.
- `PtyManager` owns the effectful driver handle and invokes tokio/bus/session effects.
- `ProviderHandle` remains side-effect-free; it must not spawn, stop, join, attach a bus, or touch a PTY.
- `PtyManager` remains the only source authority for sessions, owner retention, and event-bus delivery.
- Existing process journal remains the durable PTY fact source.
- Terminal session ownership is not migrated into `OwnershipTree` in this task.
- `source_revision`, provider generation, session identity, and config enabled state are independent facts.
- The current Tool-only `CapabilityBackend`/`ZahirFacade` is not changed and does not depend on Gateway.

## Known current defect / 当前已知缺口

`src/gateway/server/mod.rs::GatewayServer::build_router` unconditionally calls `crate::gateway::pty::attach_event_bus(self.event_bus.clone())`.

`src/gateway/pty/manager.rs::PtyManager::start_flush_loop` uses a process-global `AtomicBool STARTED`, spawns a loop, and has no explicit stop/join path. This prevents truthful disabled boot, stop/restart, and generation fencing.

`src/config/live_apply.rs` currently calls `PtyManager::close_all` when `policies.terminal.enabled` becomes false, but does not lifecycle-stop the flush driver or provide a symmetric re-enable path.

## Execution protocol / 执行协议

1. Confirm the current worktree and commit `023a552da docs(capability): finalize source-owned contract package` is present on `main`.
2. Read the design and plan before any production edit.
3. Use the subagent-driven development workflow or an equivalent task/review ledger.
4. Write failing focused tests before implementation.
5. Preserve one authority per fact and the existing lock order.
6. Run focused tests after each task, then package checks.
7. Do not silently expand to Feishu, Generic Gateway Zahir mount, Terminal MCP/ACP projection, Memory, Sandbox, new kinds, subscribe, durable recovery, EffectClaim, Safe Replay, scheduler, native unload, process isolation, RSS claims, or `src/harness/**`.
8. If boot/config wiring cannot provide the existing event bus and terminal enabled value without a new enabled-state authority, stop and report the architecture blocker.

## Required implementation shape / 必须保持的实现形状

The source-owned lifecycle driver must:

1. transition `Discovered/Cold/Disabled/Stopped → Starting` with checked provider generation;
2. create one flush driver for the captured generation;
3. publish `Ready` only after the source owns the driver handle;
4. admit handler operations and frame publication only for the current `Ready` generation;
5. transition to `Stopping` before revoking/closing;
6. close sessions through existing `PtyManager::close_all`;
7. signal and await the driver;
8. publish `Stopped` only with join/cleanup evidence;
9. preserve `Failed` and residual state when cleanup is incomplete;
10. permit later `Starting` with a new checked generation.

Completed failed drivers may be cleared only when the provider is already `Failed` and the failure evidence remains observable. Treating arbitrary completed/error/pending cleanup as success is forbidden.

## Acceptance / 验收

The task is complete only when all of these are true:

- a disabled router boot does not initialize `PtyManager`, attach the bus, spawn the flush driver, or create sessions;
- enabled activation delivers a real `pty.screen` event through the existing `GatewayEventBus`;
- repeated activation does not create a second driver;
- stop closes existing sessions and joins the driver, or returns typed residual evidence without claiming stopped;
- live disable and re-enable use the existing config path and produce a new provider generation;
- old-generation spawn/input/resize/attach/close and old flush completions fail closed without mutating the new provider;
- existing owner filtering remains unchanged and all addressed PTY faces use the same owner predicate;
- existing PTY journal and session cleanup behavior remains intact;
- focused tests, `cargo check -p alephcore --tests`, `cargo fmt --check` for the affected scope, and provider tests pass;
- final diff contains no second registry/owner map/runtime table, no new `CapabilityKind`, no Core→Gateway dependency, no Feishu/Gateway mount, and no harness changes;
- documentation states exactly what cleanup is observed and leaves native unload, OS process/thread/RSS reclamation and process isolation unproven.

## Required checks / 必须执行检查

Run and report:

- `git diff --check`
- `cargo fmt --check` for the affected scope or the repository-required equivalent
- `cargo check -p alephcore --tests`
- `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::pty`
- `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::handlers::pty`
- relevant `config::live_apply` and terminal policy tests
- `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib capability::provider`

Run broader checks only when justified by changed boundaries, and list any unrun checks explicitly.

## Stop conditions / 停止条件

Stop for architecture review if any change requires:

- a second Terminal registry, owner map, enabled-state map, runtime table, or durable store;
- moving the current Terminal session owner authority without a separate approval;
- side effects inside `ProviderHandle`;
- a detached flush loop that cannot be joined;
- changing `Zahir` or `BackendLease` to depend on Gateway;
- a claim of native unload, process/thread/RSS reclamation, process isolation, external delivery, or exactly-once;
- Feishu, Generic Gateway Zahir mounting, Memory, Sandbox, subscribe, durable recovery, EffectClaim, Safe Replay, scheduler, or harness work.

## Handoff / 交接

This prompt is subordinate to the design and implementation plan. Before production edits, explicitly record approval of:

- `docs/superpowers/specs/2026-10-31-capability-core-terminal-lifecycle-design.md`
- `docs/superpowers/plans/2026-10-31-capability-core-terminal-lifecycle-plan.md`

At completion, provide the implementation commits, focused validation results, final review verdict, deferred boundaries, and any residual risk. Do not push or merge remotes automatically.
