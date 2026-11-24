# Fix review — EE1

| ID | Verdict | Comment |
|---|---|---|
| IF4-03 | NOT_FIXED | The diff only adds `cancel_token.clone()` to `ActiveRun` and a `Default::default()` token in `SimpleExecutionEngine`; neither token is wired to the actual cancellation path, so the reported unused-token problem remains. |

## New problems

- `src/gateway/execution_engine/execute.rs:492` — The new `admit_run(..., &mut cancel_rx, cancel_token.clone())` call requires the `ActiveRun` token, but the diff does not show a corresponding removal or replacement of the `cancel_token` field or any use that propagates cancellation; the token remains dead weight and does not implement the intended cancellation behavior.
- `src/gateway/execution_engine/gate.rs:353` — The comment says “The token is consumed here on purpose,” but the diff clones the token when registering the run (`cancel_token.clone()`), and no token is consumed or dropped in the cancellation branch; the comment is therefore false and the token still remains stored unused.
- `src/gateway/execution_engine/simple.rs:114` — `cancel_token: Default::default()` creates a token for every simple-engine run even though the test and implementation do not use it for cancellation; this is the exact dead-token problem identified by the review.
- `src/gateway/execution_engine/execute.rs:544` — The seed-failure cleanup only marks the in-memory run failed and optionally persists the task, but it does not explicitly cancel or otherwise terminate any spawned run work associated with the run; returning from the seeding path may leave existing work running while the run is reported failed.
- `src/gateway/execution_engine/execute.rs:554` — The new delayed-removal task is spawned without checking whether the run was subsequently replaced or updated, so it can remove a newer registration with the same `run_id` after five seconds; this can erase a later run’s state.

## Batch verdict

needs rework: the cancellation-token findings are not fixed, and the new delayed-removal cleanup can remove a later run registration with the same ID.