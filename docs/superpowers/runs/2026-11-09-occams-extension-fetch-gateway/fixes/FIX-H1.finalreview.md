# Fix review — H1

| ID | Verdict | Comment |
|---|---|---|
| H1-01 | REGRESSION | The diff adds `let _guard = HOOKS_MUTEX.lock().unwrap();` only to `append_user_hook`, while `remove_user_hooks` and `handle_hooks_remove` remain unguarded, so the concurrent read-modify-write clobber described by the finding can still occur; additionally, the extra `}` shown after `append_user_hook` appears to introduce a syntax error. |
| H1-02 | FIXED | The RPC now uses `tokio::task::spawn_blocking(read_hooks_file).await`, moving synchronous filesystem I/O off the Tokio worker while retaining error handling. |
| H1-03 | FIXED | The diff snapshots `old_memory` and `old_compression`, then restores them in the `save_incremental` error branch before returning `INTERNAL_ERROR`. |
| H1-04 | FIXED | The update path now explicitly matches `serde_json::from_value::<Option<ActiveHoursSchedule>>` and returns `INVALID_PARAMS` on `Err`, while preserving absent/null semantics in the added test. |
| H1-05 | FIXED | All three placeholder approval handlers now return `-32601` instead of `success:true` or an empty successful list, with tests updated accordingly. |
| H1-06 | FIXED | Agent lookup and `build_run_request(...).await.map_err(...)?` occur before `runs.insert(...)` and `RunAccepted` publication, so rejected requests are no longer admitted. |
| H1-07 | FIXED | The diff replaces narrowing casts with `u32::try_from(v)` for both compression fields, leaving values unchanged when out of range. |
| H1-08 | PARTIAL | The diff adds bounded terminal-history eviction and clears completed input, but the shown cleanup runs only after completion and the stated invariant “结束时及插入时回收” is not fully established; the added test references `COMPLETED_RUN_RETENTION` and `finished_at` fields that are not present in the shown diff, so the implementation cannot be proven complete from this diff. |

## New problems

- `src/gateway/handlers/hooks_admin.rs:245` — An extra `}` appears immediately after `append_user_hook`, which likely closes the surrounding function/impl early and makes the file fail to parse.
- `src/gateway/handlers/agent.rs:2112` — The added test uses `COMPLETED_RUN_RETENTION` and `RunState.finished_at`, but neither identifier is added in the shown diff; the test therefore appears not to compile against the current source.
- `src/gateway/handlers/hooks_admin.rs:214` — `HOOKS_MUTEX` is held across filesystem I/O in `append_user_hook`; because `append_user_hook` is called directly from the tool path and the RPC mutation path is not shown as running the entire guarded transaction in `spawn_blocking`, this can still block a Tokio worker.

## Batch verdict

needs rework — the hooks mutation guard remains incomplete and the diff appears to contain syntax/test-compilation errors; agent history retention also needs a proven complete implementation.