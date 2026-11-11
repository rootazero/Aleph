# Brief — EE1: src/gateway/execution_engine — part A (execute core, tool dispatch)

**Module area**: `extension` · **Files**: 19 · **LOC**: 12940

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/execution_engine/execute.rs` | 3040 |
| `src/gateway/execution_engine/steering.rs` | 1551 |
| `src/gateway/execution_engine/event_drain.rs` | 1489 |
| `src/gateway/execution_engine/helpers.rs` | 1281 |
| `src/gateway/execution_engine/turn_permissions.rs` | 1127 |
| `src/gateway/execution_engine/goal_wait.rs` | 719 |
| `src/gateway/execution_engine/goal_continuation.rs` | 683 |
| `src/gateway/execution_engine/unattended_redacting_sink.rs` | 638 |
| `src/gateway/execution_engine/simple.rs` | 515 |
| `src/gateway/execution_engine/gate.rs` | 399 |
| `src/gateway/execution_engine/run_trace_sinks.rs` | 341 |
| `src/gateway/execution_engine/concurrency.rs` | 336 |
| `src/gateway/execution_engine/btw_promote.rs` | 206 |
| `src/gateway/execution_engine/turn_mode.rs` | 152 |
| `src/gateway/execution_engine/callback.rs` | 124 |
| `src/gateway/execution_engine/turn_model.rs` | 110 |
| `src/gateway/execution_engine/turn_thinking.rs` | 107 |
| `src/gateway/execution_engine/topic.rs` | 82 |
| `src/gateway/execution_engine/tool_refresh.rs` | 40 |

## Area contracts (invariants that must survive any fix)

### Extension area contracts (`src/extension`)

- **Capability principle (repo-wide, R8/R10)**: "Everything externally composable is a Capability." Any capability that can be discovered, invoked, replaced, subscribed to or managed by an Agent/Runtime MUST be expressed through the Capability contract (`src/capability/*`), not through a second, parallel registry/descriptor/handler map. A second owner map for the same fact is a **P1+ duplicate-source-of-truth** finding (judgment shape #1 / #10 in `docs/reference/FEATURE_LOCATOR.md` appendix E).
- **Sandbox**: the WASM runtime (`src/extension/runtime/wasm`) is the isolation boundary for third-party plugin code. Escapes from the host, unbounded host-side allocation driven by guest input, missing fuel/limits, or guest-controlled paths resolved outside the sandbox are **P0/P1**.
- **Trust levels**: plugin trust / capability allowlist checks must be **fail-closed**. A `ResolveError`/denial that is swallowed and turned into a permissive default is **P0/P1** (judgment shape #8: a fail-closed answer consumed as a value flips into permission).
- **Secret hygiene**: credential injection / secret resolution must not log, persist in world-readable files, or place secrets in error messages that reach the UI/log.
- **Manifest/parse**: manifest parsing and schema validation must reject malformed input deterministically; parser must not panic on hostile input; error messages must say what was rejected.
- **Hooks**: hook execution order, consent gating (`hooks/consent.rs`), and idempotency are observable behavior. Consent must be required *before* the effect, and a consent denial must not be downgraded to a warning.
- **Marketplace**: install path is a supply-chain boundary — signature/checksum verification, archive path traversal (`../`), and overwrite of an existing plugin must be checked. Path traversal in the installer is **P0**.
- **Public API**: `pub` items in this module are consumed by the runtime and by the Panel; preserve signatures, error variants and serialization/wire formats.


### Batch-specific focus — `EE1` (execution engine part A)

The execution engine drives tool dispatch. Prioritize: tool-call ordering and result correlation (a result attributed to the wrong call), cancellation handling, and error propagation that loses the tool's real failure.

## Process

1. `graphify query "<question about this area>"` first (orientation).
2. Read every file in the table above, in full.
3. Produce the output file described in `BRIEF_COMMON.md`:
   - cheap tier → `candidates/EE1.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/EE1.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.
