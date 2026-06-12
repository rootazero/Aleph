ISSUE|src/arena/types.rs:417|medium|Production unwrap() in has_pipeline_cycle violates no-unwrap policy|in_degree.get_mut(neighbor).unwrap() panics if graph invariant is broken
ISSUE|src/arena/manager.rs:172|medium|settle_with_facts bypasses coordinator permission check|forces Active->Settling->Archived transition without validating can_merge/coordinator permission
ISSUE|src/arena/aggregate.rs:537|medium|ArenaProgress.total_steps is never updated|exposed in snapshots/query_arena but only default 0 is ever assigned
ISSUE|src/arena/handle.rs:69|medium|Silent lock-poison recovery may operate on inconsistent state|arena.write().unwrap_or_else(|e| e.into_inner()) recovers from poison without validation; pattern repeated at lines 86,102,111,125,132,139,151,183
ISSUE|src/arena/manager.rs:74|medium|Silent lock-poison recovery may operate on inconsistent state|shared.read().unwrap_or_else(|e| e.into_inner()) recovers from poison without validation; pattern repeated at lines 101,125,169
ISSUE|src/arena/types.rs:89|low|AgentId is a plain String alias, not a type-safe newtype|pub type AgentId = String allows accidental substitution with arbitrary strings
ISSUE|src/arena/manager.rs:136|low|Internal enum Debug format exposed in JSON API|format!("{:?}", slot.status) leaks Rust variant names to clients
ISSUE|src/arena/manager.rs:148|low|Internal enum Debug format exposed in JSON API|format!("{:?}", arena.status()) leaks Rust variant names to clients
ISSUE|src/arena/manager.rs:198|low|SettleReport.events_cleared is hardcoded to 0|field always reports 0 regardless of actual events processed
ISSUE|src/arena/handle.rs:166|low|Hardcoded artifact limit in snapshot_for_context|.take(5) caps context artifacts with no configuration or documented rationale
