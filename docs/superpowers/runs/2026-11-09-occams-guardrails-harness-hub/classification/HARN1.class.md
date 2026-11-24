# Classification Report: Batch HARN1

- **Batch ID**: HARN1
- **Module**: `src/harness` (Core Infrastructure, Agent Struct, and Core Tests)
- **Files Inspected**:
  - `src/harness/mod.rs`
  - `src/harness/deps.rs`
  - `src/harness/trait_def.rs`
  - `src/harness/callback.rs`
  - `src/harness/chain_context.rs`
  - `src/harness/trace.rs`
  - `src/harness/trace_sink.rs`
  - `src/harness/agent.rs`
  - `src/harness/tests/agent.rs`
  - `src/harness/tests/harness_ext.rs`
  - `src/harness/tests/tools_surface.rs`
  - `src/harness/tests/stability.rs`
  - `src/harness/tests/budget.rs`

---

## 1. Inventory & Findings Classification

| ID | Location | Taxonomy Category | Smell / Description | Risk / Impact | Route Tag |
|---|---|---|---|---|---|
| **HARN1-ALLOC-01** | `src/harness/agent.rs:905-968` | `REDUNDANT-ALLOCATION` / `CONTROL-FLOW` | Duplicate `SessionCompleted` trace emission in `match result`. In `Ok(outcome)`, `final_text`, `terminate_reason`, `token_breakdown`, and `tool_timeline` are all `.clone()`d despite never being used again after the match block. | Redundant heap allocations (cloning vectors and string structs) on the run completion path; ~25 lines of duplicate struct initialization. | `FIX-NOW` |
| **HARN1-ALLOC-02** | `src/harness/agent.rs:207-212` | `REDUNDANT-ALLOCATION` | `is_recent_failure` calls `name.to_string()` and `canonical_args.to_string()` on every preflight check of each tool call in every turn to query `HashSet<(String, String)>`. | 2 unnecessary heap allocations per tool call preflight in every turn. | `FIX-NOW` |
| **HARN1-CODE-01** | `src/harness/agent.rs:52-54` | `ACCIDENTAL-COMPLEXITY` | Corrupted doc-comment line on `ToolCallGuardOutcome` (`/// A "failure turn" for the consecutive-failure watchdog: the turn made no net`) left over from an earlier refactor. | Documentation confusion / noise in budgeted file. | `FIX-NOW` |
| **HARN1-CODE-02** | `src/harness/callback.rs:17, 81-85` | `ACCIDENTAL-COMPLEXITY` / `INDIRECTION` | Manual 5-line `impl fmt::Debug for NoopHarnessCallback` and `use std::fmt;` for a unit struct, instead of standard `#[derive(Default, Debug)]`. | Unnecessary boilerplate taking up 6 lines in a budgeted file. | `FIX-NOW` |
| **HARN1-CORR-01** | `src/harness/tests/tools_surface.rs:172` | `CORRECTNESS` / `INVARIANT-TIGHTENING` | `prop_assert_eq!(matches!(a.category, ToolCategory::Builtin), true)` uses boolean equality assertion rather than direct assertion. | Triggers clippy boolean comparison lint. | `FIX-NOW` |
| **HARN1-ALLOC-03** | `src/harness/tests/agent.rs:90` | `REDUNDANT-ALLOCATION` | `AlwaysOkTools::metadata_schema` creates empty schema via `Arc::from(vec![])`. | Heap allocation of temporary empty `Vec`. Can use `Arc::from([])` or `Arc::default()`. | `FIX-NOW` |
| **HARN1-DEFER-01** | `src/harness/trace.rs:128-160` | `ACCIDENTAL-COMPLEXITY` | Legacy `MoAAdvisor`, `MoaAggregating`, `MoaAdvisorSpend`, `MoaTurnTrace` enum variants. | Unused by current runtime, but required to deserialize historical trace blobs without data corruption. | `REJECT` (Preserve backward compatibility) |
| **HARN1-DEFER-02** | `src/harness/deps.rs:188-221` | `INDIRECTION` | `StallTracker` stores `last_activity: Arc<Mutex<Instant>>`. | `Arc` allows unit tests to simulate lock contention across tasks. Changing to `Mutex<Instant>` would break the contention test without real production benefit. | `DEFER` (Preserve concurrency test seam) |
| **HARN1-RATCHET-01** | `src/harness/tests/budget.rs` | `R10-BUDGET` | Line count reduction across `agent.rs` (-29 lines) and `callback.rs` (-6 lines) lowers total budgeted lines from 5534 to 5499. | Ratchet `CEILING` down from 5535 to 5499 to lock in the savings under R10 discipline. | `FIX-NOW` |

---

## 2. Summary of Routes

- **FIX-NOW**: 7 items (HARN1-ALLOC-01, HARN1-ALLOC-02, HARN1-CODE-01, HARN1-CODE-02, HARN1-CORR-01, HARN1-ALLOC-03, HARN1-RATCHET-01)
- **FIX-CAREFULLY**: 0 items
- **DEFER**: 1 item (HARN1-DEFER-02)
- **REJECT**: 1 item (HARN1-DEFER-01)
