# Memory note-vault watcher owned shutdown design / 笔记目录 watcher 所有权收尾设计

**Status / 状态:** Unimplemented planning proposal / 未实施的规划提案. This session authorizes documentation and merging the completed Terminal work, not this production change. Explicit execution of the companion prompt in a later session is the approval boundary. / 本轮仅规划；后续明确执行配套 prompt 才构成本设计的实施授权。

## Intent and place in the roadmap / 意图与路线位置

Everything externally composable is a Capability. The target is a small orchestration Core and resource-owning providers that can be discovered, started, stopped and replaced. A disabled provider must not construct its threads, connections, databases or caches. Runtime coldness is distinct from native code unloading: the current distribution compiles built-ins together, and this proposal makes no executable-page or RSS claim.

终极目标是轻量 Core 与可动态组合的 provider。当前 Tool host 已生产挂载；通道已有 source-owned Bound/read-lease 契约但未通用生产挂载；Terminal source lifecycle 和 explicit live-config 已完成。Memory 与 Sandbox 尚未完成 provider 化。下一步先补 Memory 中已存在后台资源的可等待收尾，再设计整体启停，避免在不能证明旧任务结束时允许替换。

This slice is a prerequisite for Memory lifecycle, not the Memory provider itself, not a graph-only disable switch, and not a new Capability kind/backend.

## Source-backed baseline / 源码基线

Baseline after local fast-forward merge: `main` contains `88fe4fc4c`, `cf52bb05a`, `b378b87bd`; the last is the completed Terminal delivery. See [Terminal run](../runs/2026-10-31-terminal-live-config-run.md).

- `src/memory/notes/watcher.rs::NoteVaultWatcher` owns only `_debouncer`. `spawn_note_vault_watcher<S>(indexer: Arc<NoteIndexer<S>>) -> Result<NoteVaultWatcher, AlephError>` establishes the real watch, then discards the `handle.spawn(...)` consumer JoinHandle. The consumer reconciles queued batches until all senders close.
- The pinned `notify-debouncer-full` **0.7.0** implementation has `Debouncer::stop(self)` which sets stop and waits on its debouncer thread. `Drop` only sets stop; it does not join. `stop` itself ignores the native thread's join result. Therefore “stop returned” cannot be advertised as independently observed successful native-thread termination.
- `src/bin/aleph-server/commands/start/mod.rs` builds a shared NoteIndexer, launches a separate detached startup `full_rebuild_all`, and stores `_note_vault_watcher`. The watcher currently lives until scope drop, without explicit join in the common post-`run_until_shutdown` teardown.
- `graph.*` handlers query the shared NoteStore; this is not an independent graph database/provider. The memory backend, raw-memory/session storage and note indexing have other consumers. `memory.enabled` currently gates turn-memory consumption, not all startup allocation. Turning off graph UI or this watcher cannot imply all Memory is cold.
- Sandbox owns execution approval/isolation semantics. Disabling it to save resources would change the security boundary; it is a separate future design, not this slice.

Source references are repository-relative and must be rechecked at execution. `aleph_modularization_plan.md` §0.4 is a dated 2026-10-22 baseline; its former “production transitions only in tests” claims do not override current source and FEATURE_LOCATOR. Its P0–P6 table is directional research, not approval to recreate registries.

## Approaches / 方案取舍

| Approach / 方案 | Benefit / 收益 | Cost / 代价 |
| --- | --- | --- |
| Production Bound channel integration / 通道生产接线 | Advances generic discoverability and invocation | Requires a new architecture boundary across existing legacy callers, ingress and provider lifetimes; unsuitable as a small follow-up |
| Whole Memory/graph provider now / 一次完成 Memory provider | Directly targets global cold boot | Backend/indexer/session/graph consumers share resources; premature switching risks unavailable recall or retained old owners |
| Existing watcher owns shutdown / 先补 watcher 收尾 | Small, testable resource boundary; removes a detached consumer before future stop/replacement | Does not itself add cold Memory boot, runtime toggles, discovery or saved-memory guarantees |

**Recommended by the planner, not yet selected/approved by the user:** the third approach. / **规划者推荐、尚未由用户批准**：第三项。用户下一次显式执行配套 prompt 才接受该取舍。

## Fixed contract / 固定契约

1. Keep the existing `NoteVaultWatcher` as the sole owner. Keep the public spawn signature unchanged. Store the real consumer JoinHandle before returning success. No registry, global slot, provider-enabled cache, owner table or generic supervisor.
2. Add `pub async fn shutdown(&mut self) -> Result<(), AlephError>` on that type. First request stops the producer by moving the existing debouncer into **one retained** `tokio::task::spawn_blocking` stop task calling `Debouncer::stop`. Store this helper handle inside the watcher before awaiting it. Never perform the potentially blocking stop on a Tokio worker and never discard/abort the helper to satisfy a timeout.
3. Wait on stored handles by mutable reference, preserving them when the caller is cancelled. After producer stop returns, await the stored consumer through end-of-channel and complete draining batches already emitted into the channel. Do not interrupt `index_file`, embeddings or a reconciliation batch; do not silently drop an accepted queued batch to accelerate shutdown. Pending, not-yet-emitted debouncer events are not a durable journal; shutdown does not promise those were indexed. The existing startup reconcile remains the recovery mechanism.
4. Successful shutdown means the blocking stop task returned normally and the consumer JoinHandle completed normally. It does **not** mean each batch succeeded: existing per-file errors remain logged and nonfatal. It also does not prove a native thread panic was absent, because the dependency does not expose that result.
5. Observe producer and consumer completion/error separately. Keep source-local terminal failure evidence after a task JoinError, and attempt the other drain instead of turning the first error into success or skipping independent cleanup. Do not re-poll a completed JoinHandle. Once a failure was observed, later shutdown calls still return Err; repeated successful shutdown is Ok and starts no work. Internal field names and the small error accumulator representation are builder choices.
6. Shutdown has no internal timer and no hard latency promise. Draining an accepted backlog or a remote embedder can take time. External cancellation/timeout must leave pending work and its handle owned by the watcher so another call can finish the same drain. No reattachment or restart method is added; a new watcher may be spawned after the old explicit shutdown succeeds.
7. Implement synchronous Drop as a **fallback**, not proof: if explicit shutdown did not complete, issue the dependency's existing nonblocking stop for any still-owned debouncer, warn about unobserved drain/failure, and let an existing consumer finish from channel closure. Drop launches no new cleanup task, blocks on no join and reports no successful shutdown. An already-started blocking helper is not aborted. This fallback cannot keep an owner alive after it is dropped; early-startup-error cleanup remains outside the explicit guarantee.
8. At production boot retain `Option<NoteVaultWatcher>` under a non-underscore name. At the common post-`server.run_until_shutdown(...)` funnel, explicitly await shutdown before resource-owning memory locals leave scope; both normal and returned run-error paths reach the same call. Log a failure and continue existing sibling teardown without replacing the original run result. No Config guard is involved.
9. Do not move the startup `full_rebuild_all` task into this owner in this slice. Do not claim all note writers have quiesced: the startup task and other writers remain separately owned work. A later Memory maintenance design must address their joint lifetime before an actual provider disable/replacement.

## File boundaries / 文件边界

- `src/memory/notes/watcher.rs`: ownership, shutdown, Drop and existing reconciliation behavior. Keep production logic here; no generic lifecycle framework.
- `src/memory/notes/watcher/lifecycle_tests.rs` (new test-only module): retained-handle/cancellation/failure and real-store lifecycle cases. Existing watcher classification and real filesystem effect tests stay in their current module.
- `src/bin/aleph-server/commands/start/mod.rs`: small boot-binding/common-teardown edits only; this is a frozen large file, do not restructure it.
- Add a small `pub(in crate::commands::start) async fn drain_note_vault_watcher(watcher: &mut Option<NoteVaultWatcher>) -> Result<(), AlephError>` helper in existing `builder/subsystems.rs` to name and test the production boundary. It borrows the option in place and must not take/drop it before a cancellable wait. It returns Ok for None and delegates Some to shutdown. This is a convenience function, not another owner.
- `docs/reference/{FEATURE_LOCATOR,MEMORY_SYSTEM}.md`, `docs/reference/memory/NOTES.md` relevant watcher paragraphs and the new run record: update only after observed validation.

## Evidence and tests / 验收证据

Use real temp note roots, a real SQLite NoteStore and actual filesystem watch for effect tests. Private owning-module seams may delay the **real** consumer after an accepted batch or wrap its real handle, preserving its identity and ownership; they may inject panic for JoinError tests. Do not replace reconciliation with a fake, rely on debounce sleeps to orchestrate races, or manually join the task outside shutdown to make an assertion green.

Required evidence: emitted batch reaches the real store before shutdown returns; producer stop helper and real consumer both awaited; cancelled wait retains exact pending handle IDs and retry joins them; consumer panic and blocking-helper panic remain Err on repeat; other drain still attempted; repeated successful shutdown creates no worker; missing root creates neither root nor consumer; Drop fallback cannot be mistaken for joined cleanup; a new watcher after successful old shutdown receives new edits, old consumer has completed; stopped watcher holds no consumer task's indexer Arc; source integration awaits this same owner in the common teardown. Existing external-create/delete, categories, scoped corpora, bulk sentinel, NotFound-vs-other-error semantics must remain intact. Thread panic/error exposure limits are explicitly tested/documented rather than fabricated.

No runtime config API is added, so no live-applied verdict or schema/generated-client changes are required. This is not evidence of full server E2E shutdown under OS signals; distinguish static common-funnel reachability, helper behavior tests and actual live QA in the run record.

## Progression after this slice / 后续顺序

1. **This proposal:** watcher-owned completion evidence.
2. **Separate architecture proposal:** Memory resource census and source-owned maintenance boundary, covering detached startup rebuild, dreaming/reconciler, embedding, shared backend and graph/recall consumers. Decide which optional sub-capability may stop independently; preserve durable memory and model-visible unavailable errors. Only after that design add explicit cold/enable/stop/re-enable behavior and generation fencing.
3. **Separate architecture proposal:** generic ChannelRegistry production declaration/activation/Bound ingress, provider-neutral across all channels; Feishu is one instance, not the Core shape.
4. **Separate architecture proposal:** Sandbox backend resource lifecycle while preserving approval/isolation and fail-closed unavailable semantics.
5. Capability descriptor/lease/Zahir/protocol projections for new sources only after their callable/lifetime evidence is established. Code packaging/process isolation requires independent measurements and design; do not infer native unloading or RSS reduction from a joined Tokio task.

## Non-goals and stop conditions / 不做与停止条件

No new config field, watcher on/off switch, Memory/graph disable, backend close, startup-rebuild takeover, ProviderHandle integration, descriptor/lease/Zahir/MCP/ACP, second registry or owner map, channel changes, Sandbox change, dependency/lockfile change, harness/effects/replay/scheduler work, dynamic library loading, process isolation or RSS promise. Terminal's untested actual WebSocket stop/re-enable visibility interleaving stays disclosed, not silently folded into this task.

Stop and produce a source-backed report if the existing debouncer cannot safely move to the retained blocking stop task on a supported target; if cancellation requires losing its helper handle; if storing/awaiting the real consumer requires a second authority; or if integration needs broad startup restructuring. Do not silently widen this proposal to resolve those blockers.

English mirror: deliver only owned, retryable explicit shutdown for the existing note watcher and the real production drain call. Whole-Memory coldness, runtime capability publication and native reclamation remain future work.

## Planning checks / 规划检查

The source audit independently supported this bounded prerequisite. The actual local fast-forward is recorded in the Terminal run's **Post-delivery integration** section; its initial main baseline remains historical. The combined test command passes four filters after Cargo's `--` to libtest. A listing-only check of the existing compiled test binary selected 7/38/18/22 focused tests and the same union of 85 tests in the combined list. These are current selection counts, not execution results or the expected counts after new tests are added. No watcher implementation, test run or build was performed in this planning phase. / 已独立核对源码与真实合并记录；仅检查现有测试程序的选择集合（85 项并集），不冒充测试通过或未来新增测试后的计数；本阶段未实施 watcher，也未跑测试或构建。

Independent native Pi planning review identified the missing post-delivery merge record and a combined command missing `--`. Both were corrected and re-reviewed; final verdict: **Approved, no issues found**. Related relative links and `git diff --check` passed. This review approves document consistency, not production implementation; later explicit execution of the companion prompt remains required. / 独立规划审查的两项意见均已修正并复审通过；审阅通过不代替后续明确执行授权。
