# ACP Session Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Follow the user's delegation criteria when splitting the work.

**Goal** — Give all current Aleph ACP callers one lifecycle with single-owner startup, bounded shutdown, and recoverable session associations. / 现有 ACP 入口共用单所有者启动、有界关停与会话恢复。

**Architecture** — Keep the local `SessionKey` and remote ACP ID distinct. Reserve native start and retirement per key, validate the harness registration at publication, and keep cancellation independent of the session's protocol mutex. Join the existing persistence writer from shared server cleanup. / 按本地键保留启动与回收的执行权，在发布时确认 harness 注册仍有效，取消独立于协议锁，通过服务器统一收尾等待写入。

**Tech Stack** — Rust, Tokio, the existing ACP NDJSON client, serde JSON, and `utils::atomic_io`.

**Spec** — [ACP session lifecycle design](../specs/2026-11-28-acp-session-lifecycle-design.md).

## 全局约束 / Global constraints

- Work from `refactor/acp-integration` in its isolated worktree. Keep public tool names, arguments, Gateway authorization, `SessionKey` normalization, and `NativeAcp`/`Oneshot` routing. / 在隔离工作树保留公开入口、授权和路由。
- `trust_level=disabled` forbids delegation before either process launch path and during startup restore. / 原生、one-shot 和启动恢复遵守禁用委派规则。
- Reuse `acp_sessions.json`, the existing writer and atomic file operations, tool registration, Gateway invalidation, and Teams routing. ACP tools continue to use the existing Tool Capability. / 复用文件、写入、工具、通知及 Teams 接线，ACP 工具继续作为 Tool Capability 暴露。
- Keep the existing outbound protocol and restore timing. This change uses `session/load` for known IDs and leaves protocol negotiation, peer history, and caller ownership policy to their separate contracts. / 沿用出站协议和启动恢复时机，已知 ID 经 `session/load` 恢复。

## 实施文件 / File map

| Area / 范围 | Files / 文件 | Responsibility / 职责 |
|---|---|---|
| Shared admission / 共同准入 | `src/acp/manager/{harness_admin,lifecycle}.rs`, `src/builtin_tools/acp_tools.rs`, `src/teams/dispatcher/runner.rs` | Launch checks and normal caller coverage / 启动检查和入口覆盖 |
| Ownership / 所有权 | `src/acp/manager/{mod,lifecycle,harness_admin,tests}.rs`, `src/acp/session.rs` | Start, control, cancellation, and retirement / 启动、控制、取消及回收 |
| Recovery / 恢复 | `src/acp/manager/{lifecycle,harness_admin,persistence,tests}.rs`, `src/bin/aleph-server/commands/start/{mod.rs,builder/startup_resources.rs}` | Known IDs, ordered mutations, and graceful stop / 远端 ID、变更顺序及服务关停 |
| Verification and references / 验证和文档 | `src/executor/builtin_registry/builder/constructor/agent_acp_tools.rs`, `src/bin/aleph-server/commands/start/builder/handlers/settings.rs`, `docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,PROCESS_MANAGEMENT,GATEWAY}.md` | Verify dispatch, authorization, and projected state. Update reference sections from tested behavior. / 验证调度、权限与投影，按实测行为校对文档。 |

## 审阅重点 / Review focus

1. A request waits against an owner that is retiring. It receives stale or closed feedback before it can send an old frame. / 等待中的请求在向失效实例写入前收到明确结果。
2. The harness changes during `initialize`. The old process exits and cannot publish, including when the adapter stays enabled. / 初始化期间更新配置时，旧进程退出，无法发布旧实例。
3. A peer stops reading stdin and ignores cancellation. Teardown still reaches process exit within the budget. / 对端停止读入且忽略取消时，关停仍有上限。
4. A saved association has no process because `session/load` failed. Closing, updating, disabling, and unregistering clear the appropriate record. Unrelated records survive. / 恢复失败、无活动进程时，管理操作只清理相应关联。
5. Startup exits after ACP installation or overlaps a new native start. Shared cleanup preserves established IDs and drains the writer. / 启动中断或关停与新启动重叠时，共同收尾保留已建立 ID 并排空写入。

### Task 1 — Connect shared launch admission / 收敛现有启动入口

**Files** — `src/acp/manager/{harness_admin,lifecycle,tests}.rs`, `src/builtin_tools/acp_tools.rs`, `src/teams/dispatcher/runner.rs`.

**Interfaces** — Keep `prompt_named(&self, harness_id: &str, prompt_text: &str, cwd: &str, session_name: Option<&str>, mode: Option<AdapterMode>, reuse_session: bool, on_chunk: Option<AcpChunkCallback>) -> Result<String>` and the existing `ensure_session_named` and `restore_sessions` signatures. Add `AcpAdapterManager::admitted_adapter(&self, harness_id: &str) -> Result<Arc<dyn AcpAdapter>>` for launch paths. Return the existing `AcpErrorCode::HarnessDenied` for `trust_level=disabled`. Task 2 couples that decision to registration identity at publication.

- [ ] Add `disabled_harness_never_starts_native_or_oneshot` to the manager tests. Assert each native/one-shot invocation fails for a configured, enabled harness with `trust_level=disabled`, and its child start marker remains absent. Assert restore of an older record produces no launch; retain the record for later policy or user action.
- [ ] Add `disabled_acp_team_member_never_starts_harness` under the dispatcher runner. Call `execute_acp_member_task` using the shared manager, assert a failed outcome and zero launches. In the existing ACP tool tests exercise the delegated invocation with the same configuration and assert zero launches.
- [ ] Run `cargo test -p alephcore --lib disabled_`. Before the fix the Teams path or direct manager start must expose the missing admission check. Preserve the tool's existing ask and allow behavior, and preserve validation of the configured working directory.
- [ ] Route all launch paths through shared admission. Resolve the adapter and its policy from the same registration. Retain the tool's caller approval flow and existing error mapping where the tool boundary owns it.
- [ ] Rerun `cargo test -p alephcore --lib disabled_`; the focused tests pass without a subprocess start. Commit with `fix(acp): enforce shared launch admission`.

### Task 2 — Coordinate keyed owners and teardown / 收敛会话所有权及有界回收

**Files** — `src/acp/manager/{mod,lifecycle,harness_admin,tests}.rs`, `src/acp/session.rs`.

**Interfaces** — Continue to use `SessionEntry` for the live process and `CancelHandle::send_cancel(&self) -> Result<CancelDispatch>` for the independent notification. Add a private `SessionSlot` per `SessionKey` for start/retire serialization and a permanent manager stop fence. Give each live entry a retirement signal using the existing Tokio cancellation primitive. Keep current RPC-facing manager method signatures, including `shutdown_all(&self)` and `shutdown_named`.

- [ ] Add `same_key_starts_one_child_while_other_key_progresses`. Pause the first allowed ACP peer at `initialize`, start another request with the same canonical key and a third with a distinct name. Verify the third reaches handshake or prompt while the first is blocked and the first key records exactly one handshake. Use observable peer markers and a barrier, with timeouts as test guards.
- [ ] Add `harness_update_withdraws_pending_old_start`, `explicit_replacement_waits_for_old_child_exit`, and `shutdown_reaps_unresponsive_child_and_rejects_stale_waiter`. Assert the old registration cannot publish after update, explicit replacement writes `session/new` only after old process exit, and waiting prompt/mode traffic never writes to a retired peer. Use a peer that can ignore cancel and stop consuming stdin. Assert the test observes that process exit, with a deadline independent of the configured prompt timeout.
- [ ] Run `cargo test -p alephcore --lib acp::manager::tests`. Confirm a regression case fails for its intended effect on the previous ownership path.
- [ ] Move native acquisition and retirement onto the reserved slot. Carry the captured registration to publication, and recheck it atomically with committing the owner. Harness updates close admission to the prior registration before gathering affected slots. Stop closes admission before draining. Keep shared map/registration guards out of subprocess I/O. Keep one slot identity for overlapping operations and release unused slots once no operations or retained state reference them.
- [ ] Serialize prompt, mode, model, config, and auth against the live session. Recheck owner retirement after taking the session mutex. Capture the exact cancellation and retirement target in one state transition. Attempt cooperative cancel, then signal in-flight operations to release ownership and terminate/reap the child within the budget. If retirement abandons a frame write, retire that transport permanently. Preserve normal frame-write cancellation safety and `CancelDispatch` mapping. Never replay an uncertain prompt automatically.
- [ ] Delete the duplicate start/insert and split shutdown target paths. Repeat the focused command and run `cargo test -p alephcore --lib acp::session`. Commit with `refactor(acp): coordinate native session ownership`.

### Task 3 — Preserve associations through restore and server stop / 接通恢复与共同收尾

**Files** — `src/acp/manager/{mod,lifecycle,harness_admin,persistence,tests}.rs`, `src/bin/aleph-server/commands/start/{mod.rs,builder/startup_resources.rs}`.

**Interfaces** — Add `AcpPersistence::close_and_await(self) -> Result<()>`. Change `wire_persistence(&AcpAdapterManager)` to return `(AcpPersistence, Vec<PersistedAcpSession>)` after wiring the writer, with recovery triggered by the caller through `restore_sessions(&self, persisted: Vec<PersistedAcpSession>) -> Vec<String>`. Keep `PersistedAcpSession` unchanged. Let `StartupResources` own the `Arc<AcpAdapterManager>` and its writer before invoking recovery. Keep existing Gateway notification and public snapshot fields.

- [ ] Add `failed_load_keeps_saved_id_until_explicit_close`, `harness_changes_invalidate_dormant_associations`, and `stop_drains_persistence_and_restart_loads_same_id`. Seed legacy records lacking `session_name`; verify `session/load` with the saved ID, zero `session/new` on failed recovery, and a later load retry. Assert close and successful configuration changes write the expected records to disk and rejected changes retain them. Hold the writer back during stop, then assert the joined writer has written the remote ID and the next manager loads it.
- [ ] Run `cargo test -p alephcore --lib acp::manager`. The pre-change path must expose a changed remote identity or lost association.
- [ ] Seed associations before restore. Route startup restore, reuse of a dormant record, and reconnect after process death through the existing slot admission path. Retain the saved ID and reason when load fails. Use `session/new` for absent associations or explicit replacement. Store the ID when an owner is established, without requiring a successful prompt. Submit persistence mutations in the same order as transitions. Trigger the Gateway refetch after the state change has taken effect. Remove the parallel restore pool insertion and repeated writes of an unchanged ID. Migrate the existing burst, event ordering, and timestamp tests in `persistence.rs` to the owned writer and use its join as their drain barrier.
- [ ] Add the ACP resource to `StartupResources` before invoking restore. On every cleanup path close the manager, withdraw owners, retain recoverable IDs, close the persistence input, and await the writer's drain, including any in-flight atomic write. Close the input explicitly so incidental sender clones cannot keep the drain pending. Preserve the original startup error if cleanup also reports an error. Remove the separate normal-exit `shutdown_all` call. Update directly affected startup guard assertions.
- [ ] Exercise actual early exit and ordinary cleanup paths using existing server startup test patterns or a controlled server subprocess fixture. Use a bounded guard for process exit, assert no child survives the managed close, and inspect `acp_sessions.json`. The `aleph-server` binary is configured with `test = false`; verify it with `cargo check -p alephcore --bin aleph-server` and report the runtime startup path exercised separately.
- [ ] Run the focused ACP manager/persistence tests and `cargo test -p alephcore --test acp_atomic_e2e`. Commit with `fix(acp): retain session associations on server stop`.

### Task 4 — Verify normal entry points and update references / 验证入口并校对参考文档

**Files** — `src/executor/builtin_registry/builder/constructor/agent_acp_tools.rs`, `src/bin/aleph-server/commands/start/builder/handlers/settings.rs` and their existing dispatch test surface, `docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,PROCESS_MANAGEMENT,GATEWAY}.md`.

**Interfaces** — Exercise the production `ToolHandlerRegistry`/Capability path for `acp_delegate` and `acp_session_control`, the existing Teams route, and the registered `acp.sessions.list`, `acp.sessions.cancel`, and `acp.sessions.shutdown` Gateway handlers. Keep the existing Gateway admin access requirement and refetch notification.

- [ ] Verify tool discoverability and invocation using the production registrations. Observe an ACP peer's `session/prompt` and cancel notification through that path. For Gateway, observe list/state and invalidation from a committed transition, verify a non-admin request is denied, and verify a permitted close removes the intended saved association. Use the existing Teams launch regression from Task 1.
- [ ] Update `FEATURE_LOCATOR.md` and the relevant reference sections in Chinese and English. Record the launch/restore entry points, established operation semantics, removed paths, and verified limits. Align wording with the code and recorded observations.
- [ ] Run `cargo test -p alephcore --lib acp::`, focused tool, gateway, and Teams suites selected by their existing module filters, `cargo check -p alephcore --bin aleph-server`, `cargo fmt --all -- --check`, and `git diff --check`. Record any toolchain, platform, or environment limits precisely. Obtain an independent whole-branch review and reconcile its findings before the final commit.
- [ ] Commit the verified documentation and entry-point changes with `docs(acp): document and verify session lifecycle`.

Ownership and recovery change the same manager invariants and should integrate sequentially. Independent entry-point verification and reference editing may run concurrently against stable interfaces. A mechanical test or documentation task can use an authorized Flash model, with GPT-sol or Kimi for cross-module contract review and integration. / 所有权与恢复顺序集成。入口验证与文档可在接口稳定后并行，机械任务使用符合约束的 Flash 模型，跨模块契约与集成审阅交由 GPT-sol 或 Kimi。
