# Brief — EE2: src/gateway/execution_engine — part B + run_loop (streaming, cancellation)

**Module area**: `extension` · **Files**: 26 · **LOC**: 21248

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/execution_engine/run_loop/tests.rs` | 3327 |
| `src/gateway/execution_engine/tests.rs` | 3003 |
| `src/gateway/execution_engine/btw_wire_tests.rs` | 2383 |
| `src/gateway/execution_engine/run_loop/inner.rs` | 2007 |
| `src/gateway/execution_engine/run_loop/mod.rs` | 1369 |
| `src/gateway/execution_engine/slash_command.rs` | 1343 |
| `src/gateway/execution_engine/mod.rs` | 1174 |
| `src/gateway/execution_engine/run_loop/flow_scope_census.rs` | 1135 |
| `src/gateway/execution_engine/slash_skill_scope.rs` | 1042 |
| `src/gateway/execution_engine/fast_path.rs` | 689 |
| `src/gateway/execution_engine/engine.rs` | 644 |
| `src/gateway/execution_engine/tool_service_builder.rs` | 529 |
| `src/gateway/execution_engine/session_run_registry.rs` | 507 |
| `src/gateway/execution_engine/scratchpad_progress_sink.rs` | 347 |
| `src/gateway/execution_engine/agent_trace_emit_sink.rs` | 340 |
| `src/gateway/execution_engine/run_loop/project_context.rs` | 323 |
| `src/gateway/execution_engine/markdown_skill_tools.rs` | 232 |
| `src/gateway/execution_engine/settle.rs` | 176 |
| `src/gateway/execution_engine/turn_memory.rs` | 126 |
| `src/gateway/execution_engine/run_loop/author_census.rs` | 115 |
| `src/gateway/execution_engine/trace_sink_adapter.rs` | 114 |
| `src/gateway/execution_engine/persistence.rs` | 107 |
| `src/gateway/execution_engine/adapter.rs` | 82 |
| `src/gateway/execution_engine/concurrency_handle.rs` | 63 |
| `src/gateway/execution_engine/history.rs` | 36 |
| `src/gateway/execution_engine/deadline.rs` | 35 |

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


### Batch-specific focus — `EE2` (execution engine part B + run_loop)

`run_loop` is the streaming loop. Prioritize: cancellation safety (drop mid-stream leaves no half-written state), lock scope across `.await`, and the retry/continuation semantics of the loop.

## Process

1. `graphify query "<question about this area>"` first (orientation).
2. Read every file in the table above, in full.
3. Produce the output file described in `BRIEF_COMMON.md`:
   - cheap tier → `candidates/EE2.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/EE2.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.
