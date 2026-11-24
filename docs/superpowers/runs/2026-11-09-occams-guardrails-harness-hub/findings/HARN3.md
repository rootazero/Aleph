# Occam's Razor Findings & Refactoring Report: Batch HARN3

**Target Worktree**: `/home/zou/data/workspace/Aleph-wt-occams-guard-harness-hub`  
**Target Module**: `src/harness` (Act Phase, Guardrails, and Task10 Wiring)  
**Commit Message**: `fix(harness): apply Occam's razor simplifications in tool act and guardrails execution`  

---

## 1. Summary of Work Done

During this review of Batch HARN3 (`src/harness/agent/act.rs`, `src/harness/agent/guardrails.rs`, and associated tests), we conducted an Occam's Razor static analysis across all 6 assigned files:
- 2 budgeted production files: `src/harness/agent/act.rs`, `src/harness/agent/guardrails.rs`.
- 4 test files: `src/harness/tests/act.rs`, `src/harness/tests/guardrails.rs`, `src/harness/tests/task10_wiring/mod.rs`, `src/harness/tests/task10_wiring/extras.rs`.

We identified and resolved key inefficiencies in heap allocation, redundant clones, and verbose control flow:
1. **Zero-Allocation Batch Duplicate Detection**: Replaced per-call `(String, String)` clones in `act()` with borrowed `(&str, &str)` comparisons, terminating early on the first duplicate via `.any(...)`.
2. **Elimination of Deep `ToolOutput` Clone on Memo Hit**: Refactored the within-batch dedup path in `act.rs` to borrow `&output` for `callback.on_tool_call_done`, clone the trace value lazily only when a `trace_sink` is present, and move `output` directly into `SessionEvent::ToolResult`.
3. **Move Guardrail-Sanitized Args**: In `act_parallel` PASS 1, switched `sanitized[idx].clone()` to `.take()`, moving the owned `serde_json::Value` directly into the spawned execution future without cloning.
4. **Idiomatic Vector Initialization for `settled`**: Replaced verbose range-map-collect with `vec![None; tool_calls.len()]`, simplifying code and reducing lines.
5. **Conditional Trace Clones in `persist_tool_success` & `persist_tool_error`**: Avoided unconditional deep clones of `output.value` and `error_msg` when no `trace_sink` is active; moved `error_msg` directly into `push_tool_invocation`.
6. **Conditional Trace Clone in `settle_blocked_call`**: Avoided cloning `block_msg` into `push_tool_invocation` by moving it directly and evaluating trace output lazily.

All modifications strictly preserve parallel/serial dispatch invariance, cancellation safety, approval integration, guardrail fail-closed behavior, error pairing, and R10 redlines.

Line counts in budgeted files decreased:
- `act.rs`: 1279 → 1278 lines (−1 line)
- `guardrails.rs`: 163 → 159 lines (−4 lines)
Total harness budgeted lines dropped to 5476 lines, comfortably under `budget.rs::CEILING = 5535`.

---

## 2. Invariants & Preservation Proofs

| Invariant | How Preserved |
|:---|:---|
| **R10 12-File Hard Limit** | Exactly 12 production files remain in `src/harness` (8 top-level + 4 under `agent/`). Zero new files created. |
| **R10 Ratchet Budget** | Line counts decreased in both touched files (`act.rs` −1, `guardrails.rs` −4). Total budgeted lines = 5476 <= 5535. |
| **R7 / R10 Non-Cognition** | Zero autonomous recovery loops or deterministic recovery heuristics introduced. Scaffolding-only optimizations. |
| **Tool Dispatch Invariants** | Parallel and serial fast paths, resource claims partitioning, within-batch dedup, and cross-batch refusal behaviors are identical. |
| **Error Pairing & Durability** | Every tool invocation (success, error, memo-hit, guardrail-block, deferred) preserves exact `SessionEvent` pairing and timeline updates. |
| **Cancellation Safety** | `CancellationToken` checkpoints (at group boundaries and per-tool child tokens) and in-flight registrations are preserved verbatim. |
| **Guardrail Redaction & Sanitization** | Fail-closed reparse checks and redactions remain byte-identical; live callbacks and trace completions fire symmetrically. |
| **Send / Sync & Lifetime Guarantees** | Owned futures in `act_parallel` and task-local ambient call identity boundaries remain intact. |

---

## 3. Surgical Changes Applied

### A. Zero-Allocation Duplicate Probe in `AgentHarness::act` (`src/harness/agent/act.rs`)
- **Before**: Pre-collected `seen: HashSet<(String, String)>`, cloning `call.name` and canonical arg strings for every call in the batch.
- **After**: Computed `canonical: Vec<String>` once, then checked duplicates via:
  ```rust
  let mut seen = std::collections::HashSet::new();
  let has_duplicate = tool_calls
      .iter()
      .zip(&canonical)
      .any(|(call, canon)| !seen.insert((call.name.as_str(), canon.as_str())));
  ```
  Eliminates `2 * N` heap string allocations per batch and short-circuits on the first duplicate.

### B. Eliminate Deep Clone on Memo Hit (`src/harness/agent/act.rs`)
- **Before**: `let memo_output = output.clone();` was cloned solely so that `memo_output` could be used for `on_tool_call_done` and `self.emit` after `output` was moved into `SessionEvent::ToolResult`.
- **After**: Applied the repo's established "live-before-transcript" order:
  1. Borrow `&output` for `callback.on_tool_call_done(&call.id, Some(&output), None, 0)`.
  2. Lazily clone `output.value` only if `self.deps.trace_sink.is_some()`.
  3. Move `output` into `SessionEvent::ToolResult`.
  Completely eliminates the redundant deep clone of `ToolOutput`.

### C. Move Sanitized JSON Value in `act_parallel` PASS 1 (`src/harness/agent/act.rs`)
- **Before**: `sanitized[idx].clone().unwrap_or_else(|| call.arguments.clone())` cloned the sanitized JSON object.
- **After**: Replaced with `sanitized[idx].take().unwrap_or_else(|| call.arguments.clone())`, moving the owned `serde_json::Value` into the execution future.

### D. Simplified `settled` Vector Initialization (`src/harness/agent/act.rs`)
- **Before**: `(0..tool_calls.len()).map(|_| None).collect()` spanning 7 lines.
- **After**: `vec![None; tool_calls.len()]`.

### E. Lazy Trace Clones in `persist_tool_success` and `persist_tool_error` (`src/harness/agent/act.rs`)
- **Before**: Unconditional deep clones of `output.value` and `error_msg` for `self.emit`.
- **After**: Only clone if `trace_sink.is_some()`; moved `error_msg` directly into `push_tool_invocation`.

### F. Lazy Trace Clone and Direct Move in `settle_blocked_call` (`src/harness/agent/guardrails.rs`)
- **Before**: `push_tool_invocation` received `Some(block_msg.clone())` while `block_msg` was moved into `self.emit`.
- **After**: Only clone `block_msg` if `trace_sink.is_some()`, moving `block_msg` directly into `push_tool_invocation`. Compacted `format!` reason binding for sanitize error.

---

## 4. Line Budget Accounting (R10 Ratchet)

Under `src/harness/tests/budget.rs::budgeted_lines`, line counts across the 12 budgeted files:
- `src/harness/mod.rs`: 20 lines (unchanged)
- `src/harness/agent.rs`: 1050 lines (unchanged in HARN3)
- `src/harness/deps.rs`: 222 lines (unchanged)
- `src/harness/trait_def.rs`: 136 lines (unchanged)
- `src/harness/callback.rs`: 78 lines (unchanged in HARN3)
- `src/harness/chain_context.rs`: 79 lines (unchanged)
- `src/harness/trace.rs`: 261 lines (unchanged)
- `src/harness/trace_sink.rs`: 25 lines (unchanged)
- `src/harness/agent/think.rs`: 1680 lines (modified by concurrent HARN2)
- `src/harness/agent/act.rs`: 1279 → 1278 lines (−1 line)
- `src/harness/agent/guardrails.rs`: 163 → 159 lines (−4 lines)
- `src/harness/agent/prompt.rs`: 488 lines (modified by concurrent HARN2)

**Total budgeted lines**: 5476 lines.  
**Current CEILING**: 5535 lines.  
Verification: `5476 <= 5535` passes with 59 lines of headroom.

---

## 5. What Was NOT Done & Skipped Validations

1. **Deliberately skipped `cargo check` / `cargo test`**: Strictly followed instructions ("无需cargo check，直接提交。全部模块review完成后统一cargo check。").
2. **Did NOT unify test mock structs across test files**: Kept `MockSession` and `NoopTools` localized in `tests/act.rs`, `tests/guardrails.rs`, and `tests/task10_wiring/mod.rs` to prevent cross-test coupling.
3. **Did NOT alter tool completion ordering or timing semantics**: Preserved the live-event completion order (`buffer_unordered`) and transcript input order (`settled` walk).
4. **Did NOT introduce cognitive recovery or heuristic retries**: Guardrails and act phase remain strict scaffolding under R7 / R10.
