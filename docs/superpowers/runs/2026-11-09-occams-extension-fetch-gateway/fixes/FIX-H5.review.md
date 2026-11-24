# Fix review — H5

| ID | Verdict | Comment |
|---|---|---|
| H5-01 | NOT_FIXED | The diff does not touch `checkpoint.rs:223` or the branch visibility check; no change to the `Ok(_) => {}` arm that permits existing targets. |
| H5-02 | NOT_FIXED | The diff does not touch `handlers.rs:507` or the delete flow; vault deletion still precedes config save. |
| H5-03 | NOT_FIXED | The diff does not touch `handlers.rs:218` or hydrate the snapshot from vault when `api_key` is omitted. |
| H5-04 | FIXED | Lines `+397–410` and `+1126–1147` stage a `cfg_candidate`, persist it, then publish to shared state only after `save_config` succeeds. |
| H5-05 | NOT_FIXED | The diff does not touch `handlers.rs:247` or propagate construction failure before reporting success. |
| H5-06 | NOT_FIXED | The diff does not move `save_config` to the blocking pool; `helpers.rs:83` remains synchronous. |
| H5-07 | NOT_FIXED | The diff does not touch `handlers.rs:1289` or replace synchronous `cached_models` with the async variant. |
| H5-08 | NOT_FIXED | The diff does not touch `workflow.rs:32` or move template discovery to the blocking pool. |
| H5-09 | NOT_FIXED | The diff does not touch `create.rs:26` or replace millisecond-only keys with a unique identifier. |
| H5-10 | NOT_FIXED | The diff does not touch `tasks.rs:69` or apply the owner predicate to the task query. |
| H5-11 | NOT_FIXED | The diff does not touch `tasks.rs:255` or reject non-object metadata at task creation. |
| H5-12 | NOT_FIXED | The diff does not touch `canvas.rs:116` or propagate persistence failure before reporting observed success. |
| H5-13 | NOT_FIXED | The diff does not touch `tasks.rs:832` or apply the chat-visibility predicate before the storage limit. |
| H5-14 | NOT_FIXED | The diff does not touch the search handler at `sessions.rs:522` or restrict candidates to owner-visible IDs before the limit. |
| H5-15 | NOT_FIXED | The diff does not touch `mod.rs:610` or apply the watermark predicate before global truncation. |
| H5-16 | NOT_FIXED | The diff does not touch `sessions.rs:632` or introduce a strict three-value parser at the RPC boundary. |
| H5-17 | FIXED | Lines `+112–157` compute `total` for unrestricted callers via `store.count_sessions` under the same predicates, replacing the page-length return at old line 165. |
| H5-18 | FIXED | Lines `+114–154` scan the corpus in bounded `SCAN_BATCH` pages and build the visible page incrementally, replacing the `i64::MAX` fetch at old line 109. |

## New problems

- `sessions.rs:+154` — the scoped loop terminates when `batch_len < SCAN_BATCH`, but the store's `.list_sessions` at `+118` already applies `limit` and `offset` parameters from the request DTO, so this loop re-applies offset logic that the store already applied, producing incorrect pagination (the intended scan-batch offset is overwritten by `params.offset`).
- `sessions.rs:+176` — error path at line 176 constructs `format!("Failed to list sessions: {}", e)`, but this is now the unrestricted count branch, so the message should be "Failed to count sessions" to match the operation.
- `sessions.rs:+201` — `has_more` is computed as `total > params.offset.saturating_add(limit)`, but when `total == offset + limit` exactly, the last page is full and there is no next page; the correct predicate is `>=` (or equivalently `total.saturating_sub(offset) > limit`), so a full final page incorrectly reports `has_more:true`.

## Batch verdict

**Needs rework.** Only H5-04, H5-17, and H5-18 were addressed. H5-18's scoped-caller loop conflates DTO pagination with internal scan batching (the store's `limit`/`offset` are request-level, not scan-level), producing incorrect offsets. H5-17's count-based approach is correct but the `has_more` boundary is off by one. Fifteen findings remain unaddressed.