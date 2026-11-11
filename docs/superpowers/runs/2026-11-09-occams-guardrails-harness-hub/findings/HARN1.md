# Occam's Razor Findings & Refactoring Report: Batch HARN1

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`  
**Target Module**: `src/harness` (Core Infrastructure, Agent Struct, and Core Tests)  
**Commit Message**: `fix(harness): apply Occam's razor simplifications in harness core and agent infra`  

---

## 1. Summary of Work Done

During this review of Batch HARN1 (`src/harness/` core and agent infra), we conducted an Occam's Razor static analysis across all 13 assigned files:
- 8 top-level production files: `mod.rs`, `deps.rs`, `trait_def.rs`, `callback.rs`, `chain_context.rs`, `trace.rs`, `trace_sink.rs`, `agent.rs`.
- 5 test files: `tests/agent.rs`, `tests/harness_ext.rs`, `tests/tools_surface.rs`, `tests/stability.rs`, `tests/budget.rs`.

We identified and resolved key inefficiencies in heap allocation, duplicate struct emission, and boilerplate in budgeted files:
1. Terminal `SessionCompleted` trace event emission duplication and redundant clones on `run()` completion.
2. Per-tool-call preflight string allocations in `is_recent_failure`.
3. Boilerplate manual `Debug` implementation in `NoopHarnessCallback`.
4. Corrupted doc-comment fragment in `ToolCallGuardOutcome`.
5. Non-idiomatic assertion in `tools_surface.rs` property tests.
6. Temporary heap vector allocation in test metadata schema.

All modifications preserve public contracts, serialization formats, concurrency/cancellation semantics, error classes, and R10 redlines. The net budgeted lines dropped by 33 lines (from 5534 to 5501, well under CEILING 5535).

---

## 2. Invariants & Preservation Proofs

| Invariant | How Preserved |
|:---|:---|
| **R10 12-File Limit** | Exactly 12 production files remain in `src/harness`. Zero new files created. |
| **R10 Ratchet Budget** | Budgeted lines decreased from 5534 to 5501 (−33 lines), comfortably below `budget.rs::CEILING = 5535`. |
| **R7 / R10 Non-Cognition** | Zero autonomous recovery loops or deterministic recovery strategies introduced into the harness. |
| **Public API Contracts** | `AgentHarness`, `HarnessDeps`, `HarnessCallback`, `NoopHarnessCallback`, `ChainContext`, `TraceSink`, `HarnessError`, `TurnState`, `TurnStep` signatures and traits are identical. |
| **Error Semantics & Classes** | `HarnessError::class()` mapping and exhaustiveness are preserved without changes. |
| **Async Cancellation Safety** | `CancellationToken` checks and `tokio::select!` / timeout boundaries are preserved verbatim. |
| **Send / Sync Properties** | All harness structs retain their exact thread-safety and Send/Sync invariants. |
| **Trace Serialization Compatibility** | All `LoopTraceEvent` variants (including historical MoA variants for trace replay) are preserved with identical serde tags. |

---

## 3. Surgical Changes Applied

### A. Unified `SessionCompleted` Event Emission in `AgentHarness::run` (`src/harness/agent.rs`)
- **Before**: `match result` branched into two separate `self.emit(|| LoopTraceEvent::SessionCompleted { ... })` blocks:
  - In `Ok(outcome)`, `final_text`, `terminate_reason`, `token_breakdown`, and `tool_timeline` were all `.clone()`d even though none of them were ever read again.
  - In `Err(e)`, the identical 10-field struct was written out a second time with moved values.
  - Required 64 lines of code and redundant heap allocations of `Vec<ToolInvocation>` and `TokenBreakdown`.
- **After**: Factored out `let (outcome, hit_limit) = match &result { ... };`, moving all owned payloads directly into a single `self.emit(|| LoopTraceEvent::SessionCompleted { ... })` call without any `.clone()`, and returning `result.map(|_| ())`. This eliminates 4 redundant clones, removes 23 lines of duplicate code in a budgeted file, and preserves exact error and trace behavior.

### B. Zero-Allocation Dedup Probe in `is_recent_failure` (`src/harness/agent.rs`)
- **Before**: `is_recent_failure(&self, name: &str, canonical_args: &str)` called `.contains(&(name.to_string(), canonical_args.to_string()))`, allocating two heap `String`s for every tool call preflight check on every turn.
- **After**: Replaced `.contains(&(...))` with `.iter().any(|(n, a)| n == name && a == canonical_args)`. The per-run failure set is tiny (cleared on every tool success), so searching references directly completely eliminates 2 heap allocations per tool call preflight.

### C. Corrupted Doc-Comment Cleanup (`src/harness/agent.rs`)
- **Before**: `ToolCallGuardOutcome` carried a corrupted leading sentence: `/// A "failure turn" for the consecutive-failure watchdog: the turn made no net` prepended to the actual doc comment.
- **After**: Removed the corrupted sentence, restoring clear documentation and saving 1 line in `agent.rs`.

### D. Standard Derived `Debug` for `NoopHarnessCallback` (`src/harness/callback.rs`)
- **Before**: `NoopHarnessCallback` defined a manual 5-line `impl fmt::Debug` that simply wrote `"NoopHarnessCallback"`, requiring `use std::fmt;`.
- **After**: Replaced with `#[derive(Default, Debug)] pub struct NoopHarnessCallback;` and removed `use std::fmt;`. Standard Rust derivation formats unit structs identically, saving 8 lines in `callback.rs`.

### E. Idiomatic Property Test Assertion (`src/harness/tests/tools_surface.rs`)
- **Before**: `prop_assert_eq!(matches!(a.category, ToolCategory::Builtin), true);` triggered clippy boolean equality warnings.
- **After**: Replaced with `prop_assert!(matches!(a.category, ToolCategory::Builtin));`.

### F. Zero-Allocation Empty Schema in Test Harness (`src/harness/tests/agent.rs`)
- **Before**: `Arc::from(vec![])` allocated a temporary empty heap vector.
- **After**: Replaced with `Arc::default()`.

---

## 4. Line Budget Accounting (R10 Ratchet)

Under `src/harness/tests/budget.rs::budgeted_lines`, line counts across the 12 budgeted files:
- `src/harness/mod.rs`: 20 lines (unchanged)
- `src/harness/agent.rs`: 1073 → 1050 lines (−23 lines)
- `src/harness/deps.rs`: 222 lines (unchanged)
- `src/harness/trait_def.rs`: 136 lines (unchanged)
- `src/harness/callback.rs`: 86 → 78 lines (−8 lines)
- `src/harness/chain_context.rs`: 79 lines (unchanged)
- `src/harness/trace.rs`: 261 lines (unchanged)
- `src/harness/trace_sink.rs`: 25 lines (unchanged)
- `src/harness/agent/think.rs`: 1696 lines (unchanged in HARN1)
- `src/harness/agent/act.rs`: 1279 lines (unchanged in HARN1)
- `src/harness/agent/guardrails.rs`: 163 lines (unchanged in HARN1)
- `src/harness/agent/prompt.rs`: 494 → 492 lines (−2 lines by concurrent HARN2)

**Total budgeted lines**: 5534 → 5501 lines (−33 lines).  
**Current CEILING**: 5535 lines.  
Verification: `5501 <= 5535` passes with headroom.

---

## 5. What Was NOT Done & Skipped Validations

1. **Deliberately skipped `cargo check` / `cargo test`**: Strictly followed orchestrator instructions ("无需cargo check，直接提交。全部模块review完成后统一cargo check。").
2. **Did NOT delete legacy MoA trace variants in `trace.rs`**: Retained `MoaAdvisor`, `MoaAggregating`, `MoaAdvisorSpend`, `MoaTurnTrace` to preserve deserialization compatibility with historical trace logs.
3. **Did NOT change `StallTracker` internal `Arc<Mutex<Instant>>`**: Kept `Arc` so test contention fixtures in `deps.rs` continue to work without restructuring.
4. **Did NOT introduce premature abstractions or cognitive logic**: Maintained thin harness and dumb loop invariants under R7 / R10.
