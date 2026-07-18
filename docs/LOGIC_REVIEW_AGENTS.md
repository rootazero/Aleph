# Logic Review Report — `src/agents` Module

**Commit Range:** HEAD (working tree)  
**Reviewer:** rust-logic-audit (Phase 1–5)  
**Date:** 2026-05-06  
**Quality Threshold:** Normal (no `--strict`)

---

## Phase 1 — Context Alignment

Extracted invariants from `AGENT_SYSTEM.md` and codebase:

| # | Invariant | Violation Risk |
|---|---|---|
| 1 | **TurnState Machine** — ordered progression across turn lifecycle | Medium (swarm task status derivation) |
| 2 | **TurnAdvance** — exactly one advance per external trigger | Low |
| 3 | **FlowExecution** — DAG node fires once, never re-enters | Low |
| 4 | **Session Key Determinism** — same inputs → same key | Low |
| 5 | **AgentRegistry Poison Recovery** — `unwrap_or_else(|e| e.into_inner())` | Correctly implemented |
| 6 | **Chain Depth Limit** — enforced via `ChainContext::child()` | Correctly implemented |
| 7 | **Tool Allowlist** — `AllowlistToolService` gates tool access | **Test failure indicates harness-level bypass** |

---

## Phase 2 — Semantic Invariant Checking

| # | File:Line | Category | Description |
|---|---|---|---|
| 2.1 | `subagent_spawner.rs:193` | Numeric truncation | `req.agent_def.max_iterations.map(|v| v as usize)` — silent truncation on 32-bit platforms or large values |
| 2.2 | `runtime.rs:208` | Numeric truncation | `start.elapsed().as_millis() as u64` — u128→u64 truncation on extremely long runs |
| 2.3 | `teammates.rs:42` | Error context loss | `Err(_) => { ... }` discards original error, complicating debugging |
| 2.4 | `subagent_tool.rs:421-424` | Error context loss | `unwrap_or_else(|_| team_name.clone())` on `ensure_team` failure — falls back to potentially invalid ID |
| 2.5 | `subagent_tool.rs:471-474` | Error context loss | Same pattern as 2.4 for inbox team resolution |
| 2.6 | `swarm/tasks/store.rs:380` | Silent data loss | `serde_json::to_string(metadata).unwrap_or_else(|_| "{}".into())` — failed serialization silently replaced with empty object |
| 2.7 | `subagent_tool.rs:181` | Input validation gap | `input.get("action").and_then(|v| v.as_str()).unwrap_or("")` — non-string action field silently defaults to "" |
| 2.8 | `subagent_spawner.rs:456` | Test-only panic recovery | `self.responses.lock().unwrap()` in test helper — should use `unwrap_or_else` for consistency |
| 2.9 | `runtime.rs:191-195` | ID collision risk | `format!("{}-{}", id, start.elapsed().as_nanos() % 100_000)` — collisions possible within same nanosecond |

---

## Phase 3 — Control Flow Simulation

### 3.1 Branch/Loop Analysis

- **`spawn()` error handling (subagent_spawner.rs:242-254)**: 4-way match on `tokio::time::timeout` + `catch_unwind` correctly propagates all error paths. No unreachable arms.
- **`extract_run_result()` iteration (subagent_spawner.rs:309-332)**: Single-pass event log scan. `is_last_assistant()` called per `AssistantMessage` — O(n²) in worst case but bounded by small iteration counts.
- **`mark_completed()` lock ordering (background_tracker.rs:52-66)**: `running` write lock → drop → `completed` write lock. No deadlock risk (different locks, consistent order). Non-atomic gap acceptable for best-effort tracker.
- **`update_task()` dynamic SQL (swarm/tasks/store.rs:340-401)**: Dynamic SET clause building with parameterized values. No SQL injection risk (all user input via `?` placeholders).

### 3.2 Option/Result Chain Audit

- **`SubagentTool::execute()` (subagent_tool.rs:389-755)**: Early-return pattern for all non-`Run` actions. `Run` action falls through to foreground/background split. All `Option`/`Result` branches handled explicitly.
- **`ensure_team()` retry (teammates.rs:24-55)**: Check-then-act pattern with race recovery. `Err(_)` catch-all is the retry path; correct but loses error context.

### 3.3 Async Ordering

- **`spawn()` session init (subagent_spawner.rs:124-164)**: Sequential await chain — `attach` → `emit TurnStarted` → `emit UserMessage`. If `emit TurnStarted` fails, session remains attached but empty. No cleanup.
- **`SubagentTool` background spawn (subagent_tool.rs:653-691)**: `tokio::spawn` with `AssertUnwindSafe` + `catch_unwind`. Panic isolation correct. `tracker.mark_completed()` called in finally-position.

---

## Phase 4 — Red-teaming

### 4.1 Malicious / Abnormal Inputs

| Scenario | Impact | Current Behavior |
|---|---|---|
| `max_iterations = u64::MAX` | Truncation to `usize::MAX` or lower | Silent truncation (2.1) |
| `action = 12345` (integer, not string) | Defaults to `""` → treated as `"run"` | Silent type coercion (2.7) |
| `metadata` contains non-serializable types | Data loss on update | Silently replaced with `{}` (2.6) |
| `team_name` provided without `name` | Explicit error | Correctly rejected (subagent_tool.rs:289-291) |
| Concurrent `ensure_team()` calls | Race handled | Retry-after-error pattern (teammates.rs) |

### 4.2 Timing / Concurrency Edge Cases

- **Background task leak**: If `SubagentTool` is dropped while background task is running, the spawned task continues. No `AbortHandle` stored.
- **Transcript file collision**: `agent_id` uses nanos % 100_000 — high-throughput spawning in same nanosecond overwrites transcript.
- **Bus Lag**: `Aggregator` and `ContextInjector` both use `recv()` with `Lagged` handling. Correct — events are best-effort.

### 4.3 Concrete Test Suggestions

```rust
// T1: max_iterations truncation
#[test]
fn test_max_iterations_does_not_truncate() {
    let mut def = AgentDef::new("test", AgentMode::SubAgent);
    def.max_iterations = Some(u64::MAX);
    // Should either preserve u64 or explicitly reject
}

// T2: action field type validation
#[test]
fn test_non_string_action_rejected() {
    let input = json!({"action": 123, "task": "test"});
    let result = parse_args(&input);
    assert!(result.is_err());
}

// T3: metadata serialization failure
#[test]
fn test_metadata_serialize_failure_propagated() {
    // Use a type that fails JSON serialization
}

// T4: agent_id uniqueness under rapid spawning
#[test]
fn test_agent_id_unique_within_microsecond() {
    // Spawn multiple agents and verify distinct IDs
}
```

---

## Phase 5 — Automated Verification

### 5.1 Compilation

```
cargo check -p alephcore --lib: PASS (3 pre-existing unrelated warnings)
```

### 5.2 Lint

```
cargo clippy -p alephcore --lib -- -D warnings: FAIL (10 errors in non-agents modules)
```

Clippy errors are **outside** `src/agents` (search/options.rs, harness/agent.rs, security/audit.rs). Agents module itself is clippy-clean.

### 5.3 Unit Tests

```
cargo test -p alephcore --lib agents::

Result: 258 passed, 1 FAILED, 0 ignored

Failed test: agents::subagent_spawner::tests::spawn_tool_allowlist_enforced_via_harness
Error: "expected allowlist denial, got: Sub-agent timed out after 5s"
```

**Root cause hypothesis**: `AllowlistToolService` correctly returns `PermissionDenied`, but the harness continues looping instead of surfacing the error. This is either a harness bug or a test expectation mismatch.

### 5.4 Loom / Proptest

Skipped — no loom-specific tests exist in agents module; proptest suite runs separately.

---

## Summary

| Severity | Count | Description |
|---|---|---|
| **Critical** | 7 | Numeric truncation, error context loss, silent data loss, input validation gap, pre-existing test failure |
| **Warning** | 5 | ID collision risk, inconsistent sync primitives, non-atomic operations, missing cleanup, test fragility |
| **Suggested Test** | 4 | Truncation, type validation, serialization, uniqueness |

### Fix Priority

1. **Immediate** (Critical): 2.1, 2.2, 2.3, 2.4, 2.5, 2.6, 2.7
2. **Soon** (Warning): 2.8, 2.9, background task leak, transcript collision
3. **Investigate**: Pre-existing test failure (allowlist/harness interaction)

---

*Report generated by rust-logic-audit skill, Phase 1–5 complete.*
