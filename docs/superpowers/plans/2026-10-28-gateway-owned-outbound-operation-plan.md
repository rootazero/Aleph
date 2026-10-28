# Gateway Owned Outbound Operation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close both Legacy ExplicitOnly self-reentrant mid-send contracts by making every dynamic `Channel::send` an owned operation whose transport future runs without the registry's instance read guard.

**Architecture:** Prepare a `SendOperation` synchronously under the existing instance read guard, capturing the actual instance's delivery policy and performing existing validation there; release the guard before polling transport. Keep `ChannelRegistry` as the only association authority and keep stop-before-remove/publish cleanup ordering. Migrate all Channel implementors atomically because the public borrowed-send method is replaced, not supplemented.

**Tech Stack:** Rust, tokio, async-trait, `Pin<Box<dyn Future + Send + 'static>>`, Cargo, bun-independent Rust test suites.

**Spec:** `docs/superpowers/specs/2026-10-28-gateway-owned-outbound-operation-design.md`

## Global Constraints

- Work only on branch `mid-send-closure`; all production writers are serial. Do not reset `main` or the independent baseline worktree.
- Do not delete, weaken, externally rewrite, or extend the watchdog of either mid-send test; the watchdog remains `Duration::from_secs(5)`.
- Preserve the actual sending instance's `OutboundDeliveryPolicy`; never re-derive it from a successor or vanished registration.
- `unregister`/replacement return only after real old-instance `stop` completion. No unsafe, self-referencing guard, detached cleanup, fake receipt, manual active-work permission, or second association/owner/cleanup authority.
- `lifecycle_sync` and source publication gates protect only short identity/marker/publication commits; transport/stop/close/join I/O remains outside them.
- Keep one outbound API shape: replace `async fn send(&self, ...) -> ChannelResult<SendResult>`; do not add a borrowed fallback or opt-in second mode.
- `stop` closes provider-owned resources and must not drain in-flight `SendOperation`s. Operation-scoped clones may complete/fail after stop.
- Existing feishu adapter changes are mechanical signature migration only. No Stage D resource matrix, Feishu/R2 wiring, production Capability mount, or deferred provider/kind work.
- Before **every** Cargo command run `python qa/browser_live/check_memory.py`; require exit 0 and `ALLOW`. Zero-test filters, parser/import/compile/setup failures are not behavioral RED or GREEN.
- Record actual exit codes, passed/failed/ignored counts, and the known nonfatal linker `__eh_frame` warning. Do not claim unrun checks.
- Code comments in English; run/spec documentation bilingual where the existing document is bilingual.

## Review Focus

1. **Self-reentrant unregister/replacement:** old registration removal or usable Automatic successor, exactly one old send, one completed old stop, and no old plaintext persistence — pinned by Task 1's two strengthened behavioral RED tests and Task 2's three-serial-run GREEN step.
2. **Stale handle/successor policy confusion:** replay and queue permission must remain tied to the actual attempt — pinned in Task 3 by `replay_revalidates_policy_when_the_box_changes_inside_the_same_arc` and `stale_explicit_only_handle_cannot_reclassify_a_newer_automatic_registration` in the full Registry suite.
3. **Public telecom cancellation and custody:** cancellation before first poll sends nothing; public telecom never enters durable custody or retries — pinned in Task 3 by `public_telecom_cancellation_fences_registry_and_channel_lock_waits`, `public_telecom_transport_result_never_enters_durable_custody_or_retries`, and `public_telecom_does_not_flush_or_attempt_same_conversation_backlog`.
4. **Bound denial/cold/source lifecycle and Legacy positive controls:** the API migration must not turn Bound handles into Legacy permission or weaken cleanup ordering — pinned in Task 3 by `bound_compatibility_effect_paths_are_denied`, `bound_cold_lists_never_call_transport`, `bound_compat_lifecycle_uses_source`, `bound_failed_cleanup_retains_incumbent`, and `legacy_positive_controls_unchanged`.
5. **Provider-local lock/cleanup regression:** a provider must not move its own mutex across reentrant transport work or claim stop while its owned resources remain active — pinned by Task 2's per-implementor audit against the actual diff, Task 3's complete suites, and Task 4's independent checklist; any concrete finding becomes a blocking fix task before closure.

---

### Task 1: Strengthen the Executable Mid-Send RED Contracts

**Files:**
- Modify: `tests/sms_delivery_policy_test.rs:305-310` (`Sends`)
- Modify: `tests/sms_delivery_policy_test.rs:507-590` (`VanishingExplicit` and both tests)

**Interfaces:**
- Consumes: existing `ChannelRegistry::get`, `ChannelRegistry::send`, `OutboundDeliveryPolicy`, `assert_nothing_stored`.
- Produces: test-only `Sends.stops: AtomicUsize`; unchanged public/production API.

- [ ] **Step 1: Add cleanup observation without changing production code**

Add `stops: AtomicUsize` to `Sends`. In `VanishingExplicit::stop`, increment that counter with `Ordering::SeqCst` and return `Ok(())`. Do not change its self-reentrant `async fn send` yet.

- [ ] **Step 2: Strengthen the unregister contract**

Keep every existing assertion in `explicit_only_channel_unregistered_mid_send_is_never_persisted`, then add:

```rust
assert_eq!(sends.stops.load(Ordering::SeqCst), 1);
assert!(registry.get(&ChannelId::new("vanish")).await.is_none());
```

- [ ] **Step 3: Strengthen the replacement contract**

Keep every existing assertion in `explicit_only_channel_replaced_mid_send_is_never_persisted`, then add old-stop/old-send proof, an Automatic successor check, and a real successor send:

```rust
assert_eq!(sends.stops.load(Ordering::SeqCst), 1);
assert_eq!(sends.attempts.load(Ordering::SeqCst), 1);
let successor = registry.get(&ChannelId::new("vanish")).await.expect("successor");
assert_eq!(
    successor.read().await.outbound_delivery_policy(),
    OutboundDeliveryPolicy::Automatic
);
registry
    .send(
        &ChannelId::new("vanish"),
        OutboundMessage::text(PEER, "successor-visible"),
    )
    .await
    .expect("Automatic successor sends");
assert_eq!(sends.attempts.load(Ordering::SeqCst), 1);
assert_nothing_stored(&store);
```

The successor's read guard must be dropped before `registry.send`; do not store the old `"mid-send-secret"` in the successor message.

- [ ] **Step 4: Run both tests as behavioral RED**

Run each separately, with its own gate:

```bash
python qa/browser_live/check_memory.py && cargo test -p alephcore --test sms_delivery_policy_test explicit_only_channel_unregistered_mid_send_is_never_persisted -- --exact --nocapture
python qa/browser_live/check_memory.py && cargo test -p alephcore --test sms_delivery_policy_test explicit_only_channel_replaced_mid_send_is_never_persisted -- --exact --nocapture
```

Expected: each memory gate exits 0 / `ALLOW`; each test executes exactly one test and remains RED with `no deadlock: Elapsed(())`, not a compile/setup failure. The new post-send assertions are not reached while the deadlock remains.

- [ ] **Step 5: Commit the RED contract**

```bash
git add tests/sms_delivery_policy_test.rs
git commit -m "test(gateway): prove mid-send cleanup contracts"
```

### Task 2: Migrate Channel Sends to Owned Operations

**Files:**
- Modify: `crates/aleph-slot-api/src/channel.rs` (public `Channel` contract and test stub)
- Modify: `crates/aleph-slot-api/tests/channel_compat.rs`
- Modify: `src/gateway/channel.rs` (re-export and test doubles)
- Modify: `src/gateway/channel_registry.rs:1292-1334,1579-1670` (three Legacy/public send paths and fixtures)
- Modify: `src/gateway/channel_registry/binding.rs:410-413` (`send_bound_once`)
- Modify all 21 adapters: `src/gateway/interfaces/{cli,discord,email,feishu,irc,line,matrix,mattermost,nostr,qq,signal,slack,sms,telegram,voice_call,webhook,wechat,whatsapp,xmpp}` and `src/gateway/interfaces/imessage/{mod,bluebubbles/mod}.rs`
- Modify test/support implementors: `src/clarification/ask.rs`, `src/exec/approval/channel_bridge.rs`, `src/gateway/channel_registry/test_support.rs`, `src/gateway/event_emitter/origin_fanout.rs`, `src/gateway/execution_engine/run_trace_sinks.rs`, `src/gateway/inbound_router/executor.rs`, `src/gateway/reply_emitter/tests.rs`, `src/gateway/resume_coordinator.rs`
- Modify: `examples/modularization/channel/src/lib.rs`, `tests/link_acl_probe/mock_channel.rs`, `tests/sms_delivery_policy_test.rs`

**Interfaces:**
- Consumes: Task 1's strengthened tests and existing stop/unregister/replay/cancellation/admission contracts.
- Produces:

```rust
pub type SendOperation =
    Pin<Box<dyn Future<Output = ChannelResult<SendResult>> + Send + 'static>>;

fn send(&self, message: OutboundMessage) -> SendOperation;
```

- [ ] **Step 1: Replace the public borrowed-send signature**

In `crates/aleph-slot-api/src/channel.rs`, export `SendOperation` exactly as above, importing `std::future::Future` and `std::pin::Pin`. Replace the required async `send` method with the synchronous preparation method. Document that the future owns its handles, never borrows the adapter, and must not require `stop` to drain it. Document that `stop` closes provider-owned resources without joining in-flight operations. Re-export `SendOperation` through `src/gateway/channel.rs`.

- [ ] **Step 2: Release Registry guards before every dynamic transport await**

In `send_on`, retain per-attempt read acquisition, actual-instance policy capture, replay/disabled validation, retry limits, observers, and `Attempt` construction. Prepare `let operation = channel.send(message.clone());`, drop the read guard, then await `operation`. Each retry re-acquires and revalidates as before.

In `send_public_telecom`, prepare under the guard, drop it, then await; retain bypass of hooks/queue/retry.

In `send_public_telecom_if_not_cancelled`, retain all cancellation selects and the final fence. Prepare synchronously after that fence, drop the guard, and immediately await the operation with no intervening cancellation select or other await; first poll remains the transport commit boundary.

In `send_bound_once`, prepare under the capture read guard, drop it, then await inside the already-admitted task. Do not alter owner/generation/call admission, execution slots, `active_work`, or `complete_bound_work`.

Do not change `send_approved_public_telecom` / `send_private_sms`.

- [ ] **Step 3: Migrate every implementor atomically**

For each `impl Channel`, change to `fn send(&self, message: OutboundMessage) -> SendOperation`; synchronously clone required handles and return `Box::pin(async move { ... })` containing the existing transport body. Keep observable preparation counters synchronous, especially `VanishingExplicit.sends.attempts`. `VanishingExplicit` clones its registry handle and id into the operation; unregister/optional replacement and its `NotConnected("gone")` result remain inside the operation.

Re-run `rg 'impl Channel for|async fn send\(&self' --glob '*.rs'` and migrate every hit, including any implementor missing from the file list. No default panic implementation and no borrowed fallback.

- [ ] **Step 4: Audit provider stop and internal locking during migration**

For every migrated implementation, inspect its `stop` body and locks touched by the moved send body. Preserve actual provider cleanup; remove only a drain/join of the same in-flight operation if it would recreate `send → unregister → stop → send`. Do not detach cleanup or leave provider-owned connections/tasks running after successful stop. Record any provider-local mutex held across a reentrant await as a blocking finding; do not silently broaden lifecycle APIs.

- [ ] **Step 5: Compile all affected API and test surfaces**

Run, each with its own memory gate:

```bash
python qa/browser_live/check_memory.py && cargo check -p alephcore --tests
python qa/browser_live/check_memory.py && cargo check -p aleph-slot-api --tests
python qa/browser_live/check_memory.py && cargo check --manifest-path examples/modularization/channel/Cargo.toml
```

Expected: all gates `ALLOW`; all commands exit 0. The known nonfatal linker warning may be recorded, but compile errors are not acceptable.

- [ ] **Step 6: Run both focused contracts three times serially**

Run the two exact commands from Task 1 Step 4 three times each, one invocation at a time, with a fresh `python qa/browser_live/check_memory.py` before every Cargo invocation.

Expected: 6/6 gates exit 0 / `ALLOW`; 6/6 test commands exit 0; each reports `1 passed; 0 failed; 0 ignored; 0 measured; 17 filtered out`. Both old-stop counts, removal/successor assertions, exactly-one-old-send, and empty queue/dead-letter assertions pass within the unchanged 5-second watchdog.

- [ ] **Step 7: Run the complete SMS suite**

```bash
python qa/browser_live/check_memory.py && cargo test -p alephcore --test sms_delivery_policy_test
```

Expected: gate `ALLOW`, exit 0, all 18 tests passed, 0 failed/ignored. Record actual counts rather than assuming them.

- [ ] **Step 8: Commit the atomic migration**

```bash
git add crates/aleph-slot-api src/gateway src/clarification/ask.rs src/exec/approval/channel_bridge.rs examples/modularization/channel tests/link_acl_probe/mock_channel.rs tests/sms_delivery_policy_test.rs
git commit -m "refactor(gateway): make channel sends owned operations"
```

Review `git status --short` before committing; add any migrated file outside these paths explicitly. Do not include investigation artifacts or unrelated changes.

### Task 3: Run the Full Stage C Regression and Quality Gates

**Files:**
- Modify only if a concrete regression requires a blocking fix within the approved design; otherwise verification produces no source diff.

**Interfaces:**
- Consumes: Task 2's atomic migration.
- Produces: recorded full-suite results and, if necessary, a new unchecked fix task before closure.

- [ ] **Step 1: Run every required suite independently gated**

```bash
python qa/browser_live/check_memory.py && cargo test -p alephcore --test sms_delivery_policy_test
python qa/browser_live/check_memory.py && cargo test -p alephcore --lib gateway::channel_registry::tests::
python qa/browser_live/check_memory.py && cargo test -p alephcore --lib gateway::webhook_receiver::tests::
python qa/browser_live/check_memory.py && cargo test -p alephcore --lib gateway::delivery_queue::tests::
python qa/browser_live/check_memory.py && cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
python qa/browser_live/check_memory.py && cargo test -p alephcore --lib tools::registry::tests::
python qa/browser_live/check_memory.py && cargo test -p alephcore --lib gateway::interfaces::webhook::
python qa/browser_live/check_memory.py && cargo test -p alephcore --lib capability::
```

Expected: every gate `ALLOW`, every command exit 0, nonzero actual test count, 0 failed. Confirm Review Focus tests 2–4 by name in Registry output; do not infer their passage from compilation.

- [ ] **Step 2: Run static and formatting gates**

```bash
python qa/browser_live/check_memory.py && cargo check -p alephcore --tests
python qa/browser_live/check_memory.py && cargo clippy -p alephcore --tests -- -D warnings
python qa/browser_live/check_memory.py && cargo fmt --all --check
git diff --check
```

Expected: all exit 0. `git diff --check` needs no memory gate because it is not a Cargo command.

- [ ] **Step 3: Handle any failure without weakening the contract**

If any command fails, keep Stage C open, create a specific blocking fix task with the exact error and failing test, and return to the smallest relevant Task 2 step. Do not proceed to closure documentation with unresolved failures.

### Task 4: Independent Review and Stage C Closure Records

**Files:**
- Review: complete Stage C diff from `ca03a1206` through the current branch, especially all Task 2 files.
- Update only after GREEN/no unresolved findings: `docs/superpowers/runs/2026-10-28-gateway-bound-compatibility-closure-run.md`
- Update: `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md`
- Update: `docs/reference/FEATURE_LOCATOR.md`

**Interfaces:**
- Consumes: all Task 2–3 evidence and the design's verification plan.
- Produces: independent review disposition and accurate bilingual closure/deferral records; no new runtime authority.

- [ ] **Step 1: Obtain an independent read-only review**

Dispatch a fresh reviewer on an actually available model. Require findings with exact file:line and severity for: same-task/cross-task read/write cycles; cleanup before unregister/replacement return; actual-instance ExplicitOnly policy; plaintext queue/dead-letter/replay leakage; Bound denial/cold/source lifecycle/Legacy positive controls; duplicate authority; deferred-subsystem scope. Include the full Stage C diff, not only the latest commit.

If reviewer invocation fails, record the complete error and run the identical checklist manually; label it manual review and do not claim independent passage.

- [ ] **Step 2: Resolve all Critical/Important findings**

For each finding, create a focused behavioral RED and blocking fix task within the approved design. Re-run affected focused tests three times plus the full Task 3 set. If the only safe fix needs another public lifecycle change, deferred-cleanup authority, or altered unregister semantics, stop and request architecture approval.

- [ ] **Step 3: Update closure documentation truthfully**

Only when Tasks 2–3 are GREEN and no Critical/Important finding remains, update the run record, source plan, and `FEATURE_LOCATOR.md`. Include baseline A/B, root cause, actual commands/results/counts, API trade-off (in-flight operations may outlive stop), a release-note-visible warning that downstream/out-of-tree `Channel` implementors must migrate to the breaking `SendOperation` signature, reviewer availability/findings, authority-duplication conclusion, first unclosed contract if any, and all deferred providers/kinds. Keep R1 NO-GO and Stage D/E, Feishu/R2, production mount, terminal, memory graph, sandbox, and other kind backends deferred. Do not describe Stage C as closed if any required item remains open.

- [ ] **Step 4: Commit documentation and inspect the final branch**

```bash
git add docs/superpowers/runs/2026-10-28-gateway-bound-compatibility-closure-run.md docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md docs/reference/FEATURE_LOCATOR.md
git commit -m "docs(gateway): record owned-send closure evidence"
git status --short
git diff --check
```

Expected: only intended documentation is committed; final tracked status is clean. Report any intentionally retained ignored investigation artifacts separately.

- [ ] **Step 5: Deliver the final report and stop before Stage D**

List baseline A/B, root cause, approach trade-offs, RED/GREEN commands and actual results, modified files, authority duplication review, reviewer availability, first unclosed contract, deferred providers/kinds, unrun validations, and destructive/external actions not performed. Do not merge/clean worktrees, push, deploy, or begin Stage D without the next instruction.
