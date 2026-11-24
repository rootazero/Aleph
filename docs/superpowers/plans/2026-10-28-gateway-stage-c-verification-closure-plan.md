# Gateway Stage C Verification Closure — Implementation Plan / 实施计划

**Date / 日期:** 2026-10-28  
**Base / 基线:** `main` at `dc278a932`  
**Scope / 范围:** close the two Legacy ExplicitOnly mid-send integration failures and complete an independent Stage C review before Stage D / 先闭合两个 Legacy ExplicitOnly mid-send integration 失败并完成独立 Stage C review，再考虑 Stage D

> **For agentic workers:** use `superpowers:subagent-driven-development` for parallel investigation/review or `superpowers:executing-plans` for serial implementation. Production writers remain serial.

## Goal and rationale / 目标与理由

Make the full `sms_delivery_policy_test` suite pass without weakening its privacy, queueing, replacement, or lifecycle contracts, and obtain an independent read-only review of the Stage C compatibility closure. This is the next task because the current run record names these tests as the first unclosed contract; starting Stage D on known-red lifecycle behavior would make later resource-isolation evidence ambiguous.

让完整 `sms_delivery_policy_test` 在不削弱隐私、队列、替换或生命周期契约的前提下通过，并取得 Stage C compatibility closure 的独立只读 review。当前 run record 已把这些测试列为首个未闭合 contract；在已知 lifecycle 红灯上直接开始 Stage D 会污染后续资源隔离证据。

## Current facts, not assumptions / 已知事实而非结论

- Current failures:
  - `explicit_only_channel_unregistered_mid_send_is_never_persisted`
  - `explicit_only_channel_replaced_mid_send_is_never_persisted`
- Both currently reach their five-second watchdog and fail with `no deadlock: Elapsed(())`.
- `VanishingExplicit::send` awaits `ChannelRegistry::unregister` from inside `Channel::send`.
- `ChannelRegistry::send_on` currently holds the channel `RwLock` read guard across `channel.send(...).await`; Legacy unregister/stop requires the corresponding write path.
- Stage C did not intentionally change Legacy `send`/`unregister`, but there is no clean baseline reproduction yet. Do not call the failures pre-existing until A/B evidence proves it.
- Stage C required suites are GREEN. Independent reviewer `del_mvri61gw_y936` failed with `No API key found for anthropic.`; manual review is not independent approval.

## Architecture guardrails / 架构护栏

- `ChannelRegistry` remains the only Gateway association authority. Reuse existing Legacy registration entries, lifecycle markers, entry-local bookkeeping, `ProviderHandle`, and `OwnershipTree`; add no registry, owner map, descriptor map, runtime table, CapabilityKind, generalized envelope, or public Bound route.
- Preserve the policy captured from the exact channel instance that performed the send. A same-ID replacement or absent registration must not turn an ExplicitOnly failure into queue permission.
- No plaintext ExplicitOnly body may reach live queue rows, dead letters, replay, logs used as receipts, or an Automatic successor.
- Unregister/replacement must not report successful cleanup while the old channel is still running. An explicit failure is acceptable only when the operation has not claimed success and the resulting registration/resource state is honestly observable.
- Do not hold `lifecycle_sync`, the source publication gate, or a synchronous mutex across channel/transport/stop/close/join I/O.
- Do not use `unsafe`, self-referencing guards, detached untracked cleanup, sleeps as synchronization proof, fake receipts, or manual active-work mutation.
- Do not delete, weaken, or merely extend the timeout of the two failing tests. Do not rewrite a self-reentrant case into a different race unless the contract review explicitly concludes the old fixture is invalid and records why.
- A broad `Channel` trait object/API migration, changed public unregister semantics, or a new deferred-cleanup authority requires a separate architecture decision before implementation.
- No Stage D resource matrix, Feishu/R2, production Bound mount, Zahir route, public Bound send/ingress, EffectClaim, Safe Replay, durable recovery, scheduler, terminal/memory/sandbox migration, or `src/harness/**` work.

## Task 1 — Reproduce and establish baseline evidence / 复现并建立基线证据

**Files:** no production changes. Create only investigation notes under `.superpowers/sdd/2026-10-28-gateway-stage-c-verification-closure-plan/`.

- [ ] Record current `main` HEAD and clean status. Run each failing test separately on current `main`, three times serially, with its exact test filter. A filter matching zero tests is a failed verification setup, not a pass.
- [ ] Reproduce at pre-Stage-C baseline `ca03a1206` in a separate temporary worktree. Do not mutate or reset `main`. Apply only the minimum fixture compatibility adaptation needed to execute the same behavioral test at that baseline; keep any such adaptation separate from production code and document it.
- [ ] Compare current and baseline timing, actual send attempt count, registration state, old-channel stop count, replacement presence, queue length, and dead letters. Classify each failure as Stage-C regression, baseline defect, or invalid fixture only from observed evidence.
- [ ] Census every lock and lifecycle call in `send_reported` → `send_attempt` → `send_on` → `Channel::send` → `unregister`/`register` → Legacy stop. Identify which guards are held across each await and where policy markers are captured.
- [ ] Record exact commands, exit codes, passed/failed/ignored counts, and linker warnings. Before **every** Cargo command run `python qa/browser_live/check_memory.py` and require exit 0 / `ALLOW`.

### Suggested current-head commands

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --test sms_delivery_policy_test explicit_only_channel_unregistered_mid_send_is_never_persisted -- --exact --nocapture
python qa/browser_live/check_memory.py
cargo test -p alephcore --test sms_delivery_policy_test explicit_only_channel_replaced_mid_send_is_never_persisted -- --exact --nocapture
```

**Exit criterion:** a written A/B matrix and a source-backed lock/lifecycle explanation. If the same behavior cannot be executed at baseline, report that limitation rather than claiming equivalence.

## Task 2 — Compare fixes and pin executable RED contracts / 比较修复并固定可执行 RED

**Files:** `tests/sms_delivery_policy_test.rs`; focused Registry test support only when observation is otherwise impossible.

- [ ] Present two or three real implementation paths based on Task 1, including their lifecycle and API trade-offs. Reject approaches that silently return before old-resource cleanup, hold a lock across reentrant I/O, or create another authority.
- [ ] If the chosen fix changes public lifecycle semantics or the object-safe `Channel` API, stop for architecture approval before production edits.
- [ ] Strengthen the two tests so success proves behavior, not merely absence of a timeout:
  - unregister case: exactly one old send, old registration absent, old stop/cleanup observed exactly once, zero queue/dead-letter rows;
  - replacement case: exactly one old ExplicitOnly send, intended Automatic successor present and usable, old instance stopped exactly once, zero persisted old payload, and no policy inferred from the successor;
  - both return the original `NotConnected` outcome and do not manufacture an Automatic retry.
- [ ] Add an external concurrent unregister/replacement positive control if needed to distinguish ordinary mid-send retirement from same-task reentrancy. Use barriers/oneshots, not timing sleeps, for ordering.
- [ ] Preserve Automatic queue/drain positive controls and unknown-channel legacy behavior.
- [ ] Run the strengthened tests to executable behavioral RED before production changes. Parser, import, compile, and setup failures must be fixed first and are not contract RED.
- [ ] Commit the tests separately with `test(gateway): pin mid-send lifecycle contracts`.

**Exit criterion:** the chosen approach is explicit, and RED demonstrates the real lifecycle/privacy behavior at actual Registry/queue/channel boundaries.

## Task 3 — Implement the smallest source-compatible fix / 实现最小兼容修复

**Files:** only the owning Legacy Registry/lifecycle path identified in Tasks 1–2; test files from Task 2. Do not pre-authorize unrelated channel implementations.

- [ ] Implement the selected mechanism inside the existing `ChannelRegistry`/Legacy lifecycle authority. Keep association/policy decisions and actual cleanup effects in their current owning layer.
- [ ] Ensure a same-task reentrant unregister/replacement cannot deadlock on the channel read/write lock. At the same time, do not report successful unregistration/replacement before its promised observable state and old-channel cleanup are complete.
- [ ] Keep the original ExplicitOnly policy attached to the in-flight attempt through return and queue disposition. Never re-resolve that policy from a successor or absent ID.
- [ ] Keep all transport and channel stop/close/join I/O outside `lifecycle_sync` and source publication locks. Any short lock section may contain only identity/marker/publication commits.
- [ ] Preserve Bound compatibility denials and source-owned Bound lifecycle. This task fixes Legacy behavior; it must not create a Bound execution route.
- [ ] Run the two focused tests and the external concurrent controls. Repeat the focused tests three times serially to detect intermittent deadlock or cleanup races.
- [ ] Commit the implementation with `fix(gateway): close mid-send lifecycle deadlock` only after the focused tests are GREEN.

**Stop condition:** if no implementation satisfies both reentrant progress and completed-cleanup semantics without a broader API/authority change, stop with the RED evidence and request the architecture amendment. Do not silently narrow the tests.

## Task 4 — Full verification and independent review / 完整验证与独立 review

- [ ] Run the full SMS integration suite, not only the two named tests:

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --test sms_delivery_policy_test
```

- [ ] Re-run the Stage C regression set, each Cargo command preceded by the memory gate:

```bash
cargo test -p alephcore --lib gateway::channel_registry::tests::
cargo test -p alephcore --lib gateway::webhook_receiver::tests::
cargo test -p alephcore --lib gateway::delivery_queue::tests::
cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
cargo test -p alephcore --lib tools::registry::tests::
cargo test -p alephcore --lib gateway::interfaces::webhook::
cargo test -p alephcore --lib capability::
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
cargo fmt --all --check
git diff --check
```

- [ ] Record every actual exit and test count. Run broader workspace verification if shared interfaces or binaries are touched; otherwise explicitly list broader checks not run.
- [ ] Dispatch an independent read-only reviewer for both the new fix and the full Stage C diff `ca03a1206..<final-code-head>`. Review refusal-before-effect, raw handles, cold reads, source lifecycle, Legacy policy capture, lock order, reentrant cleanup, and authority duplication.
- [ ] Use an actually available reviewer model/tool. If unavailable, record the complete error and perform the same checklist manually; do not claim independent approval.
- [ ] Resolve every Critical/Important review finding through a targeted RED/GREEN cycle before documentation.

**Exit criterion:** full SMS and required Stage C suites are GREEN, and independent review is either approved or explicitly recorded as unavailable with no unresolved manual-review finding.

## Task 5 — Publish evidence and hand off Stage D / 发布证据并交接 Stage D

**Files:** Stage C run record, source plan, `docs/reference/FEATURE_LOCATOR.md`, and `docs/reference/GATEWAY.md` only where current wording is contradicted.

- [ ] Update the bilingual run record with baseline A/B evidence, root cause, chosen trade-off, changed files, all commands/counts/warnings, review result, and remaining limitations.
- [ ] Mark Stage C complete only if the full SMS suite is GREEN and no Critical/Important review finding remains. Otherwise leave it explicitly open.
- [ ] Reconfirm R1 remains NO-GO/deferred until Stage D/E resource evidence exists. Do not infer process isolation, executable unloading, all-channel migration, or production composability from this fix.
- [ ] Once this closure is complete, use Stage D in `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md` as the following task: real Webhook plus test-only polling/socket resource matrices, with positive controls and no second production channel opt-in.
- [ ] Commit reviewed evidence with `docs(gateway): record compatibility verification closure`.

## Explicit non-work / 明确未做

This plan does not implement Stage D, Feishu/R2, production mount, Zahir routes, public Bound operations, EffectClaim, Safe Replay, durable recovery, a universal scheduler, all-channel migration, terminal/memory/sandbox providers, process crash isolation, or compiled-code unloading. No push, deployment, external message, production service mutation, reset, or clean is authorized by this plan.

本计划不实现 Stage D、Feishu/R2、production mount、Zahir route、public Bound operation、EffectClaim、Safe Replay、durable recovery、universal scheduler、all-channel migration、terminal/memory/sandbox provider、进程崩溃隔离或编译代码卸载；不授权 push、部署、外部消息、生产服务变更、reset 或 clean。
