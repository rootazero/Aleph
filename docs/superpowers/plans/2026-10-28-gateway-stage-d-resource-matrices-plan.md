# Stage D Source-Owned Resource Matrices Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove with real resource observations that cold Bound channels allocate nothing and active/replaced/failed channels follow the existing source-owned cleanup contracts for two provider shapes.

**Architecture:** Add a `cfg(test)` resource-matrix module and a test-only polling/socket channel beside the existing registry test support. Reuse the real `WebhookChannel`, existing `ResourceProbe`, `LoopbackPeer`, Bound registry APIs and `OwnershipTree`; do not add a production resource authority or lifecycle API.

**Tech Stack:** Rust, tokio, rusqlite, reqwest, existing Aleph test utilities.

**Spec:** `docs/superpowers/specs/2026-10-28-gateway-stage-d-resource-matrices-design.md`

## Global Constraints

- Base is Stage C HEAD `47625530cbc26da41a791caa09b8c7cfd0b485d3`; branch is `stage-d-resources`.
- Production behavior remains unchanged unless a behavioral RED proves a generic defect. `cfg(test)` observation must record real operations only.
- No new production Bound opt-in, descriptor/source/cleanup map, CapabilityKind, public registry visibility, dependency, deferred cleanup drain or detached cleanup task.
- Cold resolve must fail provider admission because the provider is not `Ready`; failed-stop retry/unregister remains `Busy`.
- No Feishu/R2, terminal, memory graph, sandbox, Stage E or production mount work.
- Every Cargo command is separately preceded by `python qa/browser_live/check_memory.py`; require `exit 0 / ALLOW`. Zero-test filters do not pass.
- Run the two Stage C mid-send tests three serial times each before final closure; do not weaken or lengthen their watchdogs.

## Review Focus

1. **Inactive observer:** a zero receipt can be false; Task 1 requires nonzero active controls before asserting post-stop zero.
2. **Cold provider admission:** a correct binding still cannot resolve a non-Ready provider; Task 2 pins the error and zero allocation.
3. **Partial activation:** TCP can succeed before SQLite fails; Task 3 proves stream destruction without inventing worker/DB receipts.
4. **Failed cleanup:** current retry remains `Busy`; Task 3 proves real client residual and final owner-drop cleanup rather than retry success.
5. **Stale proof and lock scope:** replacement must reject old captures without deadlocking worker/forwarder cleanup; Task 2 exercises resolve/send/stop/replacement through the real registry.

---

### Task 1: Real Polling/Socket Fixture and Positive Controls

**Files:**
- Modify: `src/gateway/channel_registry/test_support.rs`
- Create: `src/gateway/channel_registry/resource_matrix.rs`
- Modify: `src/gateway/channel_registry.rs` (test-module declaration only)

**Interfaces:**
- Consumes: existing `Channel`, `ChannelInfo`, `ChannelState`, `SendOperation`, `ResourceProbe`, `LoopbackPeer`, `TempDir` and Bound start/stop APIs.
- Produces: `PollingSocketChannel` and its `ResourceProbe` events for Tasks 2–3; `active_resource_observers_have_positive_controls`.

- [ ] **Step 1: Add a compilable but behaviorally incomplete fixture and failing positive-control test**

Define `PollingSocketChannel` with configuration/receipt storage and a `Channel` implementation whose initial `start`/`stop` do not yet allocate resources. Define the event vocabulary now so later tests compile: `tcp_connect_attempt`, `tcp_connected`, `tcp_drop`, `worker_spawned`, `worker_exited`, `worker_joined`, `timer_created`, `timer_tick`, `timer_drop`, `cache_created`, `cache_drop`, `database_open_attempt`, `database_opened`, `database_closed`, `database_drop`, `send_attempt`, `send_completed`.

Add `#[cfg(test)] mod resource_matrix;` and write `active_resource_observers_have_positive_controls` for both real Webhook and polling/socket. Start through `start_bound_channel_compat`, perform the shape's send, require nonzero real active receipts, stop, then require actual cleanup. For polling, require connected TCP, spawned worker, created timer/cache/DB and at least one timer tick before stop.

- [ ] **Step 2: Run the positive control and record behavioral RED**

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::active_resource_observers_have_positive_controls -- --exact --nocapture
```

Expected: real Webhook observations may pass; polling fails because expected real connect/worker/timer/cache/DB receipts are zero. Parser/import/compile failure is not acceptable RED.

- [ ] **Step 3: Implement real resource ownership in the polling fixture**

In `test_support.rs`, make cold construction configuration-only. `start` must connect a real loopback `TcpStream`, open SQLite and execute a minimal schema, create cache/interval, then spawn one worker owning stream/DB/cache/timer/cancellation. Store its `JoinHandle` in the channel.

Use small test-only RAII wrappers to record actual stream/timer/cache/DB drop. On cancellation, explicitly close SQLite, drop worker resources and record `worker_exited`; `stop` cancels, awaits the stored handle and records `worker_joined`. `send` returns an owned `SendOperation`, records the payload attempt/completion and constructs the normal `SendResult` with `MessageId`/`Utc::now()`; it performs no external send.

The loopback peer must have a bounded positive-control tick; no sleep-derived fake receipt, detached cleanup or second registry.

- [ ] **Step 4: Run the positive control GREEN**

Repeat Step 2. Expected: `1 passed; 0 failed; 0 ignored`; polling active counters are nonzero before stop and all owned-resource active counts are zero after join.

- [ ] **Step 5: Commit the fixture and positive control**

```bash
git add src/gateway/channel_registry.rs src/gateway/channel_registry/test_support.rs src/gateway/channel_registry/resource_matrix.rs
git commit -m "test(gateway): add real polling resource controls"
```

### Task 2: Cold, Active and Replacement Matrices

**Files:**
- Modify: `src/gateway/channel_registry/resource_matrix.rs`
- Modify only if a behavioral RED proves a generic defect: the exact owning production file, with a separate minimal fix explanation.

**Interfaces:**
- Consumes: Task 1 `PollingSocketChannel` and receipt events; existing `register_bound_channel`, `capture_bound_channel`, `snapshot_bound_channels`, `resolve_bound_channel`, `send_bound_once`, `replace_bound_channel`, start/stop/unregister APIs.
- Produces: executable cold/active/old-proof/replacement evidence for both shapes.

- [ ] **Step 1: Write the cold matrix for every shape and initial state**

Implement `cold_operations_allocate_no_resources_for_either_shape` over `RealWebhook` and `PollingSocket`, and over `Discovered`, `Cold`, `Disabled`. Use local test setup equivalent to the existing `real_bound` helper and `ChannelLifecycleContract::new(OwnerRef::Runtime, LifetimeScope::Runtime, VisibilityScope::default(), ...)`.

For each row, assert zero resources after register, list/status/health, snapshot/discovery projection, capture, correct-binding resolve failure (non-Ready), wrong/missing ownership/scope rejection, unregister and old-proof rejection. A `ProviderHandle`/descriptor alone is not a provider resource.

- [ ] **Step 2: Run the cold matrix**

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::cold_operations_allocate_no_resources_for_either_shape -- --exact --nocapture
```

Expected: PASS with nonzero test count. If RED, identify the actual allocating operation; do not relax the zero assertion.

- [ ] **Step 3: Write active send/stop and replacement/old-proof tests**

Implement `active_send_stop_and_old_proof_contracts_hold_for_either_shape`: cold zero, start positive control, correct-scope resolve success, real `send_bound_once`, stop cleanup and post-stop old-capture/provider-generation rejection. Real Webhook asserts loopback HTTP body plus `request_attempt/request_ended`; polling asserts `send_attempt/send_completed` and its real resource receipts.

Implement `replacement_cleans_old_resources_and_rejects_old_provider_proof`: start the old incarnation, call the existing `replace_bound_channel` with a fresh same-ID source and new receipts, prove the old capture cannot resolve/call the replacement, then start/send/stop the replacement and require new positive/cleanup controls. `replace_bound_channel` must perform incumbent retirement; do not emulate replacement through a second registry/owner map.

- [ ] **Step 4: Run both lifecycle tests**

Run each command with its own memory gate:

```bash
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::active_send_stop_and_old_proof_contracts_hold_for_either_shape -- --exact --nocapture
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::replacement_cleans_old_resources_and_rejects_old_provider_proof -- --exact --nocapture
```

Expected: each `1 passed; 0 failed`. A generic production RED requires a minimal focused fix; public lifecycle expansion, retry recovery or deferred cleanup returns for architecture direction.

- [ ] **Step 5: Run the whole new module and commit**

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix:: -- --nocapture
git add src/gateway/channel_registry/resource_matrix.rs
git commit -m "test(gateway): cover source-owned resource matrices"
```

Expected: all currently implemented matrix tests pass, including Task 1 positive controls.

### Task 3: Activation and Cleanup Failure Evidence

**Files:**
- Modify: `src/gateway/channel_registry/resource_matrix.rs`
- Modify: `src/gateway/channel_registry/test_support.rs` only for real failure-boundary setup if needed.
- Modify: `src/gateway/interfaces/webhook/mod.rs` only for `cfg(test)` real client-drop observation.

**Interfaces:**
- Consumes: Task 1 fixture, existing `FailingStopWebhook` and existing Webhook `ResourceProbe` event names.
- Produces: partial-activation, failed-stop residual and final owner-drop evidence.

- [ ] **Step 1: Write start-failure and failed-stop tests**

Implement `start_failures_do_not_fabricate_resources_or_cleanup`:
- real Webhook invalid config fails before `client_build_attempt`;
- polling closed-port connect increments `tcp_connect_attempt` without `tcp_connected`;
- polling valid TCP plus invalid SQLite path drops the connected stream and leaves no worker/cache/timer/DB active.

Implement `failed_stop_preserves_real_residual_until_owner_drop`: start real-Webhook-backed `FailingStopWebhook`, assert `client_built > client_drop`, fail stop, assert residual remains nonzero and retry/unregister returns `Busy`, then drop the owning registry/channel and require one real `client_drop` without a second stop success.

- [ ] **Step 2: Run failure tests and record valid RED**

Run with separate memory gates:

```bash
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::start_failures_do_not_fabricate_resources_or_cleanup -- --exact --nocapture
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::failed_stop_preserves_real_residual_until_owner_drop -- --exact --nocapture
```

Expected: start-failure behavior may already pass. Failed-stop is RED only if the real owner-drop observation is absent; compile failure is not RED. Do not change retry semantics to make the test pass.

- [ ] **Step 3: Add only the missing real Drop observation**

If Step 2 proves it absent, add a `#[cfg(test)] impl Drop for WebhookChannel` that takes/drops a non-injected live client and records the existing `client_drop` event. Normal `stop` already takes the client, so it must not double-record. Do not add production lifecycle behavior, a new event type or a resource manager.

- [ ] **Step 4: Run all six matrix tests GREEN**

```bash
python qa/browser_live/check_memory.py
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix:: -- --nocapture
```

Expected: `6 passed; 0 failed; 0 ignored`. Confirm failure tests leave no live tracked worker or loopback server task.

- [ ] **Step 5: Commit failure evidence**

```bash
git add src/gateway/channel_registry/resource_matrix.rs src/gateway/channel_registry/test_support.rs src/gateway/interfaces/webhook/mod.rs
git commit -m "test(gateway): prove activation failure residuals"
```

### Task 4: Full Regression, Independent Review and Closure Records

**Files:**
- Create: `docs/superpowers/runs/2026-10-28-gateway-stage-d-resource-matrices-run.md`
- Modify: `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md` §5
- Modify: `docs/reference/FEATURE_LOCATOR.md` Stage D/resource-matrix section
- Modify only for review findings: exact owning source/test files.

**Interfaces:**
- Consumes: Tasks 1–3 commits and their RED/GREEN evidence.
- Produces: final verification evidence, independent verdict and truthful bounded Stage D closure.

- [ ] **Step 1: Run the focused mid-send tests three serial times each**

For each of `explicit_only_channel_unregistered_mid_send_is_never_persisted` and `explicit_only_channel_replaced_mid_send_is_never_persisted`, run three separate invocations, each with its own memory gate:

```bash
cargo test -p alephcore --test sms_delivery_policy_test <exact-test-name> -- --exact --nocapture
```

Expected: six total invocations, each `1 passed; 0 failed`; watchdog remains five seconds.

- [ ] **Step 2: Run the complete Stage D/Stage C verification set**

Use a serial runner or individual commands, with a fresh memory gate before every Cargo invocation:

```bash
cargo test -p alephcore --lib gateway::channel_registry::resource_matrix::
cargo test -p alephcore --test sms_delivery_policy_test
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

Expected: all exit 0; resource matrix 6 passed; other suite counts are recorded from actual output and no filter runs zero tests. Preserve nonfatal linker warnings in the run evidence.

- [ ] **Step 3: Obtain fresh independent read-only review**

Give the reviewer the spec, this plan, full Stage D diff and actual test logs. Require findings ordered Critical/Important/Minor and explicit Ready verdict, focusing on Review Focus plus authority duplication, real-vs-derived receipts, task/DB cleanup and Stage A–C compatibility. Record exact reviewer errors if unavailable; manual review is not independent approval.

- [ ] **Step 4: Resolve every Critical/Important finding and rerun affected verification**

A finding is resolved only by code/test evidence, not explanation. Re-run the focused test and any affected Stage C suite with fresh memory gates. If a fix needs new recovery semantics, public API or another authority, stop and request architecture direction.

- [ ] **Step 5: Write bilingual closure records and commit**

Record baseline, design decision, fixture limits, actual resources, RED/GREEN commands/results, suite counts, reviewer identity/verdict, remaining Minor risks, deferred Stage E/R1/providers/kinds and all unrun validation. Do not claim production polling, failed-stop retry, terminal/memory-graph/sandbox isolation or production mount.

```bash
git add docs/superpowers/runs/2026-10-28-gateway-stage-d-resource-matrices-run.md docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md docs/reference/FEATURE_LOCATOR.md
git commit -m "docs(gateway): close stage d resource matrices"
git status --short --branch
```

Expected: clean `stage-d-resources` worktree. No merge, push, worktree cleanup, external message or Stage E action without separate authorization.
