# Capability Phase 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Converge the main builtin and MCP Tool callables on the descriptor-backed `ToolHandlerRegistry`, preserve Tool lifecycle and fail-closed recovery semantics, project existing consumers from descriptors, and document the verified boundary. Plugin remains on the existing `ToolCatalog`/extension manager bypass; it is out of this phase's canonical-registry scope and is deferred to a separate ExtensionHandler spec.

**Architecture:** Keep `ToolCapabilityDescriptor` plus `ToolHandlerRegistry` as the Tool callable and identity source. Converge only genuinely callable surfaces — the main builtin set and the `AlephToolDyn`/markdown-skill callable surface (wrapped via the existing `BuiltinHandler`), alongside MCP — onto that registry; Plugin stays on its current catalog/extension-manager bypass. Retain `ToolCatalog` for slash-command routing, skill rows, health probes, conflict resolution, UI metadata, and Plugin rows, and retain `LoopToolRegistry` only as a per-run execution adapter/index where callers still require it. Reuse current `ToolRegistrationScope`, SessionEvent identity, descriptor lookup, and `boundary_repair` classification; do not add a new scheduler, journal, or replay execution path.

**Tech Stack:** Rust, Tokio, serde/serde_json, existing ArcSwap registry snapshots, existing ToolCatalog and SessionEventStore, Cargo test/check/clippy.

**Spec:** `docs/superpowers/specs/2026-10-05-capability-phase3-design.md`

**Revision (2026-10-05):** Scope narrowed per user approval. Converge only the main builtin and the markdown-skill `AlephToolDyn` callable surface (wrapped via the existing `BuiltinHandler`), alongside MCP, onto `ToolHandlerRegistry`. Plugin stays on its `ToolCatalog`/extension-manager bypass and is deferred to a separate ExtensionHandler spec. Reason: Task 1 source census confirmed Plugin has no `ToolHandler` implementation (removed 2026-05-20) and must not be synthesized from metadata.

## Global Constraints

- All work remains on worktree `/Volumes/TBU4/Workspace/Aleph-phase3`, branch `phase3-capability-architecture`; never edit `main`.
- `src/harness/` remains free of new registry, policy, lifecycle, or replay logic; existing `ToolCallRequested` and effective-input field wiring may only be changed to correct a proven field-plumbing defect.
- Phase 2B production recovery remains VerifyOnly; do not add permit consumption, handler recovery invocation, durable replay claim, per-call crash budget, or recovery outcome writes.
- `ToolCallEffectiveInput` is already represented in current source; verify its existing contract and do not treat it as Phase 3 implementation work.
- `ToolCapabilityDescriptor::from_definition` remains fail-closed (`ReplayPolicy::Unsafe`); never derive Safe replay from `idempotent` or source kind.
- Do not persist handlers, closures, plugin code, credentials, or other executable objects; do not add another journal, heavy dependency, global Capability trait, or generic registry.
- `ToolCatalog` remains responsible for non-callable routing/discovery consumers (skills, custom commands, aliases, health, conflict resolution); remove only duplicated callable/identity facts proven to have no independent consumer.
- Do not weaken generation guards, stable captured-handler semantics, structured errors, or unknown-outcome behavior.
- Run Rust commands serially; before each Cargo command on line 2, inspect `/proc/meminfo` MemAvailable and wait if below 4 GiB. On macOS use the platform-equivalent available-memory check and record the observed value.
- Before every commit, inspect the staged tree and run `git diff --cached --check`; do not stage unrelated user changes.

## Review Focus

- Duplicate or stale registration handle after replace/dispose: test that an old handle cannot remove the replacement and that a second dispose is inert (Task 2).
- ToolCatalog projection removed while a legitimate slash-command/health/skill consumer remains: test catalog routing and health behavior after callable convergence (Task 3).
- Legacy or malformed call identity, missing current descriptor, and descriptor revision/fingerprint drift: test each remains VerifyOnly and produces no handler invocation (Task 4).
- A blocked or sanitized call without a trustworthy effective-input marker: test that it cannot become replay-eligible; preserve the current marker semantics without adding a producer to `src/harness/` (Task 4).
- Registry snapshot/change-feed lag or an unknown outcome crossing an adapter: test that the caller sees the captured generation/error and no success or automatic retry is fabricated (Tasks 2-4).

---

## File Structure

| Area | Files | Responsibility in this plan |
|---|---|---|
| Canonical descriptor and callable registry | `src/tools/descriptor.rs`, `src/tools/registry.rs`, `src/tools/service.rs` | Keep descriptor identity and handler pairing canonical; remove temporary revision-1 descriptor fabrication only after all consumers have an explicit canonical source. |
| Per-run execution and source adapters | `src/tools/runtime.rs`, `src/tools/traits.rs`, `src/tools/server/`, `src/tools/adapters/registry_adapter.rs`, `src/tools/adapters/mcp_adapter.rs`, `src/tools/handlers/builtin.rs`, `src/tools/handlers/registration.rs`, `src/mcp/tool_bridge.rs`, `src/extension/lifecycle.rs`, `src/executor/builtin_registry/` | Route real Tool callables through the canonical registry, with compatibility adapters at existing runtime boundaries and owner scopes for dynamic sources. |
| Catalog projections | `src/tool_metadata/registry/`, `src/tool_metadata/types/unified/`, `src/tools/service.rs`, `src/tools/adapters/registry_adapter.rs` | Keep catalog-specific fields and non-callable rows while deriving Tool-owned schema/source/safety fields from descriptor projection helpers. |
| Lifecycle and errors | `src/tools/registration_scope.rs`, `src/extension/effects/scope.rs`, `src/tools/service.rs`, `src/tools/error_kind.rs` | Reuse existing disposers and reports; align structured error mapping without creating a second lifecycle framework. |
| Durable identity and classification | `src/session/events.rs`, `src/session/reduction.rs`, `src/session/replay.rs`, `src/session/boundary_repair.rs`, `src/gateway/resume_coordinator.rs`, `src/harness/deps.rs`, `src/orchestrator/harness_bridge/` | Verify current identity/lookup/classification wiring and make only the minimum non-harness correction required to keep classification fail-closed. |
| Tests and references | colocated Rust unit tests, `tests/resume_coordinator_integration.rs`, `tests/plugin_lifecycle_roundtrip.rs`, `docs/reference/FEATURE_LOCATOR.md`, optionally `docs/reference/TOOL_SYSTEM.md` and `docs/reference/ARCHITECTURE.md` | Pin source convergence and projection behavior, then document only verified implementation facts. |

## Interfaces

- Existing descriptor constructor: `ToolCapabilityDescriptor::from_definition(definition: &ToolDefinition, revision: u64) -> ToolCapabilityDescriptor`.
- Existing identity lookup: `ToolDescriptorLookup::tool_call_identity(&self, name: &str) -> Option<ToolCallIdentity>`; `ToolHandlerRegistry` implements it.
- Existing registry entry points: `ToolHandlerRegistry::register(&self, descriptor: ToolCapabilityDescriptor, handler: Arc<dyn ToolHandler>) -> Result<RegistrationHandle, ToolError>`; `replace` has the same arguments/result; `resolve(&self, name: &str) -> Option<Arc<dyn ToolHandler>>`; `descriptor(&self, name: &str) -> Option<Arc<ToolCapabilityDescriptor>>`; `snapshot_state(&self) -> RegistrySnapshot`.
- Existing owner scope: `ToolRegistrationScope::track(&mut self, handle: RegistrationHandle)` and `async fn dispose(self) -> ToolDisposeReport`.
- Existing recovery boundary: `repairs_for_with_policy(reduction: &RunReduction, degrade: Option<&DegradeNote>, lookup: Option<&dyn ToolDescriptorLookup>) -> Vec<SessionEvent>`; `repair_boundary_with_policy(store: &dyn SessionEventStore, session: &SessionId, reduction: &RunReduction, degrade: Option<&DegradeNote>, lookup: Option<&dyn ToolDescriptorLookup>) -> Result<RepairReport, SessionError>`.
- Existing Tool catalog is not a callable registry: preserve its route/query/state/health APIs while ensuring ToolHandlerRegistry remains the sole handler lookup.
- Any new adapter constructor or projection helper must be named and typed in the task that introduces it before a later task consumes it. Do not invent a generic `Capability` API.

## Task 1: Revalidate Source Census and Lock the Vertical-Slice Boundary

**Files:**
- Read: `src/tools/descriptor.rs`, `src/tools/registry.rs`, `src/tools/registration_scope.rs`, `src/tools/service.rs`
- Read: `src/tools/runtime.rs`, `src/tools/traits.rs`, `src/tools/server/`, `src/tools/adapters/registry_adapter.rs`, `src/tools/adapters/mcp_adapter.rs`, `src/tools/handlers/mod.rs`, `src/tools/handlers/builtin.rs`, `src/tools/handlers/registration.rs`, `src/mcp/tool_bridge.rs`
- Read: `src/extension/lifecycle.rs`, `src/executor/builtin_registry/`, `src/tool_metadata/registry/`, `src/session/boundary_repair.rs`, `src/gateway/resume_coordinator.rs`, `src/harness/deps.rs`, `src/orchestrator/harness_bridge/`
- Test: existing colocated tests plus `tests/plugin_lifecycle_roundtrip.rs`

- [ ] **Step 1: Record current registered and callable source paths**

  Trace builtin, Plugin, and MCP from construction through model-visible projection and final invocation. In particular record which builtins and markdown-skill `AlephToolDyn` entries are possible to wrap as `ToolHandler` (via the existing `BuiltinHandler`), which Plugin tools have executable handlers versus catalog-only rows, and whether catalog entries carry command-only data that cannot be reconstructed from `ToolCapabilityDescriptor`.

- [ ] **Step 2: Verify current tests and repository status without changing files**

  Run: `git status --short --branch` and `git diff --check`.
  Run focused baseline tests serially: `cargo test --lib tools::registry`, `cargo test --lib tools::handlers::registration`, `cargo test --lib mcp::tool_bridge`, `cargo test --lib session::boundary_repair`, `cargo test --test plugin_lifecycle_roundtrip`.
  Expected: record pass/fail and any pre-existing failure; no source change in this task.

- [ ] **Step 3: Gate the implementation scope on the census**

  Confirm that the in-scope callable sources — main builtin and the markdown-skill `AlephToolDyn` surface, alongside MCP — have a callable implementation compatible with `ToolHandler`, or identify the exact source that is catalog-only / non-callable. Plugin is out of scope for the canonical registry this phase: do not synthesize a `ToolHandler` from `UnifiedTool` metadata, keep its existing catalog/extension-manager bypass, and treat its convergence as a deferred ExtensionHandler spec. If a builtin or skill family has no actual handler to register, narrow that source claim and stop for a spec amendment before implementation.

- [ ] **Step 4: Record the census gate before implementation**

  Write the exact callable/non-callable source decision into the executor's task notes. If the approved scope must change because a promised builtin/skill source turns out catalog-only, stop before Task 2 and request a spec amendment; do not create a product-code or empty census commit. If the boundary is unchanged, proceed with the verified file list and test names from Steps 1-3.

## Task 2: Route Callable Tool Registration Through the Descriptor Registry

**Files:**
- Modify: `src/tools/handlers/builtin.rs`, `src/tools/handlers/registration.rs`, `src/tools/adapters/registry_adapter.rs`, `src/mcp/tool_bridge.rs`
- Modify only after Task 1 proves a callable path: the relevant `src/executor/builtin_registry/` construction boundary and the markdown-skill `AlephToolDyn` adapter boundary (`src/tools/server/`, `src/tools/markdown_skill/`)
- Not modified this phase: `src/extension/lifecycle.rs` (Plugin remains on its catalog/extension-manager bypass)
- Test: `src/tools/registry.rs`, `src/tools/handlers/registration.rs`, `src/mcp/tool_bridge.rs`, `tests/plugin_lifecycle_roundtrip.rs`

**Interfaces:**
- Consume `ToolHandler::definition(&self) -> ToolDefinition` and `ToolHandler::invoke(&self, input: Value) -> Result<ToolOutput, ToolError>`.
- Consume `ToolHandlerRegistry::{register,replace,resolve,descriptor,snapshot_state}` and `ToolRegistrationScope::{track,dispose}` as listed above.
- Produce no second handler map. Existing `LoopToolRegistry` may receive adapters only from an explicitly snapshotted canonical registry or from a separately documented temporary compatibility source that has no canonical handler yet.

- [ ] **Step 1: Add failing source-convergence tests**

  Add tests proving: (a) every callable MCP Tool is resolved by `ToolHandlerRegistry`; (b) each builtin and markdown-skill `AlephToolDyn` entry in the selected census slice has descriptor and handler from the same generation; (c) Plugin tools remain on the catalog/extension-manager bypass and are not misrepresented as callable Tool capabilities; (d) non-callable ToolCatalog rows remain discoverable as commands but are not misrepresented as callable Tool capabilities.

- [ ] **Step 2: Run the focused tests and confirm the uncovered paths fail**

  Run: `cargo test --lib tools::handlers::registration` and `cargo test --test plugin_lifecycle_roundtrip`.
  Expected: new convergence assertions fail only for the source paths not yet connected; existing MCP behavior remains intact.

- [ ] **Step 3: Add source adapters at construction boundaries**

  For each callable source confirmed in Task 1 — main builtin and markdown-skill `AlephToolDyn` (wrapped via `BuiltinHandler`), plus MCP — create its `ToolCapabilityDescriptor` from the handler's `ToolDefinition`, register descriptor and handler together, and track the returned generation-guarded handle in that source's existing owner scope. Do not register a metadata-only catalog row (including Plugin rows) as a callable handler.

- [ ] **Step 4: Preserve stable replace and invocation semantics**

  Add a replace test asserting the captured old `Arc<dyn ToolHandler>` remains callable for an already-started invocation while a subsequent `resolve` returns the replacement descriptor/handler generation. Assert stale handles cannot unregister the replacement.

- [ ] **Step 5: Run source adapter tests**

  Run: `cargo test --lib tools::registry`, `cargo test --lib tools::handlers::registration`, `cargo test --lib mcp::tool_bridge`, and `cargo test --test plugin_lifecycle_roundtrip`.
  Expected: PASS; all registered callable sources in the Task 1 boundary (builtin, markdown-skill `AlephToolDyn`, MCP) use the canonical registry; Plugin remains on its bypass; no new harness logic.

- [ ] **Step 6: Commit the source convergence**

```bash
git add src/tools/handlers/builtin.rs src/tools/handlers/registration.rs src/tools/adapters/registry_adapter.rs src/mcp/tool_bridge.rs src/executor/builtin_registry src/tools/server src/tools/markdown_skill tests/plugin_lifecycle_roundtrip.rs
 git diff --cached --check
git commit -m "refactor: route tool callables through capability registry"
```

Stage only paths actually changed; omit absent/unmodified paths.

## Task 3: Make Existing Tool Projections Consume Descriptor-Owned Fields

**Files:**
- Modify: `src/tools/descriptor.rs`, `src/tools/service.rs`, `src/tools/adapters/registry_adapter.rs`, `src/mcp/tool_bridge.rs`
- Modify catalog projection only as needed: `src/tool_metadata/types/unified/`, `src/tool_metadata/registry/`, `src/tools/handlers/registration.rs`
- Test: descriptor projection tests, `metadata_form_tests`, MCP registration/bridge tests, model-visible tool schema tests

**Interfaces:**
- Consume `ToolCapabilityDescriptor::{to_metadata_definition,to_unified_tool}` and `ToolDefinition::from_descriptor(&ToolCapabilityDescriptor) -> ToolDefinition`.
- Keep command-only fields such as aliases, routing capabilities, UI metadata, health and conflict-resolution state owned by ToolCatalog; descriptor projection owns Tool identity/schema/source and descriptor-backed safety/replay fields.
- Remove `to_metadata_form(defs: &[ToolDefinition]) -> Arc<[crate::tool_metadata::ToolDefinition]>` only after every caller has an explicit descriptor-backed or command-only conversion path; do not preserve revision `1` as a fabricated identity.

- [ ] **Step 1: Inventory all `to_metadata_form` and hand-built Tool projections**

  Use `rg -n 'to_metadata_form|ToolCapabilityDescriptor::from_definition|to_metadata_definition|to_unified_tool' src tests --glob '*.rs'`. Classify every call as callable Tool projection, command/catalog row, or test helper.

- [ ] **Step 2: Add failing parity tests for both selected consumers**

  Assert that descriptor name, description, input schema, source, confirmation, concurrency, duration, revision/fingerprint where represented, and replay policy do not drift across the MCP and model-visible Tool projection. Assert catalog-only aliases/routing/UI/health data remains intact.

- [ ] **Step 3: Route projection through descriptor helpers**

  Replace duplicated mapping only for descriptor-backed Tool entries. Preserve any catalog-specific metadata in its current owning subsystem and do not invent a descriptor field for unrelated routing/UI data.

- [ ] **Step 4: Remove temporary revision-1 descriptor fabrication**

  Delete `to_metadata_form` and its tests only when `rg` confirms there are no production callers — which requires builtin canonicalization and migration of every caller, including `ScopedToolService`'s metadata projection — and replace tests with assertions against the new canonical projection path. `to_metadata_form` is a live production path (fabricating revision-1 descriptors for loop-side `ToolDefinition`s); until every caller has a canonical source, keep its conversion explicitly compatibility-only and do not claim its identity is canonical.

- [ ] **Step 5: Run projection and catalog tests**

  Run: `cargo test --lib tools::descriptor`, `cargo test --lib tools::service`, `cargo test --lib tool_metadata::registry`, `cargo test --lib tools::handlers::registration`, `cargo test --lib mcp::tool_bridge`, plus the exact model-visible projection test discovered in Step 1.
  Expected: both consumers agree on descriptor-owned fields, while catalog routing and non-Tool rows remain unchanged.

- [ ] **Step 6: Commit the projection convergence**

```bash
git add src/tools/descriptor.rs src/tools/service.rs src/tools/adapters/registry_adapter.rs src/mcp/tool_bridge.rs src/tool_metadata
 git diff --cached --check
git commit -m "refactor: project tool views from capability descriptors"
```

Stage only paths actually changed; omit unmodified paths.

## Task 4: Verify Lifecycle, Structured Error, and Fail-Closed Recovery Contracts

**Files:**
- Modify only where tests demonstrate a gap: `src/tools/registration_scope.rs`, `src/tools/registry.rs`, `src/tools/service.rs`, `src/tools/error_kind.rs`, `src/session/boundary_repair.rs`, `src/gateway/resume_coordinator.rs`, `src/orchestrator/harness_bridge/`
- Tests: colocated lifecycle/recovery tests, `tests/resume_coordinator_integration.rs`, `src/harness/tests/act.rs` only if asserting existing field plumbing (no harness implementation expansion)

**Interfaces:**
- Consume `ToolCallIdentity`, `ToolDescriptorLookup`, `ReplayDecision`, `DanglingCall`, `repairs_for_with_policy`, and `repair_boundary_with_policy` as currently defined.
- Classification may report eligibility/refusal but is not authorization; Phase 2B stays VerifyOnly and no recovery code in this task may call `ReplayPermit::invoke`, consume a durable replay claim, apply a per-call crash budget, or write a new replay outcome.

- [ ] **Step 1: Add or identify tests for lifecycle boundary cases**

  Pin reverse-order dispose, disposer failure reporting, idempotent disposal, stale-generation handle no-op after replacement, close rejecting new registration/resolve, and an in-flight captured handler remaining stable. Reuse existing tests where they already prove the contract.

- [ ] **Step 2: Add classification-only recovery tests**

  Pin legacy/malformed identity, missing descriptor, Unsafe, schema/revision/fingerprint mismatch, duplicate/effective-input contradictions, and absent/invalid effective input to VerifyOnly/unknown repair. Assert no handler invocation, no new `ReplayPermit` consumption, no synthetic success, and idempotent repeated repair.

- [ ] **Step 3: Fix only the failing Core seam**

  Boundary repair is currently descriptor-blind in production (`repair_boundary` passes `lookup = None`). This phase MAY connect the existing `ToolDescriptorLookup` through the non-harness owner/bridge boundary for classification-only recovery (descriptor-aware `repairs_for_with_policy`/`repair_boundary_with_policy`). This is classification-only: do not call `ReplayPermit::invoke`, do not implement handler replay/claim/budget/outcome execution. If the current production path already supplies the lookup and tests prove fail-closed behavior, make no product-code change for that sub-area. Do not add a registry lookup or replay decision to `src/harness/`.

- [ ] **Step 4: Align structured error projection only where a proven lossy conversion exists**

  Preserve the internal reason for not-registered, not-visible, invocation-denied, approval-required, descriptor mismatch, registry closed, and unknown outcome through existing `ToolError`/protocol mapping. Do not add a second permission policy or retry classifier. Add a contract test for each changed transport mapping.

- [ ] **Step 5: Run lifecycle and recovery tests**

  Run: `cargo test --lib tools::registration_scope`, `cargo test --lib tools::registry`, `cargo test --lib session::events`, `cargo test --lib session::reduction`, `cargo test --lib session::boundary_repair`, `cargo test --lib session::replay`, `cargo test --test resume_coordinator_integration`, and the relevant `cargo test --lib harness::tests::act` only if existing identity plumbing is touched.
  Expected: classification remains fail-closed; no execution, claim, budget, or outcome-writing capability is introduced.

- [ ] **Step 6: Commit lifecycle and recovery verification/fixes**

```bash
git add src/tools/registration_scope.rs src/tools/registry.rs src/tools/service.rs src/tools/error_kind.rs src/session src/gateway/resume_coordinator.rs src/orchestrator/harness_bridge
 git diff --cached --check
git commit -m "fix: preserve capability lifecycle and recovery boundaries"
```

Stage only paths actually changed; omit unmodified paths.

## Task 5: Update Feature Locator and Reference Documentation

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify only if current references are inaccurate: `docs/reference/TOOL_SYSTEM.md`, `docs/reference/ARCHITECTURE.md`

- [ ] **Step 1: Locate the current Tool, registry, MCP, recovery, lifecycle, and projection entries**

  Read the relevant FEATURE_LOCATOR sections and linked reference files. Record current line anchors after implementation, not from the pre-change census.

- [ ] **Step 2: Update FEATURE_LOCATOR with verified ownership and limits**

  Document descriptor and registry entry points; `LoopToolRegistry` as runtime consumer/index; ToolRegistrationScope owner/dispose report; descriptor projections into MCP and model-visible ToolDefinition/catalog; discover versus invoke; default Unsafe and current descriptor classification; unknown outcome fail-closed semantics; and the not-yet-unified Skill/Agent/Plugin/ACP/Resource/Task/Subscription surfaces.

- [ ] **Step 3: Update secondary references only for contradictions**

  Keep `TOOL_SYSTEM.md`/`ARCHITECTURE.md` changes narrow. Do not describe a future universal Capability registry, ownership tree, or Safe replay execution as current behavior.

- [ ] **Step 4: Check links, anchors, and document consistency**

  Run: `git diff --check`; search every newly documented symbol/path with `rg`; ensure every documented statement maps to a production call site or an explicitly labeled deferred boundary.
  Expected: no stale line anchors and no future-state claims presented as implemented facts.

- [ ] **Step 5: Commit documentation**

```bash
git add docs/reference/FEATURE_LOCATOR.md docs/reference/TOOL_SYSTEM.md docs/reference/ARCHITECTURE.md
git diff --cached --check
git commit -m "docs: map capability tool architecture"
```

Stage only documentation files actually changed.

## Task 6: Full Verification, Final Architecture Review, and Delivery

**Files:**
- Read: all changed files and staged diff
- Verify: workspace Rust gates and focused integration tests

- [ ] **Step 1: Run final focused tests serially**

  Run all focused commands from Tasks 2-5 that cover changed code. Before each Cargo command, check available memory per Global Constraints.
  Expected: all pass; report pre-existing failures separately with exact command and output.

- [ ] **Step 2: Run project gates**

  Run: `cargo check`, `cargo clippy --all-targets`, and `cargo test --lib`; run any platform/workspace-specific gate documented in `docs/reference/DEVELOPMENT.md` that applies to changed crates.
  Expected: all pass, or a precise list of existing failures is recorded and no new failure is attributable to this branch.

- [ ] **Step 3: Verify architectural boundaries and stale paths**

  Search production source for handler registrations not backed by `ToolHandlerRegistry` (excluding the documented Plugin catalog/extension-manager bypass); search for production `to_metadata_form` or hard-coded temporary descriptor revisions; inspect all new `src/harness/` diffs (expected none unless a pre-approved field-plumbing defect was proven); verify no Phase 2B execution API was added or newly called. Keep ToolCatalog command/skill/health/Plugin consumers intact.

- [ ] **Step 4: Review the exact staged tree**

```bash
git status --short --branch
git diff --cached --check
git diff --cached --stat
git diff --cached --name-status
```

Confirm only intended files are staged, commit the final verified changes with `git commit -m "refactor: complete capability tool phase 3"`, then verify `git status --short --branch` is clean.

- [ ] **Step 5: Report delivery and non-goals**

  Summarize source paths converged (builtin, markdown-skill `AlephToolDyn`, MCP), the Plugin bypass retained, compatibility catalog responsibilities retained, tests/gates run, commits, and explicitly list deferred work: Plugin ExtensionHandler convergence, universal Capability kinds, durable Safe Replay execution, ownership tree, approval/hook memo migration, and ACP agent server.

## Spec Coverage Self-Check

- Single Tool descriptor/handler identity, sources, resolve, replacement, removal, and projection: Tasks 2-3.
- Existing owner scope, reverse-order disposal, failure reports, and in-flight behavior: Task 4 (and source-owner adapters in Task 2).
- Durable identity plus current descriptor classification, unknown outcome, and VerifyOnly boundary: Task 4.
- Discover/invoke distinction and structured error preservation: Tasks 2 and 4.
- MCP plus model-visible descriptor projections: Task 3.
- ToolCatalog compatibility responsibilities and stale path removal: Tasks 2-3 and 6.
- FEATURE_LOCATOR and reference accuracy: Task 5.
- Build, tests, architectural redlines, staged-tree review, clean worktree: Task 6.

## Executor Handoff Notes

The source census found MCP already registers descriptor/handler pairs in `ToolHandlerRegistry` and tracks catalog projection cleanup through `ToolRegistrationScope`; preserve this behavior rather than reimplementing it. `unregister_mcp_tools` is compatibility/emergency cleanup because name/source sweeping can remove a replacement; normal teardown must remain handle/scope-based. Main builtin and the markdown-skill `AlephToolDyn` surface are callable and can be wrapped via the existing `BuiltinHandler` (path proven by `CapabilityHandler`/`set_capability`); they are the canonical-registry candidates this phase. The current production Plugin path is catalog-only and `ToolHandler`'s extension implementation was removed; Plugin is out of scope — do not manufacture callable Plugin handlers from catalog metadata, keep its `ToolCatalog`/extension-manager bypass, and defer convergence to a separate ExtensionHandler spec. `to_metadata_form` currently fabricates temporary revision-1 descriptors and is a removal candidate, but delete it only after all production consumers are traced and migrated (builtin canonicalization must land first). Current recovery has descriptor-aware classification; production `repair_boundary` still passes `lookup = None`, so classification-only lookup wiring is the permitted recovery change this phase. `ToolCallEffectiveInput` exists in the current codebase; its Phase 2B production semantics are not a Phase 3 replay-execution authorization.
