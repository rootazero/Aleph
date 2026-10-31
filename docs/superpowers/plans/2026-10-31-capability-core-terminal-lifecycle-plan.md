# Capability Core Terminal Source-Owned Lifecycle Implementation Plan
# Capability Core Terminal Source-Owned Lifecycle 实施计划

> **Gate:** This plan is executable only after explicit approval of `docs/superpowers/specs/2026-10-31-capability-core-terminal-lifecycle-design.md`.
>
> **闸门：** 只有明确批准上述 design 后，才能执行本 plan。

## Goal / 目标

Close the Terminal provider lifecycle boundary over the existing process-global `PtyManager`: disabled boot must not initialize the terminal runtime, enabled activation must own one flush driver, stop must revoke admission and retain cleanup evidence, and re-enable must fence old provider generations.

在现有 process-global `PtyManager` 上关闭 Terminal provider lifecycle 边界：disabled boot 不初始化 terminal runtime，enabled activation 只拥有一个 flush driver，stop 撤销 admission 并保留 cleanup evidence，重新 enable 时旧 provider generation 失效。

## Why this is next / 为什么是下一步

The Generic Gateway source-owned read/lease slice is complete, but it remains source-private and Gateway production mount is deferred. Terminal has a concrete lifecycle defect aligned with the North Star: `GatewayServer::build_router` unconditionally calls `attach_event_bus`, and `PtyManager::start_flush_loop` has a process-global once-only flag with no stop/join path. Closing this boundary proves an actual on-demand provider without introducing a universal registry or a Feishu-specific route.

Generic Gateway source-owned read/lease slice 已完成，但仍是 source-private，Gateway production mount 仍 deferred。Terminal 存在一个与 North Star 直接相关的具体 lifecycle 缺口：`GatewayServer::build_router` 无条件调用 `attach_event_bus`，`PtyManager::start_flush_loop` 使用 process-global once-only flag，且没有 stop/join path。本 plan 先关闭一个真实的按需 provider 边界，不引入 universal registry，也不创建 Feishu 特例 route。

## Scope / 范围

### In scope / 包含

- Reuse one `ProviderHandle` inside the existing `PtyManager` as provider state/generation/admission authority.
- Replace the permanent static flush-loop flag with source-owned driver ownership and explicit start/stop/join evidence.
- Make router boot activation conditional on the existing `Config::policies.terminal.enabled` value.
- Make live terminal disable stop the provider driver and close live sessions; make re-enable start a new generation.
- Gate addressed `pty.*` operations at the current provider generation while preserving existing session owner checks.
- Add source-owned provider status/read evidence sufficient for tests and future descriptor work; do not expose a generic Zahir mount.
- Add focused lifecycle, concurrency, failure, stale-generation, no-start, and event-bus tests.
- Update bilingual reference text only after implementation evidence is verified.

### Out of scope / 不包含

- Feishu or any other Gateway provider.
- Generic Gateway production ingress or Gateway→Zahir mount.
- Terminal `BackendLease`, `Zahir`, MCP/ACP projection, `subscribe`, or durable recovery.
- Moving `PtyManager::owners` into `OwnershipTree` or any parallel owner map.
- New `CapabilityKind`, registry/catalog/runtime table, universal scheduler, EffectClaim, Safe Replay, external exactly-once, native unload, RSS/process isolation, or `src/harness/**` changes.

## Authority and dependency rules / 权威与依赖规则

- `Config::policies.terminal.enabled` remains the enabled-state authority.
- `PtyManager` remains the only Terminal session/owner-retention/event-bus source authority.
- The one `ProviderHandle` stored by `PtyManager` owns provider state and generation; it must not start or stop runtime effects itself.
- `PtyManager` owns the flush driver handle and invokes the effectful start/stop operations.
- Existing `PtySession` owns child/session behavior; `close_all` remains the source cleanup primitive.
- Existing process journal remains the durable PTY fact source.
- Do not use `ProjectionHost`, `ZahirFacade`, `CapabilityBackend`, or a new global registry as a shortcut around the async/source-owned boundary.

## Files and likely symbols / 文件与符号

- Modify `src/gateway/pty/manager.rs`:
  - `PtyManager`, `PtyManager::new`, `attach_event_bus`, `start_flush_loop`, `close_all`, `spawn`, `with_session`.
  - Add source-owned lifecycle driver state and provider admission helpers.
- Modify `src/gateway/pty/mod.rs`:
  - public source lifecycle entry points and test-facing status only if needed; preserve existing module authority.
- Modify `src/gateway/server/mod.rs`:
  - `GatewayServer::build_router` must receive/use the existing terminal enabled decision and activate only when enabled.
- Modify `src/config/live_apply.rs`:
  - `policies.terminal` arm must use the source lifecycle stop/start contract, not only `close_all`.
- Modify `src/gateway/handlers/pty.rs` and/or registration:
  - enforce provider generation at addressed operation admission while preserving current owner filtering.
- Modify `src/gateway/pty/resource_matrix.rs` or the existing PTY test modules if present; otherwise add focused tests beside the owning source.
- Modify exact current reference entries in `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/GATEWAY.md`, and `docs/reference/ARCHITECTURE.md` only after source/test evidence.
- Do not modify `src/capability/facade.rs`, `src/capability/backend.rs`, or `src/capability/zahir_facade.rs` unless an architecture blocker is documented and separately approved.

## Ordered tasks / 顺序任务

### Task 1 — Establish failing lifecycle and no-start tests

Write tests before implementation for:

1. disabled router boot does not initialize the PTY manager runtime, attach a bus, or create a flush driver;
2. enabled activation creates exactly one driver and delivers a real `pty.screen` frame through the existing `GatewayEventBus`;
3. repeated activation does not create a second driver;
4. stop closes sessions and makes new session operations fail closed;
5. re-enable starts a new provider generation and an old generation is rejected;
6. failed start or stop retains typed residual evidence and does not report `Stopped` without proof;
7. concurrent start/stop/disable cannot lose the driver handle or clear cleanup evidence.

Run the focused PTY tests and confirm they fail for the missing lifecycle behavior. Do not weaken tests to fit the existing once-only loop.

### Task 2 — Implement source-owned provider and driver ownership

Add one `ProviderHandle` to `PtyManager` using `ProviderLifecycle::new` and the existing in-process transport. Keep lifecycle state changes explicit and checked. Store the flush `JoinHandle` and any cancellation/stop signaling in `PtyManager`’s existing source-owned state; do not add a process-global task registry.

Replace the static `STARTED` flag with a source-owned start path that:

- transitions to `Starting` and captures the generation;
- starts the flush driver exactly once for that generation;
- publishes `Ready` only after the driver admission/ownership evidence is committed;
- returns a typed failure and transitions to `Failed` if driver startup cannot be established;
- admits frames and operations only for the current `Ready` generation.

The stop path must:

- transition to `Stopping` and revoke new admission first;
- close all sessions using existing `PtyManager::close_all`;
- signal and await the current flush driver;
- transition to `Stopped` only after join evidence; otherwise preserve `Failed`/residual state;
- allow a later `Starting` transition to receive a checked new generation.

`ProviderHandle` remains side-effect-free; the source lifecycle driver performs tokio spawn, bus attachment, session close and join.

### Task 3 — Wire boot and live configuration without a second enabled authority

Change `GatewayServer::build_router` so terminal activation is driven by the existing live `Config::policies.terminal.enabled` value. Disabled boot must stop before `manager()` or runtime driver initialization. Enabled boot uses the source-owned start path and existing event bus.

Update `apply_live_sections` so `policies.terminal` handles both directions:

- `true → false`: revoke admission, stop driver, close sessions, preserve residuals on failure;
- `false → true`: start a new generation with the already existing event bus, or return a truthful non-live/restart result if the bus/runtime owner is unavailable.

Do not add a cached boolean. The config remains the single enabled-state source; provider state/generation is separate lifecycle evidence.

### Task 4 — Gate every Terminal operation at the provider boundary

At the shared handler/admission boundary, require the current provider to be `Ready` and capture its current generation before `spawn`, `input`, `resize`, `attach`, `close`, and relevant event delivery. Preserve the existing `created_by`/`owner_admits` predicate and do not turn a missing owner record into an affirmative authorization.

Old generation completion or flush activity must be ignored or rejected without mutating a new provider generation or replacement bus. Existing session owner retention remains source-owned and is not duplicated into `OwnershipTree`.

### Task 5 — Add failure matrix and source-backed documentation

Add tests for:

- start failure;
- stop failure with residual driver;
- cancellation during start/stop;
- stale generation after disable/re-enable;
- event delivery after stop;
- no-start disabled boot;
- existing PTY owner isolation and session close behavior.

Update only exact reference entries with:

- current Terminal lifecycle authority and config gate;
- disabled boot and explicit start/stop boundary;
- provider generation/admission semantics;
- the fact that Terminal is still not a unified Zahir backend and has no durable Capability recovery;
- the unproven boundaries: native unload, OS thread/process/RSS reclamation, process isolation, universal scheduler, and other providers.

## Validation / 验证

Run from `/Volumes/TBU4/Workspace/Aleph` and record each result:

- `git diff --check`
- `cargo fmt --check` for affected files or the repository-required equivalent
- `cargo check -p alephcore --tests`
- `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::pty`
- `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::handlers::pty`
- relevant `config::live_apply` and terminal policy tests
- `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib capability::provider`

Run broader workspace tests, clippy, live TUI, browser, and runtime QA only if the implementation changes their boundaries; otherwise list them as not run. Do not claim native unload, RSS reduction, process isolation, or external delivery from these checks.

## Review gates / 审查闸门

A reviewer must verify:

- one `PtyManager` authority and one provider handle;
- no static once-only task flag or detached flush loop remains;
- disabled boot reaches no manager/runtime side effect;
- stop/join/residual evidence is fail-closed;
- provider generation is independent from session ownership and config enabled state;
- old generation cannot publish frames or admit operations after re-enable;
- no Core→Gateway dependency, new kind, second registry, Zahir mount, Feishu wiring, or harness change;
- docs distinguish observed cleanup from unproven OS/process reclamation.

## Stop conditions / 停止条件

Stop for architecture review if implementation requires:

- a second Terminal registry, owner map, runtime table, or durable store;
- moving session owner authority without an approved migration;
- making `ProviderHandle` perform tokio/PTY/bus effects;
- changing `Zahir` or `BackendLease` to depend on Gateway;
- a global task registry or detached task that cannot be joined;
- claiming `kill()` proves process/thread/native/RSS reclamation;
- adding Feishu, Generic Gateway Zahir mount, Memory, Sandbox, subscribe, durable recovery, EffectClaim, Safe Replay, scheduler, or harness changes.

## Commit boundary / 提交边界

Expected implementation commits should remain narrowly scoped, for example:

- `feat(terminal): add source-owned provider lifecycle`
- `test(terminal): cover disabled boot and generation fencing`
- `docs(terminal): record lifecycle boundary`

The final review package must show only the approved Terminal files and reference entries. Do not push or merge remotes from the implementation session.
