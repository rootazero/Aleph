# Task 4 — projection-only subscription cursor

## Implemented

- Added pure `CapabilityChangeStream::from_snapshot`, `append`, and `invalidate_and_resync` state transitions in `src/capability/facade.rs`.
- Initial state captures the immutable snapshot cursor atomically; changes begin empty.
- `append` accepts only strictly increasing committed cursors.
- Resync rejects a regressing snapshot without mutation, appends `Invalidated`, preserves the previous cursor, filters stale deltas, deduplicates `(CapabilityId, CapabilityRevision)`, and advances to the greatest committed cursor without restarting from zero.
- Kept the `Zahir` trait shape unchanged. Owner-generation validation remains at the backend lease boundary because the current value stream has no concrete committed registry source.
- The state machine does not read or write `GlobalBus`, `StateDatabase`, ACP JSON, or another store.

## Verification

- `cd /Volumes/TBU4/Workspace/Aleph-capability-phase4 && cargo test -p alephcore --lib capability::facade --no-fail-fast`: 5 passed.
- `cd /Volumes/TBU4/Workspace/Aleph-capability-phase4 && cargo check -p alephcore`: passed.
- `git diff --check`: passed.

## Deferred

- A concrete Zahir facade/backend that captures registry snapshots and committed deltas is deferred; this commit provides the tested value-state machine only.
- Projection markers for ACP persistence, StateDatabase, MCP surfaces, and GlobalBus remain for the follow-up projection seam task. No behavior or authorization semantics were changed here.
