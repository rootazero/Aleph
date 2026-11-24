# Stage H Provider-Neutral Lazy Activation Implementation Plan

- **Goal / 目标:** Add a cold declaration-discovery boundary for ExtensionManager-managed tool-only WASM extensions, so installed-plugin reads expose rows and `RuntimeOwner(Cold)` without reading guest bytes, creating a guest instance, starting a provider, opening provider resources, or publishing callable tools. Preserve explicit activation, generation fencing, receipts, admission, disposal, residuals, and all existing non-tool lifecycle behavior.
- **Architecture / 架构:** Keep `ExtensionManager` as the lifecycle authority; keep `RuntimeOwner` as the per-incarnation activation owner; keep `ProviderHandle`/`ProviderLifecycle` as the provider state and admission authority; keep `ToolHandlerRegistry` as the canonical callable publication source; keep `OwnershipTree`, `ProjectionHost`, and `ZahirFacade` unchanged as ownership/projection authorities. Add only a derived discovery cache bit and an internal read-only declaration mode under the existing lifecycle guards.
- **Tech Stack / 技术栈:** Rust, Tokio, existing `ExtensionManager`, real WASM fixture, existing `ToolHandlerRegistry`/`ProjectionHost` fixtures, repository test helpers. No new dependency, native loader, `unsafe`, or new lifecycle registry.
- **Spec / 规范:** [Stage H Provider-Neutral Lazy Activation Design](../specs/2026-10-29-stage-h-provider-lazy-activation-design.md). The bounded proof covers tool-only WASM. Channel, PTY, Memory Graph, Sandbox, MCP, Feishu, and other provider kinds remain deferred.
- **Global Constraints / 全局约束:**
  - Do not add a second catalog, coordinator, owner map, generation map, runtime table, cleanup authority, or provider registry.
  - Do not make `ToolHandlerRegistry::new_with_provider` govern the mixed production registry. Each plugin publication continues to carry its own actual producer `ProviderHandle`.
  - `ensure_discovered` must not call `unmount_all`, clear `runtime_owners`, call `mount_parsed`, read guest bytes, create a guest instance, open provider resources, or publish callable tools.
  - Full `load_all`, `reload`, startup catalog construction, and agent execution retain their existing explicit activation behavior, including non-tool provider mounts.
  - Every Cargo command is immediately preceded by `python qa/browser_live/check_memory.py`; the gate must exit 0 and report `ALLOW`.
  - Focused Stage H tests run serially, with a non-zero test count, three times. Tests must use the real fixture and existing probes, not mocks that duplicate lifecycle logic.
  - Documentation is bilingual where it describes the lifecycle contract; code comments are English. No commits, pushes, generated artifacts, or unrelated cleanup are part of this plan.
- **Review Focus / 审查重点:**
  1. Cold read really reaches declaration rows plus `RuntimeOwner(Cold)` and has zero guest/provider/publication effects.
  2. Full-load and explicit enable paths still activate the current owner exactly once and retain current generation/receipt/admission/disposer behavior.
  3. Read callers are split by action, with no hidden activation through `plugins.list`, plugin management reads, or Hub reconciliation.
  4. Reload, replacement, stale leases, failure residuals, and non-tool provider behavior remain owned by the existing paths.
  5. Documentation states the actual production boundary and does not claim universal resource or cleanup proofs.

## File map / 文件地图

Read and modify only the files below unless implementation evidence requires a narrowly scoped adjacent test file:

| File | Role in the change |
| --- | --- |
| `src/extension/mod.rs` | Add the public `ensure_discovered` entry point and the derived discovery-cache state; preserve `ensure_loaded` guard and activation ordering. |
| `src/extension/lifecycle.rs` | Factor the existing scan/parse/admit/row declaration loop into a read-only mode and retain full-load mounting in the existing mode; add real WASM lifecycle tests. |
| `src/gateway/handlers/plugins/handlers/manage.rs` | Route `plugins.list` through declaration discovery; retain explicit enable/disable loading before control. |
| `src/builtin_tools/plugin_manage.rs` | Replace the unconditional load with action-specific discovery/activation gates. |
| `src/hub/reconcile.rs` | Make installed-extension reconciliation use declaration discovery while retaining best-effort MCP/skill behavior. |
| `docs/reference/ARCHITECTURE.md` | Correct the stale read-side and lifecycle statements around the B6 boundary. |
| `docs/reference/PLUGIN_SYSTEM.md` | Correct the stale claim that production has no provider transition path and document the bounded cold-read path. |
| `docs/reference/FEATURE_LOCATOR.md` | Update the Stage H locator entry and evidence limits at the existing provider-transition section. |

Preserve these activation boundaries without editing their behavior: `src/gateway/execution_engine/run_loop/mod.rs:636-637`, `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:161-162`, and `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:677`. Do not edit `src/capability/zahir_facade.rs` or `src/capability/projection_host.rs`; their existing read-only behavior is an acceptance condition.

## Implementation sequence / 实施顺序

### 1. Add the cold declaration boundary in ExtensionManager

**Purpose:** Make a declaration-only pass reusable by read callers without changing the existing full-load authority.

**Test first:** Extend `src/extension/lifecycle.rs` under the existing `#[cfg(test)] mod tests` with the real fixture test:

```rust
#[tokio::test]
async fn ensure_discovered_declares_tool_only_wasm_without_mounting_a_vm()
```

Arrange an isolated manager with `write_cold_tool_wasm_fixture`, attach the canonical `ToolHandlerRegistry`, and call the new public method. Assert all of the following:

- `runtime_owner("lazy-wasm")` exists and is the same declared owner observed by subsequent reads.
- provider state is `ProviderState::Cold`.
- `owner.identity` is `None`.
- the fixture loader/guest barrier reports no load or guest invocation.
- canonical registry revision is unchanged and the plugin tool is absent from `describe`/capture.
- no provider `Starting`/`Ready`, instance creation, resource-open receipt, or publication receipt exists.

The test must fail against the current unconditional `load_all` behavior, then pass after the implementation.

**Implementation:**

- Extend the existing private `CacheState` in `src/extension/mod.rs` with a derived `discovered: bool` field. Document that it is only a cache of completed declaration work; it is not durable enabled state, provider state, a generation map, an owner table, or cleanup authority.
- Add the exact public signature:

```rust
pub async fn ensure_discovered(&self) -> ExtensionResult<()>
```

It must use `boot_guard` and `load_guard` in the existing order, fast-path only on `cache_state.discovered`, publish derived views after a successful declaration pass, and set `discovered` only after the pass and view publication succeed. It must not set `loaded`.
- In `src/extension/lifecycle.rs`, retain `ScanPlan` and `scan_plugins_blocking` as the owned scan result. Factor the current `discover_and_mount` loop into a private mode-aware declaration helper, for example:

```rust
#[derive(Debug, Clone, Copy)]
enum DiscoveryMode {
    DeclarationsOnly,
    FullLoad,
}

async fn reconcile_scan(
    &self,
    plan: ScanPlan,
    mode: DiscoveryMode,
) -> ExtensionResult<LoadOutcome>
```

The enum is a local strategy for the existing lifecycle method, not a new authority. In `DeclarationsOnly`, preserve scan order, shadow diagnostics, parse-error rows, trust/disabled/not-enabled admission rows, and `declare_cold` for eligible tool-only WASM. Do not migrate legacy markers, mount non-tool providers, unmount current owners, clear `runtime_owners`, clear/rebuild the canonical callable registry, or invoke guest code. In `FullLoad`, preserve the current `unmount_all`, owner clearing, settings publication, legacy marker migration, non-tool `mount_parsed`, and existing `LoadOutcome`/summary semantics.
- Keep `load_all`, `reload`, and `load_all_locked_inner` on `FullLoad`. Mark the derived discovery cache as true only after the full pass has successfully published its views; invalidate it at the existing reload/teardown boundary before a new scan.
- Keep `ensure_loaded`’s current boot-guard/load-guard/outside-lock activation/proof ordering and its non-tool full-load behavior. It may reuse the declaration helper internally, but it must still activate desired-enabled Cold tool owners through `activation_manager.activate_owner`, run `owner_activation_proof`, and publish `loaded` only after activation settles.
- Do not change `src/extension/runtime_owner.rs`, `src/capability/provider.rs`, or registry generation/receipt/admission/disposer code. The new method calls the existing paths; it does not duplicate them.

**Validation:** Run the focused lifecycle test once during iteration with the memory gate immediately before Cargo. Expected result: one non-zero matching test, with Cold owner and zero guest/provider/publication effects.

**Checkpoint:** Proposed commit `refactor(extension): split declaration discovery from activation` after the test and focused type check pass.

### 2. Route production read callers by semantic action

**Purpose:** Remove hidden activation from read APIs while keeping explicit control and run boundaries explicit.

**Test first:** Add or extend real-manager tests in `src/extension/lifecycle.rs` (using `PluginManageTool::call_on` and the existing isolated fixture) with these names:

```rust
#[tokio::test]
async fn plugin_manage_list_uses_cold_discovery_without_activation()

#[tokio::test]
async fn plugin_manage_show_and_context_status_use_cold_discovery_without_activation()
```

Each test must assert the returned operation still contains the expected row/data and the same zero-effect cold assertions from Task 1. Add a control-path regression test if the existing lifecycle test does not already cover it:

```rust
#[tokio::test]
async fn plugin_manage_enable_still_uses_explicit_activation()
```

It must assert `Ready`, one publication, the real guest result, and the existing owner identity/receipt behavior. Reuse `tool_only_wasm_production_enable_activates_the_cold_declaration` where it already proves this, extending it only if the caller gate itself is untested.

**Implementation:**

- In `src/gateway/handlers/plugins/handlers/manage.rs:24-66`, replace the list handler’s preflight with `manager.ensure_discovered().await`; preserve warning text, usage join, response type, and row semantics. At `:194-212`, keep the explicit control preflight and `set_plugin_enabled` path so enable/disable remain lifecycle operations.
- In `src/builtin_tools/plugin_manage.rs:207-218`, replace the unconditional preflight with a narrow action dispatch. `List`, `Show`, `ContextStatus`, `ConfigGet`, and `TrustStatus` call `ensure_discovered` and continue into their existing match arms. `Enable`, `Disable`, and `Reload` retain explicit lifecycle behavior. `ConfigSet`, trust mutations, and marketplace actions must not acquire activation accidentally; only call discovery where the existing branch needs installed plugin rows. Preserve `may_enable`’s discovered-row requirement and all existing error/warning behavior.
- In `src/hub/reconcile.rs:67-87`, replace only the extension-manager call with `ensure_discovered`; preserve best-effort warnings and MCP/skill reconciliation. Do not change the catalog callers in `src/builtin_tools/hub/catalog_search.rs:151` or `src/gateway/handlers/extensions/catalog.rs:48,125` unless compilation or the acceptance test proves an interface adjustment is required.
- Leave `src/gateway/execution_engine/run_loop/mod.rs:636-637` and both startup `ensure_loaded` calls unchanged as explicit activation boundaries.
- Add short English comments only where the read/control distinction is non-obvious; do not expose implementation details in the product response.

**Validation:** Run the two plugin-management tests and the existing hub/catalog-related focused tests serially, each command preceded immediately by the memory gate. Expected result: read actions remain Cold; enable/run/startup paths still activate.

**Checkpoint:** Proposed commit `fix(extension): keep plugin reads cold` after focused tests and `bun`/Rust package checks appropriate to the changed files pass. Do not run tests from repository root.

### 3. Expand Stage H real WASM evidence and regression coverage

**Purpose:** Prove the lifecycle boundary and protect the existing owner semantics across concurrency, reload, stop, replacement, and failure.

**Test first / exact acceptance cases:** Extend `src/extension/lifecycle.rs` rather than adding mocks or a second fixture. Keep the existing tests and add only missing cases:

```rust
#[tokio::test]
async fn concurrent_ensure_discovered_yields_one_declaration_and_no_publication()

#[tokio::test]
async fn ensure_discovered_then_ensure_loaded_yields_one_owner_and_one_publication()

#[tokio::test]
async fn reload_invalidates_discovery_cache_and_preserves_teardown_semantics()
```

Required assertions:

- Concurrent `ensure_discovered` calls share one declaration result, one `RuntimeOwner` `Arc`, no guest read/instance/provider start, no callable publication, and no registry revision change.
- Calling `ensure_discovered` followed by `ensure_loaded` reaches `Ready`, performs exactly one actual guest/publication path, stores one exact receipt/scope, and leaves the existing `real_wasm_boot_concurrent_ensure_loaded_yields_one_owner_and_one_publication` proof intact.
- Reload clears only the derived discovery cache before the existing teardown/re-discovery sequence; old handles remain fenced, the successor owner is not `Arc::ptr_eq` to the old owner, and the current successor generation/publication assertions remain true.
- Existing failure tests for missing/invalid bytes, publication collision, and disposer residuals remain unchanged and are explicitly included in the focused validation list. They must continue to prove closed admission and retained residuals rather than clean Ready/create/drop claims.
- Existing `ProjectionHost`/`ZahirFacade` behavior remains read-only. If a direct regression is needed, use the existing Stage G shared fixture and assert that describe/resolve/project/subscribe after cold discovery do not activate a provider.

**Validation sequence:**

1. Run the focused Stage H test filter serially three times with a non-zero filter. Use the exact module path confirmed by the implementation, based on `extension::lifecycle::tests::<name>`.
2. Run the affected package type check, `cargo check -p alephcore --tests`, and the focused lifecycle/plugin/capability/gateway suites as justified by the final diff.
3. Run `cargo clippy -p alephcore --tests -- -D warnings`, `cargo fmt --all --check`, and `git diff --check`.
4. Precede every Cargo command in steps 1–3 immediately with `python qa/browser_live/check_memory.py`; record exit 0 and `ALLOW` for each gate.
5. Run the independent read-only review over the complete diff. Resolve every finding before marking the implementation task complete.

**Checkpoint:** Proposed commit `test(extension): prove Stage H cold reads and activation` after all focused evidence is green.

### 4. Correct the bilingual lifecycle references

**Purpose:** Make the documented production contract match the implemented boundary and preserve the evidence limits.

**Implementation:**

- In `docs/reference/ARCHITECTURE.md:159-171`, state that read-side plugin listing/reconciliation uses declaration discovery and does not activate the bounded tool-only WASM provider; state separately that startup catalog, explicit enable, and agent execution remain activation boundaries. Remove any absolute claim that every read path is cold if a path intentionally remains an activation boundary.
- In `docs/reference/PLUGIN_SYSTEM.md:17-31`, replace the stale “no production provider transition” claim with the actual existing `RuntimeOwner`/`ProviderHandle`/canonical per-plugin publication path and the new read/activation split. Clarify that the registry-wide builtin compatibility provider is not replaced by a plugin provider.
- In `docs/reference/FEATURE_LOCATOR.md:6077+`, update the Stage H locator with the implementation files, real fixture evidence, exact cold/Ready/stop/replacement claims, and deferred provider kinds. Keep the evidence boundary explicit: no claim about OS mappings, engine/JIT reclamation, RSS, thread shutdown, process isolation, or universal provider behavior.
- Keep the design source link and bilingual wording consistent. Do not add absolute machine paths to repository documentation.

**Validation:** `git diff --check`; review each changed reference against the final behavior diff and the approved design. No Cargo command is needed for documentation-only changes.

**Checkpoint:** Proposed commit `docs: document Stage H cold discovery boundaries`.

### 5. Final integration and handoff evidence

Before declaring the implementation complete, inspect the full diff against the approved design and verify:

- only the mapped production, test, and reference files changed;
- no second lifecycle authority, provider map, cleanup table, or registry provider replacement was introduced;
- no read caller starts a guest runtime in the bounded cold path;
- `ensure_loaded`, explicit enable, startup, and run paths preserve their previous activation semantics;
- stale captures fail closed after stop/replacement/restart;
- failure residuals remain observable and no test overclaims resource reclamation;
- all memory gates, focused three-run tests, package checks, formatting, diff checks, and independent review results are recorded with actual outcomes.

The final report must state ownership, cold/activation/read/stop/restart behavior, generation/admission and retained-reference behavior, residuals, unproven resources, deferred providers, the final GO/NO-GO conclusion, exact commands/gates/reviewer, and skipped destructive/external actions. It must explicitly list anything not done.

## Self-review / 计划自审

- **Spec coverage:** The plan covers the approved declaration/activation split, authority preservation, read callers, explicit activation, failure/stop/replacement/restart evidence, deferred provider kinds, documentation drift, and final reporting requirements.
- **Step granularity:** Each step has a single purpose, exact files, test-first cases, implementation boundaries, validation, and a checkpoint. The lifecycle refactor is kept separate from caller routing and documentation.
- **Type/signature consistency:** The public contract is the exact `pub async fn ensure_discovered(&self) -> ExtensionResult<()>`; the existing `ScanPlan`, `LoadOutcome`, `RuntimeOwner`, and `ProviderHandle` types remain the authority. The private mode helper is deliberately local and carries no durable state.
- **Review-focus coverage:** Cold effects, activation ordering, action-specific routing, owner/generation fencing, residuals, non-tool preservation, and reference accuracy each have explicit tests or review checks.
- **Proportion:** No new abstraction is proposed for ProjectionHost, ZahirFacade, channels, PTY, memory, MCP, or other providers. The only new state is a derived cache bit required to avoid repeated declaration scans.
- **Known implementation choice:** The builder may choose the smallest exact factoring of the current `discover_and_mount` loop and private helper names, but must preserve the public signature, modes, guards, and acceptance assertions above.

**Handoff status / 交接状态:** This is an implementation plan only. Do not change production or test code until the user reviews this plan and supplies the execution method (for example, delegate implementation or main-session implementation).
