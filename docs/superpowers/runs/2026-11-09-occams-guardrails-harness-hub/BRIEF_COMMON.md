# Occam's Razor Review Brief: Guardrails, Harness, and Hub

## 1. Scope & Invariants

This review targets `src/guardrails`, `src/harness`, and `src/hub` under the Occam's Razor doctrine:
- Simplicity is the removal of unnecessary concepts, not just line count.
- Invariants to strictly preserve:
  - Public API contracts and trait implementations.
  - Serialization / deserialization (serde, wire protocols, schema compatibility).
  - Error semantics, error enums, and conversion boundaries.
  - Lock ordering and async cancellation invariants (`tokio::select!`, `CancellationToken`).
  - Send/Sync guarantees.
- **R10 Harness Redlines (`src/harness`)**:
  - Hard limit of 12 production files (8 top-level + 4 in `agent/`). No new files under `src/harness/`!
  - Line count ratchet governed by `src/harness/tests/budget.rs::CEILING`. Do not increase harness line count without justification answering R10's 3 questions.
  - Cognitive decisions (e.g. prompt wording, recovery policies) belong in prompt / nudges, NOT harness.
- **Guardrails Invariants (`src/guardrails`)**:
  - Screen operations must remain deterministic and fail-closed where appropriate.
  - Secret redaction must preserve structural validity.
- **Hub Invariants (`src/hub`)**:
  - Trust policy, signature / checksum verification, directory sandboxing, and origin security checks cannot be bypassed.
  - Path traversal guards (`..` or absolute paths in archive extraction / resource locator) must be robust.

## 2. Review Methodology

1. **Jev Tier (Classification & Filtering)**:
   - Perform static scan of assigned files.
   - Categorize smells:
     - `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
     - `REDUNDANT-ALLOCATION` (unnecessary `.clone()`, `to_string()`, redundant vectors)
     - `CONTROL-FLOW` / `INDIRECTION` (over-nested loops, unneeded wrapper layers)
     - `PANIC-RISK` / `DEFENSIVE-CHECK` (unwrap on untrusted data, missing bounds check)
     - `CORRECTNESS` / `INVARIANT-TIGHTENING` (TOCTOU, silent error discard, symlink escapes)
   - Assign route tags: `FIX-NOW`, `FIX-CAREFULLY`, `DEFER`, `REJECT`.
   - Write output to `classification/<BATCH_ID>.class.md`.

2. **Deep Reasoning Tier (Analysis & Authoring)**:
   - For items tagged `FIX-NOW` or `FIX-CAREFULLY`, verify behavioral preservation.
   - Author surgical fixes directly in the worktree: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`.
   - Maintain idiomatic Rust without dogma:
     - Prefer borrowed inputs (`&str`, `&[T]`) when caller semantics allow.
     - Early return / `?` / `if let` where clearer.
     - Avoid premature abstractions.
   - Write finding report in `findings/<BATCH_ID>.md`.
   - Commit batch with conventional commits (`fix(guardrails): ...`, `fix(harness): ...`, `fix(hub): ...`).
