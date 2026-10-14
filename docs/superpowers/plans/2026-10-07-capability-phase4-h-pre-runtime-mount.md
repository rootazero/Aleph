# Capability Phase 4 H-pre Runtime Mount Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将已批准的 capability host 挂载到真实 Aleph runtime，使真实 run-loop projection 获得同代初始快照、持续变更、lag replacement 与可证明的关闭 drain。

**Architecture:** 复用生产中唯一的 `ToolHandlerRegistry` 作为 handler+descriptor+registry revision authority，并复用同一生命周期的 `Arc<OwnershipTree>` 作为 owner generation / revoke / dispose authority。新增一个 long-lived `ProjectionHost`：每个 consumer 独立订阅、独立 bounded queue；host 从 registry 的单次 `snapshot_state()` 取得冻结的 `RegistryEntry` map 与 cursor，ownership mutation 通过同 mutex 内发送的局部通知进入 host；lag/overflow 统一 fail-closed 到 `Invalidated` + replacement snapshot。run-loop 仍调用原 `join_canonical_tools`，只替换其 registry snapshot 输入；approval、handler dispatch、session event append、插件过滤与 MCP visibility predicate 不改。

**Tech Stack:** Rust / tokio broadcast+mpsc / `ArcSwap` / serde / 现有 `alephcore` capability、tool registry 与 gateway execution engine；不新增 crate 或 feature。

**Spec:** `docs/superpowers/specs/2026-10-07-capability-phase4-h-pre-runtime-mount-design.md`（approved and committed at `052d434f3`, factual path correction committed before implementation starts）

**Worktree:** `/Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up`

## Global Constraints

- H-pre only. Do not implement Gate H ACP inbound server, Gate I Safe Replay, automatic replay, external-effect exactly-once, provider idempotency, or additional capability-kind backends.
- `ToolHandlerRegistry` remains the only callable Tool authority; never create a second registry, descriptor map, handler map, registry revision, owner counter, or durable capability table.
- `OwnershipTree` remains the only owner-generation / revoke / dispose authority; its notification source is transport-only and must not become a session event store, approval channel, or business-event bus.
- Registry cursor, owner generation, and session event sequence are three independent domains. No API or test may use one as the other.
- Registry changes are live notifications, not durable recovery. A lagged receiver must receive invalidation plus a fresh snapshot, never fabricated missing deltas.
- Existing `join_canonical_tools` filters remain authoritative: agent allow-list, slash-skill scope, visible MCP servers, name conflicts, plugin separation, and result-token metadata behavior must not be deleted or duplicated.
- Existing canonical dispatch and approval path remain authoritative. A host invalidation cannot claim that an already-running handler was prevented; the in-flight call keeps its captured handler and completes under the existing cancellation contract.
- Production installation uses the same registry instance already wired at `src/bin/aleph-server/commands/start/mod.rs:224-244`; no parallel registry is allowed.
- Close proof observes actual task/receiver completion. `sleep`, a log message, or `broadcast::RecvError::Closed` alone is not quiescence.
- No Safe Replay or I1 implementation is implied by this plan.
- English commit messages using `<scope>: <description>`.
- Before any Cargo command, apply the repository’s macOS memory preflight from `docs/reference/DEVELOPMENT.md`; below `4194304` KiB, stop without running Cargo.
- After implementation, run scoped tests/checks/clippy and the required real-machine QA; distinguish pre-existing baseline failures by exact failure identity and message.

## Review Focus

1. **Attach boundary race:** a registry mutation between subscription and initial snapshot must be represented by the post-subscribe drain or replacement snapshot, never silently lost. Test owned by Task 3: `attach_receives_initial_snapshot_then_post_subscribe_change`.
2. **Cross-domain invalidation:** `bump`, `revoke`, and `dispose` must notify without inventing a registry revision or session sequence. Tests owned by Tasks 1 and 4: `ownership_changes_emit_inside_mutation_lock` and `revoke_delivers_without_registry_revision_change`.
3. **Frozen generation pairing:** a projection must never pair a handler from one registry generation with a descriptor/cursor from another. Tests owned by Task 2: `snapshot_entry_handler_and_descriptor_share_revision` and Task 6: `run_loop_consumes_host_snapshot`.
4. **Backpressure and lag:** both registry broadcast lag and per-consumer queue overflow must converge to invalidation plus replacement snapshot, while another consumer remains live. Tests owned by Task 4: `lag_delivers_replacement_snapshot`, `overflow_isolated_to_one_consumer`, `closing_one_subscriber_does_not_affect_others`.
5. **Shutdown/TOCTOU:** close must await task completion and prevent post-close delivery, while an invocation already holding a handler remains governed by the canonical dispatch path. Tests owned by Task 5: `close_awaits_delivery_task`, `post_close_mutation_is_not_delivered`, `already_running_invocation_is_not_cut`.

---

## File Structure

### New files

- `src/capability/projection_host.rs` — runtime projection host, immutable host snapshot, per-consumer attach handle, bounded delivery and close/drain state.
- `qa/capability_hpre/run.sh` — manually invoked real-machine fixture that boots the real server and proves initial projection, live replacement, ownership invalidation and shutdown completion without making it part of `cargo test`.
- `qa/capability_hpre/drive.py` — deterministic HTTP/RPC driver for the fixture; assertions inspect effects at the real consumer boundary, not only logs.

### Modified files

- `src/capability/ownership.rs` — add local ownership-change event type/channel; emit under the same `Mutex<OwnershipInner>` linearization boundary as `bump`, `revoke`, `dispose`.
- `src/capability/zahir_facade.rs` — add one shared snapshot-entry projection helper using the existing backend predicate and ownership generation lookup; no second filtering rule.
- `src/capability/mod.rs` — register `projection_host` and add its process-global status slot if the existing slot roster requires it.
- `src/bin/aleph-server/commands/start/mod.rs:224-269,3903-3915` — mount the host after the real registry exists and close/drain it before registry-owning teardown.
- `src/gateway/execution_engine/run_loop/inner.rs:819-850` — consume the host’s frozen entry snapshot at the existing canonical join point; leave `join_canonical_tools` and all predicates unchanged.
- `docs/reference/FEATURE_LOCATOR.md` — record H-pre’s actual production consumer, mount status, and explicit H/I deferred boundary.

### Explicitly do not modify

- `src/harness/`.
- `src/session/store.rs`, `src/session/events.rs`, session sequence semantics, or `SessionService::emit_batch`.
- `src/gateway/mcp_face/mod.rs`’s canonical call/approval path; its existing `tools/list_changed` notification is not promoted into a capability durable source.
- `src/tools/registry.rs`’s registry authority or its 256-slot broadcast contract, except a narrowly justified read-only accessor only if tests prove the existing public API cannot support the host.
- Any ACP inbound server, Safe Replay, provider retry, or external-effect implementation.

## Interfaces and invariants between tasks

The following names are the plan’s concrete interfaces. New names are marked **NEW**; existing names are not to be renamed.

```rust
// src/capability/ownership.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipChange {
    Bumped { scope: LifetimeScope, generation: OwnerGeneration },
    Revoked { id: CapabilityId },
    Disposed { scope: LifetimeScope },
}

impl OwnershipTree {
    pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<OwnershipChange>;
}

// src/capability/zahir_facade.rs — NEW helper
impl ZahirFacade {
    pub fn snapshot_entries_in_scope(
        &self,
        scope: &Scope,
    ) -> (std::sync::Arc<std::collections::HashMap<String, RegistryEntry>>, Cursor);
}

// src/capability/projection_host.rs — NEW
#[derive(Clone)]
pub struct HostSnapshot {
    pub entries: std::sync::Arc<std::collections::HashMap<String, RegistryEntry>>,
    pub registry_cursor: Cursor,
}

pub enum ProjectionEvent {
    Snapshot(HostSnapshot),
    Change(CapabilityChange),
    Invalidated,
}

pub struct ProjectionHost { /* authority refs + current snapshot + task state */ }

impl ProjectionHost {
    pub fn mount(
        registry: ToolHandlerRegistry,
        tree: std::sync::Arc<OwnershipTree>,
    ) -> std::sync::Arc<Self>;
    pub fn current_snapshot(&self) -> HostSnapshot;
    pub fn attach(&self, scope: Scope) -> ProjectionHandle;
    pub async fn close_and_await(self: std::sync::Arc<Self>);
}

pub struct ProjectionHandle { /* one bounded consumer queue */ }

impl ProjectionHandle {
    pub async fn recv(&mut self) -> Option<ProjectionEvent>;
    pub fn cancel(&self);
    pub async fn close(self);
}
```

The implementer may choose the exact internal task layout, but not the externally observable ordering: subscribe before snapshot; initial snapshot before post-subscribe changes; invalidation before replacement snapshot; cancel/close await actual delivery completion; closed host never delivers later mutations.

`HostSnapshot.entries` is a frozen map of `RegistryEntry`, so handler and descriptor come from one `RegistrySnapshot` generation. `registry_cursor` is only the registry cursor. Ownership changes carry no registry cursor; they force invalidation and a replacement snapshot whose registry cursor is read independently.

---

## Task 1: Add atomic ownership invalidation notifications

**Files:**
- Modify: `src/capability/ownership.rs` (the `OwnershipInner` and `OwnershipTree::{bump,revoke,dispose}` implementations)
- Test: `src/capability/ownership.rs` existing unit-test module

**Interfaces:** Produces `OwnershipChange` and `OwnershipTree::subscribe_changes()` for Task 3. Existing return values of `bump`, `revoke`, and `dispose` do not change.

- [ ] **Step 1: Write failing tests**

Add tests with exact names:

- `ownership_changes_emit_inside_mutation_lock` — subscribe, mutate each of `bump`, `revoke`, `dispose`, and assert the matching event is immediately available; use a concurrent observer or a test hook to prove the event is not published before the protected state mutation is visible.
- `revoke_and_dispose_notifications_are_specific` — register two capability/lifetime bindings, revoke one id and dispose one lifetime, and assert event payloads identify the exact id/scope.
- `ownership_notification_does_not_change_generation_or_session_seq` — compare `OwnerGeneration` before/after subscription and verify no session event is appended by the ownership source.

Run the focused ownership tests; expected initial failure because `OwnershipChange` and `subscribe_changes` do not exist.

- [ ] **Step 2: Implement the smallest local source**

Add one `tokio::sync::broadcast::Sender<OwnershipChange>` to the ownership authority. Create it in `OwnershipTree::new`. Subscribe by cloning a receiver. In `bump`, `revoke`, and `dispose`, hold the existing ownership mutex through both the state mutation and synchronous `send`; emit only after the mutation has established the new state. Keep the sender local to ownership; do not route through `GlobalBus`, `SessionEventStore`, or approval.

`bump` emits the returned generation and scope; `revoke` emits only when the existing method reports a real state transition; `dispose` emits only when it reports a real disposal. Preserve existing irreversible and claim semantics.

- [ ] **Step 3: Run focused tests**

Run the ownership module tests. Expected: all existing ownership tests and the three new tests pass; no registry cursor or session sequence is changed.

- [ ] **Step 4: Commit**

```bash
git add src/capability/ownership.rs
git commit -m "capability: publish ownership invalidations"
```

## Task 2: Add one-generation facade entry snapshots

**Files:**
- Modify: `src/capability/zahir_facade.rs`
- Test: `src/capability/zahir_facade.rs` existing unit-test module

**Interfaces:** Produces `ZahirFacade::snapshot_entries_in_scope(&Scope) -> (Arc<HashMap<String, RegistryEntry>>, Cursor)` for Task 3 and the run-loop adapter. It must use the existing `ToolBackendAdapter::in_scope` predicate and `ToolHandlerRegistry::snapshot_state()`.

- [ ] **Step 1: Write failing tests**

Add:

- `snapshot_entry_handler_and_descriptor_share_revision` — register a tool, obtain the entry snapshot and cursor, assert each descriptor revision is not greater than the snapshot cursor and that the handler definition matches that descriptor; replace/unregister and assert a newly obtained snapshot changes atomically.
- `snapshot_entries_in_scope_filters_revoked_without_second_predicate` — register default ownership, revoke one id, and assert the helper excludes only that id while preserving another id.
- `snapshot_entries_in_scope_keeps_registry_and_owner_domains_separate` — bump ownership without registry mutation and assert the returned registry cursor is unchanged while the visibility result is recomputed.

- [ ] **Step 2: Implement the helper**

Read `snapshot_state()` once. Filter entries with the already shared kind/namespace predicate and the same ownership visibility lookup used by `describe`. Return the cloned `Arc<HashMap<String, RegistryEntry>>` plus `Cursor(snapshot.revision())`. Do not call `entries_snapshot()` plus `revision()` separately, and do not mutate ownership while taking the snapshot except the already-established lazy reconciliation path where the existing facade contract requires it.

If the helper needs an internal accessor, keep it in `ZahirFacade`; do not expose a new generic registry authority.

- [ ] **Step 3: Run focused tests and commit**

Run the facade tests and the existing capability facade tests. Expected: existing describe/resolve/subscribe/project behavior remains unchanged and the new helper tests pass.

```bash
git add src/capability/zahir_facade.rs
git commit -m "capability: expose atomic projection entries"
```

## Task 3: Build the long-lived per-consumer ProjectionHost

**Files:**
- Create: `src/capability/projection_host.rs`
- Modify: `src/capability/mod.rs`
- Test: `src/capability/projection_host.rs` unit tests

**Interfaces:** Consumes Task 1 ownership receiver and Task 2 atomic entry snapshot. Produces `ProjectionHost`, `ProjectionHandle`, `HostSnapshot`, and `ProjectionEvent` as specified above.

- [ ] **Step 1: Write failing tests for attach ordering and initial snapshot**

Add:

- `attach_receives_initial_snapshot_then_post_subscribe_change` — register before attach, attach, mutate after the host’s subscription linearization point, and assert event order is initial `Snapshot`, then the change or an explicit replacement; no post-subscribe mutation is absent.
- `attach_snapshot_is_handler_descriptor_atomic` — assert the first snapshot’s handler and descriptor correspond to one registry revision.
- `stale_cursor_is_not_durable_recovery` — construct a new host with a cursor from a previous host and assert the first event is a fresh snapshot, not replay from the old cursor.

Use deterministic barriers/channels in tests, not sleeps.

- [ ] **Step 2: Implement authority and attach linearization**

`ProjectionHost::mount` stores only the shared registry and ownership authority, plus a current immutable host snapshot. At mount, reconcile the existing facade’s default runtime bindings before taking the first projected snapshot. For `attach`, create the registry and ownership receivers before reading the snapshot. The initial `Snapshot` is enqueued before any post-subscribe event. A consumer’s supplied cursor is a hint only; it must never promise durable recovery.

The host’s registry receiver must distinguish `Lagged` from `Closed`; closed registry state is read via `registry.is_closed()` / `snapshot_state().is_closed()` and cannot be inferred from a receiver close alone.

- [ ] **Step 3: Implement registry and ownership fan-out**

Use one host task to observe registry mutations and ownership changes, rebuild a `HostSnapshot` from the authority on invalidation, and fan out to each consumer’s bounded queue. Registry changes are mapped through the existing `CapabilityChange` vocabulary. Ownership `Bumped`, `Revoked`, and `Disposed` generate `Invalidated` followed by a replacement `Snapshot`; they do not advance `registry_cursor` themselves. A registry `Lagged` event has the same invalidation/replacement behavior.

When a consumer queue is full, drop its stale pending stream and enqueue exactly the fail-closed replacement sequence for that consumer. Do not block registry publishers or let one slow consumer affect another. Preserve the replacement cursor from the fresh registry snapshot.

- [ ] **Step 4: Run attach and resync tests**

Expected PASS for the three attach tests plus:

- `registry_lag_delivers_invalidated_then_replacement_snapshot` — exceed the real 256-slot broadcast capacity using the actual registry receiver.
- `ownership_bump_invalidates_without_registry_revision`.
- `revoke_delivers_without_registry_revision_change`.
- `dispose_delivers_without_registry_revision_change`.
- `overflow_replaces_backlog_with_invalidated_snapshot`.
- `overflow_isolated_to_one_consumer`.
- `closing_one_subscriber_does_not_affect_others`.

- [ ] **Step 5: Commit**

```bash
git add src/capability/projection_host.rs src/capability/mod.rs
git commit -m "capability: add live projection host"
```

## Task 4: Implement cancellation, close, and quiescence proof

**Files:**
- Modify: `src/capability/projection_host.rs`
- Test: `src/capability/projection_host.rs`

**Interfaces:** Extends Task 3 handles without changing event ordering or authority ownership.

- [ ] **Step 1: Write lifecycle tests**

Add:

- `cancel_awaits_inflight_completion` — hold a delivery future behind a barrier, call cancel/close, assert close does not report completion until the barrier is released and the delivery task joins.
- `post_close_mutation_is_not_delivered` — close and await the host, mutate registry and ownership afterward, and assert the consumer cannot receive an event.
- `registry_close_is_not_false_quiescence` — keep the registry sender alive, close the host explicitly, and assert closure is reported only after host task and per-consumer queues are drained.
- `already_running_invocation_is_not_cut` — hold a handler `Arc` acquired before invalidation, invalidate ownership, and assert the invocation follows the existing tool-service cancellation/result path; the projection event is not represented as a dispatch success/failure.

- [ ] **Step 2: Implement explicit shutdown state**

Make `ProjectionHandle::cancel` stop new delivery for that handle and make `close(self)` await the handle’s delivery completion. Make `ProjectionHost::close_and_await` stop the host task, close or drain all consumer queues, await the task join, and only then return. Use an explicit closed state and cancellation token/oneshot; never use `sleep` and never treat `broadcast::RecvError::Closed` as proof that no mutation can arrive.

After the close linearization point, mutations may update the underlying authorities but cannot enqueue to a closed consumer. Keep in-flight handler ownership independent: the host only projects availability and does not retroactively cancel a canonical dispatch.

- [ ] **Step 3: Run lifecycle tests and commit**

```bash
git add src/capability/projection_host.rs
git commit -m "capability: prove projection shutdown completion"
```

Expected: all Task 3 tests plus the four lifecycle tests pass.

## Task 5: Mount the host in the real Aleph startup and shutdown paths

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs:224-269` and shutdown around `3903-3915`
- Modify: `src/capability/mod.rs` only if the process-global slot/roster needs registration
- Test: startup wiring tests or focused boot census tests where the existing harness supports them

**Interfaces:** Consumes `ProjectionHost::mount`, `close_and_await`, and the existing production `ToolHandlerRegistry` instance. Produces one process-lifetime host, not a second registry.

- [ ] **Step 1: Write wiring test or executable assertion**

Add a focused assertion that the production start branch mounts the host from the same registry passed to the MCP tool service and that the host has an explicit shutdown call on both orderly and fatal `run_until_shutdown` return paths. If the existing boot census is the project’s established way to prove process-global slots, extend that census rather than inventing a second roster.

- [ ] **Step 2: Implement the mount**

At the existing registry assembly point, retain the existing `Arc<ToolHandlerRegistry>` and create one `Arc<OwnershipTree>` for the runtime. Build/mount the `ProjectionHost` after the registry is fully available and before request loops can consume it. Do not replace the existing boot logger; it remains a diagnostic tap. The host must be alive until shutdown begins.

At shutdown, call and await `close_and_await` before disposing registry-owning scopes or dropping the shared registry. Ensure the call is reached by the same orderly/fatal shutdown funnel already used by `start/mod.rs`; do not add a signal-only cleanup path.

- [ ] **Step 3: Run scoped boot tests**

Run the relevant `alephcore` bin/lib tests after the memory preflight. Expected: boot census and existing MCP/startup tests remain green; no duplicate registry installation is observed.

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph-server/commands/start/mod.rs src/capability/mod.rs
 git commit -m "capability: mount projection host in runtime"
```

## Task 6: Feed the real run-loop projection without changing canonical filters

**Files:**
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:819-850`
- Modify: `src/capability/projection_host.rs` only if a read-only current snapshot accessor is needed
- Test: `src/gateway/execution_engine/run_loop/tests.rs` focused tests

**Interfaces:** Consumes the host’s frozen `HostSnapshot.entries`. Keeps `join_canonical_tools` signature and all existing filters stable.

- [ ] **Step 1: Write regression tests**

Add:

- `run_loop_consumes_host_snapshot` — install a replacement with distinguishable handler/descriptor identity, obtain the host snapshot, and assert the run-loop projection uses the paired entry rather than independently querying handler and descriptor maps.
- `canonical_dispatch_filters_unchanged` — assert agent allow-list, slash-skill scope, visible MCP server filter, duplicate-name handling, plugin separation, and max-result-token lookup still produce the pre-H-pre result.
- `revoked_projection_is_not_joined` — revoke a capability and assert it is absent from the next projected canonical tool set while unrelated allowed tools remain.
- `already_running_invocation_is_not_cut` — start an invocation, invalidate the owner, and assert the existing invocation’s terminal result remains governed by its original cancellation path.

- [ ] **Step 2: Replace only the snapshot input**

At the existing `mcp_tool_registry()` branch, read the process host’s current immutable snapshot. Pass its `entries` to the unchanged `join_canonical_tools` call. If the host is missing, preserve the existing fail-closed/compatibility behavior explicitly and record the missing-slot reason; do not silently instantiate a second registry or rebuild one from unrelated tool metadata.

Do not alter `join_canonical_tools`, `allowed_tools`, `allowed_names`, `visible_mcp_servers`, plugin compatibility projection, approval setup, handler invocation, or session event emission. A host snapshot is a projection input, not a new dispatch API.

- [ ] **Step 3: Run focused run-loop tests and commit**

Run the run-loop test target that owns the new tests. Expected: canonical filter tests and existing run-loop tests pass; only the intended host snapshot source changes.

```bash
git add src/gateway/execution_engine/run_loop/inner.rs src/gateway/execution_engine/run_loop/tests.rs
git commit -m "gateway: consume capability projection snapshot"
```

## Task 7: Add real-machine H-pre QA and update the feature locator

**Files:**
- Create: `qa/capability_hpre/run.sh`
- Create: `qa/capability_hpre/drive.py`
- Modify: `docs/reference/FEATURE_LOCATOR.md`

**Interfaces:** Consumes the real binary and its real production projection. Produces externally observable evidence, not a unit-test-only claim.

- [ ] **Step 1: Write fixture assertions before implementation-dependent wiring is considered complete**

The driver must assert effects at the consumer boundary:

- initial consumer snapshot contains a boot-registered capability;
- a post-attach registry registration/replacement produces a changed projection and replacement cursor;
- an ownership invalidation produces invalidation plus replacement, without pretending owner generation is a registry cursor;
- a deliberately slow consumer overflows its bounded queue and receives replacement rather than fabricated deltas;
- close returns only after delivery task completion and a post-close mutation cannot reach the consumer.

The fixture must label unsupported setup as `SKIP` only when the environment lacks a required real provider/port prerequisite; it must not convert product assertion failures into skips. It must build the current binary before redirecting `HOME`, and reject a stale binary as existing QA fixtures do.

- [ ] **Step 2: Implement the real-machine driver**

Use the established `qa/` process/cleanup conventions and a deterministic mock provider. Avoid grepping only boot logs: inspect the actual run-loop/MCP projection response or an explicitly instrumented diagnostic response that is itself produced by the real host. Keep the fixture outside Cargo tests because it binds ports and is timing/process shaped.

- [ ] **Step 3: Update documentation**

In `docs/reference/FEATURE_LOCATOR.md`, record the actual mounted consumer path, the host’s lifecycle and invalidation semantics, the QA command, and the explicit deferred status of ACP inbound and Safe Replay. Do not claim all Pi/Aleph provider chains have been audited.

- [ ] **Step 4: Run QA and commit**

```bash
./qa/capability_hpre/run.sh initial
./qa/capability_hpre/run.sh replacement
./qa/capability_hpre/run.sh ownership
./qa/capability_hpre/run.sh close

git add qa/capability_hpre docs/reference/FEATURE_LOCATOR.md
git commit -m "qa: verify H-pre runtime projection"
```

Expected: each scenario reports PASS with effect-level evidence; unsupported external prerequisites report SKIP with a reason and are not counted as product PASS.

## Task 8: Final scoped verification and review packet

**Files:** No product-file changes unless verification finds a defect; update only the H-pre review notes if required.

- [ ] **Step 1: Run the focused suite**

After the memory preflight, run the ownership, facade, projection-host, run-loop and startup tests. Use `cargo test -p alephcore --lib` and the repository’s required binary test target as applicable; do not substitute a single count for failure identity.

- [ ] **Step 2: Run formatting and lint**

Run `cargo fmt --check` with the repository’s existing baseline-drift handling, then scoped `cargo clippy --workspace --all-targets` (or the project-required equivalent after verifying current `DEVELOPMENT.md`). Record unrelated baseline warnings/errors separately.

- [ ] **Step 3: Run all H-pre QA scenarios**

Run the four QA scenarios from Task 7 against the exact binary just built. Capture the first failing assertion, command, and exact error text if any scenario fails.

- [ ] **Step 4: Review the diff against the spec**

Confirm: one runtime registry, one ownership tree, one live host; attach ordering; registry lag and queue overflow replacement; owner invalidation without cursor fabrication; cancellation/close completion; unchanged canonical dispatch; no H/I implementation. Confirm staged-tree contents before any commit and a clean worktree after each commit.

- [ ] **Step 5: Produce the review packet**

Report changed files, commits, exact test/QA commands and results, baseline failures, untested provider/Pi scope, and explicit non-goals. Do not merge, push, or start Safe Replay without a separate user approval.
