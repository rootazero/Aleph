# Gateway Bound Retirement, Replacement, and Unregister Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Make Bound channel retirement, replacement, unregister, and contract publication changes source-owned, drain-safe, stale-completion-safe, revision-safe, and honest on failure.

**Architecture:** Extend the existing `ChannelRegistry` canonical entry and its entry-local `EntryRuntime`; keep all real channel/forwarder/task I/O outside the source gate; publish or remove only in one short, checked source-gate commit. Reuse `ProviderHandle` for provider instance/generation and `OwnershipTree` for ownership/lifetime/generation validation. Do not add a second registry, runtime map, owner map, descriptor catalog, envelope, or CapabilityKind.

**Tech Stack:** Rust, Tokio, `ChannelRegistry`, `EntryRuntime`, Generic Webhook loopback test support, `ProviderHandle`, `OwnershipTree`, existing `cargo` checks.

**Spec:** [`docs/superpowers/specs/2026-10-27-gateway-bound-retirement-design.md`](../specs/2026-10-27-gateway-bound-retirement-design.md)

## Global Constraints

- Scope is Stage B only: Bound retirement, replacement, unregister, descriptor/schema/lifecycle-contract publication change.
- Modify only the existing Gateway registry/binding/test-support paths for implementation; do not enter Feishu/R2, production Bound opt-in/mount, Zahir route, public send/ingress, or other subsystem migration.
- Before every `cargo` invocation, run `python qa/browser_live/check_memory.py` and require exit 0.
- Source gate protects only short identity/publication/marker commits; channel/factory/transport/stop/close/join I/O runs outside the gate.
- Completion validation order is source → ownership tree → provider → original entry/ticket; never use successor lookup by textual ID.
- Revision exhaustion is checked before destructive retirement and again before final publication/removal; no wrap, half-publish, or destructive `expect`.
- Failure retains incumbent identity, operation marker, dispatch fence, and real cleanup residual; it never reports clean success or automatic `Ready`.
- Tests must observe real loopback/barrier, task/forwarder/resource state, and publication boundaries; test receipts are observations only.
- Do not modify `src/harness/**`; do not add dependencies, generalized envelopes, new registries, owner maps, descriptor maps, or `CapabilityKind`.
- Keep R1 separately NO-GO/deferred; Stage B completion does not authorize Feishu/R2 or production mounting.

## Files and responsibilities

- Modify [`src/gateway/channel_registry.rs`](../../src/gateway/channel_registry.rs): source-owned replacement/unregister entry points, retirement fence and phase orchestration, checked revision preflight/final allocation, complete successor/removal publication, and explicit Bound compatibility routing.
- Modify [`src/gateway/channel_registry/binding.rs`](../../src/gateway/channel_registry/binding.rs): only the smallest entry-local retirement/residual/completion verdict additions required by the tests; reuse existing `ChannelBindingError`, `EntryRuntime`, `ChannelCleanupResidual`, provider guards, and ownership validation.
- Modify [`src/gateway/channel_registry/test_support.rs`](../../src/gateway/channel_registry/test_support.rs): deterministic real loopback, response barrier, forwarder/task observation, and stop/close/join failure injection at an actual cleanup boundary; no fake receipt authority or manual active-work mutation.
- Modify tests in [`src/gateway/channel_registry.rs`](../../src/gateway/channel_registry.rs) and its existing `tests` module: add the six required Stage B contract tests before behavior implementation.
- Update [`docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md`](2026-10-26-gateway-source-owned-runtime-authority-plan.md), [`docs/superpowers/specs/2026-10-27-gateway-bound-retirement-design.md`](../specs/2026-10-27-gateway-bound-retirement-design.md), and a new Stage B run record under `docs/superpowers/runs/` with actual evidence only.
- Reconcile [`docs/reference/FEATURE_LOCATOR.md`](../../reference/FEATURE_LOCATOR.md) and only the relevant portions of [`docs/reference/GATEWAY.md`](../../reference/GATEWAY.md), [`docs/reference/ARCHITECTURE.md`](../../reference/ARCHITECTURE.md), [`docs/reference/DEVELOPMENT.md`](../../reference/DEVELOPMENT.md), and [`docs/reference/REDLINES.md`](../../reference/REDLINES.md) after implementation and review.

## Review Focus

- **Retirement versus ordinary stop:** a pre-retirement ticket may settle but its result must be `DiscardedStale`; ordinary same-generation stop behavior must remain unchanged. Test: `bound_retirement_discards_original_completion` and existing `bound_webhook_stop_drains_without_deadlock`.
- **Cleanup truth:** a stop/close call is not cleanup; a held real response, forwarder, and task must be joined before unregister/replacement succeeds. Test: `bound_unregister_waits_for_cleanup`.
- **Contract-only invalidation:** changing descriptor/schema/fingerprint/lifecycle contract with the same textual ID must invalidate the old capture/provider/entry proof. Test: `bound_contract_update_retires_old_publication`.
- **Concurrent publication pressure:** another entry consuming revision capacity during unlocked cleanup must cause final checked failure without removing the incumbent or publishing a partial successor. Test: `bound_revision_exhaustion_never_half_publishes`.
- **Failure residual and compatibility routing:** injected real cleanup failure must preserve the incumbent and fence, and the old `Option`/Legacy wrapper must not turn Bound failure into apparent success. Test: `bound_failed_cleanup_retains_incumbent` plus explicit wrapper assertions.

---

### Task 1: Add the six contract tests and real failure observation

**Files:**
- Modify: `src/gateway/channel_registry.rs` in the existing `#[cfg(test)] mod tests`.
- Modify: `src/gateway/channel_registry/test_support.rs` only for deterministic barriers/observations that reach an actual socket, forwarder, task, or cleanup boundary.
- Test: the six tests below in `src/gateway/channel_registry.rs`.

**Interfaces:**
- Consumes existing `real_bound`, `LoopbackPeer`, `HeldWebhook`, `ResourceProbe`, `capture_bound_channel`, `admit_bound_work`, `complete_bound_work`, `stop_bound_channel`, and `register_bound_channel` helpers.
- The test code will call the source-private operations that Task 2 exposes:
  - `ChannelRegistry::replace_bound_channel(&self, channel_id: &ChannelId, channel: Box<dyn Channel>, contract: ChannelLifecycleContract, ownership: Arc<OwnershipTree>, initial: ChannelInitialState) -> Result<ChannelId, ChannelBindingError>`
  - `ChannelRegistry::unregister_bound_channel(&self, channel_id: &ChannelId) -> Result<(), ChannelBindingError>`
- These operations are crate-private implementation seams, not public send/ingress APIs.

- [x] **Step 1: Write `bound_retirement_discards_original_completion` before changing retirement behavior.**
  - Register the real Generic Webhook Bound channel and capture it.
  - Send one admitted request to the real loopback peer; hold the response at the peer/barrier before completion handoff.
  - Start replacement using the same textual ID and a newly prepared channel/provider/contract/ownership tuple.
  - Release the response only after the incumbent retirement fence is visible.
  - Assert old completion returns `BoundCompletion::DiscardedStale`, original ticket/work settles exactly once, successor capture/provider/descriptor state remains unchanged, and source revision changes only at the successful successor publication.

- [x] **Step 2: Write `bound_replace_publishes_whole_tuple`.**
  - Prepare a successor with a distinct provider instance, descriptor fingerprint, lifecycle contract, ownership binding, and entry identity.
  - At the pre-publication barrier assert the old entry is still the only visible incumbent, old descriptor/fingerprint and source revision are unchanged, and successor resources are not visible through the map.
  - At the publication barrier assert the source revision, entry identity, provider instance/generation, descriptor fingerprint, contract, ownership binding, and desired intent become visible as one tuple; no intermediate combination may be observed.

- [x] **Step 3: Write `bound_unregister_waits_for_cleanup`.**
  - Hold a real outbound response after the socket request is observed.
  - Start `unregister_bound_channel` and assert it remains pending while original work, forwarder, or transport cleanup is unconfirmed.
  - Release the response and await actual task/forwarder/socket cleanup.
  - Assert only then does unregister return success and remove the incumbent; before that point map membership, fence, marker, and residual remain observable.

- [x] **Step 4: Write `bound_contract_update_retires_old_publication`.**
  - Capture the old publication.
  - Replace using the same textual ID but a changed descriptor/schema fingerprint or lifecycle contract.
  - Attempt completion with the old capture after the retirement fence and assert `DiscardedStale`; assert the old provider, entry, ownership proof, and descriptor cannot mutate the successor.
  - Assert an unrelated entry publication does not invalidate the old capture before the contract replacement itself linearizes.

- [x] **Step 5: Write `bound_failed_cleanup_retains_incumbent`.**
  - Use a real cleanup failure injection at stop/close/join, tied to the existing loopback/forwarder/task path.
  - Attempt replacement or unregister and assert the operation returns the concrete failure/residual.
  - Assert incumbent entry identity, operation marker, dispatch fence, mount metadata, active/unconfirmed work, and residual remain visible; successor is absent and provider/owner state is not replaced or silently reset to `Ready`.
  - Assert a retry cannot overlap the still-running operation driver.

- [x] **Step 6: Write `bound_revision_exhaustion_never_half_publishes`.**
  - Use the existing source revision exhaustion test seam only for source publication capacity; do not mutate active-work, ticket, task, or receipt state.
  - Assert exhaustion is detected before retirement/teardown and leaves map membership, incumbent identity, provider, owner, marker, cleanup resources, and revision unchanged.
  - In a second case, hold cleanup outside the gate while another real registry publication consumes the remaining capacity; assert final `PublicationExhausted` leaves the stopped incumbent and honest residual present, with no successor/removal and no revision wrap.

- [x] **Step 7: Run the new tests before production behavior implementation.**

Run each command only after the memory check shown below:

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_retirement_discards_original_completion -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_replace_publishes_whole_tuple -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_unregister_waits_for_cleanup -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_contract_update_retires_old_publication -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_failed_cleanup_retains_incumbent -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_revision_exhaustion_never_half_publishes -- --exact
```

Expected initial result: compile-only failures are repaired without implementing lifecycle behavior; after the seams exist, each test must fail at a Stage B contract assertion. Record the exact failure, test count, failure count, and warnings in the Stage B run record. Do not call a missing method or parser error a contract RED.

- [x] **Step 8: Commit the test/support RED checkpoint.**

```bash
git add src/gateway/channel_registry.rs src/gateway/channel_registry/test_support.rs
git commit -m "test(gateway): define bound retirement contracts"
```

---

### Task 2: Add source-owned retirement and checked publication primitives

**Files:**
- Modify: `src/gateway/channel_registry/binding.rs` for the smallest retirement phase/residual/completion verdict state needed by Task 1.
- Modify: `src/gateway/channel_registry.rs` for the two crate-private operations and their source-gate orchestration.

**Interfaces:**
- Produces `ChannelRegistry::replace_bound_channel(&self, channel_id: &ChannelId, channel: Box<dyn Channel>, contract: ChannelLifecycleContract, ownership: Arc<OwnershipTree>, initial: ChannelInitialState) -> Result<ChannelId, ChannelBindingError>`.
- Produces `ChannelRegistry::unregister_bound_channel(&self, channel_id: &ChannelId) -> Result<(), ChannelBindingError>`.
- Existing `register_bound_channel` remains cold first publication and continues to reject duplicate IDs; do not silently turn its legacy duplicate behavior into replacement.
- Existing `unregister(&ChannelId) -> Option<Box<dyn Channel>>` remains an explicit compatibility wrapper. It must not report authoritative Bound success; route Bound IDs to the explicit Result path only if the wrapper can preserve its existing semantics, otherwise return its existing refusal/absence without map mutation.

- [x] **Step 1: Add only the source-private operation signatures or compile scaffolding required by Task 1.**
  - Preserve existing `ChannelBindingError` variants, using `Busy`, `Retiring`, `PublicationExhausted`, and `Runtime { phase, error, residual }` consistently.
  - Do not mutate the map or provider/tree state in a scaffold; rerun one focused test to confirm the test now reaches a contract assertion rather than a compile error.

- [x] **Step 2: Implement successor preparation outside the source gate.**
  - Construct/validate the candidate provider, descriptor/fingerprint, contract, ownership binding, desired intent, and entry-local runtime privately.
  - Check source revision capacity before touching the incumbent.
  - Ensure no candidate channel transport, forwarder, or mount becomes visible through `channels`, `desired_state`, or the publication snapshot before final commit.

- [x] **Step 3: Implement the retirement fence as the first short source-gate commit.**
  - Validate the incumbent is the exact Bound entry and has no conflicting operation marker.
  - Install one entry-local retirement marker/phase, fence new admission and dispatch, and make old completion fail at the source check.
  - Release the gate before any channel stop, transport close, forwarder cancellation, task join, or drain wait.

- [x] **Step 4: Reuse the existing Bound drain/cleanup driver under the retirement phase.**
  - Wait for all original active work to end using the actual ticket/drain notification.
  - Join execution and forwarder tasks and perform actual channel cleanup outside authority locks.
  - On timeout, cancellation, or failure, preserve the incumbent, marker, fence, and exact `ChannelCleanupResidual`; return an error and never restore `Ready` automatically.
  - A retry must prove the prior driver is quiescent before starting another cleanup driver.

- [x] **Step 5: Strengthen `complete_bound_work` stale validation.**
  - Keep validation order source → tree → provider → original entry/ticket.
  - Require the original entry identity, publication/contract fingerprint, provider instance/generation, ownership binding, and live ticket to match.
  - On retirement or successor publication, return `DiscardedStale`, settle the original ticket once, and never acquire the successor by textual ID.
  - Ensure completion cannot modify successor desired state, provider, owner, source revision, or publication map.

- [x] **Step 6: Implement final revalidation and atomic replacement/removal publication.**
  - Reacquire the source gate only after cleanup succeeds.
  - Revalidate original entry, marker, source revision capacity, provider/tree proof, and cleanup result before the first mutation.
  - For replacement, publish the complete successor tuple (entry identity, provider instance/generation, descriptor/fingerprint, contract, ownership, desired intent, and checked source revision) in one non-awaiting commit.
  - For unregister, remove the incumbent only in the same successful final commit after cleanup is proven.
  - If final revision allocation fails, keep the stopped incumbent and residual/marker state; do not remove, publish, wrap, or claim success.

- [x] **Step 7: Make compatibility paths explicit.**
  - Ensure the existing Legacy `unregister`/replacement paths cannot consume a Bound `Option`/boolean refusal as permission or success.
  - Bound operations use the explicit Result path; Legacy positive controls retain their current behavior.
  - Do not modify public send/ingress or production Bound registration/mount callers.

- [x] **Step 8: Run the six focused tests and the adjacent registry/binding suites.**

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_retirement_discards_original_completion -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_replace_publishes_whole_tuple -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_unregister_waits_for_cleanup -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_contract_update_retires_old_publication -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_failed_cleanup_retains_incumbent -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::bound_revision_exhaustion_never_half_publishes -- --exact
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::binding::tests::
```

Expected result: all six Stage B tests and existing registry/binding tests pass; record exact counts and nonfatal warnings.

- [x] **Step 9: Commit the lifecycle implementation.**

```bash
git add src/gateway/channel_registry.rs src/gateway/channel_registry/binding.rs src/gateway/channel_registry/test_support.rs
git commit -m "feat(gateway): retire bound runtime publications safely"
```

---

### Task 3: Run required verification and independent review

**Files:**
- Read/verify: `src/gateway/channel_registry.rs`, `src/gateway/channel_registry/binding.rs`, `src/gateway/channel_registry/test_support.rs`.
- Review: final diff against `origin/main`.

- [x] **Step 1: Run formatting and diff checks.**

```bash
python qa/browser_live/check_memory.py
cargo fmt --all --check
python qa/browser_live/check_memory.py
git diff --check
```

Record each exit status. `git diff --check` has no cargo invocation but remains required.

- [x] **Step 2: Run compile and lint checks.**

```bash
python qa/browser_live/check_memory.py
cargo check -p alephcore --tests
python qa/browser_live/check_memory.py
cargo clippy -p alephcore --tests -- -D warnings
```

Record test/build failures separately from nonfatal macOS linker warnings, including the known possible warning `__eh_frame section too large (max 16MB) to encode dwarf unwind offsets in compact unwind table` if it appears.

- [x] **Step 3: Run the required focused suites.**

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::tests::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::binding::tests::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::interfaces::webhook::
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib capability::
```

Record actual passed/failed/ignored counts for every command. Do not summarize an unrun command as passed.

- [x] **Step 4: Run one independent read-only review of the final diff.**
  - Use a fresh read-only reviewer with the task: inspect only the Stage B diff for half-publication, stale completion reaching successor state, gate-held I/O, revision exhaustion after destruction, residual dishonesty, and authority duplication.
  - If the reviewer service is unavailable, record the exact failure and perform a manual review against the same checklist; do not claim independent review passed.
  - Fix every valid finding, rerun the affected focused test and relevant checks, and retain the reviewer result.

- [x] **Step 5: Commit review fixes only after verification.**

```bash
git add src/gateway/channel_registry.rs src/gateway/channel_registry/binding.rs src/gateway/channel_registry/test_support.rs
git commit -m "fix(gateway): close bound retirement review findings"
```

Use this commit only if review fixes are needed; do not create an empty commit.

---

### Task 4: Update evidence and reference documentation

**Files:**
- Modify: `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md` Stage B section and evidence section.
- Modify: `docs/superpowers/specs/2026-10-27-gateway-bound-retirement-design.md` with final implementation decisions and evidence.
- Create: `docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md` as a bilingual execution record.
- Modify only evidence-backed sections of `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/GATEWAY.md`, `docs/reference/ARCHITECTURE.md`, `docs/reference/DEVELOPMENT.md`, and `docs/reference/REDLINES.md` where required by the final contract.

- [x] **Step 1: Record actual implementation and verification evidence.**
  - Include every RED/GREEN command, actual exit status, test count, failure count, ignored count where relevant, and nonfatal warning.
  - State the exact bounded slice and modified files.
  - Record the authority duplication audit: one `ChannelRegistry` map/entry authority, provider facts only in `ProviderHandle`, ownership facts only in `OwnershipTree`, no new registry/map/kind/envelope.
  - Record the first unclosed contract, deferred provider/kinds, unrun validations, and destructive/external actions not taken.

- [x] **Step 2: Reconcile references without widening scope.**
  - Update references to say Stage B is implemented only for the real Generic Webhook/loopback test composition unless the final evidence proves more.
  - Preserve R1 NO-GO/deferred and all Feishu/R2, production mount, Zahir, durable recovery, EffectClaim, Safe Replay, and universal scheduler exclusions.
  - Remove only statements contradicted by the verified Stage B behavior; do not mark future stages complete.

- [x] **Step 3: Run documentation diff checks.**

```bash
git diff --check
```

- [x] **Step 4: Commit the evidence and reference updates.**

```bash
git add docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md docs/superpowers/specs/2026-10-27-gateway-bound-retirement-design.md docs/superpowers/runs/2026-10-27-gateway-bound-retirement-run.md docs/reference/FEATURE_LOCATOR.md docs/reference/GATEWAY.md docs/reference/ARCHITECTURE.md docs/reference/DEVELOPMENT.md docs/reference/REDLINES.md
git commit -m "docs(gateway): record bound retirement evidence"
```

---

## Self-review

- **Spec coverage:** Task 1 covers all six required RED contracts; Task 2 covers private preparation, fence, drain, cleanup, stale completion, complete publication/removal, revision exhaustion, and compatibility; Task 3 covers all required validation and independent review; Task 4 records evidence and reconciles references.
- **Step scan:** Each step has one observable action, an exact file or test name, and a concrete check. Production operations are crate-private and return explicit `Result` values; no public send/ingress API is introduced.
- **Type consistency:** `replace_bound_channel` and `unregister_bound_channel` are the only new operation seams; all later steps use those exact names and signatures. Existing `register_bound_channel` remains first publication and existing `unregister` remains the compatibility wrapper.
- **Review focus:** Each of the five review-focus risks has an owning test and a verification step.
- **Proportion:** The plan names interfaces, tests, failure boundaries, and commands without prescribing implementation bodies beyond the required linearization protocol.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-10-27-gateway-bound-retirement-plan.md`. The user has already identified the assistant as the main implementation agent, so Native execution is assumed; review this plan before implementation begins. After approval, use `superpowers:executing-plans` and proceed task-by-task, starting with Task 1 RED tests.

## Final implementation status / 最终实施状态

Stage B is complete for the bounded Generic Webhook/loopback composition. The implementation is committed as `a6d122bf4` (contract RED), `df513848c` (retirement implementation), and `255ebc8a6` (publication-boundary test). The six required contracts are GREEN; the final run record contains the exact commands, counts, warnings, manual review checklist, and deferred scope.

Stage B 已在受限 Generic Webhook/loopback 组合上完成。实现提交为 `a6d122bf4`（contract RED）、`df513848c`（retirement 实现）和 `255ebc8a6`（publication boundary 测试）。六个必需 contract 均 GREEN；最终 run record 记录了完整命令、测试数量、warning、手工审查清单和 deferred 范围。

Independent review was attempted through the available Pi reviewer route but was unavailable because the ACP proxy circuit was open; it is recorded as unavailable and not as a pass. Manual review was completed against the plan's five risks. R1 remains NO-GO/deferred; Feishu/R2, production mount/Zahir route, public ingress/send, EffectClaim, Safe Replay, durable recovery, universal scheduler, and remaining provider kinds remain deferred.

独立 review 已通过可用的 Pi reviewer 路线尝试，但因 ACP proxy circuit open 不可用；文档明确记录为 unavailable，不计为通过。主 agent 已按计划五项风险完成手工审查。R1 仍为 NO-GO/deferred；Feishu/R2、production mount/Zahir route、public ingress/send、EffectClaim、Safe Replay、durable recovery、universal scheduler 及其余 provider kind 仍 deferred。