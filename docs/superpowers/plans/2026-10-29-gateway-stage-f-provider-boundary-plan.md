# Stage F Provider Boundary Composition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove, through the existing `ExtensionManager`/`RuntimeOwner`/`ProviderHandle` lifecycle, that a real provider-facing WASM activation owns and admits the same exact Extism instance whose replacement and disposal produce the Stage E drop receipt.

**Architecture:** Add bounded behavior tests around the existing real plugin composition. Discovery and projection remain lazy; activation enters the existing `ProviderHandle` generation/admission path, publishes the existing canonical handler, and calls the real guest. Stop and replacement continue through existing revoke/drain/join/dispose authority. Production Zahir mount, channel integration, and any second lifecycle authority remain out of scope.

**Tech Stack:** Rust, Tokio, Extism WASM fixture, existing `ProviderHandle`/`ProviderLifecycle`, `RuntimeOwner`, `ExtensionManager`, `PluginLoader`, `WasmBinding`, `WasmMountIdentity`, canonical `ToolHandlerRegistry`, Cargo/Bun project checks.

**Spec:** `docs/superpowers/specs/2026-10-29-gateway-stage-f-provider-boundary-design.md`

## Global Constraints

- Base this work on the actual clean `main` HEAD `04fba2229`, on branch `stage-f-provider-boundary`; do not copy historical hashes into evidence.
- Keep enabled/disabled, discovery, mount/unmount, provider state/generation/admission, runtime ownership, and cleanup on their existing authorities.
- Do not add `unsafe`, `dlopen`/`dlclose`, `libloading`, Wasmtime, a new runtime/dependency, a global engine/cache, a second registry, a second owner/generation map, detached cleanup, production Zahir mount, or provider/channel integration.
- Reuse the Stage E loader probe and exact `LoadedWasmPlugin` receipt; do not equate ProviderState, generation, notification, projection, OS mapping, or RSS with exact instance drop.
- Production comments remain English; new documentation is bilingual.
- Run `python qa/browser_live/check_memory.py` immediately before every Cargo command; require exit 0 and `ALLOW`.
- Focused provider-boundary tests run serially three times. A zero-test filter is zero evidence and must be reported as such.

## Review Focus

- Cold registry/capability reads must not construct the lazy WASM runtime or read guest bytes — test `ProviderState::Cold`, `PluginStatus::Cold`/disabled view, empty canonical descriptors/projection, and zero loader receipts in Task 1.
- The guest call must pass through the published canonical handler and current provider generation — test payload A and current-generation admission in Task 1.
- A retained old canonical entry must fail after stop/replacement even if the caller still owns handler/binding/identity `Arc`s — test stale handler rejection and successor payload B in Task 1.
- Exact instance drop must wait for every retained owner/binding/identity holder and occur once — test release ordering and receipt identity in Task 1.
- Failed read/build/publication must retain real residual/error evidence and never report clean Ready/create/drop — test each failure phase in Task 1.

---

### Task 1: Add provider-boundary RED contracts in the existing lifecycle composition

**Files:**
- Modify: `src/extension/lifecycle.rs` in the existing Stage E/B3 test module near `b3_call_canonical`, `b3_cold_manager`, and the real guest lifecycle tests.
- Reuse: `src/extension/runtime/wasm/tests/guest_fixture.rs`, `src/extension/loader.rs`, `src/extension/canonical_handler.rs`, `src/tools/registry.rs`, and `src/capability/zahir_facade.rs`; modify these only if the test cannot access an already-existing fact without a new authority.
- Test evidence: `.superpowers/sdd/2026-10-29-gateway-stage-f-provider-boundary/`.

**Interfaces:**
- Consumes: `b3_cold_manager_with_registry`, `b3_enable_cold_owner`, `b3_call_canonical`, `b4_canonical_service`/`b4_execute_canonical` where appropriate, `ProviderHandle::snapshot`, `ProviderHandle::acquire_call`, `CanonicalPublication`, `PluginLoader::wasm_load_probe`, `wasm_instance_receipt`, `WasmBinding`, and the existing `RuntimeOwner` fields.
- Produces: named Stage F tests that exercise the real provider composition and establish the exact evidence matrix for implementation and final run documentation. No new production lifecycle authority.

- [ ] **Step 1: Add the Cold/discovered/disabled zero-effect test.**

  Add a test named `stage_f_cold_provider_surfaces_do_not_read_or_create_guest` (or an equally explicit name) that builds the real fixture through discovery/load, before activation:

  - assert the registry row is the discovered/disabled or declared `PluginStatus::Cold` representation appropriate to the path;
  - assert the owner is present only as the existing Cold declaration and its `ProviderHandle::snapshot()` is `(ProviderState::Cold, 0)`;
  - read the existing plugin list/status, canonical descriptor/publication, and capability facade `describe`/`project` surfaces that are already available in this test composition;
  - assert no guest-byte reads, no instance creation, no instance drops, and `!PluginLoader::is_wasm_runtime_active()`;
  - assert no callable canonical entry is visible before Ready publication.

- [ ] **Step 2: Add the activation/current-generation call test.**

  Add a test named `stage_f_provider_activation_executes_real_guest_through_current_generation` that uses `set_plugin_enabled` or `ensure_loaded` as the production activation path, not direct `activate_owner`:

  - assert the same declared owner reaches `ProviderState::Ready` with a nonzero current generation;
  - assert exactly one guest-byte read and one instance create, with no drop while the owner and published entry remain retained;
  - invoke the real published canonical handler through the existing test call route and assert payload A returns the real guest value;
  - assert the publication descriptor/handler carries the current provider generation and that current-generation admission succeeds;
  - assert no independent test counter or map decides whether the call is admitted.

- [ ] **Step 3: Extend the test to stop, stale admission, replacement, and exact drop ordering.**

  Retain the old canonical entry/handler long enough to exercise the existing stop path. Assert:

  - stop withdraws/revokes before drain and leaves the old `ProviderHandle`/binding unable to admit a call;
  - after replacement activation, the old generation, old handler, old binding, and old mount identity reject or fail closed;
  - the replacement receives a new generation and executes payload B through the new canonical handler;
  - the old exact receipt is not dropped while any retained owner, handler, binding, or identity `Arc` remains;
  - after those retained references and the old owner are released, the old receipt is dropped exactly once and before the successor receipt is allowed to drop;
  - the successor remains current and cannot be removed by the stale disposer.

- [ ] **Step 4: Add failure-phase assertions.**

  Cover the existing real failed-read, failed-build, publication-collision, and disposer-failure fixture paths separately. For each phase assert the actual error/residual semantics, including any `ProviderState::Failed` and `RuntimeOwner` failure record, and assert the precise receipt boundary:

  - failed byte read/build may record a guest-byte read but records no successful instance create or drop;
  - failed publication preserves the incumbent canonical publication/revision, leaves the loser owner Failed with the existing publication failure record, and does not commit a replacement entry;
  - failed disposal preserves the current Failed owner residual, closed admission, withdrawn canonical entry, retained receipt, and exact disposer failure record;
  - failed activation does not report a clean Ready owner or committed canonical entry.

  Keep stale-call rejection in the successful replacement test; do not invent an old canonical handler for a failed activation that never published one.

- [ ] **Step 5: Run the new filter once as RED.**

  Run immediately after a separate memory gate:

  ```bash
  python qa/browser_live/check_memory.py
  cargo test -p alephcore --lib extension::lifecycle::tests::stage_f_ -- --nocapture
  ```

  Expected: the new tests compile and expose the missing provider-boundary assertion or seam; if the current implementation already satisfies a contract, record that fact and keep the test as a regression contract rather than adding artificial production changes. Save the complete output under `.superpowers/sdd/2026-10-29-gateway-stage-f-provider-boundary/red-provider.log`.

### Task 2: Implement only the smallest composition needed by the RED contracts

**Files:**
- Modify only the exact existing lifecycle/test integration file identified by Task 1’s failure, normally `src/extension/lifecycle.rs`.
- Modify `src/extension/canonical_handler.rs`, `src/extension/loader.rs`, `src/extension/runtime/wasm/mod.rs`, or `src/tools/registry.rs` only if an existing real receipt/binding/publication fact is inaccessible and the smallest change preserves the current authority.
- Do not create a new registry, generation map, owner map, cleanup task, public plugin lifecycle API, or production Zahir mount.

**Interfaces:**
- Consumes: Task 1’s failing assertions and existing signatures: `ExtensionManager::set_plugin_enabled`, `ExtensionManager::ensure_loaded`, `RuntimeOwner::activate_owner`, `ExtensionManager::retained_owner_stop` through `unmount`, `ProviderHandle::transition`/`acquire_call`, `PluginLoader::bind_wasm`/`call_bound`, and the canonical publication API.
- Produces: passing Stage F tests with every admission decision delegated to the existing provider/canonical handler and every instance receipt tied to a real `LoadedWasmPlugin` incarnation.

- [ ] **Step 1: Fix the smallest missing access or composition point.**

  Prefer existing test-visible methods and current canonical call paths. If a seam is unavoidable, make it `#[cfg(test)]`, borrow or delegate to the existing `RuntimeOwner`/`ProviderHandle`/loader objects, and ensure it cannot independently transition state, allocate a generation, publish a capability, or dispose an instance.

- [ ] **Step 2: Preserve lifecycle ordering and failure residuals.**

  Ensure activation remains `Cold → Starting → Ready/Failed`, calls require Ready/current generation, stop closes admission before revoke/drain/join/dispose, and replacement cannot be removed by an old disposer. Keep failed owners and exact dispose errors visible through the current residual semantics.

- [ ] **Step 3: Run the focused provider-boundary filter and fix only implementation failures.**

  Before the Cargo command, run `python qa/browser_live/check_memory.py`; then run:

  ```bash
  cargo test -p alephcore --lib extension::lifecycle::tests::stage_f_ -- --nocapture
  ```

  Expected: all Stage F tests pass. Repeat this same focused filter two more times serially, with a separate `ALLOW` gate before each Cargo command, and save all three logs.

- [ ] **Step 4: Check the diff for prohibited authority.**

  Run `git diff --check` and inspect the complete diff. Confirm no `unsafe`, new dependency/runtime, production Zahir mount, second owner/generation/cleanup authority, or ProviderState-as-instance-drop assertion was introduced.

### Task 3: Run regression verification and document the bounded evidence

**Files:**
- Create after GREEN/review: `docs/superpowers/runs/2026-10-29-gateway-stage-f-provider-boundary-run.md`.
- Modify after GREEN/review: `docs/superpowers/plans/2026-10-29-gateway-stage-f-provider-boundary-plan.md` if execution details changed; `docs/reference/FEATURE_LOCATOR.md` only if the bounded contract is actually proved; `docs/reference/PLUGIN_SYSTEM.md` or `docs/reference/GATEWAY.md` only if the documented boundary needs correction.
- Logs: `.superpowers/sdd/2026-10-29-gateway-stage-f-provider-boundary/`.

**Interfaces:**
- Consumes: passing Stage F tests, baseline logs, full diff, and independent reviewer result.
- Produces: bilingual run record with exact commands and evidence classes, explicit remaining limits, and no production integration claim.

- [ ] **Step 1: Run the required narrow regression matrix.**

  Run a separate memory gate before every Cargo command and save each output:

  ```bash
  cargo test -p alephcore --lib extension::runtime::wasm:: -- --nocapture
  cargo test -p alephcore --lib extension::lifecycle::tests::stage_e_ -- --nocapture
  cargo test -p alephcore --lib extension::lifecycle::tests::b3_ -- --nocapture
  cargo test -p alephcore --lib capability:: -- --nocapture
  cargo test -p alephcore --lib gateway::channel_registry::resource_matrix:: -- --nocapture
  cargo test -p alephcore --lib gateway::channel_health_monitor::tests:: -- --nocapture
  cargo test -p alephcore --lib tools::registry::tests:: -- --nocapture
  cargo check -p alephcore --tests
  cargo clippy -p alephcore --tests -- -D warnings
  cargo fmt --all --check
  ```

  Report any filter that runs zero tests as zero evidence and run a corrected filter if the actual module path differs.

- [ ] **Step 2: Dispatch an independent read-only review after logs and diff exist.**

  Ask the reviewer to inspect the complete diff and exact test logs for: single-source provider state/generation/admission, no second lifecycle authority, retained `Arc` release ordering, failed activation residuals, stale replacement fencing, and the distinction between exact Extism drop versus OS mapping/JIT/thread/RSS behavior.

- [ ] **Step 3: Resolve Critical/Important findings and repeat review.**

  Do not close the stage with unresolved Critical/Important findings. If the review tool is unavailable, save the complete error and perform the same checklist manually without claiming independent approval.

- [ ] **Step 4: Write the bilingual run record only after GREEN.**

  Record actual HEAD/branch, baseline counts, each memory gate result, all command results, exact manifest/guest-byte/create/drop/state/generation/mapping/RSS evidence, retained Arc release requirements, failed residuals, reviewer availability/verdict, deferred provider/kind/production integrations, and destructive/external actions not performed. State explicitly that no production channel and no Zahir mount were connected.

- [ ] **Step 5: Update reference docs only when the evidence supports it.**

  Add a new `FEATURE_LOCATOR.md` entry only for the bounded provider-boundary contract actually demonstrated. Keep native compiled-code unloading, engine/JIT, OS executable mapping, thread, RSS, universal provider rollout, and production mount explicitly deferred.

- [ ] **Step 6: Commit the completed work.**

  Use conventional commit messages, keep code/tests and documentation logically reviewable, and do not merge, push, clean worktrees, or begin another stage without a new explicit instruction.

## Acceptance criteria

Stage F is GREEN only when all of the following hold:

- Disabled/discovered/Cold list, descriptor, and projection observations perform zero guest-byte reads, zero instance creation, and no WASM runtime construction.
- Activation through existing enable/ensure-loaded reaches Ready with a current provider generation and executes payload A through the real published canonical handler.
- Current-generation admission succeeds only while the provider is Ready; old generation, Stopping, Disabled/Stopped, Failed, stale binding, and stale identity calls fail closed.
- Replacement executes payload B through the successor and cannot be undone by the old disposer.
- Exact old and new `LoadedWasmPlugin` receipts are observed with correct release ordering and no double drop.
- Failed read/build/publication preserves real residual/error state and never fabricates clean create/drop/Ready evidence.
- No second lifecycle authority or production mount was introduced.
- Required memory gates, three serial focused runs, narrow regressions, static checks, diff check, and independent review are complete and GREEN.
- Documentation names what OS mapping, engine/JIT, threads, RSS, provider kinds, and production integrations remain unproven or deferred.
