# Capability Phase 2A — Durable Tool Intent Contract Implementation Plan

> Approved design: `docs/superpowers/specs/2026-10-04-capability-phase2-durable-tool-design.md`
>
> Worktree: `/Volumes/TBU4/Workspace/Aleph-capability-phase2-durable`
>
> Branch: `feature/capability-phase2-durable`
>
> This plan implements Phase 2A only. Phase 2B Safe replay execution requires a separate design approval.

## Goal and constraints

Add durable Tool Capability identity to `SessionEvent::ToolCallRequested`, preserve it through storage and reduction, and compute a typed fail-closed replay eligibility result by comparing the call-time identity with the current descriptor. Keep the existing session/WAL/recovery architecture and do not execute a Tool in Phase 2A.

Hard constraints:

- `ToolHandlerRegistry` remains the only callable Tool truth source.
- `SessionEventStore`, `RunReduction`, `boundary_repair`, and `ResumeCoordinator` remain the durable/recovery path.
- No Task Scheduler, external-effects ledger, second registry, or new protocol message.
- `idempotent` never implies `ReplayPolicy::Safe`.
- Unknown or malformed identity data is never replay eligible; the event-level decoder drops it to `None` without changing the public descriptor enum.
- `src/harness/agent/act.rs` receives only a read-only identity lookup and copies its result into the event. It must not decide replay or execute recovery.
- No Phase 2B execution path, new Tool result, or external side effect.
- Any cargo invocation in a concurrent implementation track must first run `memory_pressure`; if available memory is below 4 GiB, wait and recheck before cargo.
- Do not run `cargo fmt --all`; use `rustfmt --config skip_children=true` only on changed Rust files, then inspect the changed-file set.

## Current verified seams

- `src/session/events.rs:542` defines `SessionEvent::ToolCallRequested`; it now has optional `identity: Option<ToolCallIdentity>` between `input` and `at`, with a field-local fail-closed decoder.
- `src/session/events.rs:721` assigns `ToolCallRequested` `Durability::Barrier`; this remains unchanged.
- The two production event construction sites are `src/harness/agent/act.rs:486` (serial) and `src/harness/agent/act.rs:823` (parallel PASS 0); both copy identity from the injected read-only lookup.
- `src/harness/deps.rs` now carries `Option<Arc<dyn ToolDescriptorLookup>>`; the startup bridge explicitly passes the existing MCP registry, while builtin/test paths without a registry remain `None`.
- `src/tools/descriptor.rs` defines `ToolCallIdentity`, `ToolDescriptorLookup`, and `ReplayPolicy`; `src/tools/registry.rs` implements lookup from the current atomic descriptor generation.
- `src/session/reduction.rs` now carries identity into `DanglingCall`; `src/session/boundary_repair.rs` classifies exact dual-Safe, same schema/revision calls as `AutoReplayEligible` but Phase 2A still emits the existing unknown-outcome repair and executes no Tool.

## Task 1 — Add durable identity and lookup contract

**Files**

- `src/tools/descriptor.rs`
- `src/tools/registry.rs`
- `src/tools/mod.rs`

**Changes**

1. Add `ToolCallIdentity` near the existing descriptor types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallIdentity {
    pub schema_version: u32,
    pub revision: u64,
    pub replay_policy: ReplayPolicy,
}
```

2. Add the read-only trait:

```rust
pub trait ToolDescriptorLookup: Send + Sync {
    fn tool_call_identity(&self, name: &str) -> Option<ToolCallIdentity>;
}
```

3. Implement it for `ToolHandlerRegistry` by reading one current descriptor and copying only the durable identity fields. Do not expose handler code or mutable registry state.
4. 给 `ToolCallIdentity` 增加局部 fail-closed 反序列化，而不扩展公共 `ReplayPolicy` 枚举：在 `ToolCallRequested.identity` 的自定义 decoder 中，未知、缺失或损坏的 identity 映射为 `None`；缺少整个 identity 字段也为 `None`。这样不会把未来 replay policy 误读为 `Safe`，也不会把 `Unknown` 扩散到 descriptor 的所有 exhaustive match。
5. Export the new public types from `src/tools/mod.rs` if required by existing module visibility conventions.
6. Add unit tests for registry identity capture, default `Unsafe`, and unknown policy being non-eligible/non-safe.

**Contract**

The lookup is a snapshot read only. It is not a replay authorization API; the recovery predicate remains in `session::boundary_repair`.

**Gate**

Run `git diff --check`; inspect that only the three approved files changed. After the task batch reaches a compile point, run the memory gate and `CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets --message-format short`.

## Task 2 — Extend the durable event compatibly

**Files**

- `src/session/events.rs`
- event tests in `src/session/events.rs` or the existing session test module
- compiler-identified construction sites, limited to adding `identity: None` for legacy fixtures

**Changes**

1. Add an optional field to `ToolCallRequested`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
identity: Option<ToolCallIdentity>,
```

2. Keep `ToolCallRequested` as `Durability::Barrier`.
3. Ensure an old event JSON payload without `identity` deserializes with `None`.
4. Add a field-local fail-closed decoder for `ToolCallRequested.identity`: an unknown, missing, or malformed nested replay policy makes the optional identity `None`, while the outer legacy event remains decodable. Do not alter the public `ReplayPolicy` enum or map an unknown value to `Safe`.
5. Add event serde tests for:
   - current identity round-trip;
   - legacy JSON without identity → `None`;
   - unknown replay-policy value → non-replayable result;
   - barrier durability remains unchanged.

**Gate**

Run the memory check before cargo. Then run `CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets --message-format short` and the focused event tests.

## Task 3 — Persist and reduce identity

**Files**

- `src/session/store.rs` tests only, unless the existing codec requires a narrow production adjustment
- `src/session/reduction.rs`
- reduction tests

**Changes**

1. Add `identity: Option<ToolCallIdentity>` to `DanglingCall`.
2. Extend the reducer's dispatch accumulator to carry the identity from the original `ToolCallRequested`; populate the dangling record from that accumulator. Do not rediscover it by tool name or current registry lookup.
3. Add a store round-trip test proving `Some(identity)` survives `SessionEventStore` encode/decode.
4. Add a hand-written legacy event-row test proving an old `ToolCallRequested` without identity decodes to `None`.
5. Add a reduction test proving a dangling call retains schema version, revision, and replay policy.
6. Preserve the existing structural meaning that a `DanglingCall` has no `ToolResult`/`ToolError`; do not add a second outcome scan for Phase 2A.

**Gate**

Before cargo, check available memory. Run focused `session::store` and `session::reduction` tests serially. A failed test is not a baseline excuse; diagnose whether the failure is introduced by this task.

## Task 4 — Implement typed eligibility without executing Tools

**Files**

- `src/session/boundary_repair.rs`
- `src/session/mod.rs` only if the typed observation API needs an export
- boundary-repair tests

**Changes**

1. Extend the existing private `ReplayDecision` with `AutoReplayEligible` while retaining `VerifyOnly`.
2. Change the predicate to consume stored identity and the current descriptor lookup:

```rust
fn replay_decision(
    stored: Option<&ToolCallIdentity>,
    lookup: Option<&dyn ToolDescriptorLookup>,
) -> ReplayDecision
```

3. Eligibility requires all of:
   - stored policy is `Safe`;
   - current descriptor exists;
   - current policy is `Safe`;
   - stored/current schema versions are compatible;
   - stored/current revisions are equal;
   - the call is structurally dangling and therefore has no recorded outcome.
4. Missing identity, missing descriptor, `Unsafe`, `Unknown`, schema mismatch, or revision mismatch returns `VerifyOnly`.
5. Keep Phase 2A behavior classification-only. Existing boundary repair still writes its current synthetic unknown-outcome `ToolError`; `AutoReplayEligible` must not call a Tool, append a new replay result, or alter external state.
6. Expose eligibility only through a typed observation/test seam if needed. Do not return a `bool`, add `From<bool>`, `Default`, or a public constructor that can manufacture the success variant.
7. Add tests for Safe+Safe+matching identity and each fail-closed case, including a replacement that increments revision or changes policy. Add a purity test proving eligibility does not append events.

**Gate**

Run memory check, focused boundary-repair tests, and existing recovery tests. Confirm no call to `ToolService::execute` or `execute_with_cancel` was added.

## Task 5 — Wire identity at the two real production event points

**Files**

- `src/harness/deps.rs`
- `src/harness/agent/act.rs`
- `src/orchestrator/harness_bridge/runner_impl.rs` (the existing `HarnessDeps` assembly site)
- harness tests only where they already cover Act event emission

**Changes**

1. Add `Option<Arc<dyn ToolDescriptorLookup>>` to `HarnessDeps` beside `tools`.
2. At both `act.rs` request construction points, read the lookup by the already-resolved call name immediately before constructing `ToolCallRequested`; copy the returned identity into the event. The lookup must be read-only and must not decide whether to replay.
3. Populate the field in the bridge where `HarnessDeps.tools` and the other run dependencies are assembled. Use the existing registry object or a narrow adapter already available there; do not add a global registry lookup.
4. Keep absent lookup behavior compatible: tests and non-registry harnesses emit `identity: None`, which remains fail-closed.
5. Add/extend tests to prove serial and parallel request emission both carry identity when wired and both remain compatible when the lookup is absent.
6. Confirm the field is captured from the same current registry truth used to execute the call. Do not pair separate handler and descriptor snapshots in the bridge.

**Gate**

Check memory before cargo. Run `cargo check -p alephcore --all-targets --message-format short`, focused harness/act tests, and the existing session/recovery tests. Inspect `git diff -- src/harness` to ensure only the declared wiring appears.

## Task 6 — Documentation and feature locator

**Files**

- `docs/reference/ARCHITECTURE.md`
- `docs/reference/FEATURE_LOCATOR.md`
- this plan and the approved spec, only if implementation evidence requires wording corrections

**Changes**

Document only shipped Phase 2A behavior:

- `ToolCallRequested` now carries optional call-time descriptor identity.
- registry revision and replay policy are copied at the request boundary.
- legacy identity absence and all mismatches remain fail-closed.
- `AutoReplayEligible` is classification-only in Phase 2A; no automatic replay is shipped.
- Phase 2B remains deferred pending an independent execution contract.
- the two narrow harness wiring points are an explicit exception; recovery policy remains outside harness.

Do not claim complete Pi Durable semantics, Safe replay execution, or global Capability convergence.

**Gate**

Run `git diff --check`, verify references point at real symbols, and compare documentation claims against the final code/tests.

## Implementation status

Phase 2A is implemented in this worktree. The compile and focused tests listed in the verification matrix have been run except the full lib and final clippy gate; clippy is expected to retain the existing four `mcp_face` diagnostics recorded from Phase 1. Phase 2B execution remains deferred.


The work is deliberately split into two lines with no overlapping product files:

- **Line 1 — durable/session:** Task 1 shared types → Task 2 events → Task 3 store/reduction → Task 4 eligibility.
- **Line 2 — wiring:** wait for Task 1 types → Task 5 `HarnessDeps`/Act/bridge wiring and tests.
- **Integration:** after both lines finish, run the combined compile gate and Task 6 docs. Do not let both lines edit the same file concurrently.

Because Task 4 depends on the reducer identity shape and Task 5 depends on the exported lookup trait, no line may skip those dependencies. If an implementation task discovers a cross-line contract conflict, stop that task, update this plan/spec, and do not guess.

## Verification matrix

| Evidence | Command/condition | Expected interpretation |
|---|---|---|
| Formatting scope | `rustfmt --config skip_children=true <changed files>` + `git diff --name-only` | No unrelated formatting churn |
| Static compile | `CARGO_BUILD_JOBS=2 cargo check -p alephcore --all-targets --message-format short` | Must pass before final review |
| Event/reducer | focused `session::events`, `session::store`, `session::reduction` tests | New identity survives and legacy remains fail-closed |
| Eligibility | focused `session::boundary_repair` tests | Only exact dual Safe/current revision qualifies; no Tool executes |
| Wiring | focused Act/harness tests | Both serial and parallel production points emit identity when lookup is installed |
| Regression | existing descriptor/registry/recovery/MCP/run-loop tests | Existing Phase 1 behavior remains intact; baseline failures recorded separately |
| Clippy | `cargo clippy -p alephcore --all-targets -- -D warnings` | Report existing `mcp_face` baseline if unchanged; no new diagnostics |
| Full lib | `cargo test -p alephcore --lib --message-format short` | Report exact result; do not call known baseline failures green |
| Boundary | `git diff --check`, clean worktree, `git diff --name-only -- src/harness` | Only declared files changed; no undeclared harness logic |

## Explicit non-goals

- Phase 2B automatic Safe replay execution.
- Pi Durable's general task state machine.
- New external-effects ledger.
- Skill/Agent/Plugin/MCP/ACP unification.
- New wire protocol messages.
- Tool-name-specific recovery branches.
- Reinterpreting legacy events as replay-safe.
- Suppressing unrelated clippy diagnostics with `#[allow]`.

## Commit checkpoints

Use separate commits so each contract is reviewable:

1. `feat: persist tool capability call identity`
2. `feat: classify durable tool replay eligibility`
3. `feat: wire descriptor identity into tool requests`
4. `docs: document durable tool intent contract`

Before each commit: run `git diff --check`, inspect changed-file scope, and record the exact focused test command/results. Final commit must include only the approved files and leave the worktree clean.
