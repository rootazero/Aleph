# Fix review — H1

| ID | Verdict | Comment |
|---|---|---|
| H1-01 | PARTIAL | The diff adds `let _guard = HOOKS_MUTEX.lock().unwrap();` only in `append_user_hook`, while the finding also requires the shared remove helper and indexed RPC removal path to hold the same guard; no corresponding diff lines add those locks, so concurrent remove operations can still clobber or resurrect changes. |
| H1-04 | FIXED | `handle_create` now returns `INVALID_PARAMS` on malformed `active_hours`, and the added update test verifies malformed updates return `INVALID_PARAMS` while absent, `null`, and valid values preserve the intended semantics. |
| H1-08 | PARTIAL | The diff evicts only `RunStatus::Completed | RunStatus::Failed(_)` and drops input only for those statuses, while the test explicitly inserts `RunStatus::Cancelled` as retained history; cancelled terminal runs can therefore still accumulate indefinitely and retain their input. |

## New problems

- `src/gateway/handlers/agent.rs:369-391` — The cleanup only removes old completed/failed entries and never removes cancelled terminal entries; `RunStatus::Cancelled` is terminal but excluded, so the map can still grow without bound for cancelled runs.
- `src/gateway/handlers/hooks_admin.rs:217` — The new mutex is a `std::sync::Mutex` held across synchronous filesystem I/O, so the async callers block Tokio workers while reading and writing hooks; this conflicts with the stated `spawn_blocking` strategy and can stall the runtime under filesystem latency.
- `src/gateway/handlers/memory_config.rs:102-125` — The rollback restores `cfg.memory` and compression only when `old_compression` is `Some`; if compression was absent before the update, a save failure leaves the newly inserted compression policy in the shared configuration.
- `src/gateway/handlers/agent.rs:263-291` — Moving agent lookup and request construction before run admission changes observable behavior: a request that previously became a failed run state now returns an error without a run entry, despite the comment claiming admission is intentionally after validation.

## Batch verdict

Needs rework: H1-01 is not fully guarded across all mutation paths, H1-08 still leaks cancelled terminal runs, and the diff introduces blocking mutex I/O plus incomplete rollback and admission-behavior regressions.