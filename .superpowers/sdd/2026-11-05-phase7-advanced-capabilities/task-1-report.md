# Phase 7 Task 1: Activation & Constraints

## Status

`DONE_WITH_CONCERNS`

The requested activation and constraint functionality is implemented. Targeted tests pass. The repository has unrelated baseline failures and formatting/lint debt, so the full workspace is not clean.

## Approach and trade-offs

Activation owns the `Attached -> Active` boundary and delegates state and lease rules to `CapabilityLifecycle`. Constraints receive a caller-supplied `ConstraintContext` instead of owning a clock, resource registry, or global condition store. This keeps the core lightweight and deterministic, but callers must provide current runtime facts at every activation/runtime-validation boundary. `ConstraintContext::now` provides a UTC-hour context for callers that want a real clock.

Resource limits use the pre-activation active count and require it to be strictly below the configured maximum, because the activation consumes one capacity slot. Time windows are half-open (`start <= hour < end`) and support overnight windows. Conditional constraints use named boolean conditions and fail closed when a condition is missing or mismatched.

## Implementation

- `src/capability/activation.rs`
  - Added `CapabilityActivation` with lifecycle ownership, attach, activate, detach, revoke, and runtime validation operations.
  - Added `ActivationHook` with pre-activation, post-activation, and post-revocation hooks.
  - Constraint evaluation occurs only on activation and runtime validation, never on attach or revocation.
  - Lease and owner-generation validation happen before constraint evaluation at the activation and runtime-use boundaries.
  - Added typed `ActivationError` using `thiserror`.
- `src/capability/constraints.rs`
  - Added `ConstraintContext`.
  - Added `TimeConstraint`, `ResourceConstraint`, and `ConditionalConstraint`.
  - Added `Constraint` enum and ordered, fail-closed `ConstraintSet`.
  - Added typed `ConstraintViolation` using `thiserror`.
- `src/capability/lifecycle.rs`
  - Added `validate_activation`, which validates attached state, lease TTL, and owner generation before the activation transition.
- `src/capability/mod.rs`
  - Registered the activation and constraints modules.

## Tests

- TDD red phase: the initial test-only modules failed to compile because the requested APIs did not exist.
- `cargo test --lib capability::activation`: **14 passed, 0 failed**.
- `cargo test --lib capability::constraints`: **14 passed, 0 failed**.
- The originally specified command `cargo test --lib capability::activation capability::constraints` is not accepted by Cargo because it accepts one test filter. The two equivalent filters above were run separately.
- `cargo test --lib`: **22,018 passed, 4 failed, 20 ignored**.
  - `builtin_tools::runtime_manage::tests::a_failing_ledger_still_reports_ok_for_a_real_spawn`
  - `capability::grant_integration_tests::tests::revoked_parent_prevents_child_grant_creation`
  - `gateway::handlers::markdown_skills::tests::containment_is_component_wise`
  - `gateway::method_census::tests::every_registered_rpc_method_has_a_recorded_ruling`
  These failures are outside the changed files; the task changes do not modify the affected grant, runtime, markdown-skills, or RPC census implementations, so they were not attributed to this task.

## Verification

- `rustfmt --edition 2021 --check src/capability/activation.rs src/capability/constraints.rs src/capability/lifecycle.rs`: passed.
- `cargo fmt --all -- --check`: failed on pre-existing formatting differences in unrelated workspace files. The formatter's recursive run was reverted for unrelated capability files; no unrelated changes remain in the working tree.
- `cargo clippy --lib -- -D warnings`: failed before completing due existing errors in `desktop/shared` (`get_first` findings in `hyprland.rs` and `ocr_linux.rs`).
- `cargo clippy -p alephcore --lib --no-deps -- -D warnings`: failed on existing warnings across `grant.rs`, `resolver.rs`, `extension/projection.rs`, `memory/runtime`, `tools/registry.rs`, `approval/*`, and other unrelated modules. No activation or constraints diagnostics were emitted.
- `/verify` gstack checks: `build PASS`, `lint PASS`. The test pregate reported `TOO_BROAD` because it selected 2,997 test files; `/verify` status was `NOT_READY tree=fa48ff20df2c: tests=TOO_BROAD`.

## Not done / deferred

- No policy-engine integration was added because the Task 1 brief requires activation/lifecycle integration; policy constraint enforcement is listed as a later integration point in the broader Phase 7 plan.
- No persistent resource accounting, scheduler, timezone database, or arbitrary closure-based condition engine was introduced.
- No attempt was made to repair unrelated baseline test failures, workspace-wide formatting drift, or pre-existing Clippy findings.
- Coverage was not measured with `cargo llvm-cov`; the new modules contain 28 focused unit tests, but a numeric 80% coverage result was not independently generated.

## Fix Round 1

Commit: `fix(capability): Task 1 Round 1 - production wiring + spec gaps`

- Runtime restrictions now have an explicit `CapabilityActivation` trait and a controller implementation with shared lifecycle state. `CapabilityActivationController` is cloneable without diverging revocation state because clones share the lifecycle mutex.
- `ScopedToolService` accepts named capability activation controllers. Canonical dispatch invokes `validate_runtime` after canonical facade lease validation and before the handler, mapping context and constraint failures to `PermissionDenied`.
- `ConstraintContext::at_hour` now validates `0..=23` at construction and returns `Result`; invalid hours cannot reach resource or conditional constraints.
- Self-validating activation entry points were removed. `activate` and `activate_with_hook` require the caller's explicit current owner generation.
- Added tests for generation mismatch, shared clone revocation, detach, revocation hooks, false-valued conditional constraints, invalid hours, and dispatch-to-activation validation.

Fix-round targeted evidence:

- `cargo test --lib capability::activation`: **18 passed, 0 failed**.
- `cargo test --lib capability::constraints`: **16 passed, 0 failed**.
- `cargo test --lib activation_dispatch_tests::dispatch_runtime_validation_calls_activation_contract`: **1 passed, 0 failed**.
- `rustfmt --edition 2021 --check src/capability/activation.rs src/capability/constraints.rs src/tools/scoped/builder.rs src/tools/scoped/dispatch.rs src/tools/scoped/mod.rs`: passed.
- `cargo clippy --lib --message-format short` showed only pre-existing findings in `dispatch.rs` (lifetime elision and `?` suggestions at existing lines); no findings were emitted for the new activation or constraints code.
- Final `/verify` at the fix-round tree: `build PASS` and `lint PASS`; test pregate remained `TOO_BROAD` (2,997 selected test files), with status `NOT_READY tree=2cec1dac8028: tests=TOO_BROAD`.

Fix-round scope notes:

- `CapabilityLifecycle::transition_to` remains public because it predates Task 1 and changing it requires a Phase 8 call-site audit; the activation controller is the constrained public boundary for Task 1.
- Resource accounting remains non-atomic and deferred as requested.
- The dispatch activation map is intentionally opt-in. The production canonical dispatch seam is wired; later capability registration work must populate named controllers from its capability configuration.
