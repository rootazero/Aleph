# Severed-wire audit — `src/canvas`, `src/capability`, `src/clarification` (2026-11-20)

**Scope:** static (no diff) audit of three modules of `alephcore`, run by 9 parallel
read-only `sev-auditor` subagents (one per slice) plus a mechanical exported-symbol
scan and the repository's own wiring guards.

**Baseline:** `main` @ `d57791367` ("merge: sync origin/main (severed-wire audit round 2)").

**Outcome:** 9 slice reports (1318 lines) attached in this directory. Every candidate
is disposed below. 6 modules' worth of dead surface was cut, 12 wiring defects were
fixed, 1 new guard script was added, and 5 items are left as explicit human decisions.

---

## 1. Method

| Stage | Tool | Output |
|---|---|---|
| Semantics orientation | `graphify` over `src/` (`graphify-out/graph.json`, 2865 code files) | node/community context per slice |
| Mechanical seam scan | `scripts/capability_wiring_audit.py` (new, this audit) | 480 declared public items, severed set |
| Existing guards | `scripts/wiring_seams.py`, `scripts/rpc_wiring_audit.py`, `scripts/tool_wiring_audit.py`, `scripts/config_wiring_audit.py` | green (one known false positive: `TimeWindowConfig` — the guard greps by type name, but the value IS consumed via `compile_constraints` → `Constraint::time_window`, `src/capability/constraints.rs:87,123`) |
| Static review | 9 × `sev-auditor` (read-only), slices: `canvas`, `clarification`, `cap-core`, `cap-policy`, `cap-resolve`, `cap-special`, `cap-ownership`, `cap-facade`, `cap-ui` | the 9 `*-REPORT.md` files here |
| Routing / triage | **Jev** (`jev_evaluate`, `typesafe/jev-latest`) | 5 calls: candidate grouping, disposition routing, and the "is this a severed wire or a live contract?" classification for each group (details in §6) |
| Fixes | main worktree, direct commits (no PR) | see §2–§5 |

Every candidate was re-verified against the live tree before it was acted on.
**Several auditor claims were rejected as factually wrong** — they are listed in §5
so the next round does not re-report them.

---

## 2. Cut (dead code removed)

| Item | Location | Why it was safe |
|---|---|---|
| `CapabilityResolver` + `cache_policy.rs` + `lazy_load.rs` (+ their 3 test files, `benches/capability_resolver.rs`) | `src/capability/` | transitively dark: no production construction entry point; the only cross-references were inside the cluster itself |
| `src/capability/session.rs` | `src/capability/session.rs` | orphan module — never declared in `mod.rs`, so it never compiled into the binary |
| `src/capability/terminal.rs` (whole module, 433 lines) | `src/capability/terminal.rs` | the legacy terminal handler had **zero** references outside itself; it only fed the legacy `LegacyCapabilityHandler` path, which production never mounts |
| legacy in-memory UI handler (`UiCapability`, `UiHandler`, `UiElement`, `register_handler`) | `src/capability/ui.rs` | never registered in production; the live parts of `ui.rs` (`descriptor()`, `schema()`, `UiError`) are kept |
| `CapabilityBackend::snapshot` (trait method) | `src/capability/backend.rs` | production reads `ToolBackendAdapter::snapshot_capabilities`; the trait method had only test callers |
| `CapabilityActivationController::{from_lifecycle, enforce_runtime, lifecycle}` | `src/capability/activation.rs` | zero callers (`enforce_runtime` was a documented alias of `validate_runtime`) |
| `ZahirFacade::new_with_memory_and_policy_and_specials` | `src/capability/zahir_facade.rs` | zero callers; production uses the `…_and_events` variant |
| `PolicyRuntime::withdraw_attachment` | `src/capability/policy_runtime.rs` | production revokes by fact-source key (`withdraw_fact_source`), not by attachment |
| `ProjectionHost::mount_with_runtime` | `src/capability/projection_host.rs` | zero callers; it only wrapped `mount_with_runtime_and_events` |
| `graphify-out` (tracked symlink) | repo root | pointed at itself (`/…/Aleph/graphify-out`) → `符号链接的层数过多` for every reader; `.gitignore` already ignores the real directory |

## 3. Narrowed (public → crate-internal / test-only)

`pub(crate)` **and** `#[cfg(test)]` (the item does not exist in a production build, so it
can neither leak nor trigger the dead-code lint): `AuditCollector::query_scoped`,
`ProjectionHost::audit_collector`, `CapabilityHandlerCall::running_call_count`,
`CapabilityActivationController::new_observed`, `PolicyEngine::{with_events, can_delegate,
check_cross_session}`, `CapturedSpecialEntry::canonical_schema`,
`register_canonical_adapters_with_tools`, `AuditedPolicyEngine`, `AuditedZahir`
(+ its `Zahir` trait impl), `ActivationEventHook::new`,
`PolicyRuntime::withdraw_attachment`, `ClarificationManager::{cancel, cancel_abandoned}`,
`ProjectionHost::{mount_with_diagnostics, mount_with_capacity, register_special_tool_adapters}`.

`pub(crate)` only (still reachable from production code): `register_canonical_adapters_with_tools_and_source`.

## 4. Fixed (real wiring defects)

| ID | Defect | Fix |
|---|---|---|
| CAN-02 | `CanvasStore::get` returned the parsed document **without checking `doc.id == id`**, so a directory name and a document id could disagree (`src/canvas/store.rs`) | identity check → `CanvasError::Internal` (the gateway then answers not-found) |
| CAN-03 | the Panel's five `canvas.*` writes carried **no `idempotency_key`**, so they are rejected outright when the operator enables `require_idempotency_key` | `interfaces/webchat/src/api/canvas.rs`: `encode_write` adds a per-call key (stable across a transport retry, unique per logical operation) to `canvas.create/apply/delete/asset.put/selection.set` |
| CAN-04 | `canvas.delete` fell into the generic (read) rate-limit bucket | added to the `RpcWrite` bucket in `src/gateway/rate_limiter.rs` |
| CAN-05 | `CanvasStore::list_entries` swallowed the root `read_dir` failure and returned an empty library (and broke out of the loop mid-way) | now returns `Result<Vec<CanvasListing>, CanvasError>`; both production callers (`src/gateway/handlers/canvas.rs`, `src/builtin_tools/canvas.rs`) map the error instead of showing an empty canvas |
| SD-CLAR-01 | the Webchat client **discarded the clarification request identity**, so an answer to an older question could land on a newer one in the same session | the core contract already carried `PendingClarification.request_id` / `ClarificationResolveParams.expected_request_id` / a `stale` response; the Panel now sends `expected_request_id` and reports a superseded answer instead of resolving the wrong question |
| CUI-D1 | a fast, legitimate render ACK was rejected because `pending.sent` flipped only **after** the publish — the client cannot know the request id before the publish, so the display timed out | `src/capability/ui_surface.rs`: `sent` is set before the publish |
| ZF-01 | tool **unregistrations** were filtered out of scoped change streams (the lookup had already returned `None`), so subscribers never learned that a capability went away | `src/capability/zahir_facade.rs`: the unregistration case is matched before the lookup |
| ZF-02 (partial) | the provider adapter **fabricated** the queried scope's visibility onto provider descriptors | the declared visibility is now returned unchanged (the ownership binding for providers is still open — see §5) |

## 5. Rejected after verification (the auditor was wrong)

| Claim | Reality |
|---|---|
| `CapabilityBackend` has no production consumer | it is used in production (`src/capability/zahir_facade.rs:1292`, `:1607`, `:1609`); only the `snapshot` method was dead |
| `OwnershipTree::{register, revoke, dispose}` are test-only | they are called in production: `src/gateway/channel_registry/capability.rs:260,474,492`, `src/gateway/pty/manager.rs:1503` |
| `CapabilityKind::UI` has no caller | it is used — `src/capability/tool_adapters.rs` registers the UI adapter and its special record |
| `ActivationHook::before_activation` has no production caller | the trait method *is* called in production (`src/capability/activation.rs:203`); only the *implementations* are test-side, i.e. a designed extension point, not a severed wire |
| the `aleph/tools/ui_capability` special record can simply be deleted | it is asserted by `src/capability/special_tests.rs:454` and `src/capability/ui_surface_tests.rs:281`; its `Unavailable` handler is the deliberate "descriptor-only record" pattern (the terminal record is also `Unavailable` until its source slot binds). **Kept** — deviating from the "cut the stub" decision, see §7 |
| `TerminalCapabilitySource`'s `policy` argument is dead | true, but removing it ripples into 8 call sites and the startup wiring test (`src/bin/aleph-server/commands/start/capability_init.rs:136`). **Kept + documented** — see §7 |

## 6. Documented, not fixed (deferred by explicit decision)

`PolicyEngine::resolve` + `validate_visibility_scope` (CP-01/CP-02 — "primary access control gate" with no production caller), the whole `ambient` capability family (SW-03/CP — no production registration), `src/capability/audit.rs` (CP-08), `EventPersistence` (CP-09), the scoped-event chain (`TrustedEventScope`, `subscribe_scoped`, `EventFilter`'s three knobs, CP-10/CP-11/CP-16), the grant subsystem (`GrantRegistry` and its 7 entry points, SW-02), `reconcile_session_events` (effect-claim deferral), the activation-observation path (`CapabilityAttached`/`CapabilityDetaching` never emitted, SW-05/CP-04), the lease-TTL machinery (SW-10), `Constraint::time`/`Constraint::condition` (SW-12), `ProjectionMetadata.surface` (SW-08, open since 2026-10-30 C03-8), the multi-question `ClarificationRequest::new` constructor (SW-CLAR-03), the capability change stream `Zahir::subscribe` (C04), `CapabilityInvocationOrigin` being discarded (C01), `observe_refusal` folding `Revoked`/`Disposed` (CP-15), the typed `ConstraintViolation.constraint_type` (CP-05), `constraints_applied` always 0 (CP-06), `CapabilityRevoked.reason == "requested"` (CP-07), `CapabilityResolver`'s provider-transition notification (CR-03), `src/tools/scoped`'s per-name activation map (SW-07, outside the audited modules).

Each of these carries an in-code marker (`NOT MOUNTED IN PRODUCTION` / `DEFERRED` /
`NOT REACHED IN PRODUCTION` + the audit date) so the next reader sees the state without
re-running the audit.

## 7. Open decisions for a human

1. **VisibilityScope wildcard semantics (C03) — security relevant.** `OwnershipTree`'s
   binding key is *plain struct equality* (`src/capability/ownership.rs:357-362`), so a
   capability registered with `principal: None` is visible to **every** caller of
   `capabilities.describe`, while a per-principal record is invisible to the global
   query. The module doc promises "`None` = unrestricted", but no `VisibilityScope`
   implementation exists. Fixing this means deciding *which side* is the wildcard and
   changing the ownership key model — a security-semantics change, not a mechanical fix.
2. **Provider ownership binding (ZF-02 remainder).** Providers are registered into the
   `ProviderRegistry` but never into the `OwnershipTree`, so provider capabilities are
   listed but not invocable, and `resolve` rejects them as not visible. Binding them
   would also change what `capabilities.describe` reports (e.g. `memory.runtime`).
3. **CAN-01 — unreachable corrupt-document delete recovery.** `gate_canvas`
   (`src/gateway/handlers/canvas.rs`) maps *every* store error to not-found, so the
   owner-check + corrupt-document recovery branch in `CanvasStore::delete`
   (`src/canvas/store.rs:302-320`) can never be reached. Fixing it means deciding what
   the gateway may reveal about a document it could not read.
4. **CAN-06 — asset grace-renewal failure.** `src/canvas/assets.rs:165-210` logs and
   continues, so a dedup target can be dropped before a referencing `apply` arrives.
   Deliberate best-effort degradation today; kept as-is.
5. **PH-01 — mixed cursor.** `src/capability/zahir_facade.rs` computes
   `Cursor(snap.revision().max(self.specials.revision()))`, mixing two independent
   revision domains; a publication can be suppressed. Fixing it needs separate cursors
   and a snapshot-contract change (marked in code as a known defect).
6. **C07 — model-facing adapter descriptions are machine-generated placeholders**
   (cosmetic, but the LLM reads them).

## 8. The new guard

`scripts/capability_wiring_audit.py` — scans `src/capability`, `src/canvas`,
`src/clarification` for public declarations, strips test code
(`scripts/wiring_strip.py`), and fails when a declaration has no production consumer
**outside a triaged baseline** (the baseline may only shrink; stale entries fail too).
Current state: 480 declared, 10 severed — exactly the 10 baselined deferred items
(`effect_claim`, `reconcile_session_events`, the 7 grant entry points, `renew_lease`).

Wired into `justfile` as `check-capability-wiring` and into the `check-wiring`
aggregate.

**Limitation (documented in the script):** the guard matches by *name*, so a name that
collides with an unrelated symbol elsewhere reads as live. It is a **detector, not a
judge** — it points at candidates, a human decides.

## 9. Verification

| Check | Result |
|---|---|
| `cargo check -p alephcore -p aleph-panel --all-targets -j 4` (host) | **exit 0, 0 errors, 0 warnings** — `--all-targets` compiles the *test* code too, which is what a deletion needs |
| `cargo check -p aleph-panel --target wasm32-unknown-unknown -j 4` | **exit 0** (3 pre-existing `acquire_pty_topic` dead-code warnings) |
| `cargo test -p aleph-panel --lib i18n_census` | **25 passed** — the new Panel copy does not break the hardcoded-English ratchet |
| `python3 scripts/capability_wiring_audit.py` | exit 0 — 10 severed, exactly the baselined deferred items |
| `scripts/wiring_seams.py`, `scripts/rpc_wiring_audit.py`, `scripts/tool_wiring_audit.py` | exit 0 — no new severed wires |
| `scripts/config_wiring_audit.py` | still reports `TimeWindowConfig` — **pre-existing false positive** (the value is consumed by `compile_constraints` → `Constraint::time_window`, `src/capability/constraints.rs:87,123`; the guard greps by type name). Unchanged by this audit; out of the audited scope. |

The audit ran on a scratch worktree; the fixes were applied on `main` and committed there
(no PR), as requested.

## 10. Slice reports

`canvas-REPORT.md`, `clarification-REPORT.md`, `cap-core-REPORT.md`, `cap-policy-REPORT.md`,
`cap-resolve-REPORT.md`, `cap-special-REPORT.md`, `cap-ownership-REPORT.md`,
`cap-facade-REPORT.md`, `cap-ui-REPORT.md` — raw auditor output, one per slice, each with
per-candidate form/producer/consumer/live-caller-search/hypothesis/confidence and an
explicit "uncovered seams" section.
