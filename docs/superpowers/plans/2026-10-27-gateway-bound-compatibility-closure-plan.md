# Gateway Bound Compatibility Closure — Stage C Implementation Plan

> **Execution evidence (2026-10-28):** implementation through `66489d94b`; required Stage C suites GREEN. Independent review unavailable (`No API key found for anthropic.`); manual checklist completed. Two additional SMS mid-send integration timeouts remain open. See [`../runs/2026-10-28-gateway-bound-compatibility-closure-run.md`](../runs/2026-10-28-gateway-bound-compatibility-closure-run.md). This does not promote R1 or Stage D.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close existing Gateway Legacy compatibility surfaces so a Bound channel cannot obtain execution, resource initialization, webhook delivery, queue admission, or lifecycle authority through an older Legacy path, while preserving Legacy behavior.

**Architecture:** Keep `ChannelRegistry` as the only association/source authority. Dispatch compatibility lifecycle calls by the existing `ChannelRegistration::{Legacy, Bound}` variant; Legacy continues through its current markers and I/O, while Bound reuses `start_bound_channel`/`stop_bound_channel` and their existing source-owned driver/provider/entry checks. Add source validation to the existing webhook mount table and require Registry validation before dynamic queue admission; do not add a registry, owner map, descriptor map, runtime table, public Bound route, or generalized envelope.

**Tech Stack:** Rust, Tokio, Axum, reqwest-backed `WebhookChannel`, SQLite delivery queue, existing `ProviderHandle`, `OwnershipTree`, `ResourceProbe`, `LoopbackPeer`, and Cargo test/check tooling.

**Spec:** `docs/superpowers/specs/2026-10-27-gateway-bound-compatibility-closure-design.md`

## Global Constraints

- Scope is Stage C compatibility closure only; do not enter Stage D resource matrices, Feishu/R2, production Bound mount, Zahir, public Bound send/ingress, EffectClaim, Safe Replay, durable recovery, scheduler, new `CapabilityKind`, generalized envelope, second authority, or `src/harness/**`.
- `ChannelRegistry` remains the only Gateway association/source authority; use existing entry-local `EntryRuntime`, `ProviderHandle`, and `OwnershipTree`.
- Bound denial must happen before ticket, resource, transport, media custody, queue row, handler dispatch, inbound forwarding, or external effect creation.
- Every Cargo command must be preceded by `python qa/browser_live/check_memory.py`; require exit `0` and `ALLOW`, and record actual exit, test counts, and non-fatal linker warnings.
- Write behavioral RED tests before production changes. Parser/import/compile/test setup failures are setup failures, not contract RED.
- Keep `lifecycle_sync` out of channel/transport/stop/close/join/webhook I/O.
- Do not use fake receipts or manually mutate `active_work`/runtime state as permission; use real Registry state, loopback, webhook, queue, provider, entry, forwarder, and source-publication observations.
- Preserve Legacy positive controls and do not turn `Option`, `bool`, empty values, or fallback results into permission.
- Only update run/source/reference documentation after all contracts are GREEN and review is complete.

## Review Focus

- **Retained webhook mount after source retirement/replacement:** it must fail before handler parsing or inbound forwarding; test in Task 2.
- **Bound lifecycle compatibility:** it must use existing source-owned drivers/provider transitions and release the source gate before I/O; test in Task 3.
- **Raw queue admission with only `ChannelId`:** it must not write SQLite without current Legacy validation; test in Task 4.
- **Cold projections:** list/status/health/directory/snapshot/capture must not build a client, issue a request, spawn a forwarder, or activate a provider; test in Task 5.
- **Refusal interpretation and Legacy regression:** refusal cannot become success/fallback permission, and Legacy webhook send/mount behavior remains unchanged; test in Task 5.

---

### Task 1: Add all five behavioral RED contracts

**Files:**
- Modify: `src/gateway/channel_registry.rs` test module around `#[cfg(test)] mod tests` at `:2120+`
- Modify: `src/gateway/webhook_receiver.rs` test module around `#[cfg(test)] mod tests` at `:362+`
- Modify: `src/gateway/delivery_queue.rs` test module around `#[cfg(test)] mod tests` at `:2273+` only if shared queue assertions must live there
- Modify: `src/gateway/channel_registry/test_support.rs` only for missing real observation hooks; do not add authority state

**Interfaces:**
- Consumes existing `ChannelRegistry::register_bound_channel`, `capture_bound_channel`, `start_bound_channel`, `stop_bound_channel`, `ResourceProbe`, `LoopbackPeer`, `HeldWebhook`, `WebhookMountTable`, `DeliveryStore`, and existing provider/source publication state.
- Produces five named tests that fail for behavioral reasons against the current implementation:
  `bound_compatibility_effect_paths_are_denied`,
  `bound_held_webhook_handler_denies_dispatch`,
  `bound_cold_lists_never_call_transport`,
  `bound_compat_lifecycle_uses_source`,
  `legacy_positive_controls_unchanged`.

- [ ] **Step 1: Write `bound_compatibility_effect_paths_are_denied`** in the existing Registry test module.

  Register a real Bound Webhook declaration using the existing Stage B constructor/helpers. Exercise representative Registry effect helpers, delivery admission, and any compatibility path that returns an `Option`, boolean, or error. Assert the result is an explicit refusal and that no `ResourceProbe` client build/request/drop, `LoopbackPeer` request, queue row, ticket, transport call, or fallback success occurs. Assert the queue row count before and after rather than relying on a fake receipt.

- [ ] **Step 2: Write `bound_held_webhook_handler_denies_dispatch`** using the existing real Webhook mount and held handler support.

  Build a valid signed request, retain the handler/mount, then retire, replace, or convert the source so the mount is stale. Assert the endpoint refuses before `handler.handle` and before `InboundMessageSender::send`; use a counter/oneshot in the real handler/inbound path and assert neither fires. Keep HMAC valid so the assertion covers source validation rather than signature failure.

- [ ] **Step 3: Write `bound_cold_lists_never_call_transport`** in the Registry test module.

  Exercise `list`, `status_summary`, `health_states`, directory/conversation projection, source snapshot, and capture-like reads for `Discovered`, `Cold`, and `Disabled` Bound declarations. Assert metadata may be observed, but `ResourceProbe` records no client build, request, drop caused by startup, forwarder creation, queue write, or provider activation. Do not call `start_bound_channel` in this test.

- [ ] **Step 4: Write `bound_compat_lifecycle_uses_source`** in the Registry test module.

  Call public compatibility `start_channel`, `stop_channel`, and `restart_channel` against a Bound entry backed by `HeldWebhook`/`ResourceProbe`. Use lifecycle barriers to show the source-owned driver is active, the provider/entry runtime changes occur through the existing binding path, and the source gate is not held while channel I/O is blocked. Exercise failed cleanup or stale completion and assert it does not fabricate `Ready` or revive the entry.

- [ ] **Step 5: Write `legacy_positive_controls_unchanged`** using a real Legacy Webhook and `LoopbackPeer`.

  Assert Legacy start builds the client and mount, send reaches the loopback peer, webhook dispatch forwards inbound data, and stop unmounts/drops resources. Assert the current successful behavior remains available through the same compatibility methods.

- [ ] **Step 6: Run only the new focused tests to establish behavioral RED.**

  Before each Cargo command, run:

  ```bash
  python qa/browser_live/check_memory.py
  ```

  Then run the narrowest affected test commands. Expected result: the tests compile after setup fixes and fail on the current compatibility gaps (Bound lifecycle refusal instead of source routing, stale webhook dispatch, and/or raw queue admission). Do not modify production code to make them pass in this task. Record exit codes, counts, and any linker warning.

- [ ] **Step 7: Commit the RED tests** with message `test(gateway): pin bound compatibility closure contracts` after confirming failures are behavioral.

### Task 2: Close source-aware webhook dispatch

**Files:**
- Modify: `src/gateway/webhook_receiver.rs:58-357`
- Modify: `src/gateway/channel_registry.rs:278-315, 1017-1032` and the existing Registry mount construction path
- Test: `src/gateway/webhook_receiver.rs` and `src/gateway/channel_registry.rs` tests from Task 1

**Interfaces:**
- Consumes the existing canonical Registry source/publication identity and Legacy registration variant.
- Produces a Registry-owned mount validation operation with signature `pub(crate) async fn validate_legacy_webhook(&self, channel_id: &ChannelId, source_revision: u64) -> ChannelResult<()>`; it returns no channel handle and validates current Legacy registration/publication identity only.

- [ ] **Step 1: Define the smallest mount identity data needed by the existing `WebhookMount`** to distinguish the current Legacy registration from a retained/replaced/retired entry. Use the existing source publication revision/entry identity; do not add an owner map or descriptor map.

- [ ] **Step 2: Add the Registry-owned source validation call at the endpoint boundary.** Preserve endpoint order: table lookup → HMAC verification → source/registration validation → status validation → handler dispatch → inbound forwarding. A stale, missing, Bound, retired, or replaced mount must return a refusal before `handler.handle` or inbound send. The validation must release all Registry/source locks before HMAC, handler parsing, or inbound I/O. Mount-time validation may make a retained stale `mount()` return `false`; the dispatch-time endpoint refusal remains the behavioral contract and the Task 1 unrelated-registration positive control must remain intact.

- [ ] **Step 3: Keep Legacy mount creation and unmount/restart refresh behavior unchanged.** `start_channel`, `stop_channel`, and `restart_channel` must continue mounting only after successful Legacy start and removing stale mounts on stop/restart.

- [ ] **Step 4: Run the webhook and Registry contract tests.**

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::webhook_receiver::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::tests::bound_held_webhook_handler_denies_dispatch
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::tests::legacy_positive_controls_unchanged
  ```

  Expected: retained Bound/stale dispatch is denied before handler/inbound effects; Legacy mount tests remain GREEN. Record actual counts and warnings.

- [ ] **Step 5: Commit the webhook closure** with message `fix(gateway): fence retained webhook mounts by source`.

### Task 3: Route compatibility lifecycle through the existing source-owned path

**Files:**
- Modify: `src/gateway/channel_registry.rs:1035-1112, 1891-1941`
- Reuse/possibly adjust: `src/gateway/channel_registry/binding.rs:730-989` only for error/result mapping required by the compatibility boundary; do not create another lifecycle driver
- Test: `src/gateway/channel_registry.rs` tests from Task 1 and existing binding tests around `:1030+`

**Interfaces:**
- Existing source-owned signatures remain the authority:
  `ChannelRegistry::start_bound_channel(&Arc<Self>, &ChannelId) -> Result<LifecycleResult, ChannelBindingError>` and
  `ChannelRegistry::stop_bound_channel(&Arc<Self>, &ChannelId) -> Result<LifecycleResult, ChannelBindingError>`.
- Public compatibility methods retain `ChannelResult<()>` and map/await the existing `LifecycleResult`; no new public Bound route is added.

- [ ] **Step 1: Add registration-variant dispatch inside `start_channel` and `stop_channel`.** Legacy entries retain `legacy_entry`, `begin_legacy_operation`, `start_channel_inner`/`stop_channel_inner`, and `finish_legacy_operation`. Bound entries call the existing source-owned operation, await its oneshot result outside `lifecycle_sync`, and convert `ChannelBindingError` to the existing `ChannelError` shape without swallowing refusal.

- [ ] **Step 2: Implement Bound `restart_channel` as source-owned stop followed by source-owned start.** Do not call Legacy `channel.stop()`/`channel.start()` for a Bound entry, create a second forwarder, or fabricate Running/Ready after a failed stop/start. Keep the existing Legacy in-place restart path unchanged.

- [ ] **Step 3: Verify health monitor behavior through the same central entry.** `sweep` continues to call `registry.restart_channel`; Bound entries must be refused or source-routed before transport I/O while desired state, Busy, residual, and Disabled semantics remain unchanged.

- [ ] **Step 4: Run lifecycle RED/GREEN tests and existing binding tests.**

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::tests::bound_compat_lifecycle_uses_source
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::tests::legacy_positive_controls_unchanged
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::binding::tests::
  ```

  Expected: compatibility lifecycle uses the existing source driver/provider/entry path; Legacy lifecycle and source binding tests remain GREEN. Record whether the macOS linker warning appears.

- [ ] **Step 5: Commit lifecycle closure** with message `fix(gateway): route bound compatibility lifecycle through source`.

### Task 4: Close effect and delivery queue admission

**Files:**
- Modify: `src/gateway/delivery_queue.rs:870-918, 1020-1045` and relevant queue tests
- Modify: `src/gateway/channel_registry.rs` effect helpers and `maybe_enqueue`/queue boundary
- Modify: `src/gateway/inbound_router/executor.rs:925-980` fixture callsite
- Test: `src/gateway/channel_registry.rs` `bound_compatibility_effect_paths_are_denied`

**Interfaces:**
- Produces `pub(crate) async fn enqueue_validated_legacy(registry: &ChannelRegistry, store: &DeliveryStore, channel_id: &str, message: &OutboundMessage, last_error: &str, next_attempt_at: i64) -> Result<EnqueueOutcome, ChannelError>` for fixture/import compatibility callers. It validates Legacy registration before calling the private store write; SQL failures map to the existing `ChannelError::Internal` shape. The normal `maybe_enqueue` path calls the same private store write only after its existing Registry/policy checks.

- [ ] **Step 1: Make the raw store write private to the delivery queue module.** Keep its SQLite implementation and outcome semantics unchanged. Expose only a crate-private helper that accepts `&ChannelRegistry` plus the channel id/message/error/timestamp, validates current Legacy registration through the existing Registry boundary, then calls the private store write. Do not accept a raw Bound handle, receipt, or generalized envelope.

- [ ] **Step 2: Update direct fixture/import callsites.** Change `src/gateway/inbound_router/executor.rs` and Registry/queue tests that directly call `DeliveryStore::enqueue` to use the validated Legacy helper or the queue module's own private test path. No production dynamic path may write a queue row from an unchecked `ChannelId` alone. Preserve the three Task 1 contract assertions in `bound_compatibility_effect_paths_are_denied`: refusal/no `Queued(_)`, unchanged persisted row count, and zero pending queue rows.

- [ ] **Step 3: Confirm effect helpers deny before all side effects.** Ensure send/react/edit/typing/poll/group-icon/public telecom/SMS compatibility helpers reject Bound before channel read guards, tickets, media custody, queue flush, `enqueue_fresh`, or transport. Audit callers so `Option`, bool, empty collection, and error mapping cannot become permission or success.

- [ ] **Step 4: Run queue/effect tests.**

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::tests::bound_compatibility_effect_paths_are_denied
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::delivery_queue::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::inbound_router::executor::tests::
  ```

  Expected: Bound effect/queue attempts fail before a queue row, media read, loopback request, ticket, or transport call; Legacy queue fixtures and replay behavior remain GREEN.

- [ ] **Step 5: Commit queue/effect closure** with message `fix(gateway): deny bound compatibility effects before admission`.

### Task 5: Prove cold reads and Legacy positive controls

**Files:**
- Modify only where required by the RED test: `src/gateway/channel_registry.rs`, `src/gateway/handlers/channel.rs`, `src/gateway/handlers/discord_panel.rs`, `src/builtin_tools/channel_directory.rs`, `src/builtin_tools/channel_message.rs`
- Test: `src/gateway/channel_registry.rs`, `src/gateway/channel_health_monitor.rs`, `src/gateway/webhook_receiver.rs`, and existing `tools::registry` test module as applicable

**Interfaces:**
- Read paths remain projections over existing canonical entries and provider observations; they must not call Legacy adapter methods or activate Bound providers.
- Handler start must reject Bound before config/vault/factory/registration effects; Discord panel and builtin tools must remain Legacy-only through existing `get`/effect choke points.

- [ ] **Step 1: Make the cold-read RED test GREEN without adding a new projection authority.** If the current `list`, status, health, directory, snapshot, or capture path touches a transport/resource boundary, move the read to existing metadata/state access. Do not hide Bound rows merely to make the test pass; metadata visibility and execution permission remain separate.

- [ ] **Step 2: Add the earliest central Bound check to `handlers/channel.rs` start.** The check must happen before configuration secret injection, factory construction, Telegram-specific state creation, registration/replacement, or lifecycle start. Preserve Legacy start behavior and error mapping.

- [ ] **Step 3: Audit Discord panel and builtin channel tools.** Keep `require_connected_channel`/`get` and Registry effect guards as the source of Legacy-only permission. If a path can construct a Serenity client, call an adapter roster method, or send an effect after a Bound target, add the smallest pre-effect check; do not introduce a panel-specific authority.

- [ ] **Step 4: Run cold, health, tools, and Legacy control suites.**

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::tests::bound_cold_lists_never_call_transport
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib tools::registry::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::interfaces::webhook::
  ```

  Expected: cold reads and health inspection produce no transport/client/forwarder/provider events; existing health policy and tool/Legacy controls remain GREEN.

- [ ] **Step 5: Commit cold-read and control closure** with message `test(gateway): preserve cold projections and legacy controls`.

### Task 6: Run complete verification and independent review

**Files:**
- Modify: none until verification findings require a targeted fix
- Review: all changed Rust files and the approved design/plan

- [ ] **Step 1: Run the required focused suites, with a memory gate before every Cargo command.**

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_registry::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::webhook_receiver::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::delivery_queue::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib gateway::channel_health_monitor::tests::
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib tools::registry::tests::
  python qa/browser_live/check_memory.py
  cargo check -p alephcore --tests
  python qa/browser_live/check_memory.py
  cargo fmt --all --check
  git diff --check
  ```

  Record each command's actual exit code, passed/failed/ignored counts, and non-fatal linker warnings. If capability or Webhook boundaries changed, also run the matching `capability::` suite with its own preceding memory check.

- [ ] **Step 2: Dispatch one independent read-only review** focused on Bound bypass before side effects, refusal interpretation, raw handle leakage, cold resource initialization, source-gate/I/O separation, Legacy regressions, and deferred-scope expansion.

- [ ] **Step 3: If the reviewer service is unavailable, record the complete error and perform the same checklist manually.** Do not describe the manual checklist as an independent review.

- [ ] **Step 4: Fix any review or verification finding with a new targeted RED/GREEN cycle.** Do not mark the task complete while tests or review findings remain unresolved.

### Task 7: Update evidence and references after GREEN

**Files:**
- Modify: `docs/superpowers/plans/2026-10-26-gateway-source-owned-runtime-authority-plan.md`
- Create: `docs/superpowers/runs/2026-10-27-gateway-bound-compatibility-closure-run.md`
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify only relevant sections: `docs/reference/GATEWAY.md`, `docs/reference/ARCHITECTURE.md`, or other Gateway reference files if their current wording contradicts the verified behavior

- [ ] **Step 1: Record the bounded slice and exact evidence.** Include the actual base/HEAD, changed files, five RED/GREEN test names, every command's exit/count/warning, and independent-review availability/error.

- [ ] **Step 2: Record authority duplication review.** State that `ChannelRegistry` remains the only association/source authority and that `EntryRuntime`, `ProviderHandle`, and `OwnershipTree` remain their existing supporting roles.

- [ ] **Step 3: Record the first unclosed contract and deferred providers/kinds.** Preserve R1 NO-GO/deferred, Stage D resource matrix, Feishu/R2, production mount, Zahir, public send/ingress, EffectClaim, Safe Replay, durable recovery, scheduler, and remaining-provider boundaries.

- [ ] **Step 4: Record skipped validation and actions not taken.** List tests not run, any unresolved non-fatal warning, and explicitly state that no destructive or external action was performed.

- [ ] **Step 5: Review the documentation diff and commit evidence** with message `docs(gateway): record bound compatibility closure evidence` only after code and review are complete.

## Completion Checklist

- [ ] Five named contract tests are behavioral GREEN tests.
- [ ] Bound Legacy/effect paths fail before ticket/resource/queue/effect creation.
- [ ] Retained webhook handlers fail closed after source retirement/replacement/Bound conversion.
- [ ] Cold reads do not initialize transport, client, forwarder, provider, or queue resources.
- [ ] Compatibility lifecycle uses existing source-owned Bound operations and does not hold the source gate across I/O.
- [ ] Legacy positive controls remain GREEN.
- [ ] No second Gateway authority, CapabilityKind, public Bound route, production mount, or unrelated subsystem change exists.
- [ ] Required commands, counts, warnings, review result, first unclosed contract, skipped validations, and destructive/external action status are recorded.
