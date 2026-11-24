# Stage G Provider-Neutral Production Mount Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bounded, provider-neutral readiness evidence proving that the existing production `ProjectionHost` composes the canonical registry and shared ownership tree through the existing Extension lifecycle, without adding a new mount or authority.

**Architecture:** Reuse the production composition already present in `src/bin/aleph-server/commands/start/mod.rs`: one canonical `ToolHandlerRegistry`, one production `OwnershipTree`, one `ProjectionHost`, and the existing `ExtensionManager`/`RuntimeOwner` lifecycle. The implementation changes only test fixtures and evidence documentation; `ProjectionHost`, `ZahirFacade`, `ProviderHandle`, `RuntimeOwner`, and the WASM disposer keep their current ownership and cleanup responsibilities. The new tests use the host’s facade and a manager created with the same ownership tree so the evidence cannot pass through a test-only second tree.

**Tech Stack:** Rust, Tokio, `alephcore` unit tests, real Extism WASM fixture instrumentation, `ToolHandlerRegistry`, `OwnershipTree`, `ProjectionHost`, `ZahirFacade`, existing `ExtensionManager` lifecycle, bilingual Markdown documentation.

**Spec:** [`docs/superpowers/specs/2026-10-29-gateway-stage-g-provider-mount-design.md`](../specs/2026-10-29-gateway-stage-g-provider-mount-design.md)

## Global Constraints

- Do not add a production mount, wrapper, channel adapter, provider registry, descriptor catalog, owner map, generation map, runtime table, lifecycle state machine, or cleanup authority.
- Reuse the existing production `ToolHandlerRegistry` and `OwnershipTree`; do not promote `ChannelRegistry` to universal capability ownership.
- Keep `ExtensionManager`/`plugin_state.rs` as discovery and enabled/disabled authority; `ProviderHandle`/`ProviderLifecycle` as state/generation/admission authority; `RuntimeOwner`/`EffectScope`/loader disposer as provider/runtime cleanup authority; `ProjectionHost`/`ZahirFacade` as projection and notification only.
- Do not connect Feishu, R2, Terminal, Memory, Sandbox, or any concrete Gateway channel.
- Keep exact `LoadedWasmPlugin` receipt/drop evidence separate from ProviderState, generation, projection, broadcast notification, OS mappings, JIT, threads, and RSS claims.
- New documentation must be bilingual; new code comments must be English.
- Every Cargo command must be preceded by a separate `python qa/browser_live/check_memory.py` run that exits 0 and prints `ALLOW`.
- Do not repair the known early post-mount startup `?` drain residual in this stage; document it as a separate boundary.
- Tests must use real fixture behavior and existing instrumentation; do not replace implementation with a test-local duplicate.

## Review Focus

- **Authority identity:** the manager, canonical registry, host, facade, and ownership tree must be the same production-shaped composition; the test must not hide a second `OwnershipTree` in a helper.
- **Cold laziness:** discovered/Cold/Disabled observation must produce zero guest-byte reads, runtime creations, and instance creations while allowing manifest metadata reads.
- **Admission and stale rejection:** the host-facing call must pass through the current `ProviderHandle`; replacement must reject the old facade-facing lease/binding before guest A can run.
- **Exact drop ordering:** the old `LoadedWasmPlugin` receipt must remain retained while owner/binding/entry/lease references remain, then drop only after the existing stop/dispose path and reference release.
- **Failure and shutdown boundaries:** existing failed read/build/publication/dispose residuals and normal `ProjectionHost::close_and_await` ordering must remain visible; early startup errors must stay documented as a limitation rather than acquiring detached cleanup.

---

### Task 1: Add a shared-authority lifecycle fixture

**Files:**
- Modify: `src/extension/lifecycle.rs:1702-1730` (`isolated_manager` test helper)
- Modify: `src/extension/lifecycle.rs:2000-2120` (Stage F/B4 test helpers)
- Test: `src/extension/lifecycle.rs` new Stage G tests in the existing `#[cfg(test)]` module

**Interfaces:**
- Consumes: `ExtensionManager::new_with_ownership_tree(config: ExtensionConfig, tree: Arc<OwnershipTree>) -> ExtensionResult<Self>`; the existing `b3_cold_manager_with_registry` fixture; `ProjectionHost::mount(registry: ToolHandlerRegistry, tree: Arc<OwnershipTree>) -> Arc<ProjectionHost>`.
- Produces: `isolated_manager_with_ownership_tree(dir: &Path, tree: Arc<OwnershipTree>) -> (ExtensionManager, PathBuf)` and `b3_cold_manager_with_registry_and_ownership_tree(root: &Path, canonical: Arc<ToolHandlerRegistry>, tree: Arc<OwnershipTree>) -> (Arc<ExtensionManager>, Arc<ToolHandlerRegistry>, Arc<RuntimeOwner>)`; a `b4_canonical_service_with_facade(canonical: Arc<ToolHandlerRegistry>, facade: Arc<ZahirFacade>) -> ScopedToolService` helper for host-backed calls.

- [ ] **Step 1: Write the fixture RED.** Add a focused Stage G test skeleton that creates an `Arc<OwnershipTree>`, passes it to the new manager helper, mounts `ProjectionHost` with the same tree and canonical registry, and attempts to bind the host facade to the canonical route. The test must fail to compile until the shared-tree helper and host-facade service helper exist.

- [ ] **Step 2: Run the focused test to verify the RED.**

  Run:

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib stage_g_production_host_reuses_canonical_authorities -- --nocapture
  ```

  Expected: compilation failure naming the missing shared-authority helper or host-facade service helper; do not treat a zero-test result as evidence.

- [ ] **Step 3: Implement the shared fixture helpers.** Add `isolated_manager_with_ownership_tree` by moving the existing `ExtensionConfig` construction into the new helper and keeping `isolated_manager` as a compatibility wrapper that passes a fresh tree. Add `b3_cold_manager_with_registry_and_ownership_tree` using the shared-tree manager constructor, preserving the existing fixture manifest and `load_all` sequence. Do not change production constructors or make the test-only `b4_canonical_service` pretend to be production.

- [ ] **Step 4: Implement `b4_canonical_service_with_facade`.** Keep the existing canonical registry binding and publication entry logic, but bind the supplied `Arc<ZahirFacade>` returned by `ProjectionHost::facade()` instead of constructing `ZahirFacade::new` with a fresh test tree. Preserve the existing `ScopedToolService` and approval path.

- [ ] **Step 5: Run the focused helper test again.**

  Run the same memory-gated command from Step 2.

  Expected: compilation succeeds and the test reaches the first behavior assertion; if the test is still incomplete, it may fail on its not-yet-implemented readiness assertion, but it must no longer fail because the shared fixture is absent.

- [ ] **Step 6: Commit the self-contained fixture change.**

  ```bash
  git add src/extension/lifecycle.rs
  git commit -m "test(core): share ownership tree with stage G host fixture"
  ```

### Task 2: Prove cold, activation, replacement, and exact drop through the host

**Files:**
- Modify: `src/extension/lifecycle.rs` in the existing lifecycle test module near the Stage F/B4 tests
- Reference only: `src/capability/projection_host.rs:725-776` for `wait_until_ready` and `facade`, `src/extension/lifecycle.rs:2136-2300` for existing Stage F evidence, and `src/extension/lifecycle.rs:2640-2785` for the existing checked canonical reload path

**Interfaces:**
- Consumes: Task 1’s shared-tree fixture helpers; `ProjectionHost::wait_until_ready(&self) -> bool`; `ProjectionHost::facade(&self) -> Arc<ZahirFacade>`; `ZahirFacade::is_backed_by_registry`; `ExtensionManager::set_plugin_enabled`; `ExtensionManager::reload_plugin`; `RuntimeOwner::provider`; `PluginBatchRegistration`/`LoadedWasmPlugin` receipt probes; existing `b4_execute_canonical` approval route.
- Produces: two non-zero-test async tests named `stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation` and `stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner`.

- [ ] **Step 1: Write `stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation` as a RED.** Set up the shared tree, canonical registry, Cold manager, and `ProjectionHost::mount(canonical.as_ref().clone(), Arc::clone(&tree))`; await `host.wait_until_ready()` and obtain `host.facade()`. Assert `facade.is_backed_by_registry(canonical.as_ref())`, then assert the Cold provider and host-facing `describe`/`project` surfaces have no callable entries and the loader probe reports `guest_byte_reads() == 0`, `guest_instance_created() == 0`, and `guest_instance_dropped() == 0`. Enable via `b3_enable_cold_owner`, build the scoped service with the host facade, execute through `b4_execute_canonical`, and assert payload A, `ProviderState::Ready`, a positive provider generation matching the published entry, one guest read, one instance creation, and zero drops. Close the host through `Arc::clone(&host).close_and_await().await` after the lifecycle cleanup and assert the source/applier join outcome is successful.

- [ ] **Step 2: Run the new cold/activation test to verify the RED.**

  Run:

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation -- --nocapture
  ```

  Expected: FAIL if the host-backed service or shared-tree path is not yet fully wired; it must execute the real fixture rather than report zero matching tests.

- [ ] **Step 3: Implement the minimum cold/activation test wiring.** Use only the host facade for capability resolution and canonical dispatch. Keep the production enable path (`set_plugin_enabled`) rather than calling `activate_owner` directly. Keep manifest reads, guest-byte reads, instance creation, provider state/generation, publication, and exact receipt/drop counters as separate assertions.

- [ ] **Step 4: Run the cold/activation test to verify GREEN.**

  Run the same memory-gated command from Step 2.

  Expected: PASS with the real guest returning `"A"`, one guest instance created, no early drop, and successful host worker joins.

- [ ] **Step 5: Write `stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner` as a RED.** Use the same shared-tree production-shaped setup and host facade. Execute payload A through a service bound to the host facade, retain the old canonical entry, WASM binding, owner, host facade, and exact old receipt, and arrange the existing guest barrier if needed. Run `reload_plugin("lazy-wasm")`; assert the old generation/entry/binding/service fails closed before guest A can execute, the old receipt is not dropped while references remain, and the replacement reaches Cold. Release the old owner, entry, binding, and other retained references; assert the exact old receipt drops once. Enable the successor through `set_plugin_enabled`, create a new service from the same host facade and successor publication, assert payload B, distinct owner/receipt identity, and no old-generation admission. Close the host after successful unmount and assert both source and applier joined.

- [ ] **Step 6: Run the replacement test to verify the RED.**

  Run:

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner -- --nocapture
  ```

  Expected: FAIL until the host-backed service is used for the stale route and the retained-reference assertions are complete; a zero-test filter is invalid evidence.

- [ ] **Step 7: Implement the minimum replacement test wiring.** Reuse the existing replacement order and receipt instrumentation from `stage_f_provider_replacement_fences_old_generation_and_drops_exact_owner` and `b4_production_canonical_reload_rejects_admitted_a_before_guest_and_runs_b`, changing only the facade/source composition needed to prove the call is host-backed. Do not add a second cleanup path or infer instance drop from ProviderState/projection disappearance.

- [ ] **Step 8: Run the replacement test to verify GREEN.**

  Run the same memory-gated command from Step 6.

  Expected: PASS with stale host-facing calls rejected, payload B executed only through the successor, and exact old receipt drop observed only after retained references are released.

- [ ] **Step 9: Commit the behavior evidence.**

  ```bash
  git add src/extension/lifecycle.rs
  git commit -m "test(core): prove stage G host lifecycle reuse"
  ```

### Task 3: Record the readiness result and boundaries

**Files:**
- Modify: `docs/superpowers/specs/2026-10-29-gateway-stage-g-provider-mount-design.md` status and implementation outcome sections
- Create: `docs/superpowers/runs/2026-10-29-gateway-stage-g-provider-mount-run.md`
- Modify: `docs/reference/FEATURE_LOCATOR.md` sections `§3.5d`, `§3.5h`, `§3.5i`, and `§3.5j` as applicable to the existing capability/provider closure

**Interfaces:**
- Consumes: Task 2 test names and output; existing source-backed startup census; existing Stage E/F run evidence; reviewer result from Task 4.
- Produces: bilingual readiness record that distinguishes production host composition, lifecycle evidence, exact receipt/drop evidence, residual failure behavior, and deferred claims.

- [ ] **Step 1: Write the run-record skeleton before final verification.** Create the bilingual run record with sections for actual HEAD/branch, authority table, test matrix, command/memory-gate results, residual failures, proof limits, reviewer status, deferred integrations, and destructive/external actions. Mark test results as pending until verification completes.

- [ ] **Step 2: Update the approved design status.** Change the design status from awaiting approval to approved/implementation pending, then add the final implementation outcome only after Task 4 establishes GREEN or an explicit NO-GO. Preserve the early-error drain residual as a separate deferred decision.

- [ ] **Step 3: Update `FEATURE_LOCATOR.md` only with source-backed closure.** Record that provider-neutral production `ProjectionHost` mount already exists and is composed from the startup canonical registry/tree; record the new host-backed test evidence and exact proof limits. Keep Feishu/R2/Terminal/Memory/Sandbox, concrete channel integration, remaining capability kinds, EffectClaim producers, automatic replay, native unloading, and OS/JIT/thread/RSS claims deferred. Do not rewrite historical Stage F claims as if they were host-backed evidence.

- [ ] **Step 4: Run `git diff --check` and inspect the documentation diff.**

  Run:

  ```bash
  git diff --check
  git diff -- docs/superpowers/specs/2026-10-29-gateway-stage-g-provider-mount-design.md docs/superpowers/runs/2026-10-29-gateway-stage-g-provider-mount-run.md docs/reference/FEATURE_LOCATOR.md
  ```

  Expected: no whitespace errors, bilingual sections present, and no claim beyond the test/source evidence.

### Task 4: Run the verification matrix and independent review

**Files:**
- Reference: `src/extension/lifecycle.rs`, `src/capability/projection_host.rs`, `src/bin/aleph-server/commands/start/mod.rs`, and the complete Stage G diff
- Update: `docs/superpowers/runs/2026-10-29-gateway-stage-g-provider-mount-run.md` with actual results

**Interfaces:**
- Consumes: Tasks 1–3; real fixture probes; startup static order tests; existing Stage E/F/B3 tests.
- Produces: verified GREEN result or explicit NO-GO record, plus an independent read-only review of the complete diff.

- [ ] **Step 1: Run the two focused Stage G tests serially three times.** Before every individual Cargo invocation, run `python qa/browser_live/check_memory.py` and require exit 0 plus `ALLOW`. Run each test separately and repeat the pair three times:

  ```bash
  cargo test -p alephcore --lib stage_g_production_host_reuses_canonical_authorities_for_cold_and_activation -- --nocapture
  cargo test -p alephcore --lib stage_g_production_host_replacement_fences_stale_facade_and_drops_exact_owner -- --nocapture
  ```

  Expected: every invocation runs at least one test and passes; record each memory-gate result and duration.

- [ ] **Step 2: Run the focused existing lifecycle and projection regressions.** Memory-gate each command separately, then run:

  ```bash
  cargo test -p alephcore --lib stage_f_cold_provider_surfaces_do_not_read_or_create_guest -- --nocapture
  cargo test -p alephcore --lib stage_f_provider_activation_executes_real_guest_through_current_generation -- --nocapture
  cargo test -p alephcore --lib stage_f_provider_replacement_fences_old_generation_and_drops_exact_owner -- --nocapture
  cargo test -p alephcore --lib b3_disposer_failure_keeps_structured_failed_owner_and_never_retries -- --nocapture
  cargo test -p alephcore --lib capability::projection_host::tests -- --nocapture
  ```

  Expected: all commands execute non-zero tests and pass; failure residuals remain structured and no test claims native unload.

- [ ] **Step 3: Run startup ownership/order regressions.** Memory-gate each command separately, then run the exact startup tests in package `aleph-server`:

  ```bash
  cargo test -p aleph-server --bin aleph-server boot_mounts_projection_host_from_canonical_registry_and_awaits_readiness -- --nocapture
  cargo test -p aleph-server --bin aleph-server boot_closes_projection_host_before_registry_scope_dispose -- --nocapture
  ```

  Expected: both source-order tests execute and pass, proving mount follows `set_mcp_tool_registry` and normal close precedes registry-scope disposal.

- [ ] **Step 4: Run canonical repository checks.** Memory-gate each Cargo command separately. Run the relevant WASM/runtime tests, then:

  ```bash
  cargo check -p alephcore --tests
  cargo clippy -p alephcore --tests -- -D warnings
  cargo fmt --all --check
  git diff --check
  ```

  Run from the repository root only for `cargo check`/`clippy`/`fmt` as permitted by the project instructions; run tests from their package context. Expected: no type, lint, format, or whitespace failures. Do not report a command as passed if it was skipped by the memory gate or selected zero tests.

- [ ] **Step 5: Submit the complete diff to an independent read-only reviewer.** Ask the reviewer to check single-authority reuse, shared tree/registry identity, host shutdown ordering, stale replacement and exact Arc release, failure residuals, documentation claims, and absence of channels/new runtimes/dependencies. If the reviewer service is unavailable, preserve the full error and perform the checklist manually; do not claim independent approval.

- [ ] **Step 6: Resolve review findings and rerun affected checks.** Any Critical/Important finding creates a focused fix and reruns the relevant memory-gated test plus `git diff --check`; do not batch unrelated fixes. If a finding requires a new owner, lifecycle, public plugin API, or early-error cleanup authority, stop and record NO-GO instead of expanding scope.

- [ ] **Step 7: Complete the run record and final self-review.** Record actual commands, test counts, memory-gate outputs, reviewer availability, residual failures, exact proof limits, deferred providers/kinds, and unperformed destructive/external actions. Confirm the final diff contains no production mount or second authority.

- [ ] **Step 8: Commit the verified documentation and evidence.**

  ```bash
  git add docs/superpowers/specs/2026-10-29-gateway-stage-g-provider-mount-design.md docs/superpowers/runs/2026-10-29-gateway-stage-g-provider-mount-run.md docs/reference/FEATURE_LOCATOR.md
  git commit -m "docs(capability): record stage G provider mount readiness"
  ```

## Self-Review

- **Spec coverage:** Task 1 pins the same-tree fixture; Task 2 covers cold observation, canonical host composition, activation, provider admission, replacement, stale rejection, exact receipt drop, and normal host shutdown; Task 3 records residuals, proof limits, deferred integrations, and design status; Task 4 covers memory-gated focused runs, regressions, static startup order, repository checks, and independent review. The early-error drain is intentionally documented as deferred in Tasks 3–4 rather than changed.
- **Step scan:** Each implementation step has one test/helper/documentation action and an explicit check. The plan does not add a production API or prescribe a new lifecycle body.
- **Type consistency:** The shared fixture uses the existing `ExtensionManager::new_with_ownership_tree`, `ProjectionHost::mount`, `ProjectionHost::wait_until_ready`, `ProjectionHost::facade`, and existing `ScopedToolService`; no invented production interface is required.
- **Review focus:** Every listed failure mode is owned by Task 2 or Task 4, and existing Stage F/B3 tests remain part of the regression matrix instead of being silently replaced.
- **Proportion:** The plan adds two host-backed tests, one test-fixture seam, bounded documentation, and verification. It does not reproduce implementation bodies or create a second host design.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-10-29-gateway-stage-g-provider-mount-implementation-plan.md`. Please review the plan. Which execution approach would you prefer?

- **Subagent-driven** — a fresh subagent implements each task and a fresh reviewer checks it before the next one starts, followed by a whole-branch review. Most thorough; it adds context and coordination cost.
- **Native** — I implement every task in this session, then use one fresh read-only reviewer on the complete branch. Cheapest and fastest; the tasks are tightly coupled test/documentation changes and the plan already carries the interfaces.

For this plan I recommend **Native**, because the two behavior tests must share one fixture and host lifecycle, while the final result still requires an independent whole-diff review. Does the plan capture what you want, and which execution approach should we use?
