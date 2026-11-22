# Occam's Razor Static Analysis Classification Report: Resilience Batch

**Date:** 2026-11-11  
**Batch:** `RESILIENCE`  
**Subsystem:** `src/resilience`  
**Review Model:** Jev Static Analysis Tier (Flash Model)  
**Doctrinal Basis:** `/rust-occams-razor` and `BRIEF_COMMON.md`

---

## 1. Executive Summary

This report performs static analysis, smell classification, and action routing across all 12 source files in the Resilience batch (`src/resilience`).

### Candidate Counts by Taxonomy
- **`REDUNDANT-ALLOCATION`**: 11 candidates
- **`ACCIDENTAL-COMPLEXITY`**: 4 candidates
- **`CONTROL-FLOW` / `INDIRECTION`**: 3 candidates
- **`DEAD-CODE`**: 2 candidates
- **`CORRECTNESS` / `INVARIANT-TIGHTENING`**: 1 candidate

### Candidate Counts by Action Routing
- **`FIX-NOW`**: 15 findings (safe, high-confidence, non-breaking, eliminates allocations and unnecessary code)
- **`FIX-CAREFULLY`**: 0 findings (all eligible items are localized and strictly non-breaking)
- **`DEFER`**: 2 findings (broader schema migration and cross-subsystem boot recovery integration)
- **`REJECT`**: 4 findings (intentional design contracts, API compatibility constraints, and error type ergonomics)

---

## 2. Inventory of Inspected Files

| # | File Path | Lines | Primary Responsibility |
|---|-----------|-------|------------------------|
| 1 | `src/resilience/mod.rs` | 12 | Subsystem facade and public re-exports |
| 2 | `src/resilience/types.rs` | 529 | Core domain types: `TaskStatus`, `RiskLevel`, `Lane`, `AgentTask`, `TaskTrace` |
| 3 | `src/resilience/database/mod.rs` | 39 | Internal submodule exports and shared helper `i64_to_u64_count` |
| 4 | `src/resilience/database/channel_offsets.rs` | 69 | Telegram / channel cursor resume persistence |
| 5 | `src/resilience/database/group_chat.rs` | 186 | Group chat session and turn persistence |
| 6 | `src/resilience/database/migration.rs` | 910 | Idempotent SQLite migrations with savepoint isolation |
| 7 | `src/resilience/database/tasks.rs` | 553 | `AgentTask` CRUD, atomic state reconciliation, and orphaned task sweeps |
| 8 | `src/resilience/database/traces.rs` | 806 | `TaskTrace` logging, cursor pagination, and per-agent usage rollups |
| 9 | `src/resilience/database/memory_events.rs` | 947 | Append-only partitioned memory event sourcing and seq generation |
| 10 | `src/resilience/database/state_database/mod.rs` | 513 | `StateDatabase` struct, connection initialization, and vector extension loader |
| 11 | `src/resilience/database/state_database/schema.rs` | 368 | Core DDL definitions and obsolete table drops |
| 12 | `src/resilience/database/state_database/tests.rs` | 282 | In-memory SQLite regression and schema verification tests |

---

## 3. Detailed Findings by Taxonomy and Routing

### A. High-Confidence Immediate Fixes (`FIX-NOW`)

#### Item 1: Zero-Allocation `FromStr` & Static `as_str` Accessors for Enums
- **Location:** [`src/resilience/types.rs:83, 133, 178`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/types.rs#L83)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `TaskStatus::from_str`, `RiskLevel::from_str`, and `Lane::from_str` call `s.to_lowercase()`, which allocates an owned heap `String` on every invocation. In `from_str_or_default`, every SQLite row deserialized in `agent_task_from_row` triggers 3 heap allocations solely to match constant ASCII keywords.
- **Remedy:** Use ASCII case-insensitive comparisons `s.eq_ignore_ascii_case(...)` to eliminate heap allocations. Expose `pub const fn as_str(&self) -> &'static str` on `TaskStatus`, `RiskLevel`, and `Lane` to support zero-copy SQLite binding.

#### Item 2: Zero-Copy String Borrowing in `agent_task_from_row`
- **Location:** [`src/resilience/database/tasks.rs:23-25`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/tasks.rs#L23-L25)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `agent_task_from_row` decodes columns 4, 5, 6 as owned `String`s via `row.get::<_, String>(col)?` only to pass `&String` into `from_str_or_default`.
- **Remedy:** Replace with `row.get_ref(col)?.as_str()?` which borrows `&str` directly from rusqlite's row buffer without allocation.

#### Item 3: Zero-Allocation Enum Parameter Binding in Task Operations
- **Location:** [`src/resilience/database/tasks.rs:61-63, 107-109, 173-174`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/tasks.rs#L61)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `insert_agent_task`, `insert_agent_task_if_absent`, and `update_task_status` allocate heap `String`s via `task.status.to_string()`, `task.risk_level.to_string()`, `task.lane.to_string()`, and `status.to_string()`.
- **Remedy:** Bind `task.status.as_str()`, `task.risk_level.as_str()`, `task.lane.as_str()`, and `status.as_str()` directly to rusqlite parameters as `&'static str`.

#### Item 4: Zero-Copy Parsing in `task_trace_from_row`
- **Location:** [`src/resilience/database/traces.rs:76-78`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/traces.rs#L76-L78)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `task_trace_from_row` allocates two intermediate `String`s: `row.get::<_, String>(3)?` (only compared with `event.kind()`) and `row.get::<_, String>(4)?` (immediately parsed by `serde_json::from_str`).
- **Remedy:** Read `row.get_ref(3)?.as_str()?` and `row.get_ref(4)?.as_str()?`, deserializing directly from `&str`. Eliminates 2 heap allocations per trace row fetched during replay.

#### Item 5: Compile-Time Static SQL String Construction in `list_trace_tasks_paged`
- **Location:** [`src/resilience/database/traces.rs:233-244, 258-267`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/traces.rs#L233)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** Dynamically invokes `format!` at runtime on every page fetch to interpolate `TRACE_LIST_COLUMNS`, which is a `&'static str`.
- **Remedy:** Use `concat!` to produce compile-time constant queries (`&'static str`), removing runtime formatting and allocation.

#### Item 6: Vector Moving & Chaining in `aggregate_usage_by_agents`
- **Location:** [`src/resilience/database/traces.rs:355-364`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/traces.rs#L355-L364)
- **Taxonomy:** `REDUNDANT-ALLOCATION` / `ACCIDENTAL-COMPLEXITY`
- **Routing:** `FIX-NOW`
- **Finding:** `agent_ids` is already owned (`Vec<String>`), yet is iterated with `.map(|s| Value::Text(s.clone()))`, cloning every string. It also allocates 3 separate vectors (`id_values`, `ts_values`, `all_values`).
- **Remedy:** Consume `agent_ids.into_iter().map(Value::Text)` directly chained with `bind_extras.into_iter().map(Value::Integer)`. Eliminates all string clones and 2 intermediate vector allocations.

#### Item 7: Avoid Deep Clone of `MemoryEvent` in `append_memory_event`
- **Location:** [`src/resilience/database/memory_events.rs:51-74`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/memory_events.rs#L51)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `let envelope = envelope.clone();` deeply clones `MemoryEventEnvelope`, including the inner `MemoryEvent` enum with its note text and metadata vectors, even though `event` was already serialized to JSON and only scalar metadata fields are passed into `with_conn`.
- **Remedy:** Clone only the individual fields needed (`fact_id`, `event_type_tag`, `timestamp`, `correlation_id`, `partition`), avoiding cloning `MemoryEvent`.

#### Item 8: Compile-Time Static SQL in `get_memory_events_for_fact`
- **Location:** [`src/resilience/database/memory_events.rs:106-114, 144-147`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/memory_events.rs#L106)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** Repeatedly calls `format!` on `EVENT_COLUMNS` (`&'static str`) across both scoped and unscoped event fetch paths.
- **Remedy:** Combine SQL fragments using `concat!`, creating static `&'static str` statement slices.

#### Item 9: Eliminate Unused `_tier` Heap Allocation in `MemoryEventRow`
- **Location:** [`src/resilience/database/memory_events.rs:298, 326`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/memory_events.rs#L298)
- **Taxonomy:** `DEAD-CODE` / `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `MemoryEventRow` maps column 6 into `_tier: row.get::<_, String>(6)?`. `MemoryEventEnvelope` has no tier field, and `into_envelope()` discards `_tier` completely.
- **Remedy:** Remove `_tier` from `MemoryEventRow` and omit reading column 6 into an allocated `String`.

#### Item 10: Streamline Iterator Collection in `list_memory_fact_ids`
- **Location:** [`src/resilience/database/memory_events.rs:232-236`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/memory_events.rs#L232-L236)
- **Taxonomy:** `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY`
- **Routing:** `FIX-NOW`
- **Finding:** Uses `mut out = Vec::new(); for row in rows { out.push(...); }` instead of idiomatic iterator collection.
- **Remedy:** Simplify to `rows.collect::<Result<Vec<_>, _>>().map_err(...)`.

#### Item 11: Deduplicate Embedding Dim Persistence in `StateDatabase::in_memory`
- **Location:** [`src/resilience/database/state_database/mod.rs:181-185`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/state_database/mod.rs#L181-L185)
- **Taxonomy:** `ACCIDENTAL-COMPLEXITY`
- **Routing:** `FIX-NOW`
- **Finding:** `in_memory()` manually executes raw SQL for `schema_info` dimension recording instead of invoking `Self::record_embedding_dim(&conn, DEFAULT_EMBEDDING_DIM)?` as used in `new()` and `new_with_dim()`.
- **Remedy:** Replace duplicate statement execution with a call to `record_embedding_dim`.

#### Item 12: Preallocate Byte Vector in `serialize_embedding`
- **Location:** [`src/resilience/database/state_database/mod.rs:473-475`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/state_database/mod.rs#L473-L475)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `embedding.iter().flat_map(|f| f.to_le_bytes()).collect()` uses `flat_map` which lacks an exact size hint, causing repeated buffer reallocations during `collect()`.
- **Remedy:** Use `Vec::with_capacity(embedding.len() * 4)` and copy byte slices in a loop.

#### Item 13: Remove Useless `vec!` Allocation in In-Memory Test
- **Location:** [`src/resilience/database/state_database/tests.rs:253`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/state_database/tests.rs#L253)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `FIX-NOW`
- **Finding:** `&vec![0.1f32; 384]` allocates a temporary heap vector solely to pass a slice reference.
- **Remedy:** Replace with array reference `&[0.1f32; 384]`.

#### Item 14: Remove Stale Orphaned Comment in `group_chat.rs`
- **Location:** [`src/resilience/database/group_chat.rs:22`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/group_chat.rs#L22)
- **Taxonomy:** `DEAD-CODE` / `ACCIDENTAL-COMPLEXITY`
- **Routing:** `FIX-NOW`
- **Finding:** `/// An active session summary.` is an orphaned doc comment left immediately above `impl StateDatabase {`.
- **Remedy:** Remove the orphaned doc line.

#### Item 15: Simplify Return in `get_channel_offset`
- **Location:** [`src/resilience/database/channel_offsets.rs:21-31`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/channel_offsets.rs#L21-L31)
- **Taxonomy:** `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY`
- **Routing:** `FIX-NOW`
- **Finding:** Redundant binding `let result = ...; Ok(result)`.
- **Remedy:** Directly return `conn.query_row(...).optional().map_err(...)`.

#### Item 16: Simplified Error Mapping in `into_envelope`
- **Location:** [`src/resilience/database/memory_events.rs:360-362`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/memory_events.rs#L360-L362)
- **Taxonomy:** `CONTROL-FLOW` / `ACCIDENTAL-COMPLEXITY`
- **Routing:** `FIX-NOW`
- **Finding:** Unnecessary closure `|e: String| AlephError::other(e)`.
- **Remedy:** Simplify to `.map_err(AlephError::other)`.

---

### B. Deferred Items (`DEFER`)

#### Item 17: Boot Recovery Helpers with Pending Wiring
- **Location:** [`src/resilience/types.rs:299-314`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/types.rs#L299-L314)
- **Taxonomy:** `DEAD-CODE`
- **Routing:** `DEFER`
- **Rationale:** `should_auto_resume` and `needs_resume_confirmation` are tagged `#[allow(dead_code)]` with explicit documentation referencing `TODO(2026-08-28 rust-logic-audit)` and decision tree in `docs/engineering-reports/review-results/resilience-2026-08-28.md` (warning #15). Do not delete without coordinating with the boot recovery loop.

#### Item 18: Unused `memories` / `memories_vec` / `memories_fts` Tables
- **Location:** [`src/resilience/database/state_database/mod.rs:111, 337`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/state_database/mod.rs#L111) and [`schema.rs:18, 337`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/state_database/schema.rs#L18)
- **Taxonomy:** `ACCIDENTAL-COMPLEXITY`
- **Routing:** `DEFER`
- **Rationale:** The Subsystem Invariant #1 in `BRIEF_COMMON.md` strictly prohibits mutating existing database schema layouts. Removing or modifying these tables requires an explicit cross-subsystem migration approval.

---

### C. Rejected Items (`REJECT`)

#### Item 19: Local `to_u64` vs Centralized `i64_to_u64_count` in `traces.rs`
- **Location:** [`src/resilience/database/traces.rs:368-381`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/traces.rs#L368-L381)
- **Taxonomy:** `ACCIDENTAL-COMPLEXITY`
- **Routing:** `REJECT`
- **Rationale:** `to_u64` returns `Result<u64, rusqlite::Error>` inside `query_map`, whereas `i64_to_u64_count` returns `Result<u64, AlephError>`. Wrapping errors inside the row-mapping closure would introduce boilerplate without semantic gain.

#### Item 20: `update_task_status` Zero-Row Update Error Semantics
- **Location:** [`src/resilience/database/tasks.rs:196-200`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/tasks.rs#L196-L200)
- **Taxonomy:** `CORRECTNESS` / `INVARIANT-TIGHTENING`
- **Routing:** `REJECT`
- **Rationale:** Verified against SQLite semantics: SQLite `changes()` reports 1 row matched by the WHERE clause even if the column values already equal target values. The error path `updated == 0` correctly triggers only when `task_id` is absent.

#### Item 21: Retaining `task: &AgentTask` in Public CRUD Signatures
- **Location:** [`src/resilience/database/tasks.rs:44, 89`](file:///home/zou/data/workspace/Aleph-wt-rev-providers-resilience-routing/src/resilience/database/tasks.rs#L44)
- **Taxonomy:** `REDUNDANT-ALLOCATION`
- **Routing:** `REJECT`
- **Rationale:** Retaining `&AgentTask` in `insert_agent_task(&self, task: &AgentTask)` satisfies the non-negotiable public API stability invariant. The internal clone is necessary for `'static` closure dispatch across `spawn_blocking`.

---

## 4. Verification Guardrails for Senior Code Authoring Tier

1. **No Interim Builds:** Strictly respect workspace guidelines; do not trigger interim compiler checks.
2. **Public API Invariance:** Retain all `pub fn` signatures across `StateDatabase` and `resilience::types`.
3. **Database Schema Stability:** Do not alter DDL in `schema.rs` or migrations in `migration.rs`.
4. **Behavioral Invariance:** All query outputs, error variants, and transaction rollbacks must remain identical.
