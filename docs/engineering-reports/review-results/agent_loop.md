# Module: agent_loop

## Summary
- Files reviewed: 15
- Issues found: 10
- Issues fixed: 5 (high-confidence, directly actionable)

## Fixes Applied

1. **[6 files] `std::sync::Arc` → `crate::sync_primitives::Arc`** — `builtin_adapter.rs`, `daemon_adapter.rs`, `mcp_adapter.rs`, `memory_adapter.rs`, `registry_adapter.rs`, `factory.rs` all bypassed the sync primitives import convention, breaking loom instrumentation compatibility.

2. **[integration_probe.rs:11] `std::sync::Mutex` → `crate::sync_primitives::Mutex`** — Test infrastructure shared state (`Arc<Mutex<Vec<CapturedRequest>>>`) was invisible to loom concurrency testing.

3. **[loop_core.rs:723] `std::sync::{Arc, Mutex}` → `crate::sync_primitives::{Arc, Mutex}`** — Same issue in loop_core test module.

4. **[subagent_tool.rs:182] `retryable: true` → `retryable: false`** — When `AgentLoop::run()` returns `Err`, it's a structural failure (provider unreachable, serialization error), not a transient error. Marking retryable caused unbounded retry storms.

5. **[memory_adapter.rs:224-236] Dual lock ordering fix** — `FakeMemory::store` held both `entries` and `next_id` locks simultaneously with undocumented ordering. Fixed: acquire `next_id` first in a scoped block, release it, then acquire `entries`.

6. **[safety.rs:129-142] JSON encoding bypass in blocked pattern matching** — `SafetyGuard::check` matched regex patterns against `serde_json::Value::to_string()`, which JSON-encodes escape sequences (`\n` → `\\n`). An adversarial LLM could bypass `rm\s+-rf\s+/` by embedding a newline in the command string. Fixed: extract actual string values from the JSON tree via `collect_string_values()` before matching. Added regression test.

## Observations (Not Fixed — Design-Level)

- **[loop_core.rs:632-685] `consecutive_errors` mid-batch**: When the error threshold is reached mid-tool-batch, remaining tools in the same response still execute before the outer loop breaks. This is architecturally correct for the current single-tool-per-response pattern but would overrun with multi-tool responses.

- **[loop_core.rs:674] `tool_calls_made` metric**: Counts safety-blocked and policy-denied calls, not just executed ones. The field name is slightly misleading — consider renaming to `tool_calls_attempted` or documenting the behavior.

- **[loop_core.rs — 2496 lines]**: File exceeds the 500-line threshold from P2. Consider splitting into `loop_core.rs` (main loop), `message_management.rs` (find_safe_cut_point, message trimming), and `tests.rs`.
