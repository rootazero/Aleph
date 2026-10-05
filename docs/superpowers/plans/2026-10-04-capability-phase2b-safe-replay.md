# Capability Phase 2B — Restricted Safe Replay Implementation Plan

> Design: `docs/superpowers/specs/2026-10-04-capability-phase2b-safe-replay-design.md`
>
> Worktree: `/Volumes/TBU4/Workspace/Aleph-capability-phase2-durable`
>
> Branch: `feature/capability-phase2-durable`
>
> This plan is design-gated. Until the design and this plan are independently reviewed, do not modify Rust product code or execute a Tool during recovery.

## Status (2026-10-05, inert-restricted slice shipped)

The S0–S5 *restricted replay preparation* slice is shipped on `feature/capability-phase2-durable` and remains **inert + VerifyOnly** in production:

- **Shipped (commits `3490103d4` `feat: wire restricted replay preparation`, `7d3bf4a5a` `fix: close replay claim outcome leak`, plus the lineage `c23074731` / `09a29d16f` / `77ecfaa9c` / `eb5fd4063` / `9a4875ed9` / `731b33ff9`):** durable call-time identity + cross-restart `replay_contract_fingerprint`; durable claim fence + unanswered-call outcome precondition (`SessionEventStore::claim_replay_call` / `commit_replay_outcome` / `load_retire_generation`, `REPLAY_LEASE_TTL_MS = 300_000`); atomic `ToolHandlerRegistry::snapshot_state()`; private non-`Copy` `ReplayPermit` produced by a `ReplayPreparer` whose registry-derived handler/descriptor/closed-state come from one immutable snapshot (`src/session/replay.rs`, no `ToolHandlerRegistry` / `ScopedToolService` / MCP / platform import); the original `call_id`, `tool_name`, `turn_id`, `effective_input`, and stored `ToolCallIdentity` are not registry-derived inputs; bridge `ReplayAdapter` (`src/orchestrator/harness_bridge/replay_adapter.rs`); `ResumeCoordinator::replay_dangling_calls` prepare→claim→invoke→commit→fresh reduction→VerifyOnly sequence on the interrupted-run branch, wired with `with_replay_preparer` builder; `src/bin/aleph-server/commands/start/mod.rs` adapter injection; `NoEffect` post-claim active-cursor leak fixed (`7d3bf4a5a` deletes the `ReplayInvocation` enum and the silent `=> {}` arm in `replay_dangling_calls`).
- **Not shipped — and explicitly deferred.** Post-guardrail durable effective-input marker (`SessionEvent::ToolCallEffectiveInput { turn_id, call_id, input, at }` emitted after guardrail + dedup/cache in `src/harness/agent/act.rs:617` / `:931-933`) and the matching `release_replay_claim` store op. While the marker is absent, `effective_input` is permanently `None` in `replay_dangling_calls`, the gate forces `Refused`, and **no handler is invoked and no `ToolResult`/`ToolError` is appended through this seam** — every dangling call degrades to VerifyOnly.
- **Verification evidence recorded.** Focused replay tests 102/102, happy-path 1/1, adapter 8/8, session seam 2/2, `alephcore` `cargo check --all-targets` + `cargo check --bin aleph-server` green. Full `cargo clippy --all-targets` has 4 baseline `mcp_face` errors that predate this slice; full `cargo test` is blocked by a missing `binaries/aleph-server-aarch64-apple-darwin` artifact (build environment gap, not a code defect).
- **Decisions captured.** `.pi/decisions/2026-10-04-phase2b-replay-execution-seam.md` (accepted — two-phase seam, snapshot contract, `Option`-typed effective-input gate, coordinator ownership); `.pi/decisions/2026-10-05-phase2b-effective-input-replay-gate.md` (proposed — keep replay inert, drop `NoEffect` until a real producer appears, define marker contract without implementing it).
- **Non-goals preserved.** No exactly-once; no general ledger; no new protocol message; no `ToolHandlerRegistry`/`ScopedToolService`/platform dependency in the session seam; no replay code in `src/harness/`; delegated `repair_and_close_abandoned` stays VerifyOnly with no replay adapter and cannot obtain a permit.

## Decision record

Implement the conservative, auditable **at-least-once** scheme selected by the user after the ask-user UI was unavailable. The existing Phase 2A predicate remains an observation/classification. A separate, private permit is the only input accepted by the replay adapter.

Hard decisions:

- A stable `replay_contract_fingerprint` is required for new Safe identities. Process-local `revision` remains only an in-process TOCTOU guard.
- The fingerprint is versioned and canonicalized; it excludes registry order and revision and includes an audited implementation contract id/version. A descriptor-only hash cannot prove handler implementation identity, so Safe registration must provide and review that identity.
- The handler and descriptor must come from the same `ToolHandlerRegistry::entries_snapshot()` and remain bound in a non-`Copy` permit.
- `ResumeCoordinator` and its existing `ResumeSlot` remain process-local admission guards, not distributed claims. Before any effect, a narrow expected-head/serialized store operation must durably claim the recovery and each outcome append must prove the original call is still unanswered. This is a targeted store contract, not a general scheduler or ledger.
- A per-call cursor/budget is required; one batch-level `ResumeAttempted` stamp cannot prove each effect was attempted a bounded number of times.
- Only the interrupted-run ResumeCoordinator path is enabled. Delegated team/cron/heartbeat abandonment remains VerifyOnly through a distinct API with no replay adapter.
- Replay uses the effective input only when the durable intent proves it equals the handler input. Current pre-sanitize Phase 2A requests are VerifyOnly until that proof exists.
- Replay uses the original `call_id`, emits no second `ToolCallRequested`, and appends existing `ToolResult`/`ToolError` events. The original request count, effect invocation count, and receipt count are separate facts.
- An effect whose outcome append is lost may execute again. This is documented at-least-once behavior, not exactly-once or deduplication.
- `idempotent` never grants Safe policy, and Safe never proves exactly-once.

## Scope and file ownership

The implementation must keep the core/bridge boundary and avoid a second registry. Expected files are provisional until the design review confirms the exact seam:

- `src/tools/descriptor.rs`: fingerprint/token shape, canonical encoding, validation, identity capture.
- `src/tools/registry.rs`: Safe registration validation and one-snapshot preparation adapter; preserve `entries_snapshot()` as the paired handler/descriptor source.
- `src/session/events.rs`: backward-compatible optional identity field and fail-closed fingerprint decoding.
- `src/session/reduction.rs`: preserve the fingerprint in `DanglingCall`.
- `src/session/boundary_repair.rs`: separate VerifyOnly repair and replay-specific command/traits, private permit flow, effective-input proof, ordered outcome append, and no direct registry/platform dependency.
- `src/session/store.rs`: a narrow expected-head/serialized claim and outcome precondition, only if the existing store cannot express it; no general replay ledger.
- `src/gateway/resume_coordinator.rs`: use the durable claim before replay, persist/validate the per-call cursor, and only then continue to the existing whole-run retrigger.
- `src/orchestrator/harness_bridge/*` and startup wiring only if required to construct the bridge-side replay adapter from the existing registry and execution dependencies.
- Focused tests adjacent to the above modules and existing recovery/harness fixtures.
- `docs/reference/ARCHITECTURE.md`, `docs/reference/FEATURE_LOCATOR.md`, and the Phase 2A docs only after implementation evidence exists.

Do not modify `src/harness/` for replay execution. The two Phase 2A identity-capture wiring points remain the only harness exception.

## Task 0 — Design gate and contract freeze

**No product code.**

1. Review the companion 2B spec and this plan against the current `repair_boundary_with_policy`, `ResumeCoordinator`, `ToolHandler`, `ToolHandlerRegistry`, and store APIs.
2. Freeze the fact that the current Phase 2A order is incompatible with replay: `repair_boundary` currently writes synthetic answers before the resume stamp, and the coordinator later retriggers the whole run.
3. Freeze a targeted durable claim contract: expected head/serialized session ownership before effect, plus an unanswered-call precondition for every outcome append. `load_head_seq() + append()` and an in-memory `ResumeSlot` alone are insufficient.
4. Freeze a per-call attempt cursor/budget and its crash-after-index-k behavior. A batch-level `ResumeAttempted` count is not a per-effect bound.
5. Freeze the effective-input rule: current request events precede guardrail/sanitize, so any potentially transformed or blocked call is VerifyOnly unless effective input is durably captured.
6. Freeze the registry preparation API: one immutable state contains paired handler/descriptor, closed state, and non-reused state generation; no live recheck after snapshot.
7. Freeze API-level separation: delegated VerifyOnly repair cannot receive a replay adapter or construct a permit.
8. Record required API changes here before implementation; do not discover them by speculative edits.

**Gate:** design reviewer accepts the claim/outcome precondition, per-call budget, effective-input rule, single-state permit derivation, and duplicate-effect language. No Rust implementation starts while any of those are unresolved.

## Task 1 — Add the stable replay contract token

**Likely files:** `src/tools/descriptor.rs`, `src/session/events.rs`, `src/tools/mod.rs`, tests.

1. Add an optional fixed-size fingerprint to `ToolCallIdentity`, preserving decoding of Phase 2A and older events. A missing token is replay-ineligible, not an error that becomes Safe.
2. Add a versioned canonical encoding for replay-relevant descriptor fields. Canonicalize JSON object keys recursively; never rely on serde map iteration order.
3. Include a separately supplied audited implementation contract id/version in the token input. Do not pretend SHA-256 of descriptor JSON detects handler code changes.
4. Exclude process-local `revision`; keep schema version and explicit replay policy in the contract input.
5. Require a valid token for a descriptor explicitly registered with `ReplayPolicy::Safe`; preserve `Unsafe` defaults and do not infer policy from idempotence or source.
6. Keep the token representation stable across process restart and independent of registration order. Use existing SHA-256/hex workspace dependencies without adding a package.
7. Update field-local event decoding so unknown/malformed token data drops replay identity safely while the outer event remains readable.

**Tests:** same descriptor/token across fresh registries; registration order independence; changed schema/source/policy/implementation token mismatch; legacy/missing/malformed token fail-closed; Safe-without-token rejected or VerifyOnly according to the frozen registration contract.

**Gate:** `git diff --check`; memory check; `CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets --message-format short`; focused descriptor/events tests.

## Task 2 — Preserve token through reduction and store

**Likely files:** `src/session/reduction.rs`, `src/session/store.rs` tests.

1. Carry the optional token from `ToolCallRequested` into `DanglingCall` without rediscovering it from the current registry.
2. Preserve the existing structural meaning of dangling: no `ToolResult` or `ToolError` outcome exists for that call.
3. Verify event/store round-trip and legacy rows.
4. Keep no second replay ledger and no new protocol event.

**Tests:** token round-trip, legacy identity, malformed identity, reduction retention, answered call excluded from dangling.

**Gate:** focused session events/store/reduction tests, serially; no change to outcome semantics.

## Task 3 — Define the narrow preparation/execution seam

**Likely files:** `src/session/boundary_repair.rs`, a new narrow session trait module only if necessary, bridge adapter tests.

1. Define a narrow injected replay command for the session layer. VerifyOnly repair is a separate command and does not accept `Option<&dyn ReplayExecutor>`; the session layer may pass tool name, effective input, call id, stored identity, reduction proof, and claim context, but it must not know `ToolHandlerRegistry` or platform APIs.
2. Make the prepared permit private and non-`Copy`. It must bind the original call id, captured handler/descriptor state generation, current fingerprint, effective input proof, and recovery claim context. The executor consumes it once. The existing `ResumeAttempted` has only a run/user target; the plan must choose a backward-compatible per-call cursor/call identity field or an equivalent durable store cursor, without overloading that target.
3. Ensure the registry-side adapter derives the registry-derived permit inputs (handler, descriptor, closed status, state generation) from one immutable state. It must not call separate `snapshot()` and `descriptor_snapshot()` methods or re-read live state after capture.
4. Revalidate Safe policy, fingerprint, schema version, state generation, revision guard, name, input equivalence, uniqueness of the original dispatch, and absence of outcomes before permit creation. Any mismatch returns VerifyOnly without invocation.
5. Do not expose a public constructor from `ReplayDecision`, a bool, or raw name/input that can manufacture authorization.
6. Since `ToolHandler::invoke` currently has no `call_id`, do not claim handler-level dedup. The adapter may include the call id in audit/tracing; actual duplicate-effect tolerance remains the Safe registration contract.
7. The store/actor must provide the expected-head claim and unanswered-call outcome precondition; otherwise the adapter must refuse replay. A process-local lock is not enough for multiple coordinators/processes.

**Tests:** snapshot pairing, replacement race before permit, permit single-use/non-`Copy` compile shape, wrong call id rejection, no registry/platform dependency in session module.

**Gate:** architecture review before any test that invokes a real handler; verify no direct Tool execution path was added to `src/harness`.

## Task 4 — Integrate ResumeCoordinator ordering and crash cap

**Likely files:** `src/gateway/resume_coordinator.rs`, `src/session/boundary_repair.rs`.

1. Keep `try_claim_resume`/`ResumeSlot` as process-local admission only; acquire the durable expected-head recovery claim before any replay effect.
2. Because the current interrupted-run branch repairs before stamping and later retriggers the whole run, refactor the order to: durable claim, fresh reduction, per-call cursor stamp, permit, effect, confirmed outcome append, fresh reduction, then normal retrigger.
3. Replay dangling calls in original event order, one at a time, with one per-call budget unit. Validate `ResumeAttempted.target`/cursor against the current run and call; do not use one batch stamp as the per-effect bound.
4. Before each effect, prove one original dispatch, no outcome, no duplicate contradiction, effective-input equivalence, and a valid one-state permit.
5. Leave non-eligible calls to the distinct VerifyOnly repair. If a call budget is exhausted, stop automatic replay and surface unknown outcome; never synthesize success.
6. If outcome append fails before commit or commit status is unknown after handler invocation, keep the call unanswered and emit only non-sensitive diagnostics. Do not append a synthetic `ToolError` for that call.
7. Do not alter delegated `repair_and_close_abandoned` to execute Tools or accept a replay adapter.

**Tests:** one claim prevents concurrent replay; stamp-before-effect ordering; multiple dangling calls; partial batch; handler error; append failure after effect; process/crash injection before and after invoke; max-attempt fallback; delegated VerifyOnly.

**Gate:** focused recovery tests plus existing ResumeCoordinator tests. Review the exact event sequence and prove no second request event is appended.

## Task 5 — Bridge existing callable truth

**Likely files:** `src/orchestrator/harness_bridge/*`, startup wiring, `src/tools/registry.rs`; only if Task 3 requires it.

1. Adapt the existing `ToolHandlerRegistry` and execution path to the narrow session replay interface.
2. Construct the adapter from one existing registry object; do not add a global registry, scheduler, or lookup singleton.
3. Bind descriptor and handler from the same `entries_snapshot()` at permit creation.
4. Keep builtins/external tools without an audited Safe token VerifyOnly.
5. Ensure ResumeCoordinator receives the adapter only on the interrupted-run path; delegated callers receive no replay executor.

**Tests:** startup injection, absent adapter fail-closed, registry replacement, MCP/extension Unsafe default, Safe registration audit token requirement.

**Gate:** changed-file inspection, architecture review, `cargo check -p alephcore --all-targets` after memory check.

## Task 6 — Document and review

After implementation and tests only:

1. Update `ARCHITECTURE.md` and `FEATURE_LOCATOR.md` with the shipped execution boundary, explicit at-least-once semantics, fingerprint contract, ResumeCoordinator-only scope, and delegated VerifyOnly behavior.
2. Update Phase 2A references so they link to Phase 2B without claiming exactly-once or universal replay.
3. Add a verification record with exact commands and baseline failures.
4. Run final review against this plan, including a source scan for new registries, schedulers, outcome ledgers, second request events, and replay code in `src/harness`.

## Verification matrix

| Evidence | Expected result |
|---|---|
| Cross-restart token | Stable token remains eligible; local revision reset alone cannot authorize a changed contract |
| Contract mutation | Schema/source/policy/implementation token change is VerifyOnly |
| Registry order | Registration order does not affect token equality |
| Single-state snapshot | Handler, descriptor, closed status, and state generation come from one immutable preparation state |
| Input equivalence | Pre-sanitize or blocked calls are VerifyOnly unless effective input was durably captured |
| Durable claim | Competing coordinators cannot both acquire the expected-head claim |
| Authorization boundary | Only private non-`Copy` permit reaches executor |
| Outcome precondition | An outcome append proves the original call is still unanswered; duplicate contradiction is VerifyOnly |
| Event/effect/receipt counts | Original request, effect invocations, and receipts are measured separately; no exactly-once claim |
| Partial batch | Earlier confirmed outcomes persist; fresh reduction excludes answered calls |
| Crash window | Post-effect/pre-outcome failure may repeat only within that call's persisted budget |
| Delegated recovery | Remains VerifyOnly and has zero handler invocation |
| Architecture | No session dependency on concrete registry/platform APIs; no replay in harness |
| Regression | Phase 2A focused tests remain green; baseline failures reported, not hidden |

Required commands, each preceded by the project memory check:

```text
CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets --message-format short
cargo test -p alephcore --lib session::boundary_repair -- --test-threads=1
cargo test -p alephcore --lib gateway::resume_coordinator -- --test-threads=1
cargo test -p alephcore --lib tools::descriptor -- --test-threads=1
cargo test -p alephcore --lib tools::registry -- --test-threads=1
cargo test -p alephcore --lib tool_call_requested -- --test-threads=1
cargo clippy -p alephcore --all-targets -- -D warnings
cargo test -p alephcore --lib --message-format short
```

Do not run `cargo fmt --all`. Use scoped formatting only after implementation stabilizes, then inspect the changed-file list.

## Commit checkpoints

Keep contracts reviewable:

1. `feat: add stable tool replay contract identity`
2. `feat: add bounded replay preparation seam`
3. `feat: replay safe dangling tools under resume claim`
4. `docs: document restricted safe replay semantics`

Before each commit: `git diff --check`, changed-file scope inspection, focused tests, and an explicit check that no package dependency or unrelated worktree change entered the commit. Leave Phase 2B implementation worktree clean only after final review.

## Explicit non-goals

- Exactly-once execution, at-most-once execution, or receipt deduplication.
- A general durable Task Scheduler, persistent external-effect ledger, or outbox.
- A general store rewrite; however, the narrow expected-head/serialized claim and unanswered-call outcome precondition required by this design is in scope and must not be omitted.
- The existing `ResumeAttempted.target` cannot be overloaded from its current run/user meaning to a call id. The plan must either extend the event compatibly with a per-call cursor/call identity or use equivalent durable store metadata.
- Passing `call_id` into `ToolHandler::invoke` as if that alone provided deduplication.
- Automatic replay through team/cron/heartbeat abandonment.
- Replay of pre-sanitize/blocked calls without durable effective-input proof.
- Safe inference from `idempotent`.
- Recovery logic in `src/harness`.
- A second registry, scheduler, or protocol message.
