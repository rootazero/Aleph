# Memory Runtime Lifecycle Implementation Plan / Memory 运行时生命周期实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement independent Memory stop/re-enable and early-startup-error cleanup while chat and durable raw session archival continue. / 实现 Memory 独立启停及启动失败清理，保留聊天与 raw 会话归档。

**Architecture:** A concrete injected `MemoryRuntime` owns Memory admission, transitions, tasks and resources. `OwnershipTree` remains the only generation authority; canonical Tool descriptors capture a required Memory Resource lease. Raw storage retains its existing independent owner and connection. / 具体 runtime 是 Memory 生命周期事实源，沿用现有树和工具 registry，不建立第二套权威。

**Tech Stack:** Rust 1.99.1, Tokio, existing serde/schemars, rusqlite/sqlite-vec, existing Capability/Tool registry and native Pi review; no new dependencies.

**Spec:** [Approved design](../specs/2026-11-01-memory-runtime-lifecycle-design.md), approved by the user on 2026-11-01 after selecting live raw archival. Baseline evidence: [expanded validation](../runs/2026-11-01-memory-lifecycle-expansion-run.md).

**Status / 状态:** Plan for review; not implemented. / 待计划审阅，尚未实施。

## Global Constraints

- Preserve chat and raw session archival while Memory is off. Reject new Memory work; drain already admitted work. Include early-startup-error cleanup and resource close/reopen.
- Stop completion has no hard time bound. A caller cancellation stops waiting, not cleanup ownership. Failed cleanup retains real resources for explicit retry; only actual release permits `Off`.
- Runtime off/on is process-local and does not edit the boot configuration. A process restart uses persisted startup configuration.
- One concrete Memory owner, one canonical Tool descriptor/handler source, one `OwnershipTree` generation source. No generic supervisor, second global registry, new Capability kind or harness policy.
- Rotate **all existing visibility bindings of the Memory Resource ID only**; no `bump(Runtime)`, permanent revoke/dispose, or refresh of an old captured lease.
- No raw archival shutdown, whole-file/WAL closure claim, native unloading/RSS promise, hard deadline, distributed fencing, external-effect exactly-once, embedding hot replacement or ChannelRegistry redesign.
- No new dependencies, migrations to a second storage engine, public test-only APIs, sleep races or `Arc::strong_count` completion proofs. Code comments in English; durable documentation bilingual.
- Before **every** Cargo command, including metadata, run `python qa/browser_live/check_memory.py`; proceed only on `ALLOW` with at least 4,294,967,296 bytes available. Set `CARGO_BUILD_JOBS=1`; run Cargo serially.
- Use the existing local `note-maintenance-drain` checkout, currently based on local `main` at `c732c2d575d508834cc36090c3f97a89af436ed2`, with design commit `0b0bd9d747476ad0ab4632e5e4b97f65dbcd0cbe`. Recheck Git before execution; preserve edits. No automatic push, merge, reset or remote-based branch replacement.
- Preserve the selected execution method: native main agent is the sole writer, read-only Pi investigation when useful, fresh independent native Pi implementation review after integration, fix findings and re-review.

## Review Focus

1. **Approval paused across stop/restart:** the captured Tool dependency stays stale; approval must not refresh it. Task 5 tests this.
2. **Caller disappears during blocking I/O or cleanup:** actual work remains owned; `Off` cannot precede its completion. Tasks 3–4 test this.
3. **A second visibility scope or unrelated capability:** all Memory scopes become stale, unrelated/control bindings retain identity. Tasks 2 and 5 test this.
4. **Off chat/session end:** raw append and ordinary chat continue, while Memory-derived work neither initializes nor silently succeeds. Tasks 6 and 8 test this.
5. **Partial initialization or failed close:** preserve the primary boot error and actual retained connection/task; retry cannot rerun completed effects or publish false `Off`. Tasks 3, 4 and 9 test this.

## File Structure and Ownership / 文件结构与职责

Existing module conventions take priority; this is a targeted addition, not a startup-file rewrite. / 沿用现有模块习惯，不借机全面拆分启动代码。

| Files | Responsibility |
|---|---|
| New `crates/aleph-slot-api/src/capability.rs`; existing `src/capability/descriptor.rs`, slot-api descriptor/tool/lib | Single shared pure `CapabilityId`/`Service`/`ServiceRelation` definitions and canonical dependency metadata |
| Existing `src/capability/{ownership,versioned_registry,facade,backend,zahir_facade,projection_host}.rs`, `src/tools/registry.rs` | Binding rotation, atomic admission evidence, concrete Resource composition |
| New `src/memory/runtime/{mod,state,operation,bundle,tasks,tests}.rs`; `src/memory/mod.rs` | Concrete lifecycle, immutable captured handles, admitted operations, resource bundle, actual task ownership |
| Existing `src/memory/store/sqlite/` | One checked closeable Memory connection; unchanged storage traits/raw connection |
| Existing `src/tools/scoped/dispatch.rs`, Memory builtins and builtin registry wiring | Resolve dependency before approval, admit before invocation, preserve admitted completion |
| Existing thinker/assembler/retrieval/engine and global helper consumers | Replace persistent concrete Memory service captures with runtime access |
| Existing compression/Dream/producer/reconciler/reembed paths | Stop scheduling, inherit operations, retain/join every real task |
| Existing gateway Memory/graph handlers and session-end emission | Explicit runtime admission; raw/Memory split |
| New `src/builtin_tools/memory_runtime.rs`; startup `builder/memory_runtime.rs`, `builder/startup_resources.rs` | Existing Tool-path controls, cold bundle factory, common boot cleanup |
| New `tests/memory_runtime_lifecycle.rs`, `qa/memory_lifecycle/`; existing same-module tests | Real SQLite/Markdown, dispatch, startup and isolated live-server evidence |

### Source Census Required by Execution / 实施前清单

Commit a bilingual `docs/superpowers/runs/2026-11-01-memory-runtime-lifecycle-run.md` authorization/baseline section before production edits. Inventory each Memory `spawn`, `spawn_blocking`, concrete backend/provider capture and global slot with its file/function, owner, admission boundary and test. Update this ledger with each task; a count alone is not completion evidence.

Starting facts: `MemoryBackend = Arc<SqliteMemoryBackend>`; there are 71 synchronous connection access sites plus the `with_conn` blocking site in the audited SQLite implementation. Recount against the execution baseline. `MarkdownSessionStore` independently opens the same `metadata.db`; its connection remains raw-owned. Enumerate caches and indirect indexer/retrieval captures as well as literal `MemoryBackend` references. / 不把别名当 trait；不以搜索不到某个字段名替代消费者清单。

## Shared Interfaces / 跨任务契约

New names below are implementation decisions, not claims about existing APIs. / 以下为本计划确定的新接口，不代表当前已有实现。

- `MemoryRuntime::new(config: MemoryRuntimeConfig, ownership: Arc<OwnershipTree>) -> Arc<Self>` constructs a **cold** owner. `MemoryRuntimeConfig` contains paths/config snapshots and existing external dependencies such as raw storage/provider construction inputs, never an already-open Memory bundle.
- `MemoryRuntime::start(self: &Arc<Self>) -> Result<MemoryStatus, AlephError>` and `MemoryRuntime::stop(self: &Arc<Self>) -> Result<MemoryStatus, AlephError>` wait on a source-owned transition. `MemoryRuntime::status(&self) -> MemoryStatus` has no side effects.
- `MemoryState`: `Off`, `Starting`, `Running`, `Draining`, `Failed`. `MemoryStatus` contains state and optional historical first-error text; it does not invent a generation counter. Cleanup progress stays private.
- `MemoryRuntime::capture(self: &Arc<Self>, reference: &Reference) -> Result<MemoryHandle, ResolveError>` uses the existing `src/capability/facade.rs::Reference` and captures the exact Resource binding while Running. `MemoryHandle::admit(&self) -> Result<MemoryOperation, ResolveError>` validates that capture and admits atomically. There is no implicit refresh method.
- `MemoryOperation` is a cloneable RAII token for **one admission**. Cloning retains the same operation, not a second admission. `MemoryOperation::scope<F: Future>(self, future: F)` is an async method returning `F::Output`; it exposes the already-admitted operation to legacy Core Tool handlers through a narrow task-local scope. Children explicitly inherit it; Tokio task-local state is not assumed to propagate.
- `MemoryOperation::spawn<F>(&self, future: F) -> Result<(), AlephError>` where `F: Future<Output = ()> + Send + 'static` registers a real child with the concrete Memory task owner before returning. `MemoryOperation::run_blocking<F, R>(&self, work: F) -> Result<R, AlephError>` where `F: FnOnce() -> Result<R, AlephError> + Send + 'static`, `R: Send + 'static`, keeps the token inside the actual closure and the real join handle in the owner. Dropping a result waiter cannot detach it.
- The runtime stores one source-owned start/stop transition task and typed retained Memory tasks. Reap completed children during Running; do not accumulate completed handles forever. `JoinSet` may be used inside this concrete owner, but cancellation must never drop the set or abort unfinished children.
- `MemoryBundle` privately owns the concrete SQLite backend, embedder/manager, services, indexers, orientation, caches and worker state. Concrete service access is crate-private through an admitted `MemoryOperation`; no long-lived consumer can retrieve an unguarded bundle/backend clone. Existing standalone storage tests remain supported.

## Task 1: Preserve Canonical Tool Dependency Metadata / 统一工具依赖契约

**Files:** Create `crates/aleph-slot-api/src/capability.rs`; modify `crates/aleph-slot-api/src/{lib,descriptor,tool}.rs`, `src/capability/{descriptor,backend}.rs`. Test in the existing slot-api descriptor/tool tests and backend tests.

**Interfaces:** Move the existing pure `CapabilityId`, `Service` and `ServiceRelation` definitions unchanged to slot-api and re-export them from Core; do not create twin wire types. Add serde/JsonSchema derives there, with snake_case relation names and the existing namespace/name ID fields. Add `services: Vec<Service>` to `ToolCapabilityDescriptor` and `ToolDefinitionMetadata`, serde default/skip empty. Existing conversions preserve it. `to_descriptor` copies canonical services.

- [ ] **Step 1 — RED:** Add `required_resource_roundtrips_canonical_metadata` and `legacy_empty_services_preserve_fingerprint`. Assert a `Requires` dependency survives definition→metadata→descriptor→Capability conversion; old JSON without the field decodes to empty; empty-services serialization and existing replay-safe fingerprints remain byte-for-byte unchanged. A nonempty dependency changes the fingerprint, including when `Replay::Safe` applies.
- [ ] **Step 2 — Observe RED:** Run gated `cargo test -p aleph-slot-api` and gated `cargo test -p alephcore --lib capability::backend`. After compilation is possible, record a failing roundtrip assertion rather than treating a missing field compiler error as sufficient evidence.
- [ ] **Step 3 — Implement:** Make the pure-type move and metadata propagation. For nonempty services, extend the existing canonical fingerprint input with a version-tagged canonical services representation; retain the existing exact input for empty services. No dependency name list or second descriptor map.
- [ ] **Step 4 — GREEN:** Re-run those two commands with separate fresh gates. Test exact canonical bytes for empty metadata and deterministic ordering for service fingerprints; inspect all `matches_definition`/`from_definition`/`from_descriptor` conversions.
- [ ] **Step 5 — Commit:** `feat(capability): preserve required resource metadata`.

## Task 2: Atomic Targeted Binding Validation and Rotation / 定向轮换与原子校验

**Files:** Modify `src/capability/{ownership,versioned_registry}.rs`, `src/tools/registry.rs`; same-module tests.

**Interfaces:** Add `OwnershipTree::rotate_capability(&self, id: &CapabilityId) -> Result<(), OwnershipError>`. Define a crate-private `CapturedBinding` containing the captured `CapabilityId`, `VisibilityScope`, `OwnerGeneration`, `OwnerRef` and `LifetimeScope`, and `with_current_bindings<R>(&self, captures: &[CapturedBinding], commit: impl FnOnce() -> R) -> Result<R, BindingValidationError>`. Preserve existing single-binding API by delegating to the same validation logic. Add `ToolHandlerRegistry::with_current_capture<R>(&self, capture: &CanonicalEntryLease, commit: impl FnOnce() -> R) -> Result<R, ResolveError>` for synchronous validation+commit; it must exclude canonical/provider mutation until commit returns, not merely load an ArcSwap snapshot.

- [ ] **Step 1 — RED:** `rotation_invalidates_all_memory_scopes_only` registers at least two Memory visibility scopes plus unrelated and control bindings; after rotation all old Memory captures fail, others remain identical, new scopes use the new tree epoch. `plural_validation_has_no_partial_commit` asserts callback count is zero when either capture is stale. Test revoked/disposed and checked generation exhaustion without mutation or publication.
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --lib capability::ownership`; gated `cargo test -p alephcore --lib tools::registry`. Use barriers/controlled mutation to prove no commit passes after withdrawal, and no callback runs twice.
- [ ] **Step 3 — Implement:** Hold the existing tree mutex once for rotation/validation; publish targeted ownership notifications. Add a read-only synchronous mutation-gate callback to `VersionedRegistry` if needed. Inspect every provider/registry mutation lock order before adding this path; reuse one ordering for publishers and readers. Never call plugin code, await, perform I/O or re-enter a registry under the gates. Snapshot loading alone is not an admission fence.
- [ ] **Step 4 — GREEN:** Repeat focused commands, including existing register idempotence/provider withdrawal tests. A no-op read does not increment registry revision or publish an event. Document the complete source→registry/provider→tree→entry order and verify no inverse acquisition exists.
- [ ] **Step 5 — Commit:** `feat(capability): rotate resource bindings atomically`.

## Task 3: Make the Memory SQLite Connection Truthfully Closeable / 专属连接关闭边界

**Files:** Modify `src/memory/store/sqlite/{mod,dream_kv,sessions,dream_reports,recall_signals,embedding_meta,memory_write_decisions,query_filed}.rs`, `src/memory/store/sqlite/notes/store_impl.rs`; new private lifecycle tests under this module. Do not modify `MarkdownSessionStore` ownership.

**Interfaces:** Private holder `Arc<Mutex<Option<Connection>>>`; `SqliteMemoryBackend::with_connection<R>(&self, work: impl FnOnce(&mut Connection) -> Result<R, AlephError>) -> Result<R, AlephError>`; existing `with_conn` signature remains compatible. Crate-private `close(&self) -> Result<(), AlephError>` is idempotent after actual close and never refills an old backend.

- [ ] **Step 1 — RED:** Add `memory_connection_closes_while_raw_archival_continues`, `closed_backend_rejects_sync_and_blocking_access`, and `failed_close_retains_same_connection_for_retry`. Open both real stores in one temporary root, write/read raw after Memory close, assert integrity through the raw owner, and prove a separately constructed replacement backend works while the old one remains closed. Use a private close-failure seam only for deterministic recovery, not a public test API.
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --lib memory::store::sqlite`; gated `cargo test -p alephcore --lib memory::store::markdown`. Record a runtime failure showing old storage remains usable/close is a no-op before implementing the holder.
- [ ] **Step 3 — Implement:** Migrate all inventoried sync sites and `with_conn` through checked access. On `Connection::close` failure put back the **returned actual connection** and propagate the error. Never delete a WAL/file or interpret raw's open connection as Memory close failure. Keep transaction and storage-trait semantics unchanged.
- [ ] **Step 4 — GREEN:** Run both suites, exercise transactions and vector-backed NoteStore tests, and recount remaining raw connection locks. Runtime admission and blocking cancellation are completed in Task 4; this task alone does not claim lifecycle safety.
- [ ] **Step 5 — Commit:** `refactor(memory): add checked closeable sqlite access`.

## Task 4: Implement the Concrete Runtime and Admitted Work Owner / Runtime 与真实工作所有权

**Files:** Create `src/memory/runtime/{mod,state,operation,bundle,tasks,tests}.rs`; modify `src/memory/mod.rs`; use existing synchronization abstractions.

**Interfaces:** Implement the Shared Interfaces above. A private source-owned transition retains the partially initialized bundle, task handles, first error and cleanup progress. `MemoryRuntimeConfig`'s factory inputs are consumed only during start; no Memory connection/worker is created by `new` or `status`. Wire the bundled backend's `with_conn` through the already-admitted operation's `run_blocking`; set the same operation context inside the blocking closure with task-local `sync_scope`. Bundled access without that operation fails explicitly; standalone `SqliteMemoryBackend::new` fixtures retain their independent storage contract. Do not let an unguarded legacy `spawn_blocking` path remain reachable from production bundled work.

- [ ] **Step 1 — RED:** Add `cold_off_opens_no_memory_resources`, `cancelled_stop_keeps_transition_and_exact_task_ids`, `concurrent_stop_waiters_share_one_drain`, `blocking_work_outlives_cancelled_request`, `stop_error_is_sticky_but_cleanup_retry_reaches_off`, and `restart_never_revives_old_handle`. Use real temp stores and deterministic barriers. Assert no `Off` before a blocked closure releases, one resource creation per start, exact same IDs after waiter cancellation, and no repeated completed writes on retry.
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --lib memory::runtime`. Establish at least one runtime RED for pending/cancellation/stale admission after minimal compilable scaffolding; a missing new API is insufficient.
- [ ] **Step 3 — Implement:** Source-gated state/admission, targeted rotation from Task 2 and explicit connection close from Task 3. Rotation/exhaustion failure closes admission and returns a truthful failure; never publish Running/Off after an incomplete transition. Stop scheduling first, observe workers/children, wait for admitted work, then drop services/caches/provider references and close Memory connections. The observer must continue reaping while waiting, so children cannot deadlock behind a drain lock. No lock across await.
- [ ] **Step 4 — GREEN:** Repeat runtime tests. Test start while Draining returns busy without opening a bundle, stop while Starting joins/cancels initialization through the same owner, and no status call restarts/retries work. Historical first error survives reaching Off; explicit start creates a new bundle. Keep public controls unregistered until Task 9.
- [ ] **Step 5 — Commit:** `feat(memory): own runtime admission and draining`.

## Task 5: Compose the Resource and Capture Tool Dependencies / Resource 接线与工具准入

**Files:** Modify `src/capability/{facade,backend,zahir_facade,projection_host}.rs`, `src/tools/{registry,scoped/dispatch}.rs`; corresponding same-module tests.

**Interfaces:** Replace mandatory Tool-only lease evidence with `BackendSourceEvidence::Tool { canonical_entry: CanonicalEntryLease, provider_generation: u64 }` or `BackendSourceEvidence::MemoryResource(Weak<MemoryRuntime>)`; migrate the old public provider-generation field into Tool-specific evidence rather than fabricating a Resource provider generation. Add a captured required-Resource lease for Memory-dependent Tools. Source identity compares `Weak::ptr_eq`, not a second nonce/generation. Preserve `ResolveError`'s existing closed variants. Add memory-aware host/facade construction while existing Tool-only constructors delegate with no Memory source. The stable Resource ID is `CapabilityId { namespace: "aleph/resources".into(), name: "memory.runtime".into() }`, kind `Resource`; controls have no `Requires` dependency.

- [ ] **Step 1 — RED:** `memory_tool_captures_resource_before_approval`, `admitted_memory_result_survives_resource_stop`, `stale_tool_does_not_reacquire_after_restart`, and `resource_is_discoverable_without_fake_tool_entry`. In the first, pause approval, stop/start, approve, and assert handler count remains zero. In the second, admit, block handler, start stop, finish handler, assert the result is delivered and stop waits. Verify unrelated/plugin withdrawal still fails closed before and after handler execution.
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --lib capability::zahir_facade`; gated `cargo test -p alephcore --lib capability::projection_host`; gated `cargo test -p alephcore --lib tools::scoped`. Assert behavior, not a source-string match.
- [ ] **Step 3 — Implement:** Copy `services` from canonical descriptors; resolve Tool+Resource capture before human waiting. Immediately before invoking, hold Memory source gate→registry/provider gate→one plural tree-validation section→entry commit to issue the operation. Execute handler outside locks in `MemoryOperation::scope`. **After invocation**, validate Tool/provider publication using the captured Tool evidence, but do not re-admit or reject an already-admitted result solely because the Resource rotated during drain. Non-Memory dispatch keeps existing semantics.
- [ ] **Step 4 — GREEN:** Run focused suites. Resource resolution/admission is unavailable outside Running; status discovery remains possible Off. Include Resource in the existing real host projection path; registry revision remains registry-only, ownership invalidation triggers refresh without inventing a global cursor. No second descriptor store, fabricated `CanonicalEntryLease`, or new generic kind backend.
- [ ] **Step 5 — Commit:** `feat(capability): capture memory resource dependencies`.

## Task 6: Migrate Tools, Thinker and Long-lived Consumers / 迁移工具与上下文消费者

**Files:** Modify `src/builtin_tools/{memory_search,memory_explore,memory_trace,note_graph_query,governance_metrics,remember,flag_user_correction,memory_reflect,note_orient}.rs`, `src/builtin_tools/note_manage/`, `src/executor/builtin_registry/{config,builder,registry}/`, `src/memory/notes/query_filer/filer.rs`, `src/memory/{assembler,note_retrieval}/`, `src/thinker/memory_context_provider/`, `src/gateway/execution_engine/engine.rs`, `src/gateway/telecom/owner_memory_read.rs`, `src/orchestrator/harness_bridge/` injection sites; matching existing tests.

**Interfaces:** Long-lived consumers store `Arc<MemoryRuntime>`, not concrete backend/services. Top-level calls capture/admit once; internal service functions receive `&MemoryOperation`. Generic NoteStore/indexer/retrieval implementations remain reusable inside the bundle. Canonical Memory data Tool descriptors declare `Requires(memory.runtime)` at their own definition; no separate tool-name list.

- [ ] **Step 1 — RED:** Add `off_chat_context_skips_memory_without_opening_bundle`, `old_tool_object_cannot_bypass_new_resource_lease`, `admitted_note_manage_uses_one_bundle`, and `remember_off_reports_unavailable_without_side_effects`. Exercise actual builtins and thinker constructors with a real runtime/raw fixture; inspect persisted notes and resource counts. Distinguish optional chat enrichment (skip while Off) from explicitly requested Memory actions (error).
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --lib builtin_tools`; gated `cargo test -p alephcore --lib thinker::memory_context_provider`; gated `cargo test -p alephcore --lib executor::builtin_registry`.
- [ ] **Step 3 — Implement:** Move service/cache construction into `MemoryBundle`. Remove concrete Memory install-once globals and migrate their callers to injected runtime access; leave unrelated slots unchanged. Do not replace them with a global runtime lookup that refreshes a stale handle. Legacy direct builtin execution must explicitly admit or use an already-admitted task context; it cannot bypass canonical dispatch gating.
- [ ] **Step 4 — GREEN:** Repeat tests and gated `cargo check -p alephcore --tests`. Verify borrowed service references/backend clones cannot escape an operation into consumer fields. Optional audit logging must not quietly turn an explicit Memory action into success while Off. Record every migrated concrete capture in the census.
- [ ] **Step 5 — Commit:** `refactor(memory): route consumers through admitted operations`.

## Task 7: Own Periodic and Derived Workers / 周期与派生任务排空

**Files:** Modify `src/memory/compression/service.rs`, `src/memory/flush/mod.rs`, `src/memory/dreaming/mod.rs`, `src/memory/extensions/{scheduler,registry,mcp_adapter}.rs`, `src/memory/events/handler.rs`, `src/memory/reembed.rs`, `src/memory/embedding_manager.rs`, `src/memory/ripple/`, `src/memory/session_reflection/`, `src/bin/aleph-server/commands/start/builder/{handlers/memory,agent_init/tool_catalog_init}.rs`, `src/gateway/server/mod.rs`, `src/gateway/handlers/memory/mod.rs`; existing tests plus runtime tests.

**Interfaces:** Existing periodic start methods gain a private cooperative stop receiver and return their real handle to the bundle. Current iteration work inherits one `MemoryOperation`; stopping suppresses the next tick, not an admitted effect. Request-derived work uses Task 4's owned spawn/blocking boundary. Reembed retains both migration and progress-forwarder handles.

- [ ] **Step 1 — RED:** Add `compression_threshold_and_profile_children_are_observed`, `dream_manual_and_tick_share_admission`, `producer_tick_drains_capture_filter_insert`, `reconciler_stop_observes_current_scan`, and `reembed_stop_observes_migration_and_forwarder`. Use same-module barriers around real operations and SQLite effects; assert all retained handles are observed despite either sibling panic and that no tick starts after the gate closes.
- [ ] **Step 2 — Observe RED:** Separately gated commands: `cargo test -p alephcore --lib memory::compression`, `cargo test -p alephcore --lib memory::flush`, `cargo test -p alephcore --lib memory::dreaming`, `cargo test -p alephcore --lib memory::extensions`, `cargo test -p alephcore --lib memory::events`, `cargo test -p alephcore --lib memory::reembed`. Record nonzero selected counts; add the exact handler test filter if its tests are outside these modules.
- [ ] **Step 3 — Implement:** Retain periodic compression, threshold/profile/flush children, Dream scheduler/manual work, producer/filter/insert, reconciler, signature-check, reembed migration/progress, reflection/ripple and embedding references. Preserve cadence, nonfatal warnings and effect ordering. Replace abort-only reconciler shutdown with cooperative signal plus in-place observation. Provider clones live only inside the bundle/admitted work; no embedding hot-replacement redesign.
- [ ] **Step 4 — GREEN:** Repeat focused commands. Add a cancellation test around reembed stop and a failure test where one worker panics while another completes a real write. Drain all remaining work and retain the first error. Reconcile every spawn site in the ledger, including indirect service spawns and provider tasks, before marking complete.
- [ ] **Step 5 — Commit:** `fix(memory): retain and drain all memory workers`.

## Task 8: Split Raw Session Work and Gate RPCs / Raw 边界与 RPC 准入

**Files:** Modify `src/gateway/session_manager/ops/{emit,tests}.rs`, `src/gateway/handlers/memory/`, `src/gateway/handlers/{memory_config,dreaming,memory_curated,memory_scope}.rs`, `src/gateway/handlers/graph/{query,search,node_detail,manage}.rs`, `src/bin/aleph-server/commands/start/builder/handlers/{memory,config}.rs`; update other Memory handler registrations found by the census.

**Interfaces:** Handler closures retain the runtime accessor and capture/admit for each explicit Memory operation. Existing response schema maps unavailable states to a stable visible `memory_unavailable` error with state data; stale lease continues through existing `ResolveError` mapping. A configuration read remains available; a Memory resource mutation cannot bypass admission or implicitly start the runtime.

- [ ] **Step 1 — RED:** `off_session_end_keeps_raw_and_skips_memory_children`, `off_graph_and_memory_rpc_fail_explicitly`, `admitted_session_end_children_finish_before_off`, and `session_delete_still_omits_resume`. Use actual temporary raw storage and RPC registration/dispatch, not copied handler logic. Assert raw commit before compression/flush when enabled and correct scope/owner attribution.
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --lib gateway::session_manager`; gated `cargo test -p alephcore --lib gateway::handlers::memory`; gated `cargo test -p alephcore --lib gateway::handlers::graph`; include existing dreaming/config tests.
- [ ] **Step 3 — Implement:** Raw append remains session-owned and unconditional under its existing rules. Summary/resume, reflection, curated, capture extensions and compression are Memory-derived, each executing under an inherited operation or being skipped/rejected explicitly when Off. Remove concrete backend/embedder/compression captures from handler closures and engine setters. Do not close raw's connection or disable unrelated MCP/channel work.
- [ ] **Step 4 — GREEN:** Repeat focused suites and an actual registered RPC off→start→stop cycle. Confirm ordinary chat can return a deterministic local-provider response and append raw while Memory is Off; no Memory worker/service is initialized on that path.
- [ ] **Step 5 — Commit:** `fix(memory): preserve raw archival across runtime stop`.

## Task 9: Wire Cold Startup, Controls and Early-error Cleanup / 启动、控制与失败清理

**Files:** Create `src/bin/aleph-server/commands/start/builder/{memory_runtime,startup_resources}.rs`, `src/builtin_tools/memory_runtime.rs`; modify startup `builder/mod.rs`, `builder/subsystems.rs`, `commands/start/{mod,helpers}.rs`, builtin exports/registration and production descriptor definitions. Existing watcher implementation remains the source owner, not a redesign.

**Interfaces:** `StartupResources::new() -> Self` before the first owned acquisition; `StartupResources::cleanup(&mut self) -> Result<(), AlephError>` observes resources installed immediately after acquisition, with source-owned continuation if its waiter is cancelled. `MemoryRuntimeTool` exposes existing canonical Tool-path names `memory_runtime_status`, `memory_runtime_start`, `memory_runtime_stop`; schemas have no data-Resource Requires edge. Add no separate lifecycle RPC policy.

- [ ] **Step 1 — RED:** Add binary tests `early_registration_error_drains_existing_memory_and_host`, `partial_bundle_failure_preserves_original_error`, `returned_server_error_uses_same_memory_owner`, `cancelled_startup_cleanup_retains_actual_handles`, `memory_runtime_signal_waits_for_owned_cleanup`, and `boot_memory_disabled_is_cold_but_raw_alive`. Private same-module seams may inject failures after projection mount, after Memory connection creation and after orientation starts; use the real cleanup implementation and real effects. The signal test holds cleanup pending and asserts it cannot report success or force process exit before the owner finishes.
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --bin aleph-server memory_runtime`; gated `cargo test -p alephcore --bin aleph-server note_maintenance`; gated `cargo test -p alephcore --bin aleph-server note_watcher_drain`. Require runtime RED for early return leaving a task/resource pending, not just grep of the common funnel.
- [ ] **Step 3 — Implement:** Establish the owner before host mount/Memory/raw resources. Put fallible initialization plus `run_until_shutdown` in one async Result boundary; on every return run common cleanup and then return the **original** startup/server error, logging cleanup failures separately. Register each acquired handle immediately, including orientation and rebuild at their spawn point. Attach a successfully initialized bundle atomically; rollback retains partial resources until cleaned. Mount Memory Resource in the existing host composition and register controls only after all prior consumer/worker tasks pass. Config-disabled boot constructs only the cold runtime plus raw/session infrastructure. In `helpers.rs`, remove the unconditional five-second `exit(0)` from the owned shutdown path: signals initiate the same observable cleanup, with no timer reporting success while Memory is still draining. Retain signal tracing. An external QA timeout remains a failed-run guard, not a production stop deadline.
- [ ] **Step 4 — GREEN:** Repeat tests and gated `cargo check -p alephcore --bin aleph-server`. Prove the cleanup host belongs to this boot attempt; do not close another live host/global raw owner. Normal shutdown preserves watcher→maintenance evidence while the encompassing runtime drains its other workers before storage disposal. Drop only warns; no spawned cleanup or false success. Controls retain existing authorization checks and remain callable while Off/Failed.
- [ ] **Step 5 — Commit:** `feat(memory): wire runtime controls and startup cleanup`.

## Task 10: Production Integration and Reproducible Live E2E / 集成与真实服务证据

**Files:** Create `tests/memory_runtime_lifecycle.rs`, `qa/memory_lifecycle/{README.md,probe.py}`; register the integration target in root `Cargo.toml` using the existing test-helpers convention. Use existing QA RPC/config helpers without inheriting ungated Cargo calls. Update the run ledger; no binary/database fixture committed.

**Interfaces:** The test-helpers integration target exercises the real runtime, canonical registry, tool dispatch, raw storage and bundle factory. The live probe accepts an explicit binary path plus SHA-256, owns disposable homes/ports and only signals its exact subprocesses. Provider responses are a local deterministic stub, not a usable external account.

- [ ] **Step 1 — RED:** Add `stop_raw_chat_restart_preserves_data_and_stales_old_calls` and `failed_stop_retry_does_not_duplicate_completed_effects` using real stores. Assert old Tool/Resource handles fail after re-enable, a new handle works, unrelated Tools continue, raw append/chat works Off, and no connection is reused from the old bundle. Include stop concurrent with human approval and an admitted blocking write.
- [ ] **Step 2 — Observe RED:** Gated `cargo test -p alephcore --features test-helpers --test memory_runtime_lifecycle -- --test-threads=1`. Tests must be registered in the root manifest's existing integration-target convention; zero selected is a failure to investigate.
- [ ] **Step 3 — Implement fixture/probe:** Use production status/start/stop Tools over WebSocket. Cover initially disabled boot, start, real note write, stop, rejected Memory request, raw/chat continuation, re-enable, note persistence, then SIGTERM; restart same fixture then SIGINT. Pin `RUSTUP_HOME`/`CARGO_HOME` before scratch HOME; read per-boot tracing offsets. Copy the just-built server to a dedicated artifact path before any other Cargo command can replace it.
- [ ] **Step 4 — GREEN:** Run integration target and gated `cargo build -p alephcore --bin aleph-server`, copy/hash it, then run the probe. Assert signal trace, orderly process exit without watchdog forcing, endpoint/listener removal, Markdown effects, raw/note persistence and SQLite integrity. Retain exact assertion/pass/fail counts and artifact feature provenance. External fixture timeout is a failed test/cleanup guard, never an internal stop guarantee.
- [ ] **Step 5 — Commit:** `test(memory): verify runtime lifecycle through live server`.

## Task 11: Broad Verification, Independent Review and Local Delivery / 全面验证与本地交付

**Files:** Update `docs/superpowers/runs/2026-11-01-memory-runtime-lifecycle-run.md`, `docs/reference/{FEATURE_LOCATOR,MEMORY_SYSTEM}.md`, `docs/reference/memory/NOTES.md`, this plan/spec status, and `AGENTS.md` plus `CLAUDE.md` together for the stale production-host claim. Preserve previous watcher/maintenance/expansion records as history.

**Interfaces:** Existing test/QA commands and source census. No new production feature belongs to this task.

- [ ] **Step 1 — Reconcile coverage:** Require an owner/admission/test row for every inventoried Memory spawn, connection access, concrete capture and global cache. Inspect both normal and returned-error wiring; do not accept only source-string tests or a successful command invocation without effects.
- [ ] **Step 2 — Verify:** Apply affected-file rustfmt with `--edition 2021 --config skip_children=true --check`, `git diff --check`, and separately memory-gated serial Cargo commands:
  1. All focused suites from Tasks 1–10, selected counts recorded.
  2. `cargo check -p alephcore --tests` and `cargo check -p alephcore --bin aleph-server`.
  3. `cargo test --workspace --features alephcore/test-helpers --no-fail-fast -- --test-threads=1`.
  4. `just _stage-shell-placeholders`, then gated `cargo clippy --workspace --all-targets`.
  5. Gated `cargo check -p aleph-desktop-linux --target x86_64-unknown-linux-gnu` and `cargo check -p aleph-panel --lib --target wasm32-unknown-unknown` where the toolchain exists.
- [ ] **Step 3 — Classify blockers before claims:** Baseline clippy fails at `src/exec/secret_patterns.rs:138` because `regex::Regex` does not support the negative lookahead; one existing background-persistence test reproduces the same panic. Keep this distinct from new regressions and the other workspace failures/hangs. A repair needs its own bounded RED→GREEN commit preserving complete/incomplete PEM redaction and following prose; do not delete the rule, disable the lint, add a regex dependency or claim the whole suite shares this cause. Missing Linux Wayland pkg-config cross sysroot is an environment blocker, not native-platform evidence. Stop a hung owned test process only after recording the exact target/elapsed time; interrupted suites are not passes. This feature can be reviewed with disclosed baseline blockers, but broad verification remains incomplete until those are resolved and rerun.
- [ ] **Step 4 — Independent review:** Fresh read-only native Pi reviewer checks final diff, typed contracts, source→registry/provider→tree lock order, no stale refresh, admitted-result semantics, coldness, all workers/storage/raw boundaries, early error preservation and actual logs. Fix concrete findings, run affected checks, and get re-review. Record report verdict and process exit separately; do not treat a SIGTERM process as a successful test command.
- [ ] **Step 5 — Commit and deliver:** `docs(memory): record runtime lifecycle evidence`. Record actual selected/pass/fail counts, gates, live probe artifact/hash/features, review and unrun native platforms. Report current branch/HEAD/cleanliness; preserve local branch/worktree. No automatic push or merge. Mark lifecycle implemented only when Tasks 1–10 and review pass; mark broad validation incomplete if baseline/environment failures persist.

## Plan Self-review / 计划自审

- Coverage: canonical dependencies (1), all-scope generations/atomic gates (2), true close (3), transition/cancellation/blocking ownership (4), stale Tool+Resource admission and admitted completion (5), consumer migration/cold chat (6), every worker family (7), raw/RPC boundary (8), early errors/controls (9), real runtime and signals (10), census/review/truthful broad checks (11).
- Kept existing standalone storage traits and production watcher behavior; no temporary global switch is exposed in intermediate commits. / 中间提交不提前宣称全局开关可用。
- Names/types above are the cross-task contract. Test names specify observable behavior; private fixture mechanics and ordinary Rust bodies remain implementation choices. No plan-review agent is required; this is the author's spec/step/type/coverage review.
- Existing failing workspace/clippy checks and Linux sysroot gap are recorded, not relabeled as passing. Previous 27/27 signal assertions concern the earlier bounded implementation and must not be reused as proof of this new runtime. / 旧证据不冒充新实现验证。
