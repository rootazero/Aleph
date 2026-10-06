# Task 3 Hook memo production sink

## Implemented

- Added `SessionHookMemoSink` in `src/extension/hooks/executor.rs`.
- Hook outcomes are recorded as `SessionEvent::HookMemo` through `emit_for_ambient_call`; missing ambient call identity remains fail-closed and produces no event.
- Added `HookExecutor::with_session_memo_sink` and re-exported the sink contract.
- `ScopedToolService` clones the shared executor per call and attaches a phase-specific sink for `permission_denied`, `before_tool_call`, and `after_tool_call`. The shared executor is never mutated, so concurrent sessions cannot overwrite each other's memo session.
- Empty hook executors retain the existing `hook_count() > 0` short circuit.

The existing executor sink call sites cover blocked, error, and completed outcomes. There are no new claims for matched or skipped coverage where the existing executor does not emit those outcomes.

## Verification

- `cargo check -p alephcore`: passed.
- `git diff --check`: passed.

## Deferred

- Cross restart deduplication and external effect exactly-once remain deferred to the durable effect/recovery gates.
- Hook memo persistence uses the existing session call-log/event path and does not create a second store.
