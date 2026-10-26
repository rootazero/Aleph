# Gateway Source-owned Runtime Authority — 实施计划 / Implementation plan

日期 / Date: 2026-10-26

**Design / 设计：**[architecture addendum](../specs/2026-10-26-gateway-source-owned-runtime-authority-design.md).

**Status / 状态：**架构扩展已获授权；Stage A bounded Generic Webhook/loopback slice is implemented and committed as `02555b5f1 feat(gateway): own bound channel execution lifecycle`. Stage B–E remain open, and R1 remains separately assessed as NO-GO/deferred until the remaining compatibility/resource evidence is complete.

## 1. Starting point and execution contract / 起点与执行契约

- Reuse current Tasks 1–3 results: cold canonical Bound publication/capture in `src/gateway/channel_registry.rs` and `binding.rs`; provider guarded admission/completion in `src/capability/provider.rs`; full-binding guard in `src/capability/ownership.rs`. Re-read actual code before editing; do not reimplement these primitives or repeat the historical false independent-provider probe.
- This plan supersedes only unexecuted Tasks 4–9 of the [earlier plan](2026-10-26-r1-channel-provider-binding-authority-implementation-plan.md). Historical checkbox state/test counts are not current verification. Do not rewrite the older documents in this task.
- Single canonical entry/map and lock order from the addendum are mandatory. Source owns tickets, actual execution/cleanup and local completion; provider owns state/generation; tree owns binding/generation/tombstones. Test receipts are observations at actual boundaries, never a production permission registry.
- Start with **real Generic WebhookChannel, loopback-only, test Bound composition**. No production Bound registration/config/boot change. Then replacement/unregister, compatibility closure, resource matrices and final reference updates, in that order.
- All implementation writers run serially on these highly coupled source files. Give each stage an independent read-only diff review, fix findings, and verify before advancing. Use only actually available tools/agents; no prescribed unavailable skill or recursive child invocation.
- Before tests, consult applicable repository testing guidance; before **each cargo invocation**, run `python qa/browser_live/check_memory.py` and require exit 0 per `docs/reference/DEVELOPMENT.md`. Record actual command, exit and nonzero test count. Use barriers/oneshots for ordering; watchdog timeouts detect deadlock, not scheduling sleeps as evidence.
- No new dependencies, slot-api→Core dependency, Tool lease changes, Harness changes, general scheduler/envelope or public invocation API. Existing authorization, telecom policy and durable queue semantics are not bypassed or expanded by internal work APIs.
- Keep only specific temporary dead-code allowances for unmounted source-private APIs if required; no blanket suppressions or production mount to create a consumer. Preserve unrelated dirty files; stage explicit reviewed paths, never `git add .`.

**Why this order / 顺序理由：**prove source-to-real-transport-to-cleanup first; replacement cannot be correct before work ownership exists. Compatibility cannot be declared closed before retirement exists; resource matrix cannot substitute for actual constructors. Only verified implementation facts may enter references.

## 2. Stage A — One non-Feishu vertical slice / 单个非飞书纵向闭环

**Files / 范围：**modify `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs`, `src/gateway/interfaces/webhook/mod.rs`; create `src/gateway/channel_registry/test_support.rs` under `#[cfg(test)]`. Inspect `src/gateway/interfaces/webhook/message_ops.rs`, `src/gateway/webhook_receiver.rs`, provider/tree guards, webhook `with_client` callers and existing tests. Modify message_ops only if the client-lifetime adaptation actually requires it; otherwise leave it unchanged.

### A1. Establish actual call/resource census / 先盘点真实入口

- [ ] Derive review inventory from `src/gateway/interfaces/mod.rs`, `impl Channel`/`impl ChannelFactory`, `register_channel_plugins`, plugin loaders, `create_channel_from_config` and direct boot `register_for_boot` callers. Every implementation gets a row or explicit exclusion, not only Feishu and not only configured names.
- [ ] For each row record constructor allocations, resource owner, start/stop, spawned children/join ability, webhook/forwarder, optional outbound/directory/health/retry paths, shared runtime and Legacy/Bound status. First stage only audits Webhook completely; mark others unverified, not migrated. Keep census in review handoff until final evidence publication; it is not a runtime catalog.
- [ ] Confirm actual Webhook client allocation in `new` and extra throwaway construction in `with_client`; record existing direct-send compatibility expectations before changing behavior. A prebuilt injected client cannot establish cold-construction isolation.

### A2. Genuine RED tests / 真正失败的测试

- [ ] Add planned `bound_webhook_construct_and_capture_are_cold`: real `WebhookChannel::new` → source Bound registration → repeated capture/snapshot; receipt at actual reqwest construction must remain zero. The current eager constructor should fail this assertion; a missing-API compile RED alone does not prove runtime behavior.
- [ ] Add `bound_webhook_start_owns_provider_and_forwarder`: source start moves its own captured provider Starting→Ready, starts actual adapter/setup, stores cancellable forwarder JoinHandle, and does not expose raw ingress. Hold actual setup with deterministic barriers; Ready must wait. Same-ID unrelated ProviderHandle is never injected as valid proof.
- [ ] Add `bound_webhook_admission_reaches_actual_peer`: source capture→resolve→admit→one actual `Channel::send`→loopback peer receipt→source completion slot. Bad source/entry/provider instance, wrong generation/visibility/full binding or not Ready means zero ticket and zero peer request.
- [ ] Add `bound_webhook_stop_drains_without_deadlock`: hold peer response after transport begins; stop refuses new tickets and remains pending. Release response; authentic pre-stop completion is Accepted while same-generation Stopping; original work reaches zero, forwarder joins and actual adapter cleanup occurs before Stopped.
- [ ] Add `bound_webhook_owner_change_rejects_completion`: independently bump/revoke/dispose tree binding, and same-nonce replace owner/lifetime. The result must be discarded at acceptance and original work settled; no provider shutdown or source revision change is inferred.
- [ ] Add `bound_webhook_cancelled_observer_keeps_work_owned`, transport-error, mount-failure and cancelled-start tests: caller cancellation never frees a live ticket; failed cleanup retains marker/resources/residual; explicit cleanup retry cannot overlap a still-running driver.

### A3. Implement the minimal real chain / 最小真实链

- [ ] Before claiming independent channel progress, audit every existing holder of the shared `lifecycle_sync`/source gate, including Legacy start/stop, factory construction, replacement teardown, and `start_all`. Refactor each real-I/O path to capture intent under the gate, release it while awaiting channel/provider I/O, and reacquire only for a validated short commit. Add a deterministic mixed Legacy/Bound test that blocks Legacy channel A and proves Bound channel B completion, drain, and lifecycle progress. If any holder cannot be migrated safely, narrow the progress promise and stop Stage A for an architecture amendment.
- [ ] Add entry-local lifecycle marker/phase, original work tickets/drain notifications, finite completion handoff slots and owned task/cancel/join handles. Do not create a ChannelRuntime registry or parallel channel/provider mapping. Adapt existing residual/error types instead of an independent status authority.
- [ ] Implement source-private Bound `start`, `stop`, `restart`, `resolve`, one-attempt typed send and completion using existing provider/tree guards. Keep the implementation centralized even if split into a child module; source identity and map stay in the current registry.
- [ ] Activation: install marker and enter Starting under source gate; drop gates, run actual adapter start/setup and prepare forwarder/mount; reacquire only for marker/current-generation validation and short Ready commit. No network or map await inside provider callback. No Bound raw handler/forwarder dispatch into legacy inbound consumers.
- [ ] Admission: source→tree→provider→entry; validate full current proof, then mint a non-clone ticket. Transfer it to source-owned execution without an intervening cancellation point. Execute I/O outside gates using retained original channel, not a new get-by-ID.
- [ ] Completion: same order, real ticket and original entry only; atomically consume ticket/move result into accepted slot. Known-ended stale/error paths settle original accounting exactly once. A held result observed later cannot modify a successor; transport errors remain Accepted(Err), not silent retry.
- [ ] Stop: commit Stopping/Stopped intent under source gate, release gate before work wait; only after drain acquire channel for stop, cancel/join forwarder and release actual resources. Guard notification wait against lost wakeups. A cold stop must not invent an illegal provider transition.
- [ ] Store operation/execution handles before await; retain ownership across caller cancellation. Cleanup retry first confirms previous driver ended. Panic/task loss with unconfirmed resource descendants stays residual. Do not use Drop to report an unconfirmed live network operation drained.
- [ ] Refactor actual Webhook default constructor cold: defer real client creation to explicit start, and release its last owned runtime client/handler only after source drain. Preserve explicit `with_client` injection without a throwaway default client; label its caller-owned resources outside cold guarantees. Audit cold direct-send callers; do not lazily create a client from send/read to hide missing activation.
- [ ] Add per-instance, test-only observation hooks adjacent to real Webhook client build/drop/request and registry forwarder spawn/cancel/join/ticket/verdict boundaries. Loopback peer proves request arrival. For connection teardown use a controlled non-pooled/Connection-close test and peer termination; do not claim opaque reqwest internal task joins merely because the Client was dropped. Unobservable pooled-resource cleanup remains explicitly unproven.

### A4. Verification and review / 验证审查

Run focused new tests (exact filters once present), then:

```bash
cargo test -p alephcore --lib gateway::interfaces::webhook::
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::channel_registry::binding::tests::
cargo test -p alephcore --lib capability::provider::tests::
cargo test -p alephcore --lib capability::ownership::tests::
cargo check -p alephcore --tests
```

- [ ] Verify every filter matches actual tests; inspect `with_client` and restart regressions, source ownership during cancellation, no await under synchronous guards, and cold-read positive controls. Review must reject effect-free simulated receipts.
- [ ] Stage exit: one real non-Feishu implementation reaches actual peer and source acceptance/cleanup through the sole source, without production mount. Remaining channels/resources are unproven. Suggested commit: `feat(gateway): own bound channel execution lifecycle` (split only into independently tested commits if needed).

## 3. Stage B — Replacement, contract update and unregister / 替换、更新与注销

**Files / 范围：**registry, binding and test_support only. Consume Stage A work ownership; do not retrofit provider or tree state mirrors.

- [ ] Write `bound_retirement_discards_original_completion`, `bound_replace_publishes_whole_tuple`, `bound_unregister_waits_for_cleanup`, `bound_contract_update_retires_old_publication`, `bound_failed_cleanup_retains_incumbent`, `bound_revision_exhaustion_never_half_publishes` before changing behavior. Use held actual requests plus capture barriers; test both completion-before-retirement and retirement-before-completion.
- [ ] Prepare successor privately and validate checked source revision capacity before destructive retirement. Under source gate install marker and retire incumbent; new work and old acceptance must fail immediately. Release gates for original drain/real stop/close/join.
- [ ] Full drain must exclude any still-running original request before successor publication. Delay caller observation of an already-settled result past publication to test stale-result noninterference; do not forge duplicate work tickets to simulate impossible live transport after full drain.
- [ ] After cleanup, acquire publication locks/revalidate marker before first mutation, then atomically publish entire successor/intent/revision or remove incumbent without await. Same textual ID/new provider instance and contract-only new publication both invalidate old proof; unrelated entry publication does not.
- [ ] On failure preserve incumbent identity and honest residual, never publish successor or automatically Ready. Mount metadata may remain but dispatch is fenced. Already closed resources are not resurrected. Explicit same-entry cleanup retry is allowed only after prior operation driver quiesces.
- [ ] Test preflight exhaustion causes no teardown; test another ID consuming capacity during unlocked cleanup causes final PublicationExhausted with stopped incumbent still present, no successor/removal and no wrap. Apply checked prepublication behavior to legacy map mutations touched by the shared publication mechanism.
- [ ] Keep legacy ID/Option compatibility wrappers explicit: they cannot report authoritative Bound success. Reject legacy replacement/unregister of Bound incumbents unless routed through the same explicit Result operation. Bound takeover of Legacy rejects when quiescence cannot be proven; never wrap an already-running legacy channel with a Ready provider.
- [ ] Re-run registry/binding focused suites, `cargo check -p alephcore --tests`, review old entry handoff/mount/accounting cannot reach successor. Suggested commit: `feat(gateway): retire bound runtime publications safely`.

## 4. Stage C — Compatibility closure / 兼容闭环

**Files / 范围：**registry, binding and test_support. Inspect but do not automatically edit callers: `src/gateway/handlers/channel.rs`, `src/gateway/handlers/discord_panel.rs`, boot `subsystems.rs`, health monitor, delivery queue, inbound router, builtin channel tools, webhook receiver and pure slot Channel contract. If a boundary cannot be closed centrally, report a scoped amendment rather than silently migrating callers.

- [ ] Refresh exhaustive entry-point census: raw get/capture, map iteration, list/status/health/directory, start/stop/restart/all, public/private telecom, SMS, normal send/retry/outbox, edit/react/typing/poll/group icon, webhook wrappers/held handlers, inbound forwarder and plugin routes. Search actual current methods/callers, not only this list.
- [ ] Add table-driven tests `bound_compatibility_effect_paths_are_denied`, `bound_held_webhook_handler_denies_dispatch`, `bound_cold_lists_never_call_transport`, `bound_compat_lifecycle_uses_source`, `legacy_positive_controls_unchanged`. Assert real target/peer/effect receives nothing on each rejected Bound route; bool/Option refusal cannot be consumed as fallback permission.
- [ ] All Bound raw Arc exposure stays denied. Source-private admitted typed send remains the sole evidence path; legacy effect helpers refuse Bound before tickets/resources/queue writes. Do not introduce a general invocation envelope to handle optional operations.
- [ ] Bound cold read projections use immutable metadata and actual provider/intent observations from canonical entry, never transport health/directory/client construction. Any stored metadata is captured at registration and explicitly non-authoritative for provider state/generation.
- [ ] Bound start/stop/restart/all dispatch to same source lifecycle rather than acquiring legacy gate across I/O. Health restart respects desired intent, Busy, residual and Disabled; no implicit revival after cleanup failure.
- [ ] Source-owned Bound webhook wrapper denies verify and direct handle calls, including after unmount; forwarder cannot reach unified legacy inbound dispatch. This is deliberately no production ingress, not a completed EventSource/Subscription contract. Legacy mount/forwarding behavior remains compatible.
- [ ] Verify no production `register_bound_channel` caller exists outside tests; no new RPC, Zahir route or boot opt-in. Suggested commit: `fix(gateway): close bound runtime compatibility bypasses`.

Run matching existing suites plus focused bypass tests:

```bash
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
cargo test -p alephcore --lib gateway::delivery_queue::tests::
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
cargo test -p alephcore --lib tools::registry::tests::
cargo check -p alephcore --tests
```

## 5. Stage D — Real resource matrices and second shape / 真实资源矩阵与第二形态

**Files / 范围：**test_support, registry tests and Webhook test observation hooks; no second production channel opt-in. Production defects discovered here return to their owning stage and review before continuing.

- [ ] Add a cold, test-only polling/socket Channel with real loopback TCP connect, actual worker and timer, provider-owned cache initialization and temporary SQLite open. Retain its actual cancellation/JoinHandles and real database ownership/close path. Test server/database infrastructure is separately labelled, not counted as provider allocation.
- [ ] Add `bound_cold_resource_matrix_webhook` and `bound_cold_resource_matrix_polling_socket`: for each Discovered/Cold/Disabled repeat construction/register/list/describe-like snapshot/project/capture/resolve/rejected admission/unknown ID/wrong scope/old-proof reads. Every applicable resource attempt and active delta stays zero, tickets and effects stay zero; no read repairs owner bindings or starts anything.
- [ ] Positive controls explicitly activate and send through the same source; each applicable receipt grows at its real constructor/open/spawn/request boundary. Failure injection at actual client/connect/open boundaries counts attempts even when start fails. Partial activation cleanup failure exposes the real residual; no state-derived zero.
- [ ] Verify close/drop/cancel/join before successful stop and no leaked tasks/temp files after every test. Add cancellation and failed-cleanup matrices, not only happy-path zero counters. If no real websocket, listener, OS thread or token authentication exists in a shape, mark that row N/A; TCP/HTTP success cannot stand in for it.
- [ ] Distinguish `Client` owner drop from all upstream library resources being joined. Claim only observable boundaries; pooled client internals and unsupported production adapters remain explicit unproven items. Do not invent receipts or new dependencies to turn N/A into a pass.
- [ ] Repeat focused matrix filters three times serially, then all registry/Webhook/binding suites. Review a per-shape receipt coverage table, actual positive controls, deterministic scheduling and zero retained work/handles after tests.
- [ ] Stage exit proves one real adapter and a second generic shape, not every social channel, process isolation or executable unloading. Suggested commit: `test(gateway): prove source-owned resource boundaries`.

## 6. Stage E — Final verification, reference evidence and next gate / 最终验证、参考与门禁

**Files / 范围：**after tests/review only, update `docs/reference/GATEWAY.md` and `docs/reference/FEATURE_LOCATOR.md`. Do not amend constitution files, old design evidence or roadmap completion markers automatically; those require their own evidence reconciliation.

- [ ] Inspect full implementation diff against addendum. Confirm a single canonical association, only entry-local operation/tickets/task ownership, no generation mirrors, exact full-binding comparisons, no deadlock on stop/completion, no synthetic receipts or unconfirmed cleanup success.
- [ ] Run final commands serially, memory gate before each cargo:

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo test -p alephcore --lib capability::
cargo test -p alephcore --lib tools::registry::tests::
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::channel_registry::binding::tests::
cargo test -p alephcore --lib gateway::interfaces::webhook::
```

Also re-run Stage C webhook/delivery/health suites and both Stage D matrices. These targeted checks do not replace `docs/reference/DEVELOPMENT.md`'s broader verification set if implementation changes expand to bins/shared interfaces/workspace surfaces. Record skipped broader checks explicitly; a zero-test filter is not a pass.

- [ ] Independently review original source through real transport completion, not just worker summaries. Confirm production Bound/mount/Zahir call graph remains unchanged and slot API does not depend on Core; no Feishu, session durable writes, EffectClaim or scheduler slipped in.
- [ ] Only then publish bilingual references with actual APIs/tests, real command results, receipt coverage and limitations, compatibility denials, Legacy inventory status and the distinction between lifecycle declaration and future operation schema.
- [ ] Assess R1 separately from stage completion. Missing promised execution/resource proof blocks GO; explicitly N/A resource classes stay unproven and prohibit corresponding all-resource/all-channel claims. Even a bounded R1 GO does not authorize R2 Feishu, production ingress/send/mount or automatic replay.
- [ ] Commit only reviewed reference hunks after `git diff --check`. Suggested commit: `docs(gateway): record source-owned runtime evidence`.

## 7. Definition of done and non-work / 完成标准与未做项

- [ ] One source-owned proof reaches a real non-Feishu transport and local acceptance; no caller-created provider or parallel registry.
- [ ] Stop waits for actual work and cleanup without holding completion's gate; retirement discards original results and never affects successor.
- [ ] All Bound compatibility paths are accounted for, cold/disabled reads allocate no applicable provider runtime resources, positive controls validate the measurement, residuals never masquerade as clean success.
- [ ] Provider/source/owner/session domains remain independent; Resource lifecycle declaration never becomes public Callable/send schema.
- [ ] Actual verification and independent review are recorded; references contain only established facts, not TODOs marked complete from design approval.

**Explicit non-work / 明确未做：**no universal scheduler, durable recovery/admission, EffectClaim producer, Safe Replay, external exactly-once, generalized envelope, new CapabilityKind, Feishu implementation, production Bound opt-in/mount, Zahir route, public send/ingress operation, all-channel migration, terminal/memory/sandbox migration, process crash isolation or compiled-code unloading. Stage A also does not claim opaque reqwest pooled-task joins, websocket/listener/token-client receipts, full compatibility closure, replacement retirement semantics, or all-channel resource matrices.

## 8. Stage A evidence / Stage A 证据

The bounded slice uses the existing `ChannelRegistry` entry/map as the only Gateway association authority. It adds entry-local operation/ticket/task ownership, keeps Legacy and Bound real I/O outside `lifecycle_sync`, and retains ProviderHandle generation and OwnershipTree binding validation as independent authorities. Generic Webhook client construction is lazy and cold declaration/capture/snapshot observe zero client construction.

该纵向切片只使用既有 `ChannelRegistry` entry/map 作为 Gateway association authority；新增的是 entry-local operation/ticket/task ownership，Legacy 与 Bound 的真实 I/O 均不持有 `lifecycle_sync`，ProviderHandle generation 与 OwnershipTree binding validation 仍是独立 authority。Generic Webhook client 延迟到显式 start，cold declaration/capture/snapshot 不构造 client。

Recorded verification: `cargo fmt --all --check`, `git diff --check`, `cargo check -p alephcore --tests`, and `cargo clippy -p alephcore --tests -- -D warnings` passed. `cargo test -p alephcore --lib gateway::channel_registry::tests::` passed 54/54, and `cargo test -p alephcore --lib capability::` passed 222/222. The mixed Legacy/Bound test covers start, stop, restart, start-all, factory, replacement, create-replace and unregister progress. Only the non-fatal macOS linker warning `__eh_frame section too large (max 16MB)` appeared. An external reviewer was unavailable because the ACP proxy circuit was open; manual review found no forbidden Feishu, second-registry, new-kind, scheduler, or durable-recovery changes.

已记录验证：fmt、diff、check、clippy 全部通过；gateway filter 54/54，capability filter 222/222。mixed Legacy/Bound 测试覆盖 start、stop、restart、start-all、factory、replacement、create-replace、unregister 的进展。仅出现非致命 macOS linker warning `__eh_frame section too large (max 16MB)`。外部 reviewer 因 ACP proxy circuit open 不可用；手工审阅未发现 Feishu、第二 registry、新 CapabilityKind、scheduler 或 durable recovery 变更。
