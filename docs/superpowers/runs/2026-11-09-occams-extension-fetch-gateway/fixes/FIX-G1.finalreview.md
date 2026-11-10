# Fix review — G1

| ID | Verdict | Comment |
|---|---|---|
| G1-01 | NOT_FIXED | The diff contains no change to `event_visibility.rs`; there is no invalidation generation increment, generation check, or reload before ownership insertion, so the read/invalidate/insert race remains. |
| G1-02 | FIXED | The start path now retains the forwarder handle in `entry.forwarder_task` and only spawns when `!handle.is_finished()`, preventing stop/start from installing a second live subscriber. |
| G1-03 | PARTIAL | The caller now uses `let _operation = self.begin_legacy_operation(...)`, but the shown diff does not implement an RAII guard whose `Drop` clears the matching marker, so cancellation can still strand `runtime.operation = Some(marker.clone())`. |
| G1-04 | PARTIAL | `begin_replay` now requires `AND inflight_since IS NULL`, preventing automatic replay of a stamped row, but the diff does not add the required drain-gate reconciliation or make a failed startup reconciliation prevent `spawn_drain` from running. |
| G1-05 | FIXED | The interrupted branch now passes `reduction.open_run.as_ref().map(\|o\| o.run_id.as_str())` and `settle_refused` forwards that `closes` value to `abandon`, closing the actual open run ID rather than fabricating one. |
| G1-06 | FIXED | The oversized branch now persists only a bounded JSON diagnostic via `payload: &diagnostic_payload` instead of the original full `payload`. |

## New problems

- none

## Batch verdict

needs rework — G1-01 remains untouched, while G1-03 and G1-04 are only partially fixed; cancellation-safe marker cleanup and drain-gate reconciliation/failure fencing are still required.