# Brief — EE3: src/gateway/execution_engine — tests + slash command body + skill pregrant + btw_promote

**Module area**: `extension` · **Files**: 14 · **LOC**: 6432

## Files in this batch (read ALL of them, in full)

| File | LOC |
|---|---|
| `src/gateway/execution_engine/slash_command_body/tests.rs` | 1504 |
| `src/gateway/execution_engine/tests/loop_origin_race.rs` | 960 |
| `src/gateway/execution_engine/tests/goal_origin_race.rs` | 721 |
| `src/gateway/execution_engine/tests/goal_origin_effects.rs` | 650 |
| `src/gateway/execution_engine/slash_command_body/mod.rs` | 495 |
| `src/gateway/execution_engine/slash_skill_pregrant/tests/mod.rs` | 402 |
| `src/gateway/execution_engine/btw_promote/tests.rs` | 362 |
| `src/gateway/execution_engine/slash_skill_pregrant/mod.rs` | 251 |
| `src/gateway/execution_engine/slash_skill_pregrant/tests/what_may_be_pregranted.rs` | 251 |
| `src/gateway/execution_engine/slash_skill_pregrant/tests/who_may_pregrant.rs` | 226 |
| `src/gateway/execution_engine/slash_skill_pregrant/tests/grant_arm.rs` | 205 |
| `src/gateway/execution_engine/slash_skill_pregrant/tests/scope_and_children.rs` | 185 |
| `src/gateway/execution_engine/slash_skill_pregrant/tests/fire_sites.rs` | 175 |
| `src/gateway/execution_engine/slash_skill_pregrant/tests/kind_and_key.rs` | 45 |

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



## Process

1. `graphify query "<question about this area>"` first (orientation).
2. Read every file in the table above, in full.
3. Produce the output file described in `BRIEF_COMMON.md`:
   - cheap tier → `candidates/EE3.md` (candidate table only, no verdicts)
   - reasoning tier → `findings/EE3.md` (verified findings with severity + minimal fix)
4. Do not edit source files. Do not run cargo. Do not touch git state.

## Anti-patterns for this batch

- Do not report "this file is large" or "this module is complex" as a finding.
- Do not report missing tests as a finding (test gaps are noted in the round README, not as findings).
- Do not propose a redesign; propose the smallest change that removes the defect while preserving the
  contracts above.
