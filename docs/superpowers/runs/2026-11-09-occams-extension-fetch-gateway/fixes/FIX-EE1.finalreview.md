# Fix review — EE1

| ID | Verdict | Comment |
|---|---|---|
| EE1-01 | PARTIAL | The diff now masks `tool_timeline` errors via `changed \|= mask_in_place(masker, err);`, but it still does not mask other string-bearing timeline fields such as `invocation.input` or `invocation.result`, so the stated every-text-leaf contract remains unmet. |
| EE1-02 | FIXED | The admission wait now selects on cancellation with `_ = cancel_rx.recv() => { ... return Err(ExecutionError::Cancelled); }`, removing the registered run and dropping the slot before returning. |
| EE1-03 | NOT_FIXED | The diff contains no changes to `src/gateway/execution_engine/goal_wait.rs`; the barrier is still cleared by session id before the fallible continuation claim. |
| EE1-04 | FIXED | The seed error path now sets `run.state = RunState::Failed { ... }`, clears `run.cancel_tx`, persists `TaskStatus::Failed`, schedules removal, and returns the original error. |

## New problems

- `src/gateway/execution_engine/execute.rs:557` — The early seed-error path persists task failure only when `trace_task_persisted` is true, but the finding requires the persisted task to be settled on this path; the task-start persistence condition must match the task actually created, otherwise a started task can remain running.
- `src/gateway/execution_engine/gate.rs:341` — `cancel_token` is stored in `ActiveRun` but is never used for cancellation propagation, retaining the dead field and creating a misleading second cancellation mechanism.
- `src/gateway/execution_engine/gate.rs:370` — The comment says “The token is consumed here on purpose,” but the code selects on `cancel_rx.recv()` rather than the token, and the token is only cloned into `ActiveRun`; the comment does not describe the actual cancellation path.
- `src/gateway/execution_engine/simple.rs:117` — `cancel_token: Default::default()` adds a token that is not wired to the simple engine’s cancellation path, so it is dead weight rather than a functional fix.
- `src/gateway/execution_engine/simple.rs:454` — The added test asserts `run.cancel_token.is_none()`, but the implementation sets `cancel_token: Default::default()`, so the test is inconsistent with the code and would fail if run.

## Batch verdict

needs rework: EE1-02 and EE1-04 are addressed, but EE1-01 is only partially fixed, EE1-03 is untouched, and the diff introduces inconsistent/dead cancellation-token changes plus a likely failing test.