# Capability Core Contract-First Vertical Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the existing Capability Core expose one source-backed snapshot contract and prove that cold providers remain side-effect free until explicit lifecycle admission, without adding a second authority or wiring Feishu.

**Architecture:** Reuse `CapabilityBackend` as the read-side source contract, `ProviderHandle` as the provider state/generation/admission primitive, and `OwnershipTree` as the owner/visibility authority. Keep Tool's `CanonicalEntryLease` source-specific; this slice does not create a universal lease or a Core-wide lifecycle supervisor. The fixture and tests exercise the same primitives used by current production Tool code, while Feishu remains deferred.

**Tech Stack:** Rust, tokio, serde, existing `CapabilityDescriptor`/`Scope`/`Zahir`/`ProviderHandle`/`OwnershipTree` types, `cargo fmt`, `cargo check`, `cargo clippy`, and `cargo test`.

**Spec:** `docs/superpowers/specs/2026-10-25-capability-core-provider-contract-design.md`

## Global Constraints

- Do not modify `src/harness/**`.
- Do not add a `CapabilityKind`.
- Do not add a second callable registry, descriptor registry, handler map, enabled-state map, owner/generation/disposer map, or durable store.
- Do not create a Feishu descriptor adapter, Feishu resolve route, Feishu production mount, or Feishu→Zahir production route.
- Do not implement EffectClaim production, Safe Replay, external exactly-once, universal scheduler, or durable Feishu recovery.
- `describe`, protocol `list`, `project`, and `resolve` must not start providers, authenticate, obtain tokens, create connections, open listeners, start websockets, or allocate provider runtime resources.
- Keep provider generation, owner generation, source/registry revision, and `SessionEvent.seq` separate.
- Keep Tool's `CanonicalEntryLease` source-specific until a second real callable provider proves a different universal shape.
- Do not execute commit, push, reset, rebase, stash, or clean.

## Review Focus

- A snapshot must never pair descriptors from one source revision with another revision; test one atomic publication boundary in `CapabilityBackend`.
- A cold or disabled provider must remain cold after `describe`, `project`, and `resolve`; test both state and resource receipts.
- A provider restart must allocate a new provider generation and reject old admission; test this independently from owner generation.
- An owner bump/revoke/dispose must reject old leases without advancing the source revision; test the domains separately.
- A late completion from an old instance must not mutate replacement state; test instance/generation identity at the completion boundary.

---

### Task 1: Promote the canonical source snapshot to the backend contract

**Files:**
- Modify: `src/capability/backend.rs:26-108` (`CapabilityBackend`, `ToolBackendAdapter::snapshot_capabilities`, `impl CapabilityBackend for ToolBackendAdapter`)
- Modify: `src/capability/zahir_facade.rs:176-207,389-400` (snapshot construction and describe path)
- Test: `src/capability/backend.rs` test module
- Test: `src/capability/zahir_facade.rs` test module

**Interfaces:**
- Consumes: existing `Scope`, `CapabilityDescriptor`, `CapabilityRevision`, `CapabilitySnapshot`, and `ToolHandlerRegistry::canonical_publication()`.
- Produces: `CapabilityBackend::snapshot(&self, scope: &Scope) -> CapabilitySnapshot`, where the returned `CapabilitySnapshot::committed_cursor` is the source revision observed in the same canonical publication read as its descriptors.

- [ ] **Step 1: Write the failing contract test**

Add a backend test named `snapshot_returns_descriptors_and_revision_from_one_publication` that registers a Tool, calls `CapabilityBackend::snapshot`, and asserts:

- one descriptor is returned;
- its `CapabilityKind` remains `Tool`;
- `snapshot.committed_cursor == Cursor(CapabilityRevision.0)` for the same registry publication;
- `Scope { facet: Some(CapabilityFacet::Callable), .. }` includes the Tool;
- `Scope { facet: Some(CapabilityFacet::EventSource), .. }` excludes it.

Add a facade test named `describe_uses_backend_snapshot_contract` that asserts `describe(scope).committed_cursor` and the visible descriptors come from the same registry revision.

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p alephcore --lib capability::backend::tests::snapshot_returns_descriptors_and_revision_from_one_publication
cargo test -p alephcore --lib capability::zahir_facade::tests::describe_uses_backend_snapshot_contract
```

Expected: compilation failure because `CapabilityBackend::snapshot` and the named test behavior do not exist yet.

- [ ] **Step 3: Add `CapabilityBackend::snapshot(&self, scope: &Scope) -> CapabilitySnapshot`**

Implement the method in `src/capability/backend.rs` and make `ToolBackendAdapter` build the descriptor list and `Cursor` from one `canonical_publication()` read. Do not implement the default using `enumerate` plus a separate revision read. Keep `snapshot_capabilities` only as a private compatibility helper if existing callers still require its tuple shape; make the facade use the new snapshot contract.

- [ ] **Step 4: Make `ZahirFacade::describe` consume the source snapshot**

Change the describe path to use `CapabilityBackend::snapshot`, then apply the existing `OwnershipTree` visibility observation without copying source revision or descriptor state. Preserve the existing facet predicate and Tool-only ownership reconciliation semantics.

- [ ] **Step 5: Run focused tests and verify they pass**

Run the command from Step 2. Expected: PASS with the descriptor list, facet filter, and committed cursor agreeing.

### Task 2: Prove cold-provider read-side isolation

**Files:**
- Modify: `src/capability/provider.rs` test module
- Modify: `src/capability/zahir_facade.rs` test module
- Modify: `src/capability/projection_host.rs` test module only if the existing mount fixture is required for the read-side assertion
- Test: the existing `ToolHandlerRegistry::new_with_provider` path in `src/tools/registry.rs`

**Interfaces:**
- Consumes: `ProviderHandle::new`, `ProviderLifecycle::new`, `ProviderState::{Cold, Starting, Ready, Stopping, Stopped, Disabled}`, `ToolHandlerRegistry::new_with_provider`, `ZahirFacade::new`, and `Zahir::{describe, resolve, project}`.
- Produces: tests proving that read operations observe a cold provider without transitioning it or allocating a provider resource.

- [ ] **Step 1: Write the failing read-side tests**

Add tests named:

- `describe_does_not_start_a_cold_provider`;
- `project_does_not_start_a_cold_provider`;
- `resolve_fails_closed_without_starting_a_cold_provider`.

Construct a `ProviderHandle` from `ProviderLifecycle::new` in `Cold`, pass it through the existing canonical Tool registry construction, and assert after each operation:

- provider state is still `Cold`;
- provider generation is unchanged at `0`;
- no resource probe, listener, token, or connection receipt exists;
- `resolve` returns the existing fail-closed `ResolveError` without changing provider state.

Do not add a second registry or a production mock handler map. Use the existing provider-aware Tool registry constructor and existing Zahir facade.

- [ ] **Step 2: Run the focused tests and verify the current behavior**

Run:

```bash
cargo test -p alephcore --lib capability::zahir_facade::tests::describe_does_not_start_a_cold_provider
cargo test -p alephcore --lib capability::zahir_facade::tests::project_does_not_start_a_cold_provider
cargo test -p alephcore --lib capability::zahir_facade::tests::resolve_fails_closed_without_starting_a_cold_provider
```

Expected: the tests either pass immediately or expose the exact existing read-side transition that must be removed; no test may accept an implicit start.

- [ ] **Step 3: Implement only the smallest production correction required**

If a read path starts or mutates provider lifecycle, remove that transition from the read path. Keep Tool ownership reconciliation separate from provider activation. Do not add a generic `start`/`stop` method to `Zahir` in this task.

- [ ] **Step 4: Run the focused tests and verify they pass**

Run the command from Step 2. Expected: all three tests pass with `Cold`, generation `0`, and no resource receipt after read operations.

### Task 3: Add a resource-probe lifecycle fixture and generation-domain tests

**Files:**
- Modify: `src/capability/provider.rs` test module
- Modify: `src/capability/ownership.rs` test module only where a cross-domain assertion belongs
- Modify: `src/capability/zahir_facade.rs` test module for provider/owner interaction

**Interfaces:**
- Consumes: `ProviderHandle::{transition, acquire_call, snapshot}`, `OwnershipTree::{register, bump, revoke, dispose, generation}`, existing `BackendLease`, and `CapabilityRevision`.
- Produces: a test-only resource probe and assertions that provider generation, owner generation, and source revision are independent and fail closed.

- [ ] **Step 1: Write the failing lifecycle and late-completion tests**

Add tests named:

- `explicit_start_is_the_only_transition_that_allocates_fixture_resources`;
- `provider_restart_rejects_old_generation_without_owner_mutation`;
- `owner_bump_rejects_old_lease_without_provider_restart`;
- `late_completion_from_replaced_generation_is_rejected`.

The probe must count only test-observable resource receipts. It must not become a production registry or store. Assert:

- construction, discovery, `describe`, `project`, and `resolve` allocate zero resources;
- `Starting -> Ready` is the first resource allocation point;
- restart increments provider generation and rejects the old generation;
- owner bump changes owner generation while provider generation and source revision remain unchanged;
- revoke/dispose rejects the old lease;
- a completion carrying the old provider generation cannot update replacement state.

- [ ] **Step 2: Run the focused tests and verify their initial result**

Run:

```bash
cargo test -p alephcore --lib capability::provider::tests::explicit_start_is_the_only_transition_that_allocates_fixture_resources
cargo test -p alephcore --lib capability::zahir_facade::tests::provider_restart_rejects_old_generation_without_owner_mutation
cargo test -p alephcore --lib capability::zahir_facade::tests::owner_bump_rejects_old_lease_without_provider_restart
cargo test -p alephcore --lib capability::zahir_facade::tests::late_completion_from_replaced_generation_is_rejected
```

Expected: the new test names are initially absent; after the fixture is added, any failing assertion must identify a missing generation or late-completion fence rather than be hidden by a mock authority.

- [ ] **Step 3: Implement the smallest test fixture and missing production fence**

Keep the fixture inside `#[cfg(test)]` support or an explicitly test-only module. Reuse `ProviderHandle` and `OwnershipTree`; do not introduce a parallel lifecycle map. If production code is missing a fence, add it at the existing provider/owner validation boundary rather than in the fixture.

- [ ] **Step 4: Run the focused tests and verify they pass**

Run the command from Step 2. Expected: PASS with the four domains and late completion behavior independently observable.

### Task 4: Update the contract documentation and run the complete focused gate

**Files:**
- Modify: `docs/reference/ARCHITECTURE.md:104-106` only if implementation details change the current facet contract
- Modify: `docs/reference/FEATURE_LOCATOR.md:6089-6091` only if implementation details change the current deferred boundary
- Modify: `docs/superpowers/specs/2026-10-25-capability-core-provider-contract-design.md` to record final implementation decisions
- Test: all capability tests and the package checks below

**Interfaces:**
- Consumes: the completed Tasks 1–3 contracts and their test names.
- Produces: documentation that states exactly what this slice closes and what remains deferred.

- [ ] **Step 1: Update docs only for facts proven by the tests**

Record that the source snapshot is atomic, read-side operations do not start cold providers, and provider/owner/source revision domains remain distinct. Keep Feishu descriptor/resolve/mount and external-effect recovery deferred unless a later task actually closes them.

- [ ] **Step 2: Run formatting and static checks**

Run:

```bash
cargo fmt --all --check
git diff --check
cargo check -p alephcore --tests
cargo clippy -p alephcore --tests -- -D warnings
```

Expected: all commands exit 0.

- [ ] **Step 3: Run the complete focused capability suite**

Run:

```bash
cargo test -p alephcore --lib capability::
```

Expected: all capability tests pass. A macOS linker warning about `__eh_frame` may appear; it is not a test failure unless the command exits non-zero.

- [ ] **Step 4: Inspect the final diff and status**

Run:

```bash
git diff --check
git status --short --branch
git diff --stat
git diff --name-only
```

Confirm that no file under `src/harness/**` changed and that no Feishu production route, descriptor adapter, resolver, mount, EffectClaim producer, or durable store was added. Do not commit or push.

## Deferred after this plan

- Feishu descriptor/source mapping and production route.
- Universal non-Tool lease shape.
- EventSource and Subscription production backends.
- EffectClaim producer integration, Safe Replay, exactly-once external effects, and universal scheduler.
- Terminal, Memory, Sandbox, and remaining provider migrations.
- Full workspace tests and live TUI/production deployment validation.
