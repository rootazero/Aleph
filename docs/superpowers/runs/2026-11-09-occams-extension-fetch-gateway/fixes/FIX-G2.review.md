# Fix review — G2

| ID | Verdict | Comment |
|---|---|---|
| G2-01 | FIXED | Lines 405–411 return `Err(_) => { report.errored += 1; continue; }` on parent read failure, skipping registration and preserving the parent row. |
| G2-02 | FIXED | Lines 266–283 wrap both `send` and `reply_rx.await` in `send_and_wait`, then apply `timeout` around that combined future. |
| G2-03 | FIXED | Lines 327–335 enforce `RUN_START_CACHE_CAPACITY` (1024) before insert, evicting the first key when at capacity. |

## New problems

- none

## Batch verdict

Safe to commit. All three failure modes are addressed: attribution errors now defer registration, flush timeout bounds the full operation, and the run-start cache is bounded by eviction.