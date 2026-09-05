# src/session review (raw agent output)

## Summary
- Files scanned: src/session/{mod,actor,boundary_repair,epoch_registrar,events,in_process,marker_balance,observer,projection,reduction,service,steer_signal,store,tool_trace}.rs
- Critical: 0, Important: 4, Minor: 5
- Health: green

## Strengths
- reduction.rs exemplary: closed-set LogContradiction, single ascending scan, deterministic pairings, per-prefix "every prefix of every legal shape is green" test
- Audit 4.1 self-heal duplicated faithfully into both hot and drain arms in actor.rs
- steer_signal.rs uses watch::Sender not Notify so receivers remember sends before first poll
- boundary_repair.rs documents three repair sentences with shared semantic points
- marker_balance.rs single-purpose with all four quadrants tested
- store.rs soft-delete uses explicit BEGIN IMMEDIATE/COMMIT for retire_from
- events.rs documents every removed variant as "the next variant arrives in the same commit as the producer"
- ServiceError::ShutdownTimeout is a real signal used by both wake and detach

## Critical findings
None.

## Important findings

### I-1 EmitEvent does not reset idle_deadline while GetEvents/Subscribe do
- File: src/session/actor.rs:156-209
- Problem: idle_deadline is reset only in GetEvents (line 194) and Subscribe (line 198) arms. EmitEvent processes the event but never extends the deadline. A long harness run that only emits times out after 30 min.
- Suggested fix: Reset idle_deadline = Instant::now() + self.idle_timeout in the EmitEvent arm after finish_emitted.

### I-2 wake_lock is dropped before spawn_actor, exposing a race
- File: src/session/in_process.rs:380-446
- Problem: wake() acquires wake_lock, runs shutdown + load_head_seq, then drop(_guard) before spawn_actor. A concurrent emit_event for the same session sees no sender and falls through to spawn_actor(id, None), creating a second actor. If that second agent's seq lands at prior_head + 1 before wake's SessionWoken is appended, self-heal advances SessionWoken to prior_head + 2.
- Suggested fix: Hold wake_lock across the entire wake() body, including spawn_actor.

### I-3 is_event_retired (process-wide accessor) has zero callers
- File: src/session/store.rs:875
- Problem: pub async fn with no callers. Every consumer uses is_retired directly on the trait. The clear/rewind race against the projector's write queue is unwired.
- Suggested fix: Either call is_event_retired from the projector or delete is_event_retired and the matching capability-slot doc row.

### I-4 newest_before.max(Some(seq)) produces a non-monotonic anchor when healed
- File: src/session/projection.rs:163-174
- Problem: The cross-backend equivalence is documentary, not asserted by an actual integration test.
- Suggested fix: Add integration test comparing Rust function against SQL over the same fixture.

## Minor findings
### M-1 wake_locks accumulates entries for sessions that wake but never detach
- File: src/session/in_process.rs:477-482

### M-2 ShutdownTimeout returns leave the old actor orphaned in memory
- File: src/session/in_process.rs:415-422, 467-474

### M-3 retire_through deliberately uses no transaction
- File: src/session/store.rs:483-525

### M-4 validate_slice accepts equal seqs but reducer's matching relies on distinct
- File: src/session/reduction.rs:292-297

## Assessment
**Verdict:** green
**One-sentence summary:** Well-designed event-sourced module with two real concurrency defects (idle-timeout not reset on EmitEvent; wake_lock dropped before spawn_actor) and one unwired accessor (is_event_retired).
