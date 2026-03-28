All 38 tests pass. Here's the review summary:

---

# Module: resilience

## Summary
- Files reviewed: 12
- Issues found: 7
- Issues fixed: 7

## Fixes

### Safety Bugs (2)
1. **`memory_events.rs:147`** `since_seq as i64` silent overflow → `i64::try_from(since_seq).unwrap_or(i64::MAX)` — large u64 values would wrap to negative, producing incorrect SQL queries
2. **`memory_events.rs:233`** `limit as i64` silent overflow → `i64::try_from(limit).unwrap_or(i64::MAX)` — same issue for usize-to-i64 cast

### DRY Violations (5)
3. **`events.rs`** AgentEvent row mapping duplicated 5x → extracted `agent_event_from_row()` helper
4. **`tasks.rs`** AgentTask row mapping duplicated 4x → extracted `agent_task_from_row()` helper
5. **`sessions.rs`** SubagentSession row mapping duplicated 3x → extracted `subagent_session_from_row()` helper
6. **`traces.rs`** TaskTrace row mapping duplicated 3x → extracted `task_trace_from_row()` helper
7. **`memory_events.rs`** MemoryEventRow construction duplicated 5x → extracted `MemoryEventRow::from_row()` method

## Not Found (Clean)
- **Lock safety**: All `lock()` calls already use `.unwrap_or_else(|e| e.into_inner())` ✓
- **UTF-8 safety**: No byte slicing `&s[..n]` patterns ✓
- **SQL injection**: All queries use parameterized `params![]` (the `format!()` in `migration.rs` only interpolates hardcoded column names) ✓
- **static mut**: None used; `AtomicBool` in `events.rs` is correct ✓
- **Architecture compliance**: Module respects brain-limb separation, no platform API calls ✓
- **Dead code**: None detected ✓

## Notes
- The pre-existing binary compilation error (`E0277` in `AgentHandlersResult`) is unrelated to resilience
- The `get_idle_sessions` previously hardcoded `status: SessionStatus::Idle` — now reads from row via `from_row`, which is more correct and consistent (query already filters `WHERE status = 'idle'`)
- Schema DDL in `schema.rs` is clean and well-organized with proper indexes
