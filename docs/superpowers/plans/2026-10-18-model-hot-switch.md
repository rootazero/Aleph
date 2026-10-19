# Model Hot Switching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the active Aleph session switch its provider/model at any time so the next LLM request uses the new target, while preserving in-flight request snapshots, run/tool state, context semantics, and durable auditability across restart.

**Architecture:** Keep one session-scoped live selection authority in `providers::session_model_handle`, add a generation-bearing read snapshot, and put a request-boundary provider adapter in the existing `AiProvider` decorator chain. A request resolves the current session selection exactly when `process` or `execute_streaming_dyn` begins; the adapter delegates the whole request to that immutable target, so a concurrent selection cannot rewrite an in-flight call. The existing `select_model`/`/model` path remains the write seam for the CLI and Panel, and the existing `AssistantMessage` one-to-one call journal carries optional request-level effective-target provenance.

**Tech Stack:** Rust, Tokio, serde, existing `AiProvider` / `DefaultProviderHandle` abstractions, existing session event store and context compaction, Leptos/WASM Panel UI, JSON-RPC gateway, existing Cargo test suites.

**Spec:** `docs/superpowers/specs/2026-10-18-model-hot-switch-design.md`

## Global Constraints

- All implementation work stays in the dedicated worktree `D:/Workspace/Aleph-model-hot-switch`; do not modify `main` directly.
- Core/provider code must not call platform APIs; CLI and Panel remain I/O adapters over the shared gateway/Core seam (R1, R2, R4, R6).
- Do not add a second process-global model map or a prompt-side classifier/fallback policy; the session handle and request resolver remain the single sources of truth (R7, R9, R10).
- Selection-time validation rejects unknown/ambiguous/unauthenticated/unsupported targets and leaves the previous valid selection in force; request-time resolution revalidates live availability and fails closed if configuration changed.
- A selection affects all runs using the same canonical `SessionKey` at their next request boundary; an explicit per-request/run override remains higher precedence.
- A current streaming request keeps its resolved provider/model/auth snapshot; a selection made during that request cannot mutate it.
- If the selected model has a smaller context window, use the existing compaction/request-preparation path and surface compaction or capacity failure; never silently drop history or alter tool semantics.
- Persist session selection and request effective target through the existing session store/event projections so restart recovery can audit the actual provider/model; do not put provenance into the provider prompt.
- Do not change `sessions.patch` into a second model-pin writer.

## Review Focus

- A selection succeeds but the provider disappears before the next request: request resolution must fail closed with a visible diagnostic, never silently fall back to another provider. **Test in Task 2:** `request_resolution_rejects_provider_removed_after_selection`.
- A concurrent run selects a new model while another request is streaming: the active request must finish on its original target and the following request must use the new generation. **Test in Task 2:** `streaming_request_keeps_immutable_target_while_next_request_sees_new_generation`.
- A session selection and a per-request `model_override` coexist: the explicit request override must remain higher precedence and must not be replaced by the session adapter. **Test in Task 3:** `explicit_request_override_bypasses_live_session_selection`.
- A smaller selected context window triggers existing compaction: compaction must run through the existing seam and preserve tool/event ordering. **Test in Task 4:** `session_model_switch_reuses_context_compactor_for_narrower_window`.
- A process restarts after a selection and after a request: the selection and the effective request target must be reconstructable from durable state, including legacy rows with missing optional fields. **Tests in Task 1 and Task 5:** `selection_rehydrates_with_generation` and `assistant_message_effective_target_is_backward_compatible`.

---

### Task 1: Make the session selection a generation-bearing control seam

**Files:**
- Modify: `src/providers/session_model_handle.rs`
- Modify: `src/gateway/session_model_pin.rs`
- Modify: `src/gateway/execution_engine/turn_model.rs`
- Test: inline unit tests in `src/providers/session_model_handle.rs` and `src/gateway/session_model_pin.rs`

**Interfaces:**
- Preserve `SessionModelPref { provider: Option<String>, model: String }`, `set_session_model`, `get_session_model`, `hydrate_session_model`, and `clear_session_model` for existing callers.
- Add a public provider-layer snapshot type, for example `SessionModelSnapshot { pref: Option<SessionModelPref>, generation: u64 }`, with `get_session_model_snapshot(session_key: &str) -> SessionModelSnapshot`.
- Add a single update result, for example `SessionModelUpdate { pref: Option<SessionModelPref>, generation: u64 }`, returned by an internal setter used by `select_model`; keep legacy setter behavior for tests/callers that do not need the result.
- Make `hydrate_session_model` restore the durable generation when present and never overwrite a newer live generation.
- Extend the durable `model_pin` carrier with a generation key only if the existing session metadata path can preserve it without creating a second writer; otherwise derive a monotonic process generation and document that the durable selection row is authoritative after restart.

- [ ] **Step 1: Write failing tests** for generation monotonicity, clear/reselect behavior, cold rehydration, live rehydration non-overwrite, and two canonical session keys staying isolated.
- [ ] **Step 2: Run the focused tests** with `cargo test -p alephcore session_model_handle` (or the repository’s package name discovered from `Cargo.toml`) and confirm the new assertions fail before implementation.
- [ ] **Step 3: Implement the generation-bearing read/update seam** in `src/providers/session_model_handle.rs`; keep the map as a cache and the session row as the durable record. Do not add another map.
- [ ] **Step 4: Update `StoreBackedPinSink` and `turn_model` rehydration** so the same selection/generation pair is written and recovered, with explicit `null` clearing retained for both model and provider keys.
- [ ] **Step 5: Run the focused tests** and verify PASS, including existing session pin tests.
- [ ] **Step 6: Commit** with `git add src/providers/session_model_handle.rs src/gateway/session_model_pin.rs src/gateway/execution_engine/turn_model.rs` and `git commit -m "refactor: expose generation-bearing session model selection"`.

---

### Task 2: Add request-boundary live provider resolution

**Files:**
- Create: `src/providers/session_model_provider.rs`
- Modify: `src/providers/mod.rs`
- Modify: `src/providers/model_override_provider.rs`
- Modify: `src/providers/metering.rs`
- Modify: `src/providers/think_level_provider.rs`
- Modify: `src/providers/failover/provider.rs`
- Modify: `src/orchestrator/harness_bridge/llm.rs`
- Test: inline provider tests and `src/orchestrator/harness_bridge/llm.rs` tests

**Interfaces:**
- Add a provider-neutral identity type, for example:
  `ResolvedModelTarget { provider: String, model: String, generation: u64 }`.
- Extend `AiProvider` with a defaulted `last_request_target(&self) -> Option<ResolvedModelTarget>` accessor so existing providers remain source-compatible; forward it through every decorator that can wrap the serving provider.
- Add `SessionModelProvider::new(session_key: String, fallback: Arc<dyn AiProvider>, default_provider: Arc<dyn DefaultProviderHandle>, named: Arc<HashMap<String, Arc<dyn AiProvider>>>) -> Arc<dyn AiProvider>`.
- Its `process` and `execute_streaming_dyn` must resolve a `SessionModelSnapshot` at call entry, resolve an explicitly pinned provider from `named`, use `default_provider.current()` for an unqualified pin, stamp the selected model using `ModelOverrideProvider`, record the immutable `ResolvedModelTarget`, and delegate the entire request to that target.
- A selected provider missing from `named` returns `FlowError::ProviderUnavailable`/the existing provider error path; it must not fall back. With no session selection, delegate to the supplied fallback provider.
- Metadata methods (`supports_streaming`, native tools, protocol, behavior hints, serving model/provider) read the same current target or fallback and remain compatible with the existing streaming decorator chain.

- [ ] **Step 1: Write failing provider tests** for immediate next-call switching, provider-qualified and unqualified selections, provider removal fail-closed, streaming delegation, target generation recording, and no-selection fallback.
- [ ] **Step 2: Run the focused provider tests** with `cargo test -p alephcore providers::session_model_provider` and confirm they fail because the adapter/accessor do not exist.
- [ ] **Step 3: Implement `ResolvedModelTarget` and the defaulted `AiProvider::last_request_target` accessor**, forwarding it through `ModelOverrideProvider`, `MeteringProvider`, `ThinkLevelProvider`, and `FailoverProvider` without changing the existing public request methods.
- [ ] **Step 4: Implement `SessionModelProvider`** with one target snapshot per `process`/`execute_streaming_dyn` invocation; use the existing provider maps/handles and `ModelOverrideProvider`, not a new routing table.
- [ ] **Step 5: Update `pick_llm`/bridge construction hooks** only where needed to make the live wrapper available; preserve `BrainRef` behavior and existing `ModelOverrideProvider` precedence.
- [ ] **Step 6: Run focused tests** and verify PASS for both batch and streaming paths, then commit with `git commit -m "feat: resolve session model at each request boundary"`.

This task may run in parallel with Task 1’s test review only after Task 1’s snapshot interface names are fixed; it must consume Task 1’s generation API before implementation is merged.

---

### Task 3: Integrate live selection with run precedence and request provenance

**Files:**
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs`
- Modify: `src/gateway/execution_engine/run_loop/inner.rs`
- Modify: `src/session/events.rs`
- Modify: `src/harness/agent/think.rs`
- Modify: `src/session/store.rs`
- Modify: `src/session/projection.rs`
- Test: `src/orchestrator/harness_bridge/tests.rs`, `src/gateway/execution_engine/run_loop/tests.rs`, and session event tests

**Interfaces:**
- Preserve the existing precedence: explicit request `RunRequest.model_override` > live session selection > agent `model_hint` > flow `BrainRef`.
- `runner_impl` must construct the live session wrapper only when there is no stronger explicit request override; the wrapper may use the agent/brain provider as its fallback.
- Add optional, serde-defaulted fields to `SessionEvent::AssistantMessage`: `model: Option<String>`, `model_provider: Option<String>`, and `model_generation: Option<u64>` (or the exact equivalent finalized in Task 1). These fields are per-request facts and must not enter prompt reconstruction.
- `AgentHarness` must read the provider’s `last_request_target()` immediately after each LLM call and populate the corresponding `AssistantMessage`; preserve legacy `None` for providers that cannot prove a target.
- `SessionEvent` serialization, event-kind indexing, store reads, prompt builders, projections, and all exhaustive matches must treat the new fields as optional and ignore them for prompt content.

- [ ] **Step 1: Write failing precedence/provenance tests** for explicit request override versus live session pin, next request within the same run, same-session concurrent runs, model/provider/generation fields on `AssistantMessage`, and legacy JSON deserialization without the fields.
- [ ] **Step 2: Run the focused tests** with `cargo test -p alephcore orchestrator::harness_bridge gateway::execution_engine::run_loop session::events` and confirm the new expectations fail.
- [ ] **Step 3: Add the optional event fields and update store/projection/prompt matches** without changing event ordering or making provenance prompt-visible.
- [ ] **Step 4: Wire `last_request_target` through the harness Think path** at the existing `AssistantMessage` construction point in `src/harness/agent/think.rs`; use the same target for trace/cost/run metadata where the current code already derives serving identity.
- [ ] **Step 5: Replace the session-pin-as-run-start binding in `runner_impl.rs` only for the normal path** with `SessionModelProvider`; keep crash-resume `RunEnvelopeSnapshot` as the frozen run snapshot and keep explicit request overrides static for that run.
- [ ] **Step 6: Verify focused tests**, including resume tests that prove a later session selection does not rewrite a frozen interrupted run, then commit with `git commit -m "feat: apply session model changes to the next request"`.

---

### Task 4: Verify context compaction and both control surfaces

**Files:**
- Modify: `src/builtin_tools/select_model.rs`
- Modify: `src/gateway/execution_engine/slash_command.rs`
- Modify: `interfaces/ui/src/ui/commands.rs`
- Modify: `interfaces/ui/src/ui/widgets/provider_picker.rs`
- Modify: `interfaces/ui/src/ui/widgets/status_bar.rs`
- Modify: `interfaces/cli/src/commands/chat_cmd.rs` only if the existing CLI slash path lacks the required session key/diagnostic propagation
- Test: select-model tests, slash-command tests, UI picker tests, and gateway integration tests

**Interfaces:**
- Keep `select_model` as the only model-pin writer. `sessions.patch` remains `NOT_PATCHABLE` for `model_pin` and `model_pin_provider`.
- Keep `/model <id>` and `/model provider/model` as adapters that build `SelectModelArgs { model, provider }` and invoke the same tool/control seam; do not add a parallel Panel-only write path.
- Successful output must expose the selected provider/model and generation or a stable equivalent for CLI and Panel display; refusal output must identify the reason and state that the prior selection remains active.
- The Panel picker confirmation must continue through the gateway `/model` route, and its status pill must read the durable/session selection when no per-turn override is active.

- [ ] **Step 1: Add failing surface tests** proving CLI `/model`, Panel provider-picker confirmation, and `select_model` all update the same session preference; add refusal tests for invalid provider/model and verify the old selection remains.
- [ ] **Step 2: Run focused surface tests** with `cargo test -p alephcore gateway::execution_engine::slash_command builtin_tools::select_model` plus the relevant interface package test command and confirm new tests fail where wiring is missing.
- [ ] **Step 3: Update `select_model` diagnostics and generation propagation** while retaining the current MoA/operator gate and existing lifecycle/catalog rules.
- [ ] **Step 4: Update slash/Panel adapters only where needed** so they construct the typed selector payload and display the durable/current pin; do not write `identity_meta.custom` directly from UI code.
- [ ] **Step 5: Add the narrower-context integration test** through the existing context budget/compaction seam, asserting the selected model’s context window drives preparation and tool/event order remains unchanged.
- [ ] **Step 6: Run the focused surface and compaction tests**, then commit with `git commit -m "feat: connect model hot switch to CLI and Panel"`.

This task is the second parallel line after Tasks 1–2 publish stable selector and provider interfaces; it must merge before the final full verification task.

---

### Task 5: Full verification, documentation, and cleanup

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify: `docs/reference/SESSION_KNOBS.md`
- Modify: `docs/reference/MODEL_CATALOG.md`
- Modify: `docs/reference/SESSION_SERVICE.md`
- Modify: `docs/reference/ARCHITECTURE.md` only if the final boundary is absent there
- Delete: only confirmed dead model-binding helpers or stale comments discovered by the implementation census
- Test: repository test suites and architecture/source-derived census tests

**Interfaces:**
- Documentation must name the authoritative selection seam, request-boundary adapter, precedence, durable fields, CLI/Panel route, and explicit deferred behavior.
- `FEATURE_LOCATOR.md` must record that in-flight requests remain frozen, same-session next requests see the new generation, and provider disappearance fails closed.
- No documentation may claim automatic safe replay, external-effect exactly-once, universal durable scheduling, or runtime mount completion as a side effect of this feature.

- [ ] **Step 1: Run formatting and targeted checks**: `cargo fmt --check`; the focused provider, gateway, harness, session, and interface tests from Tasks 1–4.
- [ ] **Step 2: Run the package-level checks appropriate to changed crates**: `cargo check`, `cargo clippy --all-targets --all-features -- -D warnings` where supported, and `cargo test` for the affected packages.
- [ ] **Step 3: Run source-derived census/architecture checks** for all `AiProvider` decorators, session knob writers, `SessionEvent` serialization/projection matches, and the `/model` route; fix dead code, stale comments, or mismatched docs found by the checks.
- [ ] **Step 4: Update the four reference documents** from the final code and record any intentionally unverified Windows-only or external-provider checks as unrun rather than passed.
- [ ] **Step 5: Perform an independent review** of the complete worktree focused on R1/R2/R4/R7/R9/R10, concurrent selection semantics, provider credential isolation, resume behavior, and prompt/tool compatibility.
- [ ] **Step 6: Run the final verification set**, inspect `git diff --check`, `git status`, and the complete diff; commit with `git commit -m "feat: support live session model switching"`.

## Execution Order and Parallel Lines

1. Create/use the dedicated worktree and establish Task 1’s generation-bearing control interface.
2. Run Task 2’s provider adapter and Task 4’s surface inventory in parallel after the interface names are fixed; Task 4’s code merge waits for Task 2’s target-resolution contract.
3. Run Task 3 after Task 2’s adapter exists; its durable provenance work may proceed in parallel with Task 4’s UI test work when the event field names are stable.
4. Merge both lines, run Task 5’s full verification and independent review, then commit the final branch.
5. Between iterations, choose the next action from the first failing deterministic check; do not retry a passing area without a new failure or a changed interface.
6. If blocked, stop with the exact failing command, attempted fixes, affected files, and the missing decision or environment capability; do not claim unrun checks passed.
