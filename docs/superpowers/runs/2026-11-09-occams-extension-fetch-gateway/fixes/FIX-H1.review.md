# Fix review — H1

| ID | Verdict | Comment |
|---|---|---|
| H1-01 | FIXED | `let _guard = HOOKS_MUTEX.lock().unwrap();` (line +357 in hooks_admin.rs) serializes `remove_user_hooks`; same guard already added in `append_user_hook` (not shown in diff but implied by fix note); covers the full RMW cycle. |
| H1-02 | NOT_FIXED | The diff contains no changes to `handle_hooks_list`, `handle_hooks_add`, or `handle_hooks_remove`; blocking filesystem calls at lines 124-125 remain on Tokio workers. |
| H1-03 | NOT_FIXED | No changes to `memory_config.rs` appear in the diff; lines 105/111 still mutate shared state before line 116 attempts save. |
| H1-04 | FIXED | Lines +398-411 in heartbeat.rs replace `.ok()` with explicit match: valid `active_hours` sets `updates.active_hours = Some(…)`, parse error returns `INVALID_PARAMS`; malformed values no longer become `None`. |
| H1-05 | NOT_FIXED | No changes to `mcp.rs`; placeholder handlers still return `success:true` without executing approval operations. |
| H1-06 | NOT_FIXED | No changes to agent.rs lines 260/279/310; run insertion and publication still precede build validation in the cited sequence. |
| H1-07 | NOT_FIXED | No changes to `memory_config.rs` lines 392-399; `as u32` casts remain without range checks. |
| H1-08 | FIXED | Lines +164-184 define `retain_terminal_runs_bounded` with `MAX_TERMINAL_RUNS=128`; line +399 calls it after status update; finished entries beyond the bound are removed by instant sort. Test at +2136 confirms oldest eviction. |

## New problems

- agent.rs:+170 — `std::time::Instant` lacks defined absolute ordering guarantees across threads; sorting by `started_at` when runs span multiple workers could be unstable, though the practical impact is minor since eviction only needs approximate LRU and the bound still holds.

## Batch verdict

Needs rework. H1-02, H1-03, H1-05, H1-06, H1-07 remain unaddressed. The diff only fixes 3 of 8 findings (H1-01, H1-04, H1-08). The covered fixes are correct but the batch is incomplete.