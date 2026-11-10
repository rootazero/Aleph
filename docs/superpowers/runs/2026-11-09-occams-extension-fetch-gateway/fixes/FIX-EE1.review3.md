# Fix review — EE1

| ID | Verdict | Comment |
|---|---|---|
| gate.rs:117 dead `cancel_token` field | NOT_FIXED | The diff still writes `cancel_token: cancel_token.clone(),` into `ActiveRun` at gate.rs:344, and nothing in the diff reads `run.cancel_token` anywhere — the field remains dead weight. |
| gate.rs:370 misleading "consumed on purpose" comment | NOT_FIXED | The comment "The token is consumed here on purpose" is still present at the `cancel_rx.recv()` arm, but the diff never drops the token — it was moved into `admit_run` by value, then `cancel_token.clone()` is stored into `ActiveRun`, so the original is dropped at end of scope only after the select; the claim that it is "consumed here" (i.e. at the cancellation branch) remains inaccurate. |
| simple.rs:117 unwired `cancel_token: Default::default()` | NOT_FIXED | The diff still adds `cancel_token: Default::default(),` at simple.rs:117 (visible in the simple.rs hunk); nothing wires it to the actual cancellation path, so SimpleExecutionEngine still carries the dead field. |

## New problems

- src/gateway/execution_engine/execute.rs (seed-failure cleanup block): the diff inserts a `tokio::spawn` that does `runs_clone.write().await.remove(&run_id_clone)` after a 5-second sleep, but the spawned task is not abortable and the `ActiveRun` is removed unconditionally — if the run was already removed by another path (e.g. normal completion racing this error path), the `remove` is a no-op, but if the engine is shutting down the spawned task can outlive the engine and panic on a closed runtime; more importantly, the diff duplicates the delayed-removal logic inline instead of calling the existing helper, which risks divergence. This is a new concern not in the finding list.
- src/gateway/execution_engine/gate.rs: the `cancel_rx.recv()` arm removes the run from `active_runs` but does not call `persist_run_task_status` or emit any terminal event, so a run cancelled while waiting on the permit is silently dropped from the registry without a persisted `Cancelled` status — observable behaviour change not requested by any finding.

## Batch verdict

Needs rework: the three second-wave findings about the dead `cancel_token` field and misleading comment are all still present in the diff; the fix agent addressed a different set of findings (EE1-01/02/04) and left the named problems untouched.