# Task 1 Report — Define Context Capability wire contract

Status: READY_FOR_REVIEW
Commit: final fix-round commit (`context: close capability admission boundaries`)
Branch: `codex/context-fabric-plugin` (worktree `D:/Workspace/Aleph/.worktrees/context-fabric-plugin`)

## Scope

Task 1 adds the single serde/schemars wire contract at `src/context/fabric.rs`, mounts it from `src/context/mod.rs`, and adds only the context classification helper in `src/extension/capability.rs`. `PreflightStage::name(&self) -> &'static str` remains unchanged. No registry, manifest, lifecycle wiring, transport, or second descriptor shape was added.

## Implemented contract

- `ContextPhase`, `ContextCapabilityMode`, `ContextLifecycleEvent`, `ContextNamespace`, `ContextOperation`, `ContextQuality`, and `DegradedReason` use stable snake_case serde tags.
- `ContextOperation::wire_name()` produces `context.fabric/v1/<operation>` for all eight operations.
- `ContextCapabilityDescriptor` declares id, API/implementation versions, phases, mode, scope, input/output budgets, persistence, permissions, lifecycle events, and optional bundle id.
- `ContextBudget` is the wire envelope ceiling and is intentionally distinct from the runtime `crate::context::budget::ContextBudget` pressure state.
- `ContextCapabilityRequest`, `ContextSnapshot`, and `ContextTransformResult` are serde/schemars DTOs with additive optional budget/fingerprint fields.
- `ContextCapabilityRequest::admit` is the single executable admission funnel. It revalidates the descriptor, checks operation/phase/id/version/request id, resolves legacy or split input/output budgets independently, checks snapshot scope and budgets, and requires a host-bound `AuthenticatedCaller` for session identity.
- `AdmittedContextCall` has private fields and no public constructor. Results are checked only through `ContextTransformResult::validate_for(&AdmittedContextCall)`.
- Cross-session access is fail-closed unless namespace, permission, declared handle, and authenticated caller identity all agree.
- `canonical_fingerprint` reuses `crate::tools::descriptor::canonicalize_json` and SHA-256; the documentation explicitly treats fingerprint equality as correlation, not provenance or authentication.
- Payload redaction is explicitly a later Core-side contract requirement. `serde_json::Value` is not treated as already filtered.
- `src/extension/capability.rs` contains a classification helper only; it is not yet reachable from manifest parsing or a runtime registry, which is Task 2 scope.

## Verification

All commands below were run in `D:/Workspace/Aleph/.worktrees/context-fabric-plugin`:

```text
cargo fmt --check
  exit 0

CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo check -p alephcore --lib
  exit 0; finished dev profile [unoptimized]

CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p alephcore --lib context::fabric
  41 passed; 0 failed; 21423 filtered out

CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p alephcore --lib extension::capability
  8 passed; 0 failed; 21456 filtered out

CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0 cargo clippy -p alephcore --lib -- -D warnings
  exit 0; no clippy warnings/errors

git diff --check
  exit 0
```

The first default-profile test attempt timed out and a second default-profile attempt hit rustc LLVM OOM/`STATUS_STACK_BUFFER_OVERRUN`. The low-debug profile completed successfully. Test-build warnings were pre-existing unused/dead-code warnings in unrelated test modules. The full `cargo test -p alephcore --lib` suite was not run.

## Remaining risks and deferred scope

1. `ContextSnapshot.payload` remains schema-less; Core-side filtering of hidden reasoning, credentials, unauthorised project files, and other sessions is not implemented here.
2. No `CapabilityDeclaration::Context` variant or runtime registry/manifest/lifecycle wiring exists yet.
3. Mode/phase owner coherence and owner-conflict rules remain Task 2/4 work.
4. Persistence rebuildability is declarative; no runtime event replay check exists yet.
5. The 4,000,000-token and 64 MiB hard caps are wire policy and are not yet derived from `ContextBudgetConfig`.
6. No integration test or full `alephcore --lib` test run was performed.
7. Keep the low-debug cargo profile consistent for follow-up builds on this Windows host; mixing debug profiles caused expensive rebuilds and the default-profile LLVM OOM.
