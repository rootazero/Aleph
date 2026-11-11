# Occam's Razor Classification Report: Batch HARN3

**Run ID**: `2026-11-09-occams-guardrails-harness-hub`  
**Batch ID**: `HARN3`  
**Target Module**: `src/harness` (Act, Guardrails, and Task10 Wiring)  
**Assigned Files**:
- `src/harness/agent/act.rs` (1279 LOC)
- `src/harness/agent/guardrails.rs` (163 LOC)
- `src/harness/tests/act.rs` (3145 LOC)
- `src/harness/tests/guardrails.rs` (1492 LOC)
- `src/harness/tests/task10_wiring/mod.rs` (1584 LOC)
- `src/harness/tests/task10_wiring/extras.rs` (1567 LOC)

---

## 1. Inventory & Findings

### Finding HARN3-01: Redundant String Allocations in Batch Duplicate Detection
- **File**: `src/harness/agent/act.rs:160-180`
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Route Tag**: `FIX-NOW`
- **Description**:
  In `AgentHarness::act`, the batch duplicate probe previously cloned `call.name` (a `String`) and `canon` (canonical JSON `String`) into a `HashSet<(String, String)>` for every tool call in the batch. Moreover, it continued looping across the entire batch even after a duplicate was detected.
- **Remediation**:
  Compute `canonical` strings into `Vec<String>`, then detect duplicates across `tool_calls.iter().zip(&canonical)` using `seen: HashSet<(&str, &str)>`. This avoids `2 * N` heap string allocations per batch and breaks immediately on the first duplicate found (matching the zero-allocation pattern in `can_parallel_dispatch`).

---

### Finding HARN3-02: Unconditional Deep Clone of `ToolOutput` on Within-Batch Memo Hit
- **File**: `src/harness/agent/act.rs:545-565`
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Route Tag**: `FIX-NOW`
- **Description**:
  On a within-batch dedup memo hit, `let memo_output = output.clone();` was cloned solely so that `memo_output` could be passed to `callback.on_tool_call_done` and `self.emit` after `output` was moved into `SessionEvent::ToolResult`.
- **Remediation**:
  In line with the "live-before-transcript" ordering, pass borrowed `&output` to `callback.on_tool_call_done` first, lazily clone `output.value` only if a `trace_sink` is present, and then move `output` into `SessionEvent::ToolResult`. This completely removes the redundant deep clone of `ToolOutput`.

---

### Finding HARN3-03: Redundant Clone of Guardrail-Sanitized JSON Args in PASS 1
- **File**: `src/harness/agent/act.rs:900`
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Route Tag**: `FIX-NOW`
- **Description**:
  In `act_parallel`, `sanitized[idx].clone().unwrap_or_else(...)` cloned the sanitized `serde_json::Value`. Because `sanitized` is never read again after constructing the PASS 1 futures, cloning the JSON value is unnecessary.
- **Remediation**:
  Replace `.clone()` with `.take()`, moving the owned `serde_json::Value` directly into the execution future.

---

### Finding HARN3-04: Over-Verbose Range Iterator Collection for `settled` Vector
- **File**: `src/harness/agent/act.rs:949-955`
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Route Tag**: `FIX-NOW`
- **Description**:
  Initializing `settled` used `(0..tool_calls.len()).map(|_| None).collect()`, spanning 7 lines, whereas adjacent vectors use `vec![None; tool_calls.len()]`.
- **Remediation**:
  Use `vec![None; tool_calls.len()]`, simplifying control flow and saving 6 lines against the R10 budget ceiling.

---

### Finding HARN3-05: Unconditional Clone of `output.value` on Tool Success Regardless of Trace Sink
- **File**: `src/harness/agent/act.rs:1099-1120`
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Route Tag**: `FIX-NOW`
- **Description**:
  In `persist_tool_success`, `let output_value = output.value.clone();` unconditionally cloned the tool result's JSON value for `self.emit`. If no `trace_sink` is configured (common in tests and non-trace production runs), `self.emit` does not evaluate its closure, and the cloned value is discarded.
- **Remediation**:
  Guard the clone with `self.deps.trace_sink.as_ref().map(|_| output.value.clone())`.

---

### Finding HARN3-06: Redundant String Allocation for Error Message in `persist_tool_error`
- **File**: `src/harness/agent/act.rs:1255-1277`
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Route Tag**: `FIX-NOW`
- **Description**:
  `let error_for_timeline = error_msg.clone();` was cloned because `error_msg` was moved into `self.emit`.
- **Remediation**:
  Lazily clone for trace only when `trace_sink.is_some()`, and move the original `error_msg` directly into `push_tool_invocation(..., Some(error_msg))`.

---

### Finding HARN3-07: Redundant String Allocation in `settle_blocked_call`
- **File**: `src/harness/agent/guardrails.rs:139-160`
- **Taxonomy**: `REDUNDANT-ALLOCATION`
- **Route Tag**: `FIX-NOW`
- **Description**:
  `push_tool_invocation` received `Some(block_msg.clone())` while `block_msg` was moved into `self.emit`.
- **Remediation**:
  Lazily clone `block_msg` for trace only when `trace_sink.is_some()`, and move `block_msg` directly into `push_tool_invocation(..., Some(block_msg))`.

---

### Finding HARN3-08: Local Mock Duplication Across Harness Test Files
- **File**: `src/harness/tests/task10_wiring/mod.rs:35-180`
- **Taxonomy**: `ACCIDENTAL-COMPLEXITY`
- **Route Tag**: `REJECT`
- **Description**:
  `MockSession` and `NoopTools` appear in multiple test files (`act.rs`, `think.rs`, `task10_wiring/mod.rs`).
- **Rationale for Rejection**:
  Per P6 and Occam's Razor: "Prefer a clear local duplication over a premature abstraction." The test suites are purposefully decoupled so that mock extensions for specific tests do not cause cross-suite coupling.

---

### Finding HARN3-09: Unification of Guardrail Block Handling Between Serial and Parallel Paths
- **File**: `src/harness/agent/act.rs:500-508`, `src/harness/agent/guardrails.rs:116-162`
- **Taxonomy**: `CONTROL-FLOW`
- **Route Tag**: `DEFER`
- **Description**:
  The serial loop and parallel PASS 0 handle `ToolCallGuardOutcome::Block` via `settle_blocked_call` then `continue`. Both branches are already fully settled and unified in `settle_blocked_call`.
- **Rationale for Deferral**:
  The existing division of labor is already minimal and robustly verified by regression tests.

---

## 2. Summary Table

| ID | Location | Taxonomy | Tag | Decision |
|---|---|---|---|---|
| HARN3-01 | `act.rs:160-180` | REDUNDANT-ALLOCATION | FIX-NOW | Eliminate string clones in duplicate check; early break |
| HARN3-02 | `act.rs:545-565` | REDUNDANT-ALLOCATION | FIX-NOW | Eliminate deep clone of `ToolOutput` on memo hit |
| HARN3-03 | `act.rs:900` | REDUNDANT-ALLOCATION | FIX-NOW | Use `.take()` on sanitized args in PASS 1 |
| HARN3-04 | `act.rs:949-955` | ACCIDENTAL-COMPLEXITY | FIX-NOW | Simplify `settled` initialization with `vec![None; n]` |
| HARN3-05 | `act.rs:1099-1120` | REDUNDANT-ALLOCATION | FIX-NOW | Lazily clone `output.value` only when trace sink is active |
| HARN3-06 | `act.rs:1255-1277` | REDUNDANT-ALLOCATION | FIX-NOW | Move `error_msg` into timeline; trace clone is conditional |
| HARN3-07 | `guardrails.rs:139-160` | REDUNDANT-ALLOCATION | FIX-NOW | Move `block_msg` into timeline; trace clone is conditional |
| HARN3-08 | `task10_wiring/mod.rs:35` | ACCIDENTAL-COMPLEXITY | REJECT | Keep mock definitions decoupled across test files |
| HARN3-09 | `act.rs:500-508` | CONTROL-FLOW | DEFER | Keep existing verified control flow for blocked calls |
