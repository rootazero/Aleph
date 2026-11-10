# Fix review — G1

| ID | Verdict | Comment |
|---|---|---|
| G1-01 | FIXED | `generation` field added at line 827, captured before read (loop at 1185), `cache_ownership_if_current` checks `inner.generation != generation` (line 1254) and refuses stale insert, invalidation increments at line 1376. |
| G1-02 | FIXED | `needs_forwarder` check at line 1273 inspects existing handle liveness before spawning; new spawned handle assigned only if prior was absent or finished (lines 1286-1290), concurrent spawn aborted (line 1287). |
| G1-03 | NOT_FIXED | Diff does not introduce a drop guard clearing the marker; `finish_legacy_operation` still called only at line 1234 in normal control flow, so cancellation before that line leaves marker set. |
| G1-04 | NOT_FIXED | Diff does not touch `delivery_queue.rs`; no reconciliation added to drain passes, no refusal of already-stamped rows in `begin_replay`. |
| G1-05 | NOT_FIXED | Diff does not touch `resume_coordinator.rs`; `settle_refused` still calls `abandon` with `None`. |
| G1-06 | NOT_FIXED | Diff does not touch `delivery_queue.rs`; oversized payload still passed to `insert_dead_letter` at line 974. |

## New problems

- `src/gateway/channel_registry.rs:1287` — A concurrent start that observes `live = true` aborts the newly spawned forwarder handle but does not `await` the abort, so the forwarder may still publish messages between abort and actual task termination; a safer pattern would be to keep the mutex held across the spawn and assignment (though that requires making the spawn non-async or using a tokio Mutex).

## Batch verdict

Needs rework. G1-01 and G1-02 are correctly fixed. G1-03, G1-04, G1-05, G1-06 are not addressed at all. The G1-02 fix introduces a potential race between abort and message forwarding.