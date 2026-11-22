# Severed-Wire Audit — `src/capability/` core contract slice

- **Date:** 2026-11-20
- **Worktree:** `/home/zou/data/workspace/Aleph-wt-sev3` (HEAD `d57791367`)
- **Slice audited:** `src/capability/{mod.rs, descriptor.rs, lifecycle.rs, activation.rs, ambient.rs, session.rs, lazy_load.rs, production_init.rs, cache_policy.rs, constraints.rs, grant.rs}`
- **Method:** PRODUCED − CONSUMED symbol parity (`rg`/`grep` across the whole repo, incl. `src/`, `src/bin/`, `desktop/`, `crates/`, `benches/`, `qa/`), then read-before-write triage of every candidate. Production vs `#[cfg(test)]` distinguished for every symbol.
- **Read-only:** no source file was modified; no `git add/commit/checkout/stash`; no `cargo` invocation.

## Summary

**Verdict: the `src/capability/` core contract slice is NOT wired end-to-end.** 11 of the 11 slice files compile, but roughly a third of the slice's public surface has **zero production consumers**, and two whole subsystems (`session.rs`, `grant.rs`, `ambient.rs`) never reach a binary at all.

| Disposition | Count | IDs |
|---|---|---|
| CUT (severed, provable) | 7 | SW-01, SW-02, SW-03, SW-04, SW-05, SW-06, SW-10 |
| DECIDE (severed but the fix is a product call) | 3 | SW-07, SW-08, SW-09 |
| CONNECT (re-verified as healthy) | 6 | SW-11 … SW-16 |

Headline results:

1. **`src/capability/session.rs` (306 lines) is an orphan module that is never compiled** (form 6). `src/capability/mod.rs` has no `pub mod session;`. It cannot even compile if the declaration were added — it constructs a `ProjectionMetadata { attributes }` field that does not exist on the type.
2. **`src/capability/grant.rs` (892 lines) has zero production consumers** (form 1). Its own module doc claims "Grant validation happens at resolve time through `OwnershipTree`" — `OwnershipTree` never appears in the file, and no resolve path calls `validate_grant_chain`.
3. **`src/capability/ambient.rs` — the entire ambient capability family (`aleph/ambient`) is unwired** (form 1). `CapabilityKind::Ambient` is never produced by a production descriptor.
4. **The whole activation-observation path is dark in production** (forms 1+2). `CapabilityActivationController::new_observed` is the only constructor that populates `self.events`, and its only caller is a `#[cfg(test)]` file. As a result `CapabilityEvent::CapabilityAttached` and `CapabilityEvent::CapabilityDetaching` are **never emitted in production**, even though `src/capability/audit.rs` classifies both as audit-worthy.
5. **The lease-TTL/expiry machinery is unreachable in production** (form 1). The only production constructor passes `ttl: None` (`src/capability/policy_runtime.rs:147`), so `Lease::is_expired()` is always false and `LifecycleError::LeaseExpired` can never be raised.

---

## Slice inventory (production surface)

| File | Lines | Public surface | Production consumer |
|---|---|---|---|
| `src/capability/mod.rs` | 426+ | `CapabilitySlot`, `MutableCapabilitySlot`, `ALL_SLOTS` (49 entries), `MissingSemantics`, `Outcome` | yes — `src/diagnostics/checks/capability_wiring.rs:122,250,288`; `src/spend/mod.rs:629` |
| `src/capability/descriptor.rs` | 143 | `CapabilityKind` (12 variants), `CapabilityFacet(s)`, `CapabilityDescriptor`, `ProjectionMetadata`, `SchemaRef`, `CapabilityRevision` | yes — `src/capability/backend.rs:368` (`to_descriptor`), `src/gateway/channel_registry/binding.rs:204,220` |
| `src/capability/lifecycle.rs` | 392 | `LifecycleState`, `Lease`, `CapabilityLifecycle` | partially — only via `activation.rs`; TTL arm dead |
| `src/capability/activation.rs` | 649 | `CapabilityActivation` trait, `CapabilityActivationController`, `ActivationHook` | partially — trait used; `new_observed`/`activate_with_hook` test-only |
| `src/capability/ambient.rs` | 348 | `AMBIENT_NAMESPACE`, `AmbientCapability`, `AmbientHandler`, 3 × `register_*_handler` | **none** |
| `src/capability/session.rs` | 306 | `SESSION_NAMESPACE`, `SessionCapabilitySink`, `SessionCapabilityRegistry` | **none (module not compiled)** |
| `src/capability/lazy_load.rs` | 45 | `pub(super) fn lookup` | **none (transitively dead)** |
| `src/capability/production_init.rs` | 16 | `build_capability_policy` | yes — 5 production call sites |
| `src/capability/cache_policy.rs` | 119 | `ResolverConfig`, `ResolverCache`, `EvictionPolicy` | **none (transitively dead)** |
| `src/capability/constraints.rs` | 493 | `Constraint`, `ConstraintSet`, `ConstraintContext`, `ResourceConstraint`, `TimeConstraint`, `ConditionalConstraint` | yes — `src/config/types/policies/capability_constraints.rs:67,75,80`; `src/capability/zahir_facade.rs:145,223` |
| `src/capability/grant.rs` | 892 | `Grant`, `GrantId`, `GrantRegistry`, `GrantChange`, `GrantError` | **none** |

---

### SW-01 — `src/capability/session.rs` is an orphan module that never compiles into the binary
- **Form:** 6 (never-compiled far-end)
- **Producer:** `src/capability/session.rs:26` — `pub const SESSION_NAMESPACE: &str = "aleph/sessions"`; `:68` — `pub trait SessionCapabilitySink`; `:79` — `pub struct SessionCapabilityRegistry`; `:150` — `impl SessionCapabilitySink for SessionCapabilityRegistry`
- **Consumer:** **no consumer found** (grepped: `capability::session`, `SessionCapabilitySink`, `SessionCapabilityRegistry`, `SESSION_NAMESPACE`, `aleph/sessions` → every hit is inside `src/capability/session.rs` itself)
- **Live caller search:**
  - `grep -n "^pub mod \|^mod " src/capability/mod.rs` → module list is `src/capability/mod.rs:35-74` (+ `#[cfg(test)]` blocks at `:46`, `:55`, `:65`, `:71`, `:77`, `:80`). **`session` is absent.** The whole file is dead weight in the tree.
  - `grep -rn "capability::session\|SessionCapabilitySink\|SessionCapabilityRegistry\|SESSION_NAMESPACE" --include=*.rs .` → hits only inside `src/capability/session.rs`.
  - `grep -rn "aleph/sessions" --include=*.rs --include=*.md .` → only `src/capability/session.rs:26`.
- **Extra proof it could not compile even if declared:** `src/capability/session.rs:129-132` builds
  `metadata: ProjectionMetadata { surface: Some("session".to_string()), attributes }`,
  but `ProjectionMetadata` (`src/capability/descriptor.rs:117-120`) has **only** `pub surface: Option<String>` — there is no `attributes` field (`grep -n "attributes" src/capability/descriptor.rs` → empty). `session.rs:246,249` also indexes `descriptor.metadata.attributes["id"]`, a field that does not exist.
- **Provenance:** commit `858a0c0cf` "feat(phase5): Task 3 + Task 4 - SessionKey alignment and SessionCapability registration" touched **only** `src/capability/session.rs` (306 lines) and a plan doc. It did **not** add the `mod` declaration, did **not** add `ProjectionMetadata.attributes`, and did **not** add the "SessionManager hooks: get_or_create, close, delete, stop" the commit message claims (no `src/session/manager.rs` exists; `src/session/` holds `actor.rs`, `store.rs`, `service.rs`, …).
- **Symptom:** the plan doc `docs/superpowers/plans/2026-11-05-phase6-capability-system.md:22` advertises "Session Capability | Task 4 | ✅", but the capability layer has **no** session capability at runtime — not a broken wire, a wire that was never run. The file will silently rot (it references `ProviderId`, `VisibilityScope { session: … }` shapes that may drift further).
- **Hypothesis:** **CUT** — either wire it (declare the module, add `ProjectionMetadata.attributes`, add the session-manager hooks) or delete the file and the "✅" claim. As it stands it is a 306-line lie in the tree.
- **Confidence:** HIGH

### SW-02 — `GrantRegistry` has zero production consumers; the doc's "resolve time through `OwnershipTree`" validation does not exist
- **Form:** 1 (no consumer) + 2 (stub far-end)
- **Producer:** `src/capability/grant.rs:144` — `pub struct GrantRegistry`; `:158` `pub fn new(max_depth)`; `:190` `pub fn create_grant`; `:424` `pub fn validate_grant_chain`; `:470` `pub fn migrate_session`; `:113` `pub enum GrantChange`; `:175` `pub fn subscribe`
- **Consumer:** **no consumer found** (grepped: `GrantRegistry`, `GrantChange`, `validate_grant_chain`, `capability::grant`, `crate::capability::grant`)
- **Live caller search:**
  - `grep -rn "GrantRegistry" --include=*.rs .` → **only** `src/capability/grant.rs` and `src/capability/grant_integration_tests.rs`.
  - `src/capability/grant_integration_tests.rs` is test-only: `src/capability/mod.rs:80-81` declares `#[cfg(test)] mod grant_integration_tests;`.
  - `grep -rn "validate_grant_chain" --include=*.rs .` → `src/capability/grant.rs:424,839,857` (own `#[cfg(test)] mod tests`, which starts at `src/capability/grant.rs:557`) + `src/capability/grant_integration_tests.rs:93,106,435,444`. **No production caller.**
  - `grep -rn "GrantChange" --include=*.rs .` → `src/capability/grant.rs` only. `GrantRegistry::subscribe` has **zero** subscribers, so even the broadcast channel has no listener.
- **Stub-far-end evidence (the doc lies):** `src/capability/grant.rs:20` says "Grant validation happens at resolve time through [`OwnershipTree`]". But `src/capability/grant.rs:34` imports only `use crate::capability::ownership::{OwnerGeneration, VisibilityScope};` — **`OwnershipTree` never appears in `grant.rs`**, and no resolve path (`src/capability/resolver.rs`, `src/capability/zahir_facade.rs`, `src/capability/facade.rs`, `src/capability/ownership.rs`) calls into the grant module (`grep -rn "grant" src/capability/ownership.rs src/capability/resolver.rs src/capability/zahir_facade.rs src/capability/facade.rs` → no hit).
- **Adjacent confirmation:** `src/capability/policy.rs:333` "Different session: requires explicit grant (not implemented in this phase)"; `src/capability/policy.rs:342` "Cross-session grants are deferred to a later phase" — and `check_cross_session` hard-fails with `PolicyDenial::VisibilityMismatch`.
- **Symptom:** cross-session capability delegation is impossible in production. `GrantRegistry` is a complete, tested, 892-line implementation of a feature the runtime never consults; a reviewer reading `grant.rs`'s header would believe resolve-time grant enforcement exists. AGENTS.md's "Capability Phase 4 入口" deferred list does **not** mention grants, so this is not covered by the documented deferral.
- **Hypothesis:** **CUT** — the grant subsystem is either meant to be live (then it must be called from the resolve path) or it must be listed in the constitution's deferred set and its doc header corrected.
- **Confidence:** HIGH

### SW-03 — The whole ambient capability family (`aleph/ambient`) has zero consumers
- **Form:** 1 (no consumer)
- **Producer:** `src/capability/ambient.rs:17` — `pub const AMBIENT_NAMESPACE: &str = "aleph/ambient"`; `:58` — `pub trait AmbientCapability`; `:174` — `pub struct AmbientHandler`; `:185` — `impl LegacyCapabilityHandler for AmbientHandler`; `:208` — `kind: CapabilityKind::Ambient`; `:223`/`:232`/`:242` — `pub fn register_process_handler` / `register_user_handler` / `register_environment_handler`
- **Consumer:** **no consumer found** (grepped: `AMBIENT_NAMESPACE`, `AmbientCapability`, `AmbientHandler`, `register_process_handler`, `register_user_handler`, `register_environment_handler`, `ambient::`, `aleph/ambient`)
- **Live caller search:**
  - `grep -rn "register_process_handler\|register_user_handler\|register_environment_handler" --include=*.rs .` → definitions at `ambient.rs:223,232,242`; call sites at `ambient.rs:315,340` — **both inside `ambient.rs`'s own `#[cfg(test)] mod tests` (starts at `ambient.rs:253`)**.
  - `grep -rn "ambient::" --include=*.rs .` → **empty**.
  - `grep -rn "AMBIENT_NAMESPACE\|AmbientHandler\|AmbientCapability" --include=*.rs .` → all hits inside `src/capability/ambient.rs`.
  - `grep -rn "aleph/ambient" --include=*.rs --include=*.md .` → only `src/capability/ambient.rs:17`.
- **Registry-parity check (the register arm exists, the caller does not):** `LegacyCapabilityHandlerRegistry` *does* have production registrars — `src/capability/terminal.rs:237` and `src/capability/ui.rs:223` both call `registry.register_handler(...)`. `ambient.rs` never does. So the "legacy handler registry" has two live arms and one dark arm.
- **Kind-production check:** `grep -rn "CapabilityKind::Ambient" --include=*.rs .` → `src/capability/ambient.rs:208` (the only producer), `src/capability/backend.rs:473` (test fixture), `src/capability/ambient.rs:318` (test), `src/capability/events_tests.rs:32` (test), and `src/gateway/channel_registry/binding.rs:179` — a kind→string *match arm*, not a producer. **No production descriptor ever carries `CapabilityKind::Ambient`.**
- **Symptom:** `aleph/ambient/process.id`, `aleph/ambient/user.id`, `aleph/ambient/environment.<NAME>` are unreachable capabilities. `ProcessId::current()`, `UserId::current()` and `EnvironmentVariable::new()` (`ambient.rs:228,235,246`) are only reachable from the file's own tests. An agent that asks for the ambient surface is told "not found" — the same shape as a client ghost, but with no client call to point at.
- **Hypothesis:** **CUT**
- **Confidence:** HIGH

### SW-04 — `CapabilityResolver` + `cache_policy.rs` + `lazy_load.rs` are transitively dark
- **Form:** 1 (no consumer) for the subsystem; the sub-modules are collateral
- **Producer:** `src/capability/resolver.rs:15` — `pub struct CapabilityResolver`; `src/capability/resolver.rs:25` — `ResolverConfig::default()`; `src/capability/cache_policy.rs:20` — `pub struct ResolverConfig`; `:31` — `pub struct ResolverCache`; `:11` — `pub enum EvictionPolicy`; `src/capability/lazy_load.rs:8` — `pub(super) fn lookup`
- **Consumer:** **no production consumer found**
- **Live caller search:**
  - `grep -rn "CapabilityResolver" --include=*.rs .` → `src/capability/resolver.rs`, `src/capability/resolver_tests.rs`, `src/capability/resolver_source_tests.rs`, `src/capability/resolver_audited_tests.rs`, `benches/capability_resolver.rs:13,125`, and the `#[cfg(test)]` include at `src/capability/resolver.rs:130` (`#[path = "resolver_tests.rs"]`). **No production constructor.**
  - `grep -rn "lazy_load::lookup\|lookup(" src/capability/resolver.rs` → the sole caller is `src/capability/resolver.rs:118`, inside the dead resolver.
  - `grep -rn "ResolverCache" --include=*.rs .` → `src/capability/resolver.rs:6,17,34` and `src/capability/lazy_load.rs:4,11` only.
  - `grep -rn "ResolverConfig\|EvictionPolicy" --include=*.rs .` → `ResolverConfig::default()` at `src/capability/resolver.rs:25`; `with_config` has **zero** production callers.
- **Production path actually used instead:** `ZahirFacade` — `src/gateway/mcp_face/mod.rs:224` (`Arc<crate::capability::zahir_facade::ZahirFacade>`) and `src/gateway/pty/manager.rs:1445`.
- **Symptom:** the resolver's tunable knobs (`ResolverConfig.max_entries`, `eviction`, `max_concurrent_loads`) are never configured in production — they are `#[derive(Default)]` literals in a dead module. An operator reading `cache_policy.rs` would believe there is a bounded metadata cache with LRU/FIFO eviction and a concurrency ceiling; there is not one on the live path.
- **Hypothesis:** **CUT**
- **Confidence:** HIGH

### SW-05 — The activation-observation path is dark in production: `CapabilityAttached` / `CapabilityDetaching` are never emitted
- **Form:** 1 (no consumer) + 2 (stub far-end) — a two-end wire where the producer arm is gated behind a test-only constructor
- **Producer:** `src/capability/activation.rs:161-167` — `CapabilityEvent::CapabilityAttached { id }` (inside `attach()`); `src/capability/activation.rs:291-297` — `CapabilityEvent::CapabilityDetaching { id }` (inside `detach()`); `src/capability/activation.rs:251-274` — `observe_failure()` → `CapabilityEvent::PolicyViolation` (construct at `:270`)
- **Consumer (alive):** `src/capability/audit.rs:69` — `CapabilityEvent::CapabilityAttached { .. } => "attached"`; `:71` — `CapabilityEvent::CapabilityDetaching { .. } => "detaching"`; `:92` and `:94` classify both as audit-worthy event kinds
- **Live caller search:**
  - `grep -rn "CapabilityEvent::CapabilityAttached" --include=*.rs src/` → **only** `src/capability/activation.rs:163` (producer) + `src/capability/audit.rs:69,92` (consumer).
  - `grep -rn "CapabilityEvent::CapabilityDetaching" --include=*.rs src/` → **only** `src/capability/activation.rs:293` (producer) + `src/capability/audit.rs:71,94` (consumer).
  - `grep -rn "ActivationEventHook" --include=*.rs src/` → `src/capability/events.rs:910` (struct), `:918` (`new`), `:962` (`impl ActivationHook`), `src/capability/activation.rs:10` (import), `:89` (field), `:115` (the single construction site).
  - `src/capability/activation.rs:115` is inside `CapabilityActivationController::new_observed` (`:107-121`). `grep -rn "new_observed" --include=*.rs src/` → **exactly one** call site: `src/capability/events_tests.rs:290`. `events_tests.rs` is pulled in via `#[path = "events_tests.rs"]` under `#[cfg(test)]` at `src/capability/events.rs:986` → **test-only**.
  - The production controller is built at `src/capability/policy_runtime.rs:147`: `CapabilityActivationController::new(None, binding.generation, ConstraintSet::default())` — and `new` sets `events: None` (`src/capability/activation.rs:100`). Every `if let Some(events) = &self.events` guard (`activation.rs:161`, `:223`, `:290`, `:304`, `:253`) therefore evaluates to false in production.
- **Symptom:** the audit sink (`src/capability/audit.rs`) has two event kinds it can label and persist, but production never delivers them — capability attachment and detachment leave **no audit trail**. `CapabilityEvent::PolicyViolation` from the activation path (`activation.rs:253-269`) is likewise unreachable, even though `src/capability/policy.rs`/`policy_runtime.rs` actively deny calls. (Contrast: `CapabilityEvent::CapabilityActivated` *is* produced in production via a different producer, `src/capability/events.rs:509`.)
- **Hypothesis:** **CUT**
- **Confidence:** HIGH

### SW-06 — `ActivationHook::before_activation` has no production implementation and no production caller
- **Form:** 1 (no consumer)
- **Producer:** `src/capability/activation.rs:33-48` — `pub trait ActivationHook` with `before_activation` (`:40`), `after_activation` (`:45`), `after_revocation` (`:48`); `:185` — `pub fn activate_with_hook<H: ActivationHook>`; `:312` — `pub fn revoke_with_hook<H: ActivationHook>`
- **Consumer:** **no production consumer found**
- **Live caller search:**
  - `grep -rn "activate_with_hook" --include=*.rs src/` → `src/capability/activation.rs:181` (internal: `activate()` passes `&NoopActivationHook`), `:185`, `:191`, `:195`, `:593`, `:608` (own test mod), `src/capability/activation_reentry_tests.rs:64,95,133`, `src/capability/events_tests.rs:544`. **Every external call site is a test.**
  - `grep -rn "impl .*ActivationHook for" --include=*.rs src/` → `src/capability/activation.rs:53` (`NoopActivationHook` — all-default no-op), `:621` (`RecordingHook`, test), `:644` (`RejectingHook`, test), `src/capability/activation_reentry_tests.rs:52,84,118` (test), `src/capability/events_tests.rs:531` (test), and `src/capability/events.rs:962` (`impl ActivationHook for ActivationEventHook` — the only non-test impl, and it implements **only** `after_activation`/`after_revocation`, not `before_activation`; it is itself reachable only through the test-only `new_observed`, see SW-05).
  - `grep -rn "revoke_with_hook" --include=*.rs src/` → `src/capability/activation.rs:312` (definition), `:543` (test), `src/capability/events_tests.rs:550` (test).
- **Symptom:** the "a hook can veto an activation *before* the lifecycle transition" extension point — the mechanism a plugin/extension would use to pre-authorize or refuse a capability — is unreachable in production. `activate()` hard-wires `&NoopActivationHook` (`activation.rs:181`), so `before_activation` can never fail outside tests.
- **Hypothesis:** **CUT**
- **Confidence:** HIGH

### SW-07 — `src/tools/scoped/` per-name activation map is never populated in production; `validate_compatibility_activation` always returns `Ok(())`
- **Form:** 1 (no consumer) on a *live* function — the consumer exists but is unreachable
- **Producer:** `src/tools/scoped/builder.rs:88` — `pub fn with_capability_activation(name, Arc<dyn CapabilityActivation>)`; `:93` — `self.capability_activations.insert(...)` into `src/tools/scoped/mod.rs:121-122`; `src/tools/scoped/builder.rs:104` — `pub fn with_capability_constraint_context(source)`
- **Consumer:** `src/tools/scoped/dispatch.rs:1533` — `fn validate_compatibility_activation(&self, name, current_generation)`; `:1538-1539` — `let Some(activation) = self.capability_activations.get(name) else { return Ok(()); };`
- **Live caller search:**
  - `grep -rn "with_capability_activation" --include=*.rs src/` → `src/tools/scoped/memory_runtime_tests.rs:203,231,254` and `src/tools/scoped/tests.rs:5109,5265` — **both files are `#[cfg(test)]`**. No production builder call site invokes it.
  - `grep -rn "with_capability_constraint_context" --include=*.rs src/` → `src/tools/scoped/memory_runtime_tests.rs:204,232,255` and `src/tools/scoped/tests.rs:5110,5223,5266` — **both `#[cfg(test)]`**.
  - `grep -rn "capability_activations" --include=*.rs src/` → `builder.rs:33` (empty init), `builder.rs:93` (insert), `mod.rs:121` (declaration), `dispatch.rs:1538` (read). **No other writer exists.**
- **So what:** in production the map is **permanently empty**, and `validate_compatibility_activation` takes the `else` arm on every single tool invocation → the handler-seam runtime restriction never runs.
- **Mitigating context (why this may be intentional):** `src/tools/scoped/builder.rs:79-81` says "Compatibility opt-in … Production rules are installed through the shared Policy, not this map", and `src/tools/scoped/dispatch.rs:1530-1532` says "Preserve the old opt-in API … Production does not install a per-name policy here." The live path is `PolicyRuntime` (`src/capability/policy_runtime.rs:147-197` → `rule.constraints.evaluate(context)` → `controller.activate(...)` → `controller.validate_runtime(...)`), reached from `src/capability/policy.rs:114` (`PolicyEngine.runtime`).
- **Symptom:** the public builder API and the `Arc<dyn CapabilityActivation>` type-erasure it needs exist **only** to feed a map that production never fills; a reader cannot tell which of the two restriction mechanisms is authoritative without reading four doc comments. The `dispatch.rs:1533` function is a live production function whose only reachable behaviour is `Ok(())`.
- **Hypothesis:** **DECIDE** — either mark these two builder methods `#[doc(hidden)]`/test-only, or wire them. Today the code is *documented* as a compat shim, but the shim is not marked as such at the type level.
- **Confidence:** MEDIUM (the doc comments are explicit; the risk is reader confusion, not a missing authorization)

### SW-08 — `ProjectionMetadata.surface` is never populated in production (2026-10-30 finding **C03-8**, still open)
- **Form:** 3 (inert field) / 1 (no consumer)
- **Producer:** `src/capability/descriptor.rs:117-120` — `pub struct ProjectionMetadata { pub surface: Option<String> }`; the doc says *"`None` = not projected anywhere yet"* and names the expected values (`"mcp"`, `"acp-json"`, `"state-database"`)
- **Consumer:** **no production populator found**
- **Live caller search:**
  - `grep -rn "surface: Some\|surface: None" --include=*.rs .` → 3 hits for `surface: Some(`:
    - `src/gateway/channel_registry/capability.rs:56` — inside `pub(crate) async fn project_bound_capabilities` (`capability.rs:47`), whose **only** caller is `#[tokio::test] projections_share_filtered_ids_and_name_each_target` at `src/gateway/channel_registry/capability.rs:415` → **test-only**.
    - `src/gateway/channel_registry.rs:2773` — inside the test fn `stage_b_contract` → **test-only**.
    - `src/capability/session.rs:130` — inside the orphan module of SW-01 → **never compiled**.
  - Every production descriptor uses `ProjectionMetadata::default()` (i.e. `surface: None`): `src/capability/zahir_facade.rs:2077`, `src/memory/runtime/mod.rs:407`, `src/gateway/channel_registry/binding.rs:1247,1356`, `src/gateway/channel_registry/capability.rs:232`.
  - `grep -rn "\.project(" --include=*.rs src/` → `Zahir::project` (`src/capability/facade.rs:409`) has **zero production callers**: `src/extension/lifecycle.rs:2576,2653` are inside `#[cfg(test)] mod tests` (starts at `src/extension/lifecycle.rs:1845`); `src/capability/zahir_facade.rs:2314,2722` are inside `#[cfg(test)] mod tests` (starts at `src/capability/zahir_facade.rs:2115`); `src/capability/resolver_tests.rs:78` is a test; `src/capability/events.rs:904` is the `impl Zahir for AuditedZahir` forwarder, and `AuditedZahir::new` is only called from test files (`resolver_tests.rs:171`, `events_scoped_tests.rs:686`, `events_tests.rs:126,578,683`, `resolver_audited_tests.rs:36`).
- **Symptom:** the field's own doc defines `None` as "not projected anywhere yet", but in production **every** projected capability carries `None` regardless of whether it *was* projected — the signal cannot distinguish "not projected" from "projected, surface unknown". The surface taxonomy (`"mcp"` / `"acp-json"` / `"state-database"`) exists only as prose.
- **Hypothesis:** **DECIDE** — this is the exact question the 2026-10-30 report deferred (C03-8): either populate `surface` at the projection sites, or delete the field and its doc. It has not moved in 3 weeks.
- **Confidence:** HIGH (facts) / the *decision* is a product call

### SW-09 — `CapabilityKind` has 12 variants while the constitution documents 9; the `UI` compatibility alias has no caller
- **Form:** 5 (name / contract drift) + 1 (no consumer)
- **Producer:** `src/capability/descriptor.rs:27-40` — `pub enum CapabilityKind { Tool, Ambient, Terminal, Ui, Skill, Agent, Task, Resource, EventSource, Subscription, Plugin, Hook }` (**12**); `:41` — `pub const UI: Self = Self::Ui;` (doc: "Compatibility spelling for callers that use the acronym from the wire contract")
- **Consumer:** **no consumer found for `CapabilityKind::UI`** (grepped: `CapabilityKind::UI` → zero hits repo-wide, distinct from `CapabilityKind::Ui` which has hits)
- **Live caller search:**
  - `grep -rn "CapabilityKind::UI" --include=*.rs .` → **empty** (zero callers, production or test).
  - The 12 variants vs. the constitution: `AGENTS.md` "Capability Phase 4 入口" states "**9 类 `CapabilityKind`**". The enum now carries 12.
- **Kinds that are never produced (context, NOT reported as a severed wire):** `Agent`, `Task`, `EventSource`, `Subscription`, `Plugin`, `Hook` appear only as kind→string match arms in `src/gateway/channel_registry/binding.rs:183-189` and are never produced by a backend. AGENTS.md explicitly defers "其余 kind backend", so this is **covered by the documented deferral** and is reported here only as context for the 9-vs-12 drift.
- **Symptom:** a reader who trusts the constitution's "9 kinds" mis-maps the enum; the `UI` alias advertises a wire-contract spelling (`UI` vs `Ui`) that no producer or consumer uses.
- **Hypothesis:** **DECIDE** — update `AGENTS.md` to 12 (or 9 if 3 are meant to go), and either wire `CapabilityKind::UI` or delete the alias.
- **Confidence:** HIGH

### SW-10 — The lease-TTL / expiry machinery in `lifecycle.rs` is unreachable in production
- **Form:** 1 (no consumer) — a whole safety mechanism with no live producer
- **Producer:** `src/capability/lifecycle.rs:61` — `pub struct Lease`; `:73` `Lease::new`; `:83` `Lease::is_expired`; `:90` `Lease::time_remaining`; `:108` `Lease::renew`; `:150` — `CapabilityLifecycle::new(ttl, …)`; `:181` `CapabilityLifecycle::validate_activation`; `:206` `CapabilityLifecycle::validate_lease`; `:228` `CapabilityLifecycle::renew_lease`; `:252` `CapabilityLifecycle::time_remaining`; `:258` `CapabilityLifecycle::owner_generation`
- **Consumer:** **no production consumer found**
- **Live caller search:**
  - `grep -rn "renew_lease\|Lease::renew\|time_remaining" --include=*.rs src/` → `renew_lease` defined at `lifecycle.rs:228`, called only from `lifecycle.rs` internals/tests; `Lease::renew` (`lifecycle.rs:108`) only from `renew_lease`; `Lease::time_remaining` (`lifecycle.rs:90`) only from `CapabilityLifecycle::time_remaining` (`lifecycle.rs:252`); `CapabilityLifecycle::time_remaining` has **zero** callers repo-wide.
  - `grep -rn "CapabilityLifecycle::new" --include=*.rs src/` → `src/capability/lifecycle.rs:153` (internal) and `src/capability/activation.rs:98` (`CapabilityActivationController::new`).
  - **The decisive fact:** the only production construction of the controller is `src/capability/policy_runtime.rs:147` — `CapabilityActivationController::new(None, binding.generation, ConstraintSet::default())` — **`ttl = None`**. With `ttl == None`, `Lease::is_expired()` (`lifecycle.rs:83`) is always `false`, so `LifecycleError::LeaseExpired` (`lifecycle.rs:126`) can never be raised by `validate_activation` (`lifecycle.rs:181`) or `validate_lease` (`lifecycle.rs:206`).
  - `grep -rn "ttl" src/config/types/policies/capability_constraints.rs` → **no config field** exposes a lease TTL. The knobs are `time` / `resource` / `required_conditions` only.
  - `CapabilityLifecycle::owner_generation` (`lifecycle.rs:258`) → only `src/capability/lifecycle.rs:363` (test).
  - `Lease` the type → used only inside `src/capability/lifecycle.rs`.
- **Symptom:** the "a lease expires and the capability becomes non-invocable" behaviour modelled by `Lease`, `LifecycleError::LeaseExpired`, `renew_lease` and `time_remaining` is **unreachable in production** — the live path always creates a TTL-less lease. Time-based expiry must instead be handled by the ownership-generation bump; the code and its doc do not say so.
- **Hypothesis:** **CUT**
- **Confidence:** HIGH

### SW-11 — `CapabilityActivationController::enforce_runtime` / `from_lifecycle` / `lifecycle()` have zero callers
- **Form:** 1 (no consumer)
- **Producer:** `src/capability/activation.rs:278` — `pub fn enforce_runtime` (doc: "Alias for [`Self::validate_runtime`]"); `:124` — `pub fn from_lifecycle`; `:147` — `pub fn lifecycle`
- **Consumer:** **no consumer found**
- **Live caller search:**
  - `grep -rn "enforce_runtime" --include=*.rs src/` → **only** `src/capability/activation.rs:278,283` (definition + self-call). Zero callers anywhere, production or test.
  - `grep -rn "from_lifecycle" --include=*.rs src/` → **only** `src/capability/activation.rs:124`. Zero callers.
  - `grep -rn "\.lifecycle()" --include=*.rs src/` → `src/gateway/server/browser_live_runtime.rs:322,344,355` and `src/builtin_tools/browser_tools/present_tests.rs:199,871,928` are a **different type**; `CapabilityActivationController::lifecycle()` has zero callers.
- **Symptom:** three public entry points on the production controller type exist with no user; the "alias" `enforce_runtime` in particular suggests two spellings of the same call that both exist to satisfy an unimplemented caller.
- **Hypothesis:** **CUT**
- **Confidence:** HIGH

### SW-12 — `Constraint::time` and `Constraint::condition` have no production callers; `ConstraintContext::with_active_resources` is reached only through `now()`
- **Form:** 1 (no consumer)
- **Producer:** `src/capability/constraints.rs:225` — `pub fn time` (doc: "Alias for [`Self::time_window`]"); `:235` — `pub fn condition` (doc: "Require a named condition to be true"); `:45` — `pub fn with_active_resources`
- **Consumer:** **no consumer found** (for `time` / `condition` in production)
- **Live caller search:**
  - `grep -rn "Constraint::time(" --include=*.rs src/` → **empty** (zero callers).
  - `grep -rn "Constraint::condition\b" --include=*.rs src/` → `src/tools/scoped/memory_runtime_tests.rs:223`, `src/tools/scoped/tests.rs:5126`, `src/capability/activation.rs:425,451,468,475,503,540,568` (test mod), `src/capability/events_tests.rs:558`, `src/capability/constraints.rs:462` (test). **The production config path uses `Constraint::conditional`** — `src/config/types/policies/capability_constraints.rs:80` — so `Constraint::condition` is test-only.
  - `grep -rn "Constraint::time_window\|Constraint::resource_limit\|Constraint::conditional" --include=*.rs src/` → each has exactly one **production** construction site: `src/config/types/policies/capability_constraints.rs:67,75,80`. (These three are healthy — see SW-13.)
  - `with_active_resources` (`constraints.rs:45`) → `src/capability/activation.rs:413,557` (tests) and `src/capability/constraints.rs:35` (inside `now()`). Production reaches it only through `ConstraintContext::now(n)` — `src/capability/zahir_facade.rs:145,223` (`:38` in constraints.rs).
- **Symptom:** two constraint constructors are dead. `Constraint::condition` is the more dangerous one: it is the *natural* spelling for a config author ("require condition X"), it is exercised by every unit test, and the production config compiler uses the two-argument `Constraint::conditional` instead — so a test author can write `Constraint::condition("ready")` and be green while production goes through a different constructor with an explicit `expected` flag.
- **Hypothesis:** **CUT**
- **Confidence:** HIGH

### SW-13 — `Constraint` variant `Time` is reachable, but `ResourceConstraint`'s unit accounting has a single live unit
- **Form:** 3 (inert config) — *partial*, reported for completeness
- **Producer:** `src/capability/constraints.rs:153-161` — `ResourceConstraint::evaluate`; `src/config/types/policies/capability_constraints.rs:38-40` — `pub enum ResourceUnit { ActiveCalls, PtySessions }`
- **Consumer:** `src/capability/policy_runtime.rs:362,366,400,403,446`; `src/capability/zahir_facade.rs:131-132,179-180`
- **Live caller search:** `grep -rn "ResourceUnit" --include=*.rs src/` → production hits confirmed above; `Constraint::resource_limit` is constructed once, in `src/config/types/policies/capability_constraints.rs:75`.
- **Symptom:** *not a severed wire.* Recorded here only to note that `ResourceConstraint` counts **occupied slots excluding the candidate** (`constraints.rs:153-161`) while the config's `max_active` is an operator knob — the two readings of "max active" differ by one. That is a semantic-clarity item, not a cut wire.
- **Hypothesis:** **CONNECT**
- **Confidence:** MEDIUM

### SW-14 — `ProjectionMetadata` / `CapabilityFacet(s)` / `CapabilityDescriptor.services` are all consumed in production
- **Form:** CONNECT (re-verified healthy)
- **Producer:** `src/capability/descriptor.rs:56-68` (`CapabilityFacet`), `:71-99` (`CapabilityFacets`), `:117-120` (`ProjectionMetadata`), `:126-140` (`CapabilityDescriptor`)
- **Consumer:** `src/gateway/channel_registry/binding.rs:204` (`descriptor.services.iter()`), `:213-220` (`descriptor.supports(facet)` → tag), `src/gateway/channel_registry/capability.rs:209` (`descriptor.supports(facet)`), `src/gateway/handlers/capabilities.rs:50-51` (`descriptor.facets.contains(CapabilityFacet::Callable / ExternalEffect)`), `src/capability/backend.rs:368` (`to_descriptor`, `pub(super)`), `src/capability/events.rs:580`
- **Live caller search:** `grep -rn "CapabilityFacet::" --include=*.rs src/` and `grep -rn "\.services\b" --include=*.rs src/` → production hits as listed (plus tests).
- **Symptom:** none — the descriptor's facet/service axes reach the wire (`/capabilities` handler, channel binding tags).
- **Hypothesis:** **CONNECT**
- **Confidence:** HIGH

### SW-15 — `build_capability_policy` and the `CapabilitySlot` / `ALL_SLOTS` machinery are live
- **Form:** CONNECT
- **Producer:** `src/capability/production_init.rs:9` — `pub fn build_capability_policy(config: &Config, tree: Arc<OwnershipTree>) -> Result<Arc<PolicyEngine>, PolicyDenial>`; `src/capability/mod.rs:334` — `pub static ALL_SLOTS` (49 entries)
- **Consumer:** `src/bin/aleph-server/commands/start/mod.rs:298`; `src/gateway/handlers/tools_invoke.rs:390,436,453`; `src/gateway/server/connection/mod.rs:1866`; `src/gateway/mcp_face/mod.rs:888`; `src/diagnostics/checks/capability_wiring.rs:122,250,288`; `src/spend/mod.rs:629` (`static GLOBAL_POLICY: MutableCapabilitySlot<SpendPolicy>`), `:639` `install_policy`, `:666` `update_policy`, `:713` `global_policy_slot`
- **Live caller search:** `grep -rn "build_capability_policy" --include=*.rs src/` → 5 production call sites as listed; `grep -rn "ALL_SLOTS" --include=*.rs src/` → `src/diagnostics/checks/capability_wiring.rs:122,250,288`.
- **Symptom:** none.
- **Hypothesis:** **CONNECT**
- **Confidence:** HIGH

### SW-16 — The `ConstraintSet` config compiler and `OwnershipTree::generation` are live
- **Form:** CONNECT
- **Producer:** `src/capability/ownership.rs:568` — `pub fn generation(&self, capability, visibility) -> Option<OwnerGeneration>`; `src/config/types/policies/capability_constraints.rs:44` — `pub constraints: ConstraintSet`
- **Consumer:** `src/gateway/channel_registry/binding.rs:428`; `src/gateway/channel_registry/capability.rs:144`; `src/capability/ownership.rs:568` is also used by `src/capability/facade.rs`/backend lease paths; `ConstraintSet` is compiled by `src/config/types/policies/capability_constraints.rs:67,75,80` and evaluated in `src/capability/policy_runtime.rs:174-183`
- **Live caller search:** `grep -rn "\.generation(&" --include=*.rs src/` → `src/gateway/channel_registry/binding.rs:428`, `src/gateway/channel_registry/capability.rs:144` (production) + tests; `grep -rn "Constraint::" src/config/types/policies/capability_constraints.rs` → all three variants compiled.
- **Symptom:** none.
- **Hypothesis:** **CONNECT**
- **Confidence:** HIGH

---

## 非连线类静态缺陷

Static defects that are not severed wires but that the same sweep surfaced.

### NS-01 — `ProjectionMetadata`'s doc names a taxonomy the struct does not encode
- `src/capability/descriptor.rs:112-120`: the struct doc says "The `projection_only` / `notification_only` markers are enforced at the facade layer (Task 4), not encoded here." and the field doc lists the surfaces `"mcp"`, `"acp-json"`, `"state-database"`. The struct carries **only** `surface: Option<String>` — there is no `projection_only` / `notification_only` marker anywhere in the slice, and no code in the slice (or in `src/capability/facade.rs`) enforces them. AGENTS.md likewise says "`StateDatabase` / ACP JSON / MCP 类型与工具视图是 projection-only，`GlobalBus` 是 notification-only" — a rule stated in three places and implemented in none.

### NS-02 — `src/capability/grant.rs` header points at a type that is not imported
- `src/capability/grant.rs:20` writes "[`OwnershipTree`]" as an intra-doc link, but `OwnershipTree` is not in scope in that file (import at `:34` is `use crate::capability::ownership::{OwnerGeneration, VisibilityScope};`). A `cargo doc` build will either fail the link or resolve it to nothing. The prose is the same claim audited in SW-02.

### NS-03 — `CapabilityKind` count in the constitution is stale
- `AGENTS.md` "Capability Phase 4 入口" says "9 类 `CapabilityKind`"; `src/capability/descriptor.rs:27-40` defines **12**. The kind→string table in `src/gateway/channel_registry/binding.rs:183-189` lists them individually, so the code is self-consistent — the constitution is the stale side.

### NS-04 — `MAX_DELEGATION_DEPTH` is a hard-coded constant with no config surface
- `src/capability/grant.rs:121` — `const MAX_DELEGATION_DEPTH: u32 = 5;`, used only by `GrantRegistry::default()` (`grant.rs:150`). No config key, no environment override. (Moot while SW-02 stands, but it will need a knob the moment grants go live.)

### NS-05 — `CapabilityKind::UI` compatibility alias is dead weight
- `src/capability/descriptor.rs:41` — `pub const UI: Self = Self::Ui;`, doc "Compatibility spelling for callers that use the acronym from the wire contract." Zero callers repo-wide (see SW-09). Kept here as a distinct note because it is a *wire-contract spelling* with no wire.

### NS-06 — `src/capability/session.rs` references a `SessionManager` API that does not exist in the tree
- The commit that added the file (`858a0c0cf`) claims "SessionManager hooks: get_or_create, close, delete, stop". No `src/session/manager.rs` exists. `src/session/` contains `actor.rs`, `store.rs`, `service.rs` and friends. The claimed integration points were never created.

### NS-07 — `src/capability/ambient.rs` exposes `EnvironmentVariable::from_value` as a public constructor
- `src/capability/ambient.rs:140` — `pub fn from_value(name, value)`, doc "Construct a deterministic source for tests and in-memory bridges." It is a public API on a module with no production consumer (SW-03); combined with `UserId::current()` reading `$USER`/`$USERNAME` from the process environment (`ambient.rs:98-103`), wiring this family later would introduce ambient-environment reads into the capability path with no policy gate. Flag now, before SW-03 is "fixed" by wiring it.

### NS-08 — `Constraint::condition` vs `Constraint::conditional` naming trap
- `src/capability/constraints.rs:235` (`condition`, one-arg, implies `expected = true`) vs `:240` (`conditional`, two-arg, explicit). The production config compiler uses `conditional` (`src/config/types/policies/capability_constraints.rs:80`); every unit test uses `condition`. A test can therefore assert a constraint that production never builds. (Cross-listed from SW-12.)

---

## 回归 / 未修（与既往报告对照）

| Prior finding | Prior report | Status today | Note |
|---|---|---|---|
| **C03-8** — `ProjectionMetadata.surface` never populated | `review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md` | **未修 (still open)** | The only populator (`src/gateway/channel_registry/capability.rs:56`) is still test-only; `Zahir::project` still has zero production callers. → SW-08 |
| **C03-10** — decline-then-install keeps roster stamp `Declined` | same | deferred, unchanged | Not in the slice; not re-verified here. |
| **C08-1** — `validate_lease` / generation | same | reported fixed in the prior report; re-verified healthy | `OwnershipTree::generation` has production callers (`src/gateway/channel_registry/binding.rs:428`, `src/gateway/channel_registry/capability.rs:144`) → SW-16 |
| **C08-4** — reconcile default visibility | same | deferred/reverted; unchanged | Not in the slice. |
| **C04-2**, **C04-3**, **C07-3** | same | open in the prior report; not in this slice | Not re-verified (outside slice). |
| **Full slice** — `src/capability/{census.rs, mod.rs}` | `review-results/severed-wire-2026-08-17/capability/REPORT.md` ("Findings: None") | **still clean for the covered files** | `ALL_SLOTS` grew 46 → **49** entries; the 3 new slots are registered with live accessors (`src/diagnostics/checks/capability_wiring.rs:122,250,288`; `src/spend/mod.rs:629`). The 2026-08-17 report did **not** cover `descriptor.rs`, `lifecycle.rs`, `activation.rs`, `ambient.rs`, `session.rs`, `lazy_load.rs`, `production_init.rs`, `cache_policy.rs`, `constraints.rs`, `grant.rs` — this audit's findings are all in those uncovered files. |

---

## 未覆盖的接缝

Seams this audit did **not** cover, and why.

1. **Consumers outside `src/capability/` that were not grepped exhaustively.** I grepped the whole repo for each symbol, but I did not read `src/gateway/channel_registry/*`, `src/tools/scoped/dispatch.rs`, `src/capability/policy.rs`, `src/capability/policy_runtime.rs`, `src/capability/facade.rs`, `src/capability/backend.rs`, `src/capability/events.rs`, `src/capability/audit.rs`, or `src/capability/ownership.rs` line-by-line. A wire whose producer AND consumer both live in those files, and whose symbol name is unique enough to escape my greps, would be missed.
2. **`src/capability/` files outside the assigned slice.** `backend.rs`, `ownership.rs`, `policy.rs`, `policy_runtime.rs`, `events.rs`, `audit.rs`, `facade.rs`, `projection_host.rs`, `zahir_facade.rs`, `provider*.rs`, `special*.rs`, `terminal.rs`, `ui.rs`, `tool_adapters.rs`, `ui_surface.rs`, `diagnostic_control.rs`, `effect_claim.rs`, `event_audience.rs`, `versioned_registry.rs`, `census.rs` were **not** audited — only grepped as counterparties. In particular I did **not** audit `census.rs`'s roster guard (the `ALL_SLOTS` ↔ accessor parity test) beyond counting entries.
3. **The `src/capability/grant_integration_tests.rs` and `grant.rs` test bodies** were read only far enough to establish that they are `#[cfg(test)]` and to inventory the API; I did not assess whether their assertions actually cover the behaviors they claim.
4. **Non-Rust surfaces.** `docs/superpowers/plans/2026-11-05-phase6-capability-system.md` and `docs/superpowers/plans/2026-11-05-sessionkey-alignment-plan.md` were cited as *claim* sources; I did not audit all plan documents for other "✅" claims that lack code (a plan-vs-code sweep would be a separate task).
5. **Runtime behaviour.** Nothing was executed: no `cargo check`, no `cargo test`, no server boot. Every claim is static (type-level, call-graph-level). A wire that is connected but *ineffective at runtime* (e.g. a constraint that always evaluates true, or a policy that is installed but never consulted) would not be caught by this method. The `ProjectionMetadata.surface` finding (SW-08) is exactly the class where a static "no caller" reading needs runtime confirmation before acting — I marked it DECIDE rather than CUT for that reason.
6. **`desktop/`, `crates/`, `plugins/`, `qa/` subtrees.** Greps were repo-wide (`--include=*.rs .`), so any Rust file anywhere in the tree was covered for the *symbol names* I searched. But I did not perform a *semantic* search (e.g. for a capability registered under a different spelling, or a consumer that reaches the subsystem through a trait object with a different method name).
7. **The `CapabilityKind` → string table in `src/gateway/channel_registry/binding.rs:183-189`.** I confirmed the kinds are match arms and never produced, but did not verify whether the table is exhaustive over the 12 variants or silently maps some kinds to a default string.
8. **`OwnershipTree::register_if_absent` / `reconcile_registry_bindings` / `to_descriptor` fingerprinting.** These are outside the slice; I confirmed they have production callers but did not audit their internal consistency (e.g. whether the fingerprint is recomputed on every descriptor mutation).

---

*Generated 2026-11-20 by a read-only severed-wire audit. No source file was modified.*
