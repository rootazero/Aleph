# Occam's Razor Review Brief: Orchestrator, PII, and Projects

## 1. Scope & Subsystem Invariants

This review targets `src/orchestrator`, `src/pii`, and `src/projects` under Occam's Razor doctrine:
- **Simplicity is the removal of unnecessary concepts, not merely line count reduction.**
- Every abstraction, clone, intermediate allocation, and indirection must earn its existence.
- Non-negotiable invariants (public API surface, error semantics, database schema, concurrency properties, and async cancellation safety) must remain completely stable.

---

### Subsystem Invariants

#### 1. PII (`src/pii`)
1. **Rule Engine & Detection (`engine.rs`, `rules/`)**:
   - Severity ordering (`PiiSeverity::Critical > High > Medium > Low > Info`) and overlap disambiguation must preserve priority resolution.
   - Masking and redaction strings must never leak raw token slices.
   - Regex patterns must remain thread-safe (`OnceLock` / `LazyLock` or pre-compiled).
   - Zero-allocation borrowing (`&str` or match indices) where possible before committing redaction masks.
2. **Allowlist & Platform Filtering (`allowlist.rs`, `engine.rs`)**:
   - Case-insensitive platform normalization, exact match exclusion, and CIDR / domain checks must preserve fail-closed semantics.

#### 2. Projects (`src/projects`)
1. **Storage & Transactional Invariants (`store.rs`, `authz.rs`)**:
   - SQLite table schema (`projects`, `project_members`, `project_workspaces`, `project_bindings`) and migrations must remain backwards-compatible.
   - Member roles (`ProjectRole::Admin`, `Member`, `Viewer`) and authorization checks must fail closed.
   - Lock acquisition order between project stores and roster publishers must avoid inversion.
2. **Attribution, Events & Context (`attribution_backfill.rs`, `events.rs`, `run_context.rs`, `binding.rs`)**:
   - Workspace paths, directory normalization, and canonical paths must preserve cross-platform consistency.
   - Event publication to project streams must guarantee at-least-once ordering.

#### 3. Orchestrator (`src/orchestrator`)
1. **Dispatch & Lifecycle (`dispatch.rs`, `resolver.rs`, `flow_registry.rs`)**:
   - Flow execution state machine transitions (`Init` -> `Running` -> `Completed`/`Failed`/`Cancelled`) must be strictly atomic.
   - `CancellationToken` propagation must be honored across all async joins and child task branches.
   - Redundant clones on flow payloads and agent state must be eliminated.
2. **Dependencies & Budget (`deps_builder/`)**:
   - Token budget math, context reservation windows, and provider fallbacks must preserve arithmetic bounds without saturation panic.
3. **Harness Bridge (`harness_bridge/`)**:
   - Bridge adapters between orchestrator flows and execution harness runners must avoid unnecessary intermediate boxing and serialization copies.
   - Replay adapter and session seed hydration must preserve deterministic event sequencing.

---

## 2. Methodology & Model Tiering

### Jev Tier (Classification, Filtering, Routing)
The **Jev tier** (lightweight fast-scanning tier using Flash models):
1. **Candidate Sweep**: Scan assigned batch files for Occam's Razor anti-patterns:
   - Redundant allocations: repeated `format!`, unnecessary `.to_string()`, `.to_vec()`, `.clone()`.
   - Redundant clones on types that already implement `Copy` or where borrowed references `&str` / `&[T]` suffice.
   - Over-nested control flow: `if let Some(...) { if let Some(...) { ... } }` flattening.
   - Unused private helper functions, dead code, stale imports.
   - Unnecessary `Box<dyn ...>` or trait indirection where concrete types or generic parameters are simpler.
2. **Taxonomy Classification**: Classify candidate issues into:
   - `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
   - `REDUNDANT-ALLOCATION`
   - `CONTROL-FLOW` / `INDIRECTION`
   - `PANIC-RISK` / `DEFENSIVE-CHECK`
3. **Smell Routing**: Assign each finding an action:
   - `FIX-NOW`: High confidence, clean win, invariant-safe.
   - `REJECT`: Unnecessary abstraction, stylistic churn, or violates non-negotiable invariants.

### Reasoning Tier (Compiler-grade Invariants & AST Mutation)
Parent agent executes:
- Topological cross-checks against the semantic graph (`graphify-out/`).
- Invariant safety checks.
- Code mutation and verification in worktree.
- Directly commit to branch without PR.
