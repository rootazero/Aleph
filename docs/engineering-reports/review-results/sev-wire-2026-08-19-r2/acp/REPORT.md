# Severed-wire audit — `src/acp/` (2026-08-19 round)

Scope: `src/acp/` (~18 files across `mod.rs`, `adapter.rs`, `adapters/{custom,generic,mod}.rs`,
`incoming.rs`, `manager/{mod,harness_admin,lifecycle,persistence,session_key,tests}.rs`,
`mock_server.rs`, `output_format.rs`, `protocol.rs`, `session.rs`, `tests.rs`,
`transport.rs`) + `src/config/types/acp.rs` (consumed only by ACP).

Method: skill methodology — 7 seam lenses (registration parity,
call-vs-handler, classifier-vs-handler, event emit-vs-subscribe,
config-reader, path/route, stub sweep). Read-first triage per
`triage-playbook.md`.

## Module map

ACP = Agent Communication Protocol (sister of A2A). Manager pattern over
multiple adapters:

- `AcpAdapter` trait (`adapter.rs`) — backend-agnostic surface.
- `CustomAcpAdapter` / `GenericAcpAdapter` (`adapters/custom.rs`,
  `adapters/generic.rs`) — two built-in adapters.
- `manager/` coordinates adapter lifecycle, persistence, session keys.
- `protocol.rs` carries the `AcpErrorCode` enum + `AcpRequest` envelope.
- `mock_server.rs` is the in-process mock harness used by tests.

## Findings

### CUT (4)

- **`acp-01` CUT (low)** — `src/acp/protocol.rs:321,348,367,639`
  `AcpErrorCode::HarnessUnavailable` had **zero production callers**.
  Verified by `grep -rn "HarnessUnavailable\|harness_unavailable"
  src/ interfaces/ shared/ desktop/`: only references were the variant
  definition, the `as_str()` match arm, the `is_retryable()` match arm,
  and one `assert!(AcpErrorCode::HarnessUnavailable.is_retryable())`
  test. No producer; nothing ever returns this code. Variant removed;
  `as_str()` arm removed; `is_retryable()` arm removed; test assertion
  removed.

- **`acp-02` CUT (low)** — `src/acp/protocol.rs:329,354,644,655`
  `AcpErrorCode::Cancelled` — same pattern. The doc comment on
  `as_str()` calls these tokens *"part of the gateway contract — never
  rename without migrating panel + downstream consumers"*, but no
  downstream consumer was found (the matching `cancelled` literals
  elsewhere in the workspace all live in unrelated enums:
  `WizardStatus::Cancelled`, `ClarificationResult::cancelled`,
  `GenerationError::cancelled`). Variant removed; `as_str()` arm
  removed; two test assertions removed.

- **`acp-03` CUT (low)** — `src/acp/mock_server.rs:89` (legacy
  `"prompt"` arm). The producer side of every prompt RPC emits
  `AcpRequest::prompt` which always sets `method: "session/prompt"`
  (verified at `protocol.rs:87`, `:451`, plus all `tests.rs:36, 101,
  133, 447`). The mock arm `"prompt" =>` only ever matches the legacy
  bare-name form, which no test exercises either
  (`mock_server_prompt` test at `tests.rs:442` uses
  `build_request_line("session/prompt", ...)`). The arm and its
  preceding "Support legacy method names for backwards compatibility"
  comment are removed.

- **`acp-04` CUT (low)** — `src/acp/mock_server.rs:105` (legacy
  `"cancel"` alias in `"session/cancel" | "cancel"`). Same rationale:
  `AcpRequest::cancel` always emits `"session/cancel"` (verified at
  `protocol.rs:101`, `:443`, `:452`, `:459`, `:594`; `tests.rs:48, 56,
  463, 487`). Alias collapsed to `"session/cancel" =>`.

### DECIDE (1, deferred)

- **`acp-05` DECIDE (very-low)** —
  `src/acp/manager/mod.rs:20`
  `pub use persistence::{load_persisted_sessions, save_persisted_sessions, wire_persistence};`
  re-exports two functions that have **zero external callers** —
  `grep -rn 'load_persisted_sessions\|save_persisted_sessions' src/
  interfaces/ shared/ desktop/` shows only the `pub` definition at
  `manager/persistence.rs:24,36` and the two internal callers at
  `manager/persistence.rs:59,123`. Borderline: defensible as a small
  public API for tests / diagnostics / future tool callers. Listed,
  not fixed.

## Already-clean surfaces (no action)

The 7 seam lenses produce no other findings:

- Every `AcpAdapter` trait method (`execute_oneshot`, `spawn_session`,
  `cancel_session`, etc.) is either overridden by both built-in adapters
  or wired through `manager/mod.rs` via `wire_persistence`.
- `GenericAcpAdapter::new(...)` (programmatic 7-arg constructor at
  `adapters/generic.rs:68-89`) — looks like dead parallel API vs.
  `from_entry`, but is the only path for runtime-only construction
  without an `AcpAdapterEntry`. Tested by
  `test_generic_harness_build_config*`. Kept.
- Stub sweep: zero `unimplemented!()` / `todo!()` / `// TODO` hits in
  production code under `src/acp/`.
- Config parity: `src/config/types/acp.rs` fields are all read in
  `manager/{harness_admin,lifecycle}.rs`.
- Path/route parity: `AcpRequest::prompt` always emits `session/prompt`;
  `AcpRequest::cancel` always emits `session/cancel`. Method-name
  drift absent (form 5).

## Cross-cutting concerns

None. No `Cargo.toml`, top-level `src/lib.rs`, or other-module changes
required.

## Almost-cut, kept (with reasoning)

- `AcpAdapter::execute_oneshot` default impl (`adapter.rs:115-122`)
  looks like a stub (`Err(tool("Harness does not support oneshot"))`),
  but it's the legitimate "no support" sentinel for the trait. Both
  built-in adapters override; external implementors may rely on it.
  Kept.
- `AcpAdapter::spawn_session` default impl (`adapter.rs:94-104`)
  looks orphan at a glance but is reached through static dispatch by
  `CustomAcpAdapter` for `NativeAcp`-mode custom harnesses. Kept.