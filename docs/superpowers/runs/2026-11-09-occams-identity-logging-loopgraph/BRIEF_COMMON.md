# Occam's Razor Review Brief: Identity, Logging, and Loop Graph

## 1. Scope & Invariants

This review targets `src/identity`, `src/logging`, and `src/loop_graph` under Occam's Razor doctrine:
- Simplicity is the removal of unnecessary concepts, not merely line count reduction.
- Every abstraction, clone, intermediate allocation, and indirection must earn its existence.

### Subsystem Invariants

#### Identity (`src/identity`)
1. **Cryptographic & Wire Formats**:
   - Ed25519 keypair generation, signature verification, and Blake3/SHA-256 hash digests must preserve exact byte format and hex encoding contracts.
   - Canonical JSON serializations for hash calculations and envelope generation must retain field ordering and whitespace rules.
2. **Ledger Integrity**:
   - Hash chain integrity: Each record references `prev_hash`; index must be strictly monotonic.
   - Ledger writes are append-only.
   - Genesis record, sequence indexing, and transaction rollback boundaries must remain fail-closed.
3. **Actor Scoping**:
   - `tokio::task_local!` actor tracking (`as_actor`, `current_actor`) must preserve scoping rules across asynchronous context boundaries.
4. **Key Lifecycle**:
   - Key states (`Active`, `Retired`, `Revoked`) must follow strict state machine transitions without TOCTOU windows.

#### Logging (`src/logging`)
1. **Level Control & Tracing**:
   - Dynamic tracing filter reloading via `tracing_subscriber::reload::Handle` must preserve thread safety and lock-free / non-blocking hot logging paths.
   - Fallback hierarchy for log directory resolution (`ALEPH_LOG_DIR` -> data dir fallback) must be deterministic and safe across platforms (Linux, macOS, Windows).
2. **Error Semantics**:
   - `LoggingError` variants must be preserved for callers.

#### Loop Graph (`src/loop_graph`)
1. **Topological Invariants & Store**:
   - SQLite tables (`loop_nodes`, `loop_edges`) and indices must match schema expectations.
   - Node and edge CRUD operations must preserve foreign key integrity and prevent orphan dangling references.
   - Cadence ranking, topological ordering, and cycle detection must remain mathematically sound and terminate.
2. **Eventing & Snapshots**:
   - `TopologyEventBus` broadcast semantics: non-blocking dispatch, drop lag handling.
   - `SnapshotStore` sequential event appending and state replay must remain deterministic.
3. **Service Wiring**:
   - Goal/workflow/team settlement notification hooks (`notify_goal_settled`, `notify_workflow_settled`, `notify_team_settled`) are vital orchestration cross-links; their return semantics (`bool` poked) must remain exact.

---

## 2. Methodology & Model Tiering

### Jev Tier (Classification, Filtering, Routing)
The **Jev tier** (fast pattern matching, classification, taxonomy tagging):
1. **Scan**: Scan assigned batch files for Occam's Razor anti-patterns.
2. **Classify**: Categorize candidate issues into:
   - `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
   - `REDUNDANT-ALLOCATION` (unnecessary `.clone()`, `to_string()`, redundant vectors)
   - `CONTROL-FLOW` / `INDIRECTION` (over-nested loops, unneeded wrapper layers)
   - `PANIC-RISK` / `DEFENSIVE-CHECK` (unwrap on untrusted data, missing bounds check)
   - `CORRECTNESS` / `INVARIANT-TIGHTENING` (TOCTOU, silent error discard, off-by-one)
3. **Route**: Tag each finding with:
   - `FIX-NOW`: Direct safe simplification or bug fix.
   - `FIX-CAREFULLY`: Nuanced invariant or allocation tightening requiring careful semantics check.
   - `DEFER`: Architectural or wire-breaking changes.
   - `REJECT`: Deliberate design patterns or required test scaffolding.
4. Output: Write `classification/<BATCH_ID>.class.md`.

### Deep Reasoning Tier (minimax-M3 / Invariant Analysis & Code Repair)
The **Deep Reasoning tier**:
1. Verify semantic preservation for all items routed for fix.
2. Author surgical patches directly in worktree `/home/zou/data/workspace/Aleph-wt-rev-identity-logging-loopgraph`.
3. Apply Occam's Razor decision hierarchy:
   - Borrowed references (`&str`, `&[T]`) instead of owned copies where public API allows.
   - Idiomatic Rust: `?`, `let ... else`, `if let`, early return.
   - Remove unused private helpers and redundant clones.
4. Document findings and verification in `findings/<BATCH_ID>.md`.
5. Direct commit per batch without PR and without interim cargo check.
