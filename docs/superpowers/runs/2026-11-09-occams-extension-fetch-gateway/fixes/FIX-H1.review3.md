# Fix review — H1

| ID | Verdict | Comment |
|---|---|---|
| H1-01 | NOT_FIXED | The diff adds `static HOOKS_MUTEX` and locks it in `append_user_hook` (`let _guard = HOOKS_MUTEX.lock().unwrap();`), but the reviewer explicitly noted `remove_user_hooks` and `handle_hooks_remove` remain unguarded — the diff shows no `HOOKS_MUTEX.lock()` in either of those two entry points, so concurrent remove vs. append can still clobber. |
| H1-04 | NOT_FIXED | The diff fixes `handle_create` (now returns INVALID_PARAMS on parse failure), but the reviewer flagged `handle_update:386` which still uses `updates.active_hours = serde_json::from_value(val.clone()).ok();` — the diff for heartbeat.rs shows no change to `handle_update`, so malformed values on update are still silently dropped. |
| H1-08 | PARTIAL | The diff adds eviction logic in the completion task (`const MAX_TERMINAL_RUNS: usize = 128; ... runs.remove(&id);`), but the reviewer's concern is that completed runs accumulate indefinitely — the eviction only happens when a new run completes, not on insertion, and the `input` field is not cleared on completion (only on eviction), so memory still grows with prompt size until the 128-run threshold is hit. |
| NEW: mcp.rs inconsistency | FIXED | All three placeholder handlers (`handle_list_pending_approvals`, `handle_respond_approval`, `handle_cancel_approval`) now consistently return error -32601. |
| NEW: hooks_admin.rs blocking | NOT_FIXED | `HOOKS_MUTEX` is still a `std::sync::Mutex` held across filesystem I/O in `append_user_hook`; the diff does not move the locked section into `spawn_blocking` or otherwise address the blocking concern. |

## New problems

- `src/gateway/handlers/agent.rs:254` — The admission-control reordering (agent lookup and `build_run_request` before `RunState` creation) changes observable behavior: previously a failed agent lookup would register a `Failed` run status; now it returns an error without any run record, which may break callers expecting a run status to query.

## Batch verdict

Needs rework: H1-01 and H1-04 are still not fixed (the exact lines the reviewer flagged are unchanged), H1-08 is only partially addressed, and the blocking mutex concern is unaddressed.