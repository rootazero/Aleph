# Terminal Async Live-Config Closure Implementation Plan
# Terminal 异步配置闭环实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. A native task/review ledger is acceptable where equivalent project rules require it.
>
> **Status / 状态:** Proposed and unimplemented. Planning authorization is not production implementation approval. Execute when the user explicitly invokes the companion implementation prompt/plan. / 提案，尚未实施；下一会话用户明确执行配套实施 prompt/plan 后开始。

**Goal:** In a running Gateway, complete Terminal enable/disable through the existing explicit config path without requiring restart solely for cold bus access or pending driver join. / 补齐 Gateway 显式配置启停：cold enable 能使用既有 bus，正常 disable 等待真实 join。

**Architecture:** Make the one live-apply executor async and pass an optional reference to the existing Gateway event bus. Reuse the manager-owned activate/deactivate and cancellation-safe stored driver. Keep the shared Config read-locked during bounded application so newer intent cannot race an old snapshot; never hold its write guard across join.

**Tech Stack:** Existing Rust, Tokio (`test-util` is already a dev feature), serde, PTY implementation, GatewayEventBus and config patcher. No new dependencies. / 使用既有栈，无新增依赖。

**Spec:** [Terminal live-config design](../specs/2026-10-31-capability-core-terminal-live-config-design.md), layered on the completed [Terminal lifecycle design](../specs/2026-10-31-capability-core-terminal-lifecycle-design.md) and [run](../runs/2026-10-31-terminal-lifecycle-run.md).

## Global Constraints / 全局约束

- `Config::policies.terminal.enabled` remains the only enabled intent; one `ProviderHandle` remains side-effect-free state/generation/admission authority.
- `PtyManager` remains sole Terminal session/owner-retention/journal-integration/bus/driver source. No second registry, owner map, enabled map, runtime table, bus singleton or durable store.
- Context wiring clones the same `GatewayServer::event_bus()` reference. Disabled construction/boot never calls `manager()` or attaches that bus to PTY.
- Local async join budget is **5 seconds**. It does not preempt synchronous PTY I/O or impose a hard RPC wall-clock limit. Timeout/cancellation must not abort/drop the manager's stored driver.
- Keep `created_by`/`owner_admits`, retained owners, checked generations, source lock order, genuine cleanup failures and successful-wait-only `child_exited` evidence unchanged.
- No Terminal descriptor/BackendLease/Zahir/MCP/ACP, Generic Gateway mount, Feishu, Memory/Sandbox, new CapabilityKind, subscribe, durable recovery, EffectClaim, Safe Replay, scheduler, harness, native-unload/isolation/RSS work.
- Preserve dry-run, persistence-error, identical-patch no-op and exact-target reload-impact semantics. Recovery after incomplete application uses explicit `config.reload`; an identical patch/self_config update does not retry. No automatic retries, background reconcile task or expansion of file-watcher behavior.
- Before tests, read the applicable Rust test guidance in the current checkout and TDD skill. Tests assert effects; source census alone is insufficient. Preserve serial global PTY keys.
- Run `python qa/browser_live/check_memory.py` before **each** Cargo invocation (at least 4 GiB). Use `CARGO_BUILD_JOBS=1` for tests; long builds use the harness's background execution support.
- Implementation uses a clean checkout/worktree based on the local `main` that contains this handoff, since these local commits have not been pushed. Do not use a stale remote base. Short branch name, e.g. `terminal-live-config`; no remote push/merge.

## Review Focus / 审查重点

1. An offline or wrong-Gateway bus context must never be replaced by a global fallback; absent manager remains cold until enabled with an explicit bus (Task 1/2).
2. Cancelled/timed-out disable followed by enable must join the retained old generation before starting a new one; residual failure never counts as Live (Task 1).
3. Concurrent config writers and whole-file reload must not allow an older enabled snapshot to restart a provider after a newer disabled intent (Task 2).
4. Dry-run, failed save and rollback failure must not perform Terminal effects; successful persistence and unsuccessful live application remain distinguishable (Task 2).
5. Passing one live target, such as search or spend, cannot accidentally mark Terminal live; dedicated config endpoints and shared self_config must still reach the one executor (Task 1/2).

## File map / 文件职责

| Files / 文件 | Responsibility / 职责 |
|---|---|
| `src/config/live_apply.rs` | Async central executor, optional bus, bounded source stop, observed result and unit tests |
| `src/config/patcher.rs` | Optional reference wiring; async apply/rollback under Config read guard; real persistence tests |
| `src/gateway/handlers/config.rs` | Explicit whole-config reload with the supplied bus; release write guard before joining |
| `src/bin/aleph-server/commands/start/mod.rs` | Production patcher construction gets existing bus; narrow wiring only |
| `src/bin/aleph-server/commands/start/builder/handlers/config.rs` | Pass existing bus through explicit RPC reload registration; watcher loop unchanged |
| `src/gateway/handlers/execution_config.rs`, `src/gateway/handlers/search_config/{update,delete,test}.rs` | Await the executor for their exact targets; `None` bus is sufficient |
| `src/gateway/server/mod.rs` | Adapt current boot/live-apply tests; cold wiring and same-bus integration |
| `src/gateway/pty/manager.rs` | Owning-module tests for retained-driver timeout/cancellation evidence; do not redesign production lifecycle |
| `src/builtin_tools/self_config.rs` | Shared-patcher integration tests only if needed; no second apply path |
| `src/config/types/policies/terminal.rs`, current reference docs | Correct sync-fallback prose after implementation evidence |

Use existing modules; do not scaffold a new service. `src/capability/**`, `src/harness/**`, protocol and dependency manifests are outside the implementation allowlist. If an unlisted production consumer exists, identify it and explain its direct call-chain need before editing; do not silently expand architecture.

## Task 1: Async central executor and observed source completion / 中央执行器与真实完成

**Interfaces consumed:** Existing `PtyManager::activate(&self, bus: Arc<GatewayEventBus>) -> Result<u64, TerminalResidual>`, `PtyManager::deactivate(&self) -> Result<(), TerminalResidual>`, `pty::try_manager`, `pty::attach_event_bus`, `LifecycleSnapshot`, `classify_verified`.

**Interfaces produced:**

```rust
pub async fn apply_live_sections(
    cfg: &Config,
    top_sections: &[&str],
    event_bus: Option<&Arc<GatewayEventBus>>,
) -> Vec<&'static str>
```

- [ ] Write `terminal_cold_enable_uses_supplied_gateway_bus` with the existing router test's fresh-subprocess wrapper pattern: the parent re-executes the same exact test with a private probe environment flag and checks its exit status; only the child asserts cold global state. This wrapper must also work inside the combined suite. False config plus context does not initialize the manager; enable through executor with supplied bus returns `policies.terminal`; spawn a real short command that prints a unique marker; receive its `pty.screen` on a subscriber created before enable; repeated enable keeps generation/driver unchanged. Do not substitute a test-only local manager for this wiring proof.
- [ ] Write `terminal_disable_waits_for_driver_join_before_reporting_live`: real active session, apply false, result contains target only after `Stopped`, retained driver absent, old session operations rejected. No extra manual `deactivate()` after the executor to make it pass.
- [ ] Write `terminal_stop_timeout_and_cancel_preserve_driver` beside owning manager tests: use a controlled join barrier, actual source-owned stored handle, and Tokio paused time for the 5-second budget where applicable. At timeout result is non-live, handle/residual remains owned, and a later source retry can finish. Cancellation after observed Stopping likewise retains ownership. Existing driver failure tests must still distinguish Err from success. Test fixtures may access private owning-module fields; do not add production lifecycle APIs purely for tests.
- [ ] Write `terminal_enable_finishes_pending_stop_before_new_generation`: arrange cancelled disable in Stopping, release the join barrier, enable via executor, assert old join evidence precedes new Ready and generation strictly increases. Existing old-generation operation/frame tests remain green.
- [ ] Write negative tests for enable without bus, different Ready bus, absent-manager disable, and coarse `policies` requests whose spend application succeeds while Terminal does not. Each test checks exact `classify_verified` result and source state.
- [ ] Run new behavior tests before implementation and capture RED. Missing-signature compiler failure may prove the new boundary initially, but timeout, state and real-bus assertions must also fail for their intended missing behavior; do not claim all cases RED from one compiler error.

Run after the memory gate:

```bash
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib config::live_apply
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::pty
```

- [ ] Implement the async signature and Terminal arm from the spec, with a local 5-second wait budget and optional bus. Never report pending/Err as Live. Re-enable joins a Stopping provider first. Keep one declaration table; do not add a sync twin or detached cleanup.
- [ ] Update all existing call sites/tests mechanically to await the new signature so the repository compiles; production bus construction and full reload semantics are completed in Task 2. Exact-target execution/search calls pass `None`. Enumerate with `rg -n 'apply_live_sections\(' src` and inspect every result.
- [ ] Run the focused Task 1 tests GREEN, then review source lock ordering, stored handle cancellation and provider purity. Record exact results and commit a buildable change: `refactor(config): await terminal lifecycle application`.

## Task 2: Existing-bus wiring and all explicit config faces / bus 接线与全部显式配置入口

**Interfaces produced:**

```rust
pub fn with_event_bus(self, event_bus: Arc<GatewayEventBus>) -> Self

pub async fn handle_reload_with_subsystems(
    request: JsonRpcRequest,
    watcher: Arc<ConfigWatcher>,
    app_config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse
```

The first method belongs to `ConfigPatcher`; add only `event_bus: Option<Arc<GatewayEventBus>>` initialized to `None`. Existing `new(...)` and `with_vault(...)` remain compatible. Treat these signatures as the contract for this task.

- [ ] Write `patch_from_disabled_boot_activates_same_bus_and_disable_joins` using temp config/backup plus the real `ConfigPatcher`, then exercise the actual RPC `handle_patch_config` with that patcher. Assert persistence, target-specific live outcome, same-bus screen delivery, closed session/driver after disable, and larger generation on re-enable.
- [ ] Write `rollback_applies_terminal_lifecycle_after_persisting_snapshot`: enable/disable snapshots restore both persisted intent and observed source state; dry-run leaves generation/driver unchanged. Missing context or source failure leaves persisted success and non-live warning, never fabricated live completion.
- [ ] Write explicit reload integration proving the handler receives the supplied bus, updates shared Config and awaits Terminal stop before adding `live:policies.terminal` to `reloaded`. Use existing config load/path fixtures; do not read or overwrite the user's real config. Keep failed subsystem outcomes visible.
- [ ] Write `reload_finishes_retained_stop_after_identical_patch_is_noop`: through the real patcher/RPC, persist disabled intent and time out or cancel its pending join; repeat the identical patch and assert unchanged no-op response, no live application and no backup/file/mtime change. Release the driver barrier, then invoke the real `handle_reload_with_subsystems` and verify `Stopped`, no stored driver and `live:policies.terminal`. Exercise shared-patcher self_config no-op too. Do not bypass the config recovery entry by directly calling `manager.deactivate()`.
- [ ] Write shared-patcher `self_config` update/rollback coverage through the actual tool API or its existing real fixture. Assert response reload impact and source state, not just that a patcher field was installed.
- [ ] Write `latest_config_intent_fences_inflight_terminal_apply`: use barriers around a controlled stop, begin a second enabled/disabled writer through shared Config, prove it cannot commit while the older apply holds a read guard; after release, source state follows the final committed intent. Cover whole-config reload's write-to-read handoff using the latest Config rather than an unlocked clone.
- [ ] Add/save-error and dry-run tests asserting zero lifecycle effects; include unrelated execution/search config regression coverage. Run these tests RED before wiring fixes. Source-only census can supplement, not replace them.
- [ ] Implement patcher context builder, then supply `event_bus.clone()` at the production construction in `start/mod.rs` **before** the patcher is shared with RPC/tools. No eager `manager()`/attach in builders.
- [ ] Propagate the existing bus from `setup_config_watcher` into the explicit reload registration. Do not alter its automatic file-watcher event loop.
- [ ] Await centralized live apply from patcher apply/rollback using the current shared Config read guard. Reload first installs under the existing write boundary, releases that guard, then obtains a read guard and awaits application of the latest shared Config. Never await Terminal join while holding a Config write guard. Audit driver/session/bus paths for reverse Config acquisition.
- [ ] Check execution/search migration preserves original response payloads and target semantics. They request only their own targets with `None`; no Terminal effects arise from those targets. Keep failed live apply separate from successful persistence.
- [ ] Run Task 2 GREEN, plus Task 1 regression filters; review source and production assembly with an independent reader. Commit `feat(terminal): complete explicit live config toggles`.

## Task 3: Integration acceptance, references and handoff / 组合验收与文档

- [ ] Run `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib -- --exact config::live_apply::tests::terminal_cold_enable_uses_supplied_gateway_bus --test-threads=1` after the memory gate. Its self-reexec wrapper proves the initial `try_manager().is_none()` in a fresh child before activation; run the same wrapper in the combined suite. A process-global OnceLock cannot be reset by cleanup, so this cold assertion must not depend on suite ordering. If the owning-module test path changes, record and use its actual fully qualified name.
- [ ] Run the required filters together at least once, preserving global PTY/spend serial keys, to catch shared-state fixture races. Then run required package/format/diff checks below. Do not retain the old 194 count as if new tests had run.
- [ ] Independent final review must cover bus identity, lock ordering, every config face, no write guard across join, timeout/cancellation ownership, Stopping re-enable, true failure residual and exact-target verdicts. Fix concrete findings with RED/GREEN evidence and re-review.
- [ ] Update current sections of `docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,GATEWAY,TERMINAL_RUNTIME}.md` and `src/config/types/policies/terminal.rs`: normal Gateway explicit toggles now await/land; missing context and true failure still downgrade. Keep file-watcher and cleanup-claim limits explicit. Do not rewrite the prior run as though it originally delivered this new slice.
- [ ] Add `docs/superpowers/runs/2026-10-31-terminal-live-config-run.md` with actual baseline, implementation commits, commands/counts, RED/GREEN evidence, reviewer verdict and remaining limits. Correct the planning link in FEATURE_LOCATOR to actual completion only when verified.
- [ ] Run `git diff --check`; inspect final scope for capability/harness/protocol/dependency edits and duplicate authorities. Commit `docs(terminal): record async live config validation`. Leave all commits local unless user separately authorizes remote operations.

## Required verification / 必需验证

Each Cargo line below requires a fresh successful memory gate; run sequentially and record failures honestly. The config and self_config filters are broader than the previous phase because the async API and bus wiring cross those boundaries.

```bash
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::pty
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::handlers::pty
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib config::live_apply
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib config::patcher
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::handlers::config
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib builtin_tools::self_config
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::handlers::execution_config
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::handlers::search_config
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib config::types::policies::terminal
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib capability::provider
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::server::tests::terminal_
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::event_visibility::tests::pty_frames_reach_only_the_owner_and_outlive_the_session
cargo check -p alephcore --tests
cargo check -p alephcore --bin aleph-server
```

Combine the same filters once as `cargo test -p alephcore --lib -- <filter1> <filter2> ...` with `CARGO_BUILD_JOBS=1`. Run affected-file formatting with `rustfmt --check --edition 2021 --config skip_children=true <all changed Rust files>` (repository's bounded equivalent for this scope), then `git diff --check`. No tests run from an unrelated package. Use the actual new test names when invoking `--exact`; add dedicated binary registration tests to the ledger if implemented.

## Stop conditions and limits / 停止条件与限制

Stop and report a concrete architecture blocker if existing bus/shared Config cannot be passed without a second authority, a source path needs Config while join waits, driver ownership would be detached, or desired behavior requires a new scheduler/mount/owner contract. Routine reference wiring and async call-site migration above are already the proposed scope; do not ask for reapproval for each file.

The prior actual WebSocket async-visibility timing test remains unrun unless needed by a changed boundary. This slice proves observed driver join plus existing cleanup evidence, not descendant-process/thread reclamation, native unload, OS memory savings, process isolation or exactly-once delivery. Full workspace/clippy/cross-platform/live Panel checks may remain unrun when unjustified; list them explicitly in delivery.

## Planner self-review / 规划自检

Spec goals 1–4 map to Task 1/2; goals 5–7 and all five Review Focus cases map to Task 2 plus owning-module Task 1 tests. New signatures agree across tasks; Task 1 includes compile migration, Task 2 adds production bus context. No new registry or enabled authority is needed. Read-only source reconnaissance independently recommended this slice over Terminal descriptors or Memory. Builder check found an identical-patch recovery gap; the design now fixes recovery at explicit reload, and Task 2 requires handler-level proof while preserving no-op semantics. Planning checks cover paths/links/diff only; no new production behavior has been tested in this planning session.
