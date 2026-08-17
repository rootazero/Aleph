# src/context — Severed Wire Audit (2026-08-17 round)

**Scanned:** 21 files under `src/context/` (incl. `budget/`, `budget/cheap_passes/`, `compact/`, `retrieval/`)

**Summary:** 1 candidate (0 CUT, 0 CONNECT, 1 DECIDE)

---

## context-001 · DECIDE · high confidence

- **Form:** inert_config · **Seam:** config_reader
- **Producer:** `src/context/budget/mod.rs:186` — `ContextBudgetConfig::diminishing_window` / `diminishing_threshold`
- **Consumer:** none (only `assert_cfg_eq` at `orchestrator/deps_builder/context_budget.rs:1160,1162` compares two configs that carry identical hardcoded values — no decision ever branches on the fields)
- **Rationale:** The fields are SET in production wiring (orchestrator/deps_builder/context_budget.rs:423-424 with hardcoded values 4 and 500; context/budget/mod.rs:281-289 inside ContextBudget::new; and a parallel hardcoded pair in tests) but NEVER READ by any non-test logic to make a decision. No code path consults them, no `ContextBudget` field stores them, no getter. The comment "deliberately not exposed as toml knobs (KISS: every run inherits the same compaction cadence)" suggests a "diminishing returns" detector that was never implemented.

**Trade-off:**
- **CONNECT:** implement `ContextBudget::diminishing_returns_detected(&self) -> bool` that consults the last `diminishing_window` `note_compaction_effect` calls and returns true when each freed fewer than `diminishing_threshold` tokens, then bias circuit breaker / escalate from `CompactAndContinue` to `CompactToFit`. This is a real new feature, not a wire fix.
- **CUT:** delete the two struct fields, drop them from every construction site, remove them from `assert_cfg_eq`. Painless-wire heuristic strongly favors CUT: zero observed production pain from a missing detector, no decision branches on the values.

**Decision:** CUT (painless-wire heuristic; the comment itself says "deliberately not exposed").