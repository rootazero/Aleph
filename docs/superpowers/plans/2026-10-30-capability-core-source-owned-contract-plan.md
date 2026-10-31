# Capability Core Source-Owned Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved, bounded Generic Gateway source-owned read/lease slice while preserving `ChannelRegistry` as the only Gateway authority and keeping the Capability layer derived.

**Architecture:** Extend the existing Generic Gateway bound declaration so its source publication revision, descriptor publication revision, entry identity, provider generation, and owner evidence remain explicit and independent. Add source-owned asynchronous read/lease methods over the existing `ChannelRegistry` entries; do not create a second registry, descriptor catalog, runtime table, or generic Zahir mount. The first slice covers `describe`, `list`, `project`, and source-owned `resolve`/lease validation; Gateway `subscribe`, durable recovery, Feishu wiring, and other provider migrations remain deferred.

**Tech Stack:** Rust workspace, Tokio async locks/tasks already used by Gateway, existing `CapabilityDescriptor`/`Scope`/`Reference`/`Projection` value types, `ProviderHandle`, `OwnershipTree`, SHA-256 descriptor fingerprints, `cargo test` and `cargo check` from the Rust workspace.

**Spec:** `docs/superpowers/specs/2026-10-30-capability-core-source-owned-contract-design.md`

## Global Constraints

- Keep `ChannelRegistry` as the canonical Gateway association, desired-intent, lifecycle, replacement, stop/drain, and webhook publication authority.
- Do not add a second registry, owner map, descriptor catalog, handler map, enabled-state map, runtime table, or durable store.
- Do not add or modify `CapabilityKind`.
- Keep provider generation in `ProviderHandle`; keep owner/lifetime/generation/revoke/dispose in `OwnershipTree`.
- Keep `source_revision`, `descriptor_revision`, `provider_generation`, `owner_generation`, and `SessionEvent.seq` independent and non-interchangeable.
- Use `OwnerRef::Runtime` and `LifetimeScope::Runtime` for the process-owned Generic Gateway binding; take `VisibilityScope` from `ChannelLifecycleContract`.
- Anchor reads at the `ChannelRegistry` source cut, then validate provider and owner evidence; if entry identity or source revision changes, retry only under a bounded source rule or fail closed.
- Do not implement Gateway `subscribe`, live observer wiring, durable recovery, Feishu → Zahir routing, Feishu production mounting, EffectClaim producers, Safe Replay, external exactly-once, universal scheduling, native unload, process isolation, RSS reduction, or `src/harness/**` changes.
- Do not claim that a successful provider state transition proves network delivery, authentication, process completion, native unload, or external exactly-once.
- Keep all public documentation bilingual where this plan adds or changes it; code comments remain English.

## Review Focus

- **Stale entry during resolve:** a replacement or unregister between source capture and validation must return a typed stale/fail-closed result and must never admit the old channel.
  - Test in Task 3: capture old entry, replace/unregister it, then validate the old source lease and assert `StaleEntry`/equivalent failure.
- **Counter confusion:** source revision and descriptor revision must remain independently observable and must not be silently reused during replacement.
  - Test in Task 1: assert both values and their overflow/failure behavior across first publication, replacement, and failed publication.
- **Owner transfer without epoch bump:** an owner identity change must invalidate a captured lease even if the owner generation is unchanged.
  - Test in Task 3: replace the binding owner under the existing `OwnershipTree` contract and assert validation fails closed.
- **Disabled/cold read side:** descriptor/list/project/resolve reads must not start provider runtime, refresh credentials, bind listeners, spawn tasks, or open external resources.
  - Test in Task 2: use a cold/disabled bound channel and assert read operations return declarations while provider state and runtime fields remain unchanged.
- **Generation exhaustion:** source publication and provider/owner generation overflow must fail before destructive mutation or publication.
  - Test in Task 1 and Task 3: exercise checked exhaustion fixtures and assert the incumbent entry remains authoritative.

---

### Task 1: Separate Generic Gateway publication and descriptor revisions

**Files:**
- Modify: `src/gateway/channel_registry/binding.rs` — extend the source-owned bound declaration/capture with an independent descriptor publication revision and preserve its validation error vocabulary.
- Modify: `src/gateway/channel_registry.rs:prepare_bound_entry`, `register_bound_channel`, and bound snapshot/capture paths — assign and carry the independent descriptor revision without changing `ChannelRegistry` ownership.
- Modify: `src/gateway/channel_registry/binding.rs` replacement/unregister commit paths around `commit_bound_replacement` and `commit_bound_unregistration` — advance descriptor revision only on successful descriptor publication, while source revision continues to fence registry publication.
- Test: existing bound-channel unit/resource-matrix tests under `src/gateway/channel_registry/binding.rs` and `src/gateway/channel_registry/resource_matrix.rs`.

**Interfaces:**
- Consumes: `ChannelLifecycleContract`, `ChannelRegistrationEntry`, `ProviderHandle`, `OwnershipTree`, `CapabilityDescriptor`, `canonical_descriptor_fingerprint`, and `ChannelBindingError`.
- Produces: source-owned bound captures that expose both `source_revision` and independent `descriptor_revision`; replacement/unregister paths that fail closed on exhaustion before mutating the incumbent.

- [ ] **Step 1: Write the failing revision tests**

  Add tests with explicit assertions for:
  - first bound publication produces a source revision and a descriptor revision;
  - replacement changes both publication facts but does not treat either as provider or owner generation;
  - descriptor publication failure leaves the incumbent entry, source revision, and descriptor revision unchanged;
  - source publication exhaustion returns `ChannelBindingError::PublicationExhausted` before entry removal;
  - descriptor revision exhaustion returns its typed checked error before replacement publication.

- [ ] **Step 2: Run the focused tests to verify the new assertions fail**

  Run from the repository root:

  `cargo test -p alephcore --lib gateway::channel_registry`

  Expected: the new revision assertions fail because the current bound descriptor revision is derived from the source revision and no independent exhaustion path exists.

- [ ] **Step 3: Implement the independent source-owned revision state**

  Keep source publication sequencing in `ChannelRegistry::source_revision`. Add the smallest source-local descriptor publication counter/state needed by the bound declaration. Allocate it only during the successful source publication commit; use checked increment and return a typed failure before removing or replacing the incumbent. Carry the value through `BoundChannelCapture` and recompute the descriptor fingerprint only after the descriptor revision is committed.

- [ ] **Step 4: Run the focused tests to verify the revision contract**

  Run:

  `cargo test -p alephcore --lib gateway::channel_registry`

  Expected: PASS, including overflow and incumbent-preservation assertions.

- [ ] **Step 5: Run formatting and type checking for the affected package**

  Run from the repository root:

  `cargo fmt --check`

  `cargo check -p alephcore --tests`

  Expected: both commands pass.

---

### Task 2: Add the source-owned Generic Gateway read model

**Files:**
- Create: `src/gateway/channel_registry/capability.rs` — source-owned async read/projection types and methods; this module must hold no parallel map or lifecycle state.
- Modify: `src/gateway/channel_registry.rs` — register the module and expose the source-owned methods through `ChannelRegistry` or its existing `binding` module boundary.
- Modify: `src/gateway/channel_registry/binding.rs` — reuse the existing bound capture and descriptor fingerprint rather than introducing another capture type with duplicated authority.
- Test: `src/gateway/channel_registry/capability.rs` and existing bound resource-matrix tests.

**Interfaces:**
- Consumes: `ChannelRegistry`’s existing source lock/cut, `BoundChannelCapture`, `CapabilityDescriptor`, `CapabilitySnapshot`, `CapabilityChange` value types where applicable, `Projection`, `Reference`, `Scope`, `TransportTarget`, `ProviderHandle`, and `OwnershipTree`.
- Produces: source-owned async methods with signatures equivalent to:
  - `pub(crate) async fn describe_bound_capabilities(&self, scope: &Scope) -> Result<CapabilitySnapshot, ChannelBindingError>`
  - `pub(crate) async fn list_bound_capabilities(&self, scope: &Scope) -> Result<Vec<CapabilityDescriptor>, ChannelBindingError>`
  - `pub(crate) async fn project_bound_capabilities(&self, target: TransportTarget, scope: &Scope) -> Result<Projection, ChannelBindingError>`
  - `pub(crate) async fn resolve_bound_capability(&self, reference: &Reference) -> Result<BoundChannelCapture, ChannelBindingError>`

  The exact placement may remain in `binding.rs` if that avoids an unnecessary module split, but the source-owned API must remain asynchronous because `ChannelRegistry` uses Tokio synchronization and must not block the runtime.

- [ ] **Step 1: Write the failing read-side tests**

  Add tests that register cold and disabled bound channels, then assert:
  - `describe_bound_capabilities` returns immutable descriptors filtered by `Scope.kind`, namespace, facet, and `Scope.visibility`;
  - `list_bound_capabilities` is the same source-backed snapshot view and does not create another catalog;
  - `project_bound_capabilities` returns only visible IDs and metadata for `Mcp`, `AcpJson`, or `StateDatabase` targets without mutating the registry;
  - `resolve_bound_capability` returns the existing bound capture for a visible ID and rejects an unknown, foreign-visibility, revoked, disposed, or stale entry;
  - all read operations leave the provider in `Cold`/`Disabled` and do not create runtime drivers, listeners, refresh tasks, or webhook mounts.

- [ ] **Step 2: Run the focused tests to verify they fail**

  Run:

  `cargo test -p alephcore --lib gateway::channel_registry::capability`

  Expected: FAIL because the source-owned read/projection methods do not exist.

- [ ] **Step 3: Implement the source-cut read algorithm**

  For each operation, acquire the existing `ChannelRegistry::lifecycle_sync` boundary, read the existing `channels` map and `source_revision` as one source cut, filter only `ChannelRegistration::Bound` entries, and release the source lock before any independent provider/owner validation that can await or call source code. Do not call `Channel::start`, `ProviderHandle::transition`, `TokenManager`, webhook mount APIs, task spawning, or external I/O.

  `list` must be a view over the same descriptor vector as `describe`; it must not maintain a second list. `project` must derive IDs/metadata from that same cut. `resolve` must capture source identity, entry identity, descriptor fingerprint, provider handle/generation, owner/lifetime/visibility evidence, and source/descriptor revisions, then fail closed if the source revision or entry identity no longer matches during bounded validation.

- [ ] **Step 4: Implement scope and visibility filtering at the source boundary**

  Use the `ChannelLifecycleContract.visibility` supplied at registration as the source fact. Apply `Scope.visibility` only as a query filter; never create one ownership binding per requested scope. Use the existing `OwnerRef::Runtime` and `LifetimeScope::Runtime` values for this process-owned slice unless a test fixture explicitly exercises an already-supported narrower source contract.

- [ ] **Step 5: Run focused read-side tests**

  Run:

  `cargo test -p alephcore --lib gateway::channel_registry::capability`

  Expected: PASS, including no-start assertions and scope/visibility filtering.

---

### Task 3: Close source-owned Gateway lease validation and replacement fencing

**Files:**
- Modify: `src/gateway/channel_registry/binding.rs` — add or complete the source-owned validation method over `BoundChannelCapture`, reusing existing `validate_channel_capture`, provider admission, owner binding validation, descriptor fingerprint, and revision checks.
- Modify: `src/gateway/channel_registry/capability.rs` — call source validation at the lease use boundary and map failures to existing `ChannelBindingError` variants.
- Modify: `src/capability/facade.rs` only if a source-neutral proof value is required by the existing contract; preserve the current Tool-specific `CanonicalEntryLease` path and do not make `facade.rs` depend on Gateway types.
- Modify: `src/gateway/channel_registry/binding.rs` replacement/retirement paths only where needed to preserve late-completion fencing.
- Test: existing replacement/retirement tests in `src/gateway/channel_registry/resource_matrix.rs` plus new capability binding tests.

**Interfaces:**
- Consumes: `BoundChannelCapture`, `ChannelRegistry::validate_channel_capture`, `OwnershipTree::with_current_binding`, `ProviderHandle::acquire_call`, source and descriptor revision fields.
- Produces: a source-owned lease validation result that distinguishes at least `NotFound`, `WrongVisibility`, `StaleEntry`, `StaleDescriptor`, `Retiring`, `Provider`, and `Owner` without exposing a raw `ChannelHandle` to legacy execution adapters.

- [ ] **Step 1: Write stale lease and late completion tests**

  Add tests that:
  - capture an old bound entry, replace it, and reject validation of the old capture;
  - unregister after capture and reject validation;
  - stop/revoke the provider generation and reject new admission;
  - change owner identity without relying on a generation bump and reject the old owner tuple;
  - attempt an old completion after replacement and prove it cannot withdraw, publish over, or mutate the replacement;
  - exercise a retiring entry with active work and assert the source returns a residual/busy result rather than claiming disposal.

- [ ] **Step 2: Run the focused replacement tests to verify they fail**

  Run:

  `cargo test -p alephcore --lib gateway::channel_registry::resource_matrix`

  Expected: the new lease assertions fail until the source-owned validation path is connected.

- [ ] **Step 3: Implement validation in source-owned lock order**

  Preserve the source lock order documented by the existing code: source admission/capture first, then ownership validation, then provider/entry-local validation as required by the existing contract. Keep callbacks synchronous and local where the existing ownership/provider APIs require it; do not introduce a cross-authority transaction. If an entry identity, source revision, descriptor fingerprint, provider generation, owner generation, owner identity, lifetime, or visibility check is missing, return a fail-closed error.

- [ ] **Step 4: Run replacement, overflow, and lease tests**

  Run:

  `cargo test -p alephcore --lib gateway::channel_registry::resource_matrix`

  `cargo test -p alephcore --lib capability::ownership`

  Expected: PASS with stale leases rejected and incumbent/replacement fencing preserved.

- [ ] **Step 5: Run package verification**

  Run:

  `cargo check -p alephcore --tests`

  Expected: PASS.

---

### Task 4: Add contract documentation and source-backed test evidence

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` only in the exact entries describing Generic Gateway Bound evidence, after the code and tests prove the new behavior.
- Modify: `docs/reference/GATEWAY.md` only where the current bounded Bound contract has changed; preserve explicit exclusions for Feishu/R2, production ingress, all-channel compatibility, durable replay, crash recovery, and external exactly-once.
- Modify: `docs/reference/ARCHITECTURE.md` only to record the approved Generic Gateway read/lease slice and its exact deferred boundaries.
- Update: `docs/superpowers/specs/2026-10-30-capability-core-source-owned-contract-claim-census.md` with source-backed current claims and dated history after implementation verification.
- Test/verification: documentation diff and repository-wide reference links.

**Interfaces:**
- Consumes: the completed source-owned API and passing tests from Tasks 1–3.
- Produces: reference text that distinguishes current implementation evidence from design-only and deferred claims; no new runtime authority.

- [ ] **Step 1: Write the documentation verification checklist**

  Before editing references, verify that each updated claim points to a concrete source symbol and test. Keep the census entries for Feishu, Terminal, Memory, Sandbox, EffectClaim, Safe Replay, and other kinds unchanged unless their source evidence actually changed.

- [ ] **Step 2: Update the exact reference entries**

  State explicitly that Generic Gateway now has the bounded read/lease slice, that `subscribe` and durable recovery remain deferred, and that Feishu production mounting remains prohibited by the B8.2 gates.

- [ ] **Step 3: Run documentation and repository checks**

  Run:

  `git diff --check`

  `cargo check -p alephcore --tests`

  `cargo test -p alephcore --lib gateway::channel_registry`

  `cargo test -p alephcore --lib capability::ownership`

  Expected: all commands pass; no `src/harness/**` or unrelated provider files appear in the diff.

- [ ] **Step 4: Commit the bounded slice**

  Review the diff for a single source-owned change set, then use a conventional commit message such as:

  `feat(gateway): add source-owned capability read lease`

---

## Self-review / 自审

- **Spec coverage:** Tasks 1–3 cover identity, independent revisions, source-cut reads, no-start behavior, visibility, lease validation, replacement fencing, late completion, and bounded Generic Gateway scope. Task 4 covers the requested stale/current reference alignment. `subscribe`, durable recovery, effect claims, and other providers are explicitly deferred rather than left implicit.
- **Step scan:** Each task separates failing tests, implementation, verification, and commit/documentation work. The plan does not authorize implementation before plan review.
- **Type consistency:** The plan reuses existing `CapabilityDescriptor`, `CapabilitySnapshot`, `Projection`, `Reference`, `Scope`, `TransportTarget`, `BoundChannelCapture`, `ProviderHandle`, and `OwnershipTree` vocabulary. Any source-neutral `BackendLease` change is conditional and must preserve the Capability core → Gateway dependency direction.
- **Review focus:** The five failure classes in the header each have an owning test task.
- **Proportion:** The plan stops at one bounded source-owned slice and does not prescribe Feishu, Terminal, Memory, Sandbox, or a central registry migration.

## Handoff / 交接

This plan is saved for review. It does not authorize production changes until the plan itself is accepted. The next-session prompt records the same scope and stop conditions for an implementation agent.
