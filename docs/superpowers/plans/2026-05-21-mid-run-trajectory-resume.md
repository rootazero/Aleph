# Mid-Run Trajectory Resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a long agent run is interrupted by a crash, `SIGKILL`, or a deliberate server restart, the resident `aleph-server` detects the interrupted run on its next boot and automatically re-triggers it so the task is not silently lost.

**Architecture:** Two new `SessionEvent` variants (`RunStarted` / `RunFinished`) act as durable run markers on the existing event-sourced session log. The orchestrator emits a marker pair around every `harness.run()`. A new boot-scan `ResumeCoordinator` queries the log for sessions whose newest marker is a `RunStarted` (= interrupted), repairs the crash boundary by appending synthetic `ToolError`s for dangling tool calls, and re-triggers each via the existing `ExecutionAdapter` — exactly like cron/heartbeat — with `metadata["resume"] = "true"`, which the engine→orchestrator boundary converts into a new `FlowInput::Resume` variant that skips re-seeding.

**Tech Stack:** Rust, async-trait, tokio, serde, rusqlite.

**Spec:** [`docs/superpowers/specs/2026-05-21-mid-run-trajectory-resume-design.md`](../specs/2026-05-21-mid-run-trajectory-resume-design.md)

**Worktree:** Implementation runs in a dedicated worktree off `main`. Spec + plan live on `main`.

**MERGE POLICY:** Do NOT merge this branch into `main` after implementation. Stop at "implementation complete, tests green, branch ready" and wait for the user's explicit merge instruction.

**Cargo concurrency cap:** This machine OOM-kills past 3 concurrent cargo processes. Before EVERY cargo command, prefix this gate:
```bash
until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && <cargo command>
```
Use background execution for cargo runs (compiles take 5-20 min).

---

## File Structure

| File | New/Modified | Responsibility |
|------|--------------|----------------|
| `src/session/events.rs` | Modified | Add `RunStarted` / `RunFinished` variants + `RunOutcome` enum. |
| `src/session/state.rs` | Modified | Add no-op projection arms for the two new variants. |
| `src/session/store.rs` | Modified | Add `event_type` strings, `extract_turn_id` arms, `load_run_markers()` trait method + `SqliteEventStore` impl. |
| `src/orchestrator/flow_spec.rs` | Modified | Add `FlowInput::Resume` variant. |
| `src/orchestrator/harness_bridge/session_seed.rs` | Modified | Add `Resume` arm to `seed_session` (no-op). |
| `src/orchestrator/harness_bridge.rs` | Modified | Add `Resume` arm to `last_user_query`; emit `RunStarted`/`RunFinished` around `harness.run()`. |
| `src/gateway/execution_engine/run_loop.rs` | Modified | At the `RunRequest`→`FlowInput` site, branch on `metadata["resume"]` to emit `FlowInput::Resume`. |
| `src/gateway/resume_coordinator.rs` | New | `ResumeCoordinator` — scan, recency/cap filter, crash-boundary repair, re-trigger. |
| `src/gateway/mod.rs` | Modified | Register the new `resume_coordinator` module + re-export. |
| `src/config/types/resume.rs` | New | `ResumeConfig` struct + defaults. |
| `src/config/types/mod.rs` | Modified | `pub mod resume;` + `pub use resume::*;`. |
| `src/config/structs.rs` | Modified | Add `pub resume: ResumeConfig` field to `Config` + Default-impl entry. |
| `src/bin/aleph-server/commands/start/mod.rs` | Modified | Keep the `Arc<dyn SessionEventStore>` reachable; spawn `ResumeCoordinator` as a detached task after cron/heartbeat. |
| `tests/resume_coordinator_integration.rs` | New | Integration tests: full interrupted-run repair + re-trigger; `enabled=false` no-op. |

---

## Task 1: `RunStarted` / `RunFinished` / `RunOutcome` event variants + projection + store tags

**Files:**
- `src/session/events.rs`
- `src/session/state.rs`
- `src/session/store.rs`

- [ ] In `src/session/events.rs`, after the `ToolOutputMetadata` struct (ends at line 79) and before the `// NOTE: PartialEq` comment (line 81), add the `RunOutcome` enum:
  ```rust
  /// Terminal disposition of a harness run.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum RunOutcome {
      /// Run reached its natural end (model stop / final reply).
      Completed,
      /// Run was deliberately cancelled (user `/stop`). NOT resumed.
      Cancelled,
      /// Run ended with an error. NOT resumed (the error is in the log;
      /// re-running would likely hit the same error).
      Errored,
      /// Resume gave up on this run — cap reached or too old. Terminal.
      Abandoned,
  }
  ```
- [ ] In `src/session/events.rs`, in the `SessionEvent` enum, after the `SessionDetached { at: Timestamp }` variant (ends at line 98) and before `TurnStarted` (line 100), add the two run-marker variants:
  ```rust
      /// A harness run began on this session.
      RunStarted {
          run_id: String,
          at: Timestamp,
      },
      /// A harness run reached a terminal state on this session.
      RunFinished {
          run_id: String,
          outcome: RunOutcome,
          at: Timestamp,
      },
  ```
- [ ] In `src/session/state.rs`, in `SessionState::apply` (the exhaustive match starting line 36), after the `SessionEvent::SessionDetached { .. }` arm (ends line 45) and before the `SessionEvent::TurnStarted` arm (line 47), add no-op projection arms:
  ```rust
              SessionEvent::RunStarted { .. } => {
                  // Run markers are observational; resume detection scans the
                  // event log directly. No state mutation.
              }
              SessionEvent::RunFinished { .. } => {
                  // See RunStarted.
              }
  ```
- [ ] In `src/session/store.rs`, in `extract_turn_id` (the exhaustive match at lines 260-281), add `RunStarted` and `RunFinished` to the no-turn-id group: change the final `None`-returning arm (lines 277-280) to:
  ```rust
          SessionEvent::SessionCreated { .. }
          | SessionEvent::SessionWoken { .. }
          | SessionEvent::SessionDetached { .. }
          | SessionEvent::RunStarted { .. }
          | SessionEvent::RunFinished { .. }
          | SessionEvent::CompactionPerformed { .. } => None,
  ```
- [ ] In `src/session/store.rs`, in `event_type_tag` (the exhaustive match at lines 289-310), after the `SessionEvent::SessionDetached { .. } => "session_detached",` line (line 292) add:
  ```rust
          SessionEvent::RunStarted { .. } => "run_started",
          SessionEvent::RunFinished { .. } => "run_finished",
  ```
- [ ] In `src/session/state.rs`, in the `#[cfg(test)] mod tests` block (after the existing `budget_updated_is_absolute` test, before the closing `}` of the module at line 303), add a projection no-op test:
  ```rust
      #[test]
      fn run_markers_are_no_op_projections() {
          use crate::session::events::RunOutcome;
          let mut s = SessionState::default();
          let before_turns = s.completed_turns;
          s.apply(&SessionEvent::RunStarted {
              run_id: "run-1".into(),
              at: now_ms(),
          });
          s.apply(&SessionEvent::RunFinished {
              run_id: "run-1".into(),
              outcome: RunOutcome::Completed,
              at: now_ms(),
          });
          assert!(s.current_turn.is_none());
          assert_eq!(s.completed_turns, before_turns);
          assert_eq!(s.wake_count, 0);
      }
  ```
- [ ] In `src/session/events.rs`, add a `#[cfg(test)]` module at the end of the file (after `now_ms`, line 225) verifying serde round-trip via JSON (the enum has no `PartialEq`, so compare on serialized form):
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn run_started_serde_round_trips() {
          let ev = SessionEvent::RunStarted {
              run_id: "run-abc".into(),
              at: 1_700_000_000_000,
          };
          let json = serde_json::to_string(&ev).unwrap();
          let back: SessionEvent = serde_json::from_str(&json).unwrap();
          assert_eq!(serde_json::to_string(&back).unwrap(), json);
          assert!(json.contains("\"type\":\"run_started\""));
      }

      #[test]
      fn run_finished_serde_round_trips_each_outcome() {
          for outcome in [
              RunOutcome::Completed,
              RunOutcome::Cancelled,
              RunOutcome::Errored,
              RunOutcome::Abandoned,
          ] {
              let ev = SessionEvent::RunFinished {
                  run_id: "run-xyz".into(),
                  outcome,
                  at: 1_700_000_000_000,
              };
              let json = serde_json::to_string(&ev).unwrap();
              let back: SessionEvent = serde_json::from_str(&json).unwrap();
              assert_eq!(serde_json::to_string(&back).unwrap(), json);
              assert!(json.contains("\"type\":\"run_finished\""));
          }
      }

      #[test]
      fn run_outcome_renames_snake_case() {
          assert_eq!(
              serde_json::to_string(&RunOutcome::Completed).unwrap(),
              "\"completed\""
          );
          assert_eq!(
              serde_json::to_string(&RunOutcome::Abandoned).unwrap(),
              "\"abandoned\""
          );
      }
  }
  ```
- [ ] Run the new tests (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib session::events session::state::tests::run_markers session::store
  ```
  Expected: `run_started_serde_round_trips`, `run_finished_serde_round_trips_each_outcome`, `run_outcome_renames_snake_case`, `run_markers_are_no_op_projections` all pass; pre-existing `session::store` tests still pass.
- [ ] Commit: `git add src/session/events.rs src/session/state.rs src/session/store.rs && git commit -m "session: add RunStarted/RunFinished run-marker events"`

---

## Task 2: `FlowInput::Resume` variant + exhaustive-match arms + engine→orchestrator conversion

**Files:**
- `src/orchestrator/flow_spec.rs`
- `src/orchestrator/harness_bridge/session_seed.rs`
- `src/orchestrator/harness_bridge.rs`
- `src/gateway/execution_engine/run_loop.rs`

> **Conversion-path mapping (the one genuine unknown — verified):** The production `RunRequest.input: String` → `FlowInput` conversion is `super::helpers::history_to_flow_input(history.clone(), request.input.clone())` at `src/gateway/execution_engine/run_loop.rs:549-550`. `dispatch_via_orchestrator` in `execution_engine/orchestrator.rs:15` has **no callers** (confirmed dead path). `flow_run_tool.rs` and `orchestrator/tests/dispatch.rs` only *construct* `FlowInput::Prompt(...)` — they never `match` on `FlowInput` exhaustively, so they need **no** `Resume` arm. The only two exhaustive matches on `FlowInput` are `seed_session` (`session_seed.rs:25`) and `last_user_query` (`harness_bridge.rs:748`).

- [ ] In `src/orchestrator/flow_spec.rs`, in the `FlowInput` enum (lines 15-31, `#[non_exhaustive]`), after the `Multimodal(Vec<MessageContent>)` variant (line 30) add:
  ```rust
      /// Resume an interrupted run. Carries no input: the session event log
      /// already holds the full trajectory (including the original
      /// `UserMessage`). `seed_session` treats this as a no-op so replay is
      /// not corrupted by a duplicate user message.
      Resume,
  ```
- [ ] In `src/orchestrator/harness_bridge/session_seed.rs`, in `seed_session` (the exhaustive match starting line 25), after the `FlowInput::Multimodal(msgs)` arm (ends line 91) add:
  ```rust
          FlowInput::Resume => {
              // No-op: the session log already contains the original
              // UserMessage and the full prior trajectory. The harness
              // replays it and continues; re-seeding would duplicate the
              // user message.
          }
  ```
- [ ] In `src/orchestrator/harness_bridge.rs`, in `last_user_query` (the exhaustive match at lines 748-758), after the `FlowInput::History { prompt, .. } => prompt.clone(),` arm (line 757) add:
  ```rust
          FlowInput::Resume => String::new(),
  ```
- [ ] In `src/gateway/execution_engine/run_loop.rs`, replace the `FlowInput` construction at lines 549-550. The current code is:
  ```rust
          // Build FlowRequest
          let flow_input =
              super::helpers::history_to_flow_input(history.clone(), request.input.clone());
  ```
  Replace it with a resume-aware branch:
  ```rust
          // Build FlowRequest. A resumed run carries no fresh input — the
          // session event log already holds the full trajectory. The
          // `ResumeCoordinator` sets `metadata["resume"] = "true"`; the
          // harness bridge then skips seeding and replays the log.
          let flow_input = if request.metadata.get("resume").map(String::as_str)
              == Some("true")
          {
              crate::orchestrator::FlowInput::Resume
          } else {
              super::helpers::history_to_flow_input(history.clone(), request.input.clone())
          };
  ```
- [ ] Run a compile check (concurrency gate, background — confirms every `FlowInput` match site is exhaustive):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo check -p alephcore
  ```
  Expected: `Finished` with no `non-exhaustive patterns` errors. If the compiler flags any other `match input { FlowInput::... }` site not listed above, add a `FlowInput::Resume => { /* no-op */ }` arm there and note it.
- [ ] Commit: `git add src/orchestrator/flow_spec.rs src/orchestrator/harness_bridge/session_seed.rs src/orchestrator/harness_bridge.rs src/gateway/execution_engine/run_loop.rs && git commit -m "orchestrator: add FlowInput::Resume + metadata-resume conversion"`

---

## Task 3: Orchestrator emits `RunStarted` / `RunFinished` around `harness.run()`

**Files:**
- `src/orchestrator/harness_bridge.rs`

> **Verified context:** `AgentHarnessRunner::run` holds `self.session_service: Arc<dyn SessionService>`. The session is seeded at line 207; the harness runs at line 342 (`let run_result = harness.run(&session_id, &mut cb, &cancel).await;`); the result is unwrapped at lines 347-350 with `run_result.map_err(...)?` which early-returns on error. We must emit `RunFinished` in BOTH paths, so the marker emit happens BEFORE the `?`.

- [ ] In `src/orchestrator/harness_bridge.rs`, locate the harness-run block (lines 342-350):
  ```rust
          let run_result = harness.run(&session_id, &mut cb, &cancel).await;
          // Flush the trace sink regardless of success or error (no-op when None).
          if let Some(sink) = trace_sink.as_ref() {
              sink.flush();
          }
          run_result.map_err(|e| match e {
              crate::harness::trait_def::HarnessError::Cancelled => FlowError::Cancelled,
              other => error::classify_harness_error(other, &provider_name),
          })?;
  ```
  Replace it with a marker-emitting version. Generate a local `run_id`, emit `RunStarted` before the run, classify the outcome, emit `RunFinished` in both paths, then propagate the error:
  ```rust
          // Resume run markers. `run_id` is a locally-minted UUID — the marker
          // pair only needs to correlate within one session log, so the
          // gateway scheduler's run id is not required here. A crash between
          // these two emits leaves a trailing `RunStarted` with no
          // `RunFinished`, which is exactly what `ResumeCoordinator` detects.
          let run_marker_id = uuid::Uuid::new_v4().to_string();
          if let Err(e) = self
              .session_service
              .emit_event(
                  &session_id,
                  SessionEvent::RunStarted {
                      run_id: run_marker_id.clone(),
                      at: crate::session::events::now_ms(),
                  },
              )
              .await
          {
              tracing::warn!(error = %e, "failed to emit RunStarted marker");
          }

          let run_result = harness.run(&session_id, &mut cb, &cancel).await;
          // Flush the trace sink regardless of success or error (no-op when None).
          if let Some(sink) = trace_sink.as_ref() {
              sink.flush();
          }

          // Classify the outcome BEFORE the `?` so `RunFinished` is emitted
          // on the error path too. Ok → Completed; Cancelled → Cancelled;
          // any other error → Errored.
          let run_outcome = match &run_result {
              Ok(()) => crate::session::events::RunOutcome::Completed,
              Err(crate::harness::trait_def::HarnessError::Cancelled) => {
                  crate::session::events::RunOutcome::Cancelled
              }
              Err(_) => crate::session::events::RunOutcome::Errored,
          };
          if let Err(e) = self
              .session_service
              .emit_event(
                  &session_id,
                  SessionEvent::RunFinished {
                      run_id: run_marker_id.clone(),
                      outcome: run_outcome,
                      at: crate::session::events::now_ms(),
                  },
              )
              .await
          {
              tracing::warn!(error = %e, "failed to emit RunFinished marker");
          }

          run_result.map_err(|e| match e {
              crate::harness::trait_def::HarnessError::Cancelled => FlowError::Cancelled,
              other => error::classify_harness_error(other, &provider_name),
          })?;
  ```
- [ ] Run a compile check (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo check -p alephcore
  ```
  Expected: `Finished`. `SessionEvent` and `RunOutcome` are reachable — `SessionEvent` is already imported at `harness_bridge.rs:36` (`use crate::session::events::SessionEvent;`); `RunOutcome` and `now_ms` are referenced via fully-qualified paths above, so no new `use` is needed.
- [ ] Run the orchestrator tests (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib orchestrator
  ```
  Expected: all `orchestrator::*` tests pass (the `MockHarness` in `dispatch.rs` does not touch the session service, so the marker emits are inert there).
- [ ] Commit: `git add src/orchestrator/harness_bridge.rs && git commit -m "orchestrator: emit run markers around harness.run()"`

---

## Task 4: `SessionEventStore::load_run_markers()` cross-session query

**Files:**
- `src/session/store.rs`

> **Verified context:** The `SessionEventStore` trait is at `store.rs:36-63` with `append`/`load_all_events`/`load_events_range`/`load_head_seq`. The only impl is `SqliteEventStore` (line 142). The `session_id` column is `serde_json::to_string(&SessionId)` (see `session_id_to_string`, line 252) — deserialize back with `serde_json::from_str`. `SessionEventRecord { seq, event, created_at_ms }`. The `(session_id, event_type)` index already exists (migration line 91-92), so the `WHERE event_type IN (...)` scan is index-served.

- [ ] In `src/session/store.rs`, in the `SessionEventStore` trait (ends line 63), after `load_head_seq` (lines 61-62) add a new trait method:
  ```rust

      /// Cross-session scan for resume detection. Returns, per session, that
      /// session's `RunStarted` / `RunFinished` events in `seq` order.
      /// Sessions with no run markers are omitted. Served by the existing
      /// `(session_id, event_type)` index.
      async fn load_run_markers(
          &self,
      ) -> Result<Vec<(SessionId, Vec<SessionEventRecord>)>, SessionError>;
  ```
- [ ] In `src/session/store.rs`, in `impl SessionEventStore for SqliteEventStore` (ends line 241, after `load_head_seq`), add the implementation:
  ```rust

      async fn load_run_markers(
          &self,
      ) -> Result<Vec<(SessionId, Vec<SessionEventRecord>)>, SessionError> {
          let conn = self.conn.lock().await;
          let mut stmt = conn
              .prepare(
                  "SELECT session_id, seq, payload_json, created_at
                   FROM session_events
                   WHERE event_type IN ('run_started', 'run_finished')
                   ORDER BY session_id, seq ASC",
              )
              .map_err(|e| SessionError::Storage(e.to_string()))?;

          let rows = stmt
              .query_map([], |row| {
                  let session_id: String = row.get(0)?;
                  let seq: i64 = row.get(1)?;
                  let payload: String = row.get(2)?;
                  let created_at: i64 = row.get(3)?;
                  Ok((session_id, seq, payload, created_at))
              })
              .map_err(|e| SessionError::Storage(e.to_string()))?;

          // Group consecutive rows by session_id. The SQL `ORDER BY
          // session_id, seq` guarantees all of one session's markers are
          // contiguous, so a running group key is enough — no HashMap.
          let mut grouped: Vec<(SessionId, Vec<SessionEventRecord>)> = Vec::new();
          for row in rows {
              let (session_id_str, seq, payload, created_at) =
                  row.map_err(|e| SessionError::Storage(e.to_string()))?;
              let session_id: SessionId = serde_json::from_str(&session_id_str)?;
              let event: SessionEvent = serde_json::from_str(&payload)?;
              let record = SessionEventRecord {
                  seq: seq as EventSeq,
                  event,
                  created_at_ms: created_at,
              };
              match grouped.last_mut() {
                  Some((sid, records)) if *sid == session_id => {
                      records.push(record);
                  }
                  _ => grouped.push((session_id, vec![record])),
              }
          }
          Ok(grouped)
      }
  ```
  > Note: `SessionId` = `SessionKey`, which derives `PartialEq` (it is used as a `HashMap` key in `in_process.rs`), so `*sid == session_id` compiles.
- [ ] In `src/session/store.rs`, in the `#[cfg(test)] mod tests` block, after the existing `head_seq_returns_max` test (ends line 486, before the module's closing `}` at line 487), add `load_run_markers` tests. First add a helper near `turn_started` (after line 416):
  ```rust
      fn run_started(run_id: &str, at: i64) -> SessionEvent {
          SessionEvent::RunStarted {
              run_id: run_id.to_string(),
              at,
          }
      }

      fn run_finished(run_id: &str, at: i64) -> SessionEvent {
          SessionEvent::RunFinished {
              run_id: run_id.to_string(),
              outcome: crate::session::events::RunOutcome::Completed,
              at,
          }
      }
  ```
  Then the tests:
  ```rust
      #[tokio::test]
      async fn load_run_markers_empty_when_no_markers() {
          let store = make_store();
          let sid = sample_session_id();
          let tid = uuid::Uuid::new_v4();
          let at = now_ms();
          store.append(&sid, 1, &turn_started(tid, at), at).await.unwrap();
          let markers = store.load_run_markers().await.unwrap();
          assert!(markers.is_empty());
      }

      #[tokio::test]
      async fn load_run_markers_groups_by_session_in_seq_order() {
          let store = make_store();
          let sid = sample_session_id();
          let tid = uuid::Uuid::new_v4();
          let at = now_ms();
          // Interleave a non-marker event between two markers.
          store.append(&sid, 1, &run_started("r1", at), at).await.unwrap();
          store.append(&sid, 2, &turn_started(tid, at), at).await.unwrap();
          store
              .append(&sid, 3, &run_finished("r1", at + 5), at + 5)
              .await
              .unwrap();
          store
              .append(&sid, 4, &run_started("r2", at + 10), at + 10)
              .await
              .unwrap();

          let markers = store.load_run_markers().await.unwrap();
          assert_eq!(markers.len(), 1, "exactly one session has markers");
          let (got_sid, records) = &markers[0];
          assert_eq!(*got_sid, sid);
          assert_eq!(records.len(), 3, "3 markers, non-marker excluded");
          assert_eq!(records[0].seq, 1);
          assert_eq!(records[1].seq, 3);
          assert_eq!(records[2].seq, 4);
          assert!(matches!(records[0].event, SessionEvent::RunStarted { .. }));
          assert!(matches!(records[1].event, SessionEvent::RunFinished { .. }));
          assert!(matches!(records[2].event, SessionEvent::RunStarted { .. }));
      }

      #[tokio::test]
      async fn load_run_markers_separates_distinct_sessions() {
          let store = make_store();
          let sid_a = SessionKey::ephemeral("sess-a");
          let sid_b = SessionKey::ephemeral("sess-b");
          let at = now_ms();
          store.append(&sid_a, 1, &run_started("ra", at), at).await.unwrap();
          store.append(&sid_b, 1, &run_started("rb", at), at).await.unwrap();
          let markers = store.load_run_markers().await.unwrap();
          assert_eq!(markers.len(), 2);
      }
  ```
- [ ] Run the new store tests (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib session::store
  ```
  Expected: `load_run_markers_empty_when_no_markers`, `load_run_markers_groups_by_session_in_seq_order`, `load_run_markers_separates_distinct_sessions` pass; pre-existing `session::store` tests still pass.
- [ ] Commit: `git add src/session/store.rs && git commit -m "session: add load_run_markers cross-session query"`

---

## Task 5: `ResumeCoordinator` — scan + recency + cap + crash-boundary repair

**Files:**
- `src/gateway/resume_coordinator.rs` (new)
- `src/gateway/mod.rs`
- `src/config/types/resume.rs` (new)
- `src/config/types/mod.rs`
- `src/config/structs.rs`

> This task builds the `ResumeCoordinator` struct, the `ResumeConfig` config type, and the pure scan/repair logic — everything except the actual `ExecutionAdapter` re-trigger (Task 6). The re-trigger seam is left as a TODO stub that Task 6 fills.

- [ ] Create `src/config/types/resume.rs`:
  ```rust
  //! Mid-run trajectory resume configuration.

  use schemars::JsonSchema;
  use serde::{Deserialize, Serialize};

  /// `[resume]` config section — boot-scan auto-resume of interrupted runs.
  #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
  pub struct ResumeConfig {
      /// Master switch. When false the `ResumeCoordinator` is not spawned.
      #[serde(default = "default_resume_enabled")]
      pub enabled: bool,

      /// Don't resume runs interrupted more than this many seconds ago
      /// (default: 86400 = 24h). Older candidates are marked `Abandoned`.
      #[serde(default = "default_resume_max_age_secs")]
      pub max_age_secs: u64,

      /// Abandon a run after this many consecutive crash-loops (default: 3).
      #[serde(default = "default_resume_max_attempts")]
      pub max_attempts: u32,

      /// Cap simultaneous resumes at boot to protect the freshly-booted
      /// process and provider rate limits (default: 4).
      #[serde(default = "default_resume_max_concurrent")]
      pub max_concurrent: usize,
  }

  fn default_resume_enabled() -> bool {
      true
  }

  fn default_resume_max_age_secs() -> u64 {
      86_400
  }

  fn default_resume_max_attempts() -> u32 {
      3
  }

  fn default_resume_max_concurrent() -> usize {
      4
  }

  impl Default for ResumeConfig {
      fn default() -> Self {
          Self {
              enabled: default_resume_enabled(),
              max_age_secs: default_resume_max_age_secs(),
              max_attempts: default_resume_max_attempts(),
              max_concurrent: default_resume_max_concurrent(),
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn defaults_are_sane() {
          let c = ResumeConfig::default();
          assert!(c.enabled);
          assert_eq!(c.max_age_secs, 86_400);
          assert_eq!(c.max_attempts, 3);
          assert_eq!(c.max_concurrent, 4);
      }

      #[test]
      fn serde_with_missing_fields_uses_defaults() {
          let parsed: ResumeConfig = toml::from_str("").unwrap();
          assert!(parsed.enabled);
          assert_eq!(parsed.max_attempts, 3);
      }

      #[test]
      fn serde_round_trip() {
          let c = ResumeConfig {
              enabled: false,
              max_age_secs: 100,
              max_attempts: 9,
              max_concurrent: 1,
          };
          let toml = toml::to_string(&c).unwrap();
          let back: ResumeConfig = toml::from_str(&toml).unwrap();
          assert!(!back.enabled);
          assert_eq!(back.max_age_secs, 100);
          assert_eq!(back.max_attempts, 9);
          assert_eq!(back.max_concurrent, 1);
      }
  }
  ```
- [ ] In `src/config/types/mod.rs`, add the module declaration. After `pub mod prompt;` (the `pub mod` list ends before `pub use` re-exports) add `pub mod resume;`, and in the re-export section (alongside the other `pub use <module>::*;` lines) add `pub use resume::*;`.
- [ ] In `src/config/structs.rs`, in the `Config` struct, after the `pub context_budget: Option<ContextBudgetToml>` field (line 204) add:
  ```rust
      /// Mid-run trajectory resume — boot-scan auto-resume of interrupted runs.
      #[serde(default)]
      pub resume: crate::config::types::ResumeConfig,
  ```
- [ ] In `src/config/structs.rs`, in `impl Default for Config` (the struct-literal in `fn default()`), after the `context_budget: None,` line (line 399) add:
  ```rust
              resume: crate::config::types::ResumeConfig::default(),
  ```
- [ ] Create `src/gateway/resume_coordinator.rs` with the struct, the report type, and the pure scan/repair logic. The re-trigger method (`retrigger`) is a stub here — Task 6 implements it:
  ```rust
  //! ResumeCoordinator — boot-scan auto-resume of interrupted agent runs.
  //!
  //! Cycle 6 of the long-task hardening directive. See
  //! `docs/superpowers/specs/2026-05-21-mid-run-trajectory-resume-design.md`.
  //!
  //! A run is **interrupted** iff a session's event log ends with one or more
  //! `RunStarted` events and no `RunFinished` after the last one. This module
  //! scans for that shape, repairs the crash boundary (synthetic `ToolError`
  //! for each dangling tool call), and re-triggers each surviving candidate.
  //!
  //! R10-safe: `src/harness/` is untouched. The harness already replays the
  //! event log on every `run()`; resume only re-triggers it.

  use std::sync::Arc;

  use crate::config::types::ResumeConfig;
  use crate::session::events::{now_ms, RunOutcome, SessionEvent, SessionEventRecord};
  use crate::session::service::SessionId;
  use crate::session::store::SessionEventStore;

  /// Summary of one `resume_interrupted_runs` pass — for the boot log line
  /// and for tests.
  #[derive(Debug, Default, Clone, PartialEq, Eq)]
  pub struct ResumeReport {
      /// Sessions inspected that had at least one run marker.
      pub scanned: usize,
      /// Interrupted runs successfully re-triggered.
      pub resumed: usize,
      /// Runs marked `Abandoned` (too old or crash-loop cap reached).
      pub abandoned: usize,
      /// Sessions skipped (clean — newest marker is `RunFinished`).
      pub skipped: usize,
  }

  /// Classification of one session's run-marker tail.
  #[derive(Debug, PartialEq, Eq)]
  pub(crate) enum ScanVerdict {
      /// Newest marker is `RunFinished` — nothing to do.
      Clean,
      /// Interrupted; the `usize` is the count of trailing consecutive
      /// `RunStarted` events (the crash-loop attempt counter).
      Interrupted { trailing_starts: usize },
  }

  /// Classify a session's run markers (already in `seq` order, as returned by
  /// `load_run_markers`). Counts the trailing run of consecutive `RunStarted`
  /// events — events after the last `RunFinished`, or all of them if there is
  /// no `RunFinished`.
  pub(crate) fn classify_markers(markers: &[SessionEventRecord]) -> ScanVerdict {
      let mut trailing_starts = 0usize;
      for record in markers.iter().rev() {
          match &record.event {
              SessionEvent::RunStarted { .. } => trailing_starts += 1,
              SessionEvent::RunFinished { .. } => break,
              // load_run_markers only ever returns run markers, but be
              // defensive: a non-marker breaks the trailing run.
              _ => break,
          }
      }
      if trailing_starts == 0 {
          ScanVerdict::Clean
      } else {
          ScanVerdict::Interrupted { trailing_starts }
      }
  }

  /// Walk a full session event log and return a synthetic `ToolError` for
  /// every `ToolCallRequested` whose `call_id` has no matching `ToolResult`
  /// or `ToolError`. The returned events are ready to append to the log; the
  /// caller emits them in order. An already-answered call yields nothing.
  pub(crate) fn compute_boundary_repairs(
      events: &[SessionEventRecord],
  ) -> Vec<SessionEvent> {
      use std::collections::HashSet;

      let mut answered: HashSet<&str> = HashSet::new();
      for record in events {
          match &record.event {
              SessionEvent::ToolResult { call_id, .. }
              | SessionEvent::ToolError { call_id, .. } => {
                  answered.insert(call_id.as_str());
              }
              _ => {}
          }
      }

      let at = now_ms();
      events
          .iter()
          .filter_map(|record| match &record.event {
              SessionEvent::ToolCallRequested {
                  turn_id, call_id, ..
              } if !answered.contains(call_id.as_str()) => {
                  Some(SessionEvent::ToolError {
                      turn_id: *turn_id,
                      call_id: call_id.clone(),
                      error: "interrupted by server restart".to_string(),
                      at,
                  })
              }
              _ => None,
          })
          .collect()
  }

  /// Boot-scan coordinator. Constructed at boot with the durable event store,
  /// the config, and (Task 6) the re-trigger collaborators.
  pub struct ResumeCoordinator {
      event_store: Arc<dyn SessionEventStore>,
      config: ResumeConfig,
      // Task 6 adds: execution_adapter, agent_registry, channel_registry cell.
  }

  impl ResumeCoordinator {
      /// Construct a coordinator. Task 6 widens this signature with the
      /// re-trigger collaborators.
      pub fn new(event_store: Arc<dyn SessionEventStore>, config: ResumeConfig) -> Self {
          Self {
              event_store,
              config,
          }
      }

      /// Scan for interrupted runs and re-trigger each. Best-effort: any
      /// failure is logged and skipped; never panics, never blocks boot.
      /// A no-op when `config.enabled` is false — this self-guard is what
      /// makes the disabled path directly testable (the boot wiring also
      /// skips spawning the coordinator, so the two guards are defensive
      /// duplicates, both cheap).
      pub async fn resume_interrupted_runs(&self) -> ResumeReport {
          let mut report = ResumeReport::default();

          if !self.config.enabled {
              tracing::debug!("resume disabled ([resume] enabled = false); skipping scan");
              return report;
          }

          let marker_groups = match self.event_store.load_run_markers().await {
              Ok(g) => g,
              Err(e) => {
                  tracing::warn!(error = %e, "resume scan failed; skipping resume");
                  return report;
              }
          };

          for (session_id, markers) in marker_groups {
              report.scanned += 1;
              match classify_markers(&markers) {
                  ScanVerdict::Clean => {
                      report.skipped += 1;
                  }
                  ScanVerdict::Interrupted { trailing_starts } => {
                      self.handle_interrupted(&session_id, &markers, trailing_starts, &mut report)
                          .await;
                  }
              }
          }

          tracing::info!(
              scanned = report.scanned,
              resumed = report.resumed,
              abandoned = report.abandoned,
              skipped = report.skipped,
              "resume scan complete"
          );
          report
      }

      /// Handle one interrupted candidate: recency filter, cap check,
      /// crash-boundary repair, then re-trigger.
      async fn handle_interrupted(
          &self,
          session_id: &SessionId,
          markers: &[SessionEventRecord],
          trailing_starts: usize,
          report: &mut ResumeReport,
      ) {
          // The dangling RunStarted is the last marker (classify_markers
          // guarantees `markers` is non-empty here).
          let last = markers
              .last()
              .expect("Interrupted verdict implies non-empty markers");

          // Recency filter — abandon runs interrupted too long ago.
          let age_ms = now_ms().saturating_sub(last.created_at_ms);
          if age_ms > (self.config.max_age_secs as i64).saturating_mul(1000) {
              tracing::info!(
                  session = ?session_id,
                  age_ms,
                  "resume: candidate too old; abandoning"
              );
              self.abandon(session_id).await;
              report.abandoned += 1;
              return;
          }

          // Cap check — abandon crash-looped runs.
          if trailing_starts as u32 >= self.config.max_attempts {
              tracing::warn!(
                  session = ?session_id,
                  trailing_starts,
                  max_attempts = self.config.max_attempts,
                  "resume: crash-loop cap reached; abandoning"
              );
              self.abandon(session_id).await;
              report.abandoned += 1;
              return;
          }

          // Crash-boundary repair — append a synthetic ToolError for each
          // dangling tool call so the provider API sees a balanced log.
          if let Err(e) = self.repair_boundary(session_id).await {
              tracing::warn!(
                  session = ?session_id,
                  error = %e,
                  "resume: boundary repair failed; skipping candidate"
              );
              return;
          }

          // Re-trigger. Task 6 implements `retrigger`.
          match self.retrigger(session_id).await {
              Ok(()) => report.resumed += 1,
              Err(e) => {
                  tracing::warn!(
                      session = ?session_id,
                      error = %e,
                      "resume: re-trigger failed; skipping candidate"
                  );
              }
          }
      }

      /// Emit `RunFinished { Abandoned }` so a terminal run is not re-scanned
      /// on the next boot. Best-effort.
      async fn abandon(&self, session_id: &SessionId) {
          let ev = SessionEvent::RunFinished {
              run_id: format!("abandoned-{}", uuid::Uuid::new_v4()),
              outcome: RunOutcome::Abandoned,
              at: now_ms(),
          };
          let seq = self.next_seq(session_id).await;
          if let Err(e) = self
              .event_store
              .append(session_id, seq, &ev, now_ms())
              .await
          {
              tracing::warn!(session = ?session_id, error = %e, "resume: abandon marker append failed");
          }
      }

      /// Append synthetic `ToolError`s for any dangling tool calls.
      async fn repair_boundary(
          &self,
          session_id: &SessionId,
      ) -> Result<(), crate::session::service::SessionError> {
          let events = self.event_store.load_all_events(session_id).await?;
          let repairs = compute_boundary_repairs(&events);
          if repairs.is_empty() {
              return Ok(());
          }
          let mut next = self.event_store.load_head_seq(session_id).await? + 1;
          for ev in repairs {
              self.event_store
                  .append(session_id, next, &ev, now_ms())
                  .await?;
              next += 1;
          }
          Ok(())
      }

      /// Allocate the next append seq for a session.
      async fn next_seq(&self, session_id: &SessionId) -> u64 {
          self.event_store
              .load_head_seq(session_id)
              .await
              .map(|h| h + 1)
              .unwrap_or(1)
      }

      /// Re-trigger an interrupted run. **Stub** — implemented in Task 6.
      async fn retrigger(
          &self,
          _session_id: &SessionId,
      ) -> Result<(), crate::session::service::SessionError> {
          // Task 6: resolve the agent, build a RunRequest with
          // metadata["resume"]="true", call ExecutionAdapter::execute under
          // a max_concurrent semaphore.
          Ok(())
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::session::events::{ToolOutput, TurnId};

      fn rec(seq: u64, event: SessionEvent, created_at_ms: i64) -> SessionEventRecord {
          SessionEventRecord {
              seq,
              event,
              created_at_ms,
          }
      }

      fn run_started(at: i64) -> SessionEvent {
          SessionEvent::RunStarted {
              run_id: format!("r-{at}"),
              at,
          }
      }

      fn run_finished(at: i64) -> SessionEvent {
          SessionEvent::RunFinished {
              run_id: format!("r-{at}"),
              outcome: RunOutcome::Completed,
              at,
          }
      }

      fn tool_requested(call_id: &str) -> SessionEvent {
          SessionEvent::ToolCallRequested {
              turn_id: TurnId::new_v4(),
              call_id: call_id.to_string(),
              name: "bash_exec".to_string(),
              input: serde_json::json!({}),
              at: 1,
          }
      }

      fn tool_result(call_id: &str) -> SessionEvent {
          SessionEvent::ToolResult {
              turn_id: TurnId::new_v4(),
              call_id: call_id.to_string(),
              output: ToolOutput {
                  value: serde_json::json!("ok"),
                  metadata: Default::default(),
              },
              at: 2,
          }
      }

      #[test]
      fn classify_clean_when_last_marker_is_finished() {
          let markers = vec![
              rec(1, run_started(10), 10),
              rec(2, run_finished(20), 20),
          ];
          assert_eq!(classify_markers(&markers), ScanVerdict::Clean);
      }

      #[test]
      fn classify_interrupted_single_dangling_start() {
          let markers = vec![
              rec(1, run_started(10), 10),
              rec(2, run_finished(20), 20),
              rec(3, run_started(30), 30),
          ];
          assert_eq!(
              classify_markers(&markers),
              ScanVerdict::Interrupted { trailing_starts: 1 }
          );
      }

      #[test]
      fn classify_counts_consecutive_trailing_starts() {
          // Three crash-loops after the last finish.
          let markers = vec![
              rec(1, run_finished(10), 10),
              rec(2, run_started(20), 20),
              rec(3, run_started(30), 30),
              rec(4, run_started(40), 40),
          ];
          assert_eq!(
              classify_markers(&markers),
              ScanVerdict::Interrupted { trailing_starts: 3 }
          );
      }

      #[test]
      fn classify_interrupted_when_no_finish_at_all() {
          let markers = vec![rec(1, run_started(10), 10)];
          assert_eq!(
              classify_markers(&markers),
              ScanVerdict::Interrupted { trailing_starts: 1 }
          );
      }

      #[test]
      fn repair_yields_one_tool_error_per_dangling_call() {
          let events = vec![
              rec(1, tool_requested("c1"), 1),
              rec(2, tool_result("c1"), 2),
              rec(3, tool_requested("c2"), 3),
              // c2 never answered → one repair.
          ];
          let repairs = compute_boundary_repairs(&events);
          assert_eq!(repairs.len(), 1);
          match &repairs[0] {
              SessionEvent::ToolError { call_id, error, .. } => {
                  assert_eq!(call_id, "c2");
                  assert_eq!(error, "interrupted by server restart");
              }
              other => panic!("expected ToolError, got {other:?}"),
          }
      }

      #[test]
      fn repair_yields_nothing_when_all_calls_answered() {
          let events = vec![
              rec(1, tool_requested("c1"), 1),
              rec(2, tool_result("c1"), 2),
          ];
          assert!(compute_boundary_repairs(&events).is_empty());
      }

      #[test]
      fn repair_treats_tool_error_as_an_answer() {
          let events = vec![
              rec(1, tool_requested("c1"), 1),
              rec(
                  2,
                  SessionEvent::ToolError {
                      turn_id: TurnId::new_v4(),
                      call_id: "c1".into(),
                      error: "prior failure".into(),
                      at: 2,
                  },
                  2,
              ),
          ];
          assert!(compute_boundary_repairs(&events).is_empty());
      }
  }
  ```
- [ ] In `src/gateway/mod.rs`, register the new module. Add `pub mod resume_coordinator;` alongside the other `pub mod` lines, and (matching the style of sibling re-exports in that file) add `pub use resume_coordinator::{ResumeCoordinator, ResumeReport};`.
- [ ] Run the new unit tests + config tests (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib resume_coordinator config::types::resume
  ```
  Expected: all `classify_*`, `repair_*`, and `ResumeConfig` tests pass.
- [ ] Commit: `git add src/gateway/resume_coordinator.rs src/gateway/mod.rs src/config/types/resume.rs src/config/types/mod.rs src/config/structs.rs && git commit -m "gateway: add ResumeCoordinator scan + repair + ResumeConfig"`

---

## Task 6: `ResumeCoordinator` re-trigger + boot wiring + `[resume]` config wiring

**Files:**
- `src/gateway/resume_coordinator.rs`
- `src/bin/aleph-server/commands/start/mod.rs`

> **Verified context — re-trigger precedent:** `src/tasks/cron/executor.rs` builds a `RunRequest` (lines 97-106) with `run_id = Uuid::new_v4().to_string()`, resolves the agent via `registry.get(agent_id).await` (`AgentRegistry` = `crate::gateway::agent_instance::AgentRegistry`, `get` is **async**, returns `Option<Arc<AgentInstance>>`), builds `emitter = Arc::new(CollectingEventEmitter::new())`, and dispatches `adapter.execute(request, agent, emitter).await` (line 123). `ExecutionAdapter::execute(&self, request: RunRequest, agent: Arc<AgentInstance>, emitter: Arc<dyn EventEmitter+Send+Sync>) -> Result<(), ExecutionError>` (`execution_adapter.rs:35`). `RunRequest` has no `resume` field — the resume signal rides `metadata` (Task 2's conversion site already reads `metadata["resume"]`).
>
> **Verified context — `SessionId` → re-trigger fields:** `SessionId = SessionKey`. `SessionKey::agent_id()` yields the agent id string (used by cron at `executor.rs:516` and elsewhere). `SessionKey::to_key_string()` yields the canonical string form used for `RunRequest`/`session_hint`. `RunRequest.session_key` takes the `SessionKey` directly.
>
> **Verified context — boot:** `ExecutionAdapter` + `AgentRegistry` come from `agent_result.execution_adapter` / `agent_result.agent_registry` (both `Option<Arc<_>>`), used at `start/mod.rs:1476-1478` (cron) and `1539-1543` (heartbeat). `session_service_for_orchestrator` (an `Option<Arc<dyn SessionService>>`) is built at line 627 by `build_sqlite_session_service`. **The `ResumeCoordinator` needs the `Arc<dyn SessionEventStore>` directly** (for `load_run_markers`), which `build_sqlite_session_service` currently builds privately and discards. This task changes that function to also return the store.

- [ ] In `src/gateway/resume_coordinator.rs`, widen the imports at the top of the file — add:
  ```rust
  use tokio::sync::Semaphore;

  use crate::gateway::agent_instance::AgentRegistry;
  use crate::gateway::event_emitter::CollectingEventEmitter;
  use crate::gateway::execution_adapter::ExecutionAdapter;
  use crate::gateway::execution_engine::RunRequest;
  ```
- [ ] In `src/gateway/resume_coordinator.rs`, widen the `ResumeCoordinator` struct (replace the existing struct + `new`) to carry the re-trigger collaborators and the concurrency semaphore:
  ```rust
  /// Boot-scan coordinator. Constructed at boot with the durable event store,
  /// the config, and the re-trigger collaborators (execution adapter + agent
  /// registry). Mirrors the cron / heartbeat system-initiated-run precedent.
  pub struct ResumeCoordinator {
      event_store: Arc<dyn SessionEventStore>,
      config: ResumeConfig,
      execution_adapter: Arc<dyn ExecutionAdapter>,
      agent_registry: Arc<AgentRegistry>,
      /// Bounds the boot resume burst. `max_concurrent` permits.
      semaphore: Arc<Semaphore>,
  }

  impl ResumeCoordinator {
      /// Construct a coordinator.
      pub fn new(
          event_store: Arc<dyn SessionEventStore>,
          config: ResumeConfig,
          execution_adapter: Arc<dyn ExecutionAdapter>,
          agent_registry: Arc<AgentRegistry>,
      ) -> Self {
          let permits = config.max_concurrent.max(1);
          Self {
              event_store,
              config,
              execution_adapter,
              agent_registry,
              semaphore: Arc::new(Semaphore::new(permits)),
          }
      }
  ```
  > Note: the closing `}` of `impl ResumeCoordinator` stays where it is — only the struct + `new` are replaced. `resume_interrupted_runs`, `handle_interrupted`, `abandon`, `repair_boundary`, `next_seq` from Task 5 are unchanged.
- [ ] In `src/gateway/resume_coordinator.rs`, replace the Task-5 stub `retrigger` method with the real implementation:
  ```rust
      /// Re-trigger an interrupted run. Resolves the agent from the session
      /// key, builds a `RunRequest` with `metadata["resume"] = "true"` (the
      /// engine→orchestrator boundary converts that into `FlowInput::Resume`,
      /// which skips re-seeding), and dispatches it through the same
      /// `ExecutionAdapter` cron / heartbeat use. A `max_concurrent`
      /// semaphore bounds the boot burst.
      async fn retrigger(
          &self,
          session_id: &SessionId,
      ) -> Result<(), crate::session::service::SessionError> {
          use crate::session::service::SessionError;
          use std::collections::HashMap;

          let permit = self
              .semaphore
              .clone()
              .acquire_owned()
              .await
              .map_err(|e| SessionError::Other(format!("resume semaphore closed: {e}")))?;

          let agent_id = session_id.agent_id().to_string();
          let agent = self
              .agent_registry
              .get(&agent_id)
              .await
              .ok_or_else(|| {
                  SessionError::Other(format!("resume: agent '{agent_id}' not registered"))
              })?;

          let mut metadata: HashMap<String, String> = HashMap::new();
          metadata.insert("resume".to_string(), "true".to_string());

          let request = RunRequest {
              run_id: uuid::Uuid::new_v4().to_string(),
              // Empty input — `FlowInput::Resume` ignores it; the session log
              // already holds the original UserMessage.
              input: String::new(),
              session_key: session_id.clone(),
              timeout_secs: None,
              metadata,
              attachments: Vec::new(),
              pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
              sandbox_override: None,
          };

          let collector = Arc::new(CollectingEventEmitter::new());
          let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
              Arc::clone(&collector) as _;

          tracing::info!(session = ?session_id, agent_id, "resume: re-triggering interrupted run");

          let result = self
              .execution_adapter
              .execute(request, agent, emitter)
              .await
              .map_err(|e| SessionError::Other(format!("resume execute failed: {e}")));

          drop(permit);
          result
      }
  ```
  > Note: `crate::orchestrator::harness_bridge` emits a fresh `RunStarted`/`RunFinished` pair for this resumed run (Task 3), so a successful resume self-terminates its own marker tail; a re-crash leaves another trailing `RunStarted`, which the cap counter (`max_attempts`) catches on the next boot.
- [ ] In `src/bin/aleph-server/commands/start/mod.rs`, change `build_sqlite_session_service` (lines 239-266) so it also returns the `Arc<dyn SessionEventStore>`. Change the return type and the two `Some(...)` / `Some(Arc::new(...))` returns:
  - Change the signature return type from
    ```rust
    ) -> Option<Arc<dyn alephcore::session::service::SessionService>> {
    ```
    to
    ```rust
    ) -> Option<(
        Arc<dyn alephcore::session::service::SessionService>,
        Arc<dyn alephcore::session::store::SessionEventStore>,
    )> {
    ```
  - At the end of the function, replace the final block (lines 261-265):
    ```rust
        let store: Arc<dyn alephcore::session::store::SessionEventStore> =
            Arc::new(alephcore::session::store::SqliteEventStore::new(conn));
        Some(Arc::new(
            alephcore::session::in_process::InProcessActorSessionService::new(store),
        ))
    ```
    with:
    ```rust
        let store: Arc<dyn alephcore::session::store::SessionEventStore> =
            Arc::new(alephcore::session::store::SqliteEventStore::new(conn));
        let service: Arc<dyn alephcore::session::service::SessionService> = Arc::new(
            alephcore::session::in_process::InProcessActorSessionService::new(store.clone()),
        );
        Some((service, store))
    ```
- [ ] In `src/bin/aleph-server/commands/start/mod.rs`, update the call site at lines 627-628. The current code is:
  ```rust
      let session_service_for_orchestrator =
          build_sqlite_session_service(&alephcore::gateway::SessionManagerConfig::default().db_path);
  ```
  Replace it with a split that keeps both halves:
  ```rust
      let session_service_and_store =
          build_sqlite_session_service(&alephcore::gateway::SessionManagerConfig::default().db_path);
      let session_service_for_orchestrator = session_service_and_store
          .as_ref()
          .map(|(svc, _store)| svc.clone());
      let session_event_store_for_resume = session_service_and_store
          .as_ref()
          .map(|(_svc, store)| store.clone());
  ```
  > Every existing use of `session_service_for_orchestrator` (the `.clone()` at 635, the tuple match at 1107-1110) keeps compiling — its type is still `Option<Arc<dyn SessionService>>`.
- [ ] In `src/bin/aleph-server/commands/start/mod.rs`, after the heartbeat spawn block (the `}` closing the `if let Some(ref hb_svc) = heartbeat_service { ... }` block, ending at line 1643) and before the GroupChat block (`// Initialize GroupChat Orchestrator + Executor`, line 1645), insert the resume-coordinator spawn:
  ```rust
      // Spawn the boot-scan ResumeCoordinator (after cron + heartbeat, so the
      // execution subsystems exist). Detached — boot is NOT blocked on it.
      {
          let app_cfg = app_config.read().await;
          let resume_cfg = app_cfg.resume.clone();
          drop(app_cfg);
          if !resume_cfg.enabled {
              if !args.daemon {
                  println!("Resume coordinator: disabled ([resume] enabled = false)");
              }
          } else if let (Some(event_store), Some(exec_adapter), Some(registry)) = (
              session_event_store_for_resume.clone(),
              agent_result.execution_adapter.clone(),
              agent_result.agent_registry.clone(),
          ) {
              let coordinator = alephcore::gateway::ResumeCoordinator::new(
                  event_store,
                  resume_cfg,
                  exec_adapter,
                  registry,
              );
              tokio::spawn(async move {
                  let report = coordinator.resume_interrupted_runs().await;
                  tracing::info!(
                      scanned = report.scanned,
                      resumed = report.resumed,
                      abandoned = report.abandoned,
                      skipped = report.skipped,
                      "ResumeCoordinator boot scan finished"
                  );
              });
              if !args.daemon {
                  println!("Resume coordinator: boot scan spawned");
              }
          } else if !args.daemon {
              println!("Resume coordinator: skipped (no session event store / execution adapter)");
          }
      }

  ```
  > `app_config` is the `Arc<RwLock<Config>>` already in scope (read at line 1648 immediately below as `app_config_for_channels` — confirm the exact in-scope binding name when implementing; it is `app_config` at the orchestrator-init site, line 1130 `app_config.read().await`). Use whichever `Arc<RwLock<Config>>` binding is live at this point.
- [ ] Run a full compile check (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo check -p alephcore && cargo check --bin aleph-server
  ```
  Expected: both `Finished`. If `app_config` is not the live binding name at the insertion point, the compiler will say `cannot find value` — fix by using the correct `Arc<RwLock<Config>>` binding (grep `app_config` in the function).
- [ ] Run the resume + boot-adjacent tests (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib resume_coordinator
  ```
  Expected: all Task-5 `classify_*` / `repair_*` tests still pass (the struct widening did not change them).
- [ ] Commit: `git add src/gateway/resume_coordinator.rs src/bin/aleph-server/commands/start/mod.rs && git commit -m "gateway: wire ResumeCoordinator re-trigger + boot scan"`

---

## Task 7: Integration tests + audit + final review — STOP, do not merge

**Files:**
- `tests/resume_coordinator_integration.rs` (new)

> The spec's §7 "Integration" requires two end-to-end tests: (1) a seeded interrupted run is repaired and re-triggered with `metadata["resume"] == "true"`; (2) `resume.enabled = false` → `execute` is never called. We test against a real `SqliteEventStore` (in-memory) and a mock `ExecutionAdapter` that records its calls.

- [ ] Create `tests/resume_coordinator_integration.rs`:
  ```rust
  //! Integration tests for the mid-run trajectory resume boot scan.
  //!
  //! Spec: docs/superpowers/specs/2026-05-21-mid-run-trajectory-resume-design.md §7.

  use std::collections::HashMap;
  use std::sync::Arc;

  use async_trait::async_trait;
  use tokio::sync::Mutex;

  use alephcore::config::types::ResumeConfig;
  use alephcore::gateway::agent_instance::AgentRegistry;
  use alephcore::gateway::event_emitter::EventEmitter;
  use alephcore::gateway::execution_adapter::ExecutionAdapter;
  use alephcore::gateway::execution_engine::{ExecutionError, RunRequest, RunStatus};
  use alephcore::gateway::agent_instance::AgentInstance;
  use alephcore::gateway::ResumeCoordinator;
  use alephcore::routing::session_key::SessionKey;
  use alephcore::session::events::{now_ms, RunOutcome, SessionEvent, TurnId};
  use alephcore::session::store::{
      migrate_add_session_events, SessionEventStore, SqliteEventStore,
  };

  /// Mock `ExecutionAdapter` that records every `execute` call's
  /// `(session_key, metadata)` so the test can assert resume signalling.
  struct RecordingAdapter {
      calls: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
  }

  impl RecordingAdapter {
      fn new() -> Self {
          Self {
              calls: Arc::new(Mutex::new(Vec::new())),
          }
      }
  }

  #[async_trait]
  impl ExecutionAdapter for RecordingAdapter {
      async fn execute(
          &self,
          request: RunRequest,
          _agent: Arc<AgentInstance>,
          _emitter: Arc<dyn EventEmitter + Send + Sync>,
      ) -> Result<(), ExecutionError> {
          self.calls
              .lock()
              .await
              .push((request.session_key.to_key_string(), request.metadata.clone()));
          Ok(())
      }

      async fn cancel(&self, run_id: &str) -> Result<(), ExecutionError> {
          Err(ExecutionError::RunNotFound(run_id.to_string()))
      }

      async fn get_status(&self, _run_id: &str) -> Option<RunStatus> {
          None
      }

      async fn active_run_count(&self) -> usize {
          0
      }
  }

  /// Build an `AgentRegistry` containing one agent whose id matches the
  /// `SessionKey` under test, so `retrigger`'s `registry.get(agent_id)`
  /// resolves.
  async fn registry_with_agent(agent_id: &str) -> Arc<AgentRegistry> {
      use alephcore::gateway::agent_instance::AgentInstanceConfig;
      use alephcore::gateway::session_manager::{SessionManager, SessionManagerConfig};

      let temp = tempfile::tempdir().unwrap();
      let sm = Arc::new(
          SessionManager::new(SessionManagerConfig {
              db_path: temp.path().join("sessions.db"),
              ..Default::default()
          })
          .expect("session manager"),
      );
      let cfg = AgentInstanceConfig {
          agent_id: agent_id.to_string(),
          workspace: temp.path().join("ws"),
          agent_dir: temp.path().join("agents").join(agent_id),
          ..Default::default()
      };
      // `AgentRegistry::register` takes `AgentInstance` BY VALUE (not `Arc`)
      // and is `async` (verified: agent_instance.rs:551). `get` then returns
      // `Arc<AgentInstance>`.
      let agent = AgentInstance::new(cfg, sm).unwrap();
      let registry = Arc::new(AgentRegistry::new());
      registry.register(agent).await;
      // Keep `temp` alive for the test by leaking it — the dirs must outlive
      // the registry. Acceptable in a test binary.
      std::mem::forget(temp);
      registry
  }

  fn store() -> Arc<dyn SessionEventStore> {
      let conn = rusqlite::Connection::open_in_memory().unwrap();
      migrate_add_session_events(&conn).unwrap();
      Arc::new(SqliteEventStore::new(conn))
  }

  /// Seed a complete interrupted run: user message, a turn, a dangling tool
  /// call, then a trailing `RunStarted` with no `RunFinished`.
  async fn seed_interrupted_run(store: &Arc<dyn SessionEventStore>, sid: &SessionKey) {
      let tid = TurnId::new_v4();
      let at = now_ms();
      let events: Vec<SessionEvent> = vec![
          SessionEvent::TurnStarted {
              turn_id: tid,
              trigger: alephcore::session::events::TurnTrigger::UserMessage,
              at,
          },
          SessionEvent::UserMessage {
              turn_id: tid,
              content: alephcore::session::events::MessageContent {
                  text: "do a long task".into(),
                  blocks: vec![],
                  thinking: None,
                  thinking_signature: None,
              },
              at: at + 1,
          },
          SessionEvent::RunStarted {
              run_id: "run-1".into(),
              at: at + 2,
          },
          SessionEvent::ToolCallRequested {
              turn_id: tid,
              call_id: "dangling-1".into(),
              name: "bash_exec".into(),
              input: serde_json::json!({"cmd": "sleep 999"}),
              at: at + 3,
          },
          // <-- process dies here: no ToolResult, no RunFinished.
      ];
      for (i, ev) in events.into_iter().enumerate() {
          store
              .append(sid, (i as u64) + 1, &ev, now_ms())
              .await
              .unwrap();
      }
  }

  #[tokio::test]
  async fn interrupted_run_is_repaired_and_retriggered() {
      let store = store();
      let sid = SessionKey::main("main");
      seed_interrupted_run(&store, &sid).await;

      let adapter = Arc::new(RecordingAdapter::new());
      let calls = adapter.calls.clone();
      let registry = registry_with_agent(sid.agent_id()).await;

      let coordinator = ResumeCoordinator::new(
          store.clone(),
          ResumeConfig::default(),
          adapter as Arc<dyn ExecutionAdapter>,
          registry,
      );
      let report = coordinator.resume_interrupted_runs().await;

      assert_eq!(report.scanned, 1);
      assert_eq!(report.resumed, 1);
      assert_eq!(report.abandoned, 0);
      assert_eq!(report.skipped, 0);

      // The crash boundary was repaired: a synthetic ToolError for the
      // dangling call was appended to the log.
      let all = store.load_all_events(&sid).await.unwrap();
      let synthetic_errors: Vec<_> = all
          .iter()
          .filter_map(|r| match &r.event {
              SessionEvent::ToolError { call_id, error, .. } => {
                  Some((call_id.clone(), error.clone()))
              }
              _ => None,
          })
          .collect();
      assert_eq!(synthetic_errors.len(), 1);
      assert_eq!(synthetic_errors[0].0, "dangling-1");
      assert_eq!(synthetic_errors[0].1, "interrupted by server restart");

      // `execute` was called exactly once, carrying the resume signal.
      let calls = calls.lock().await;
      assert_eq!(calls.len(), 1);
      assert_eq!(calls[0].0, sid.to_key_string());
      assert_eq!(calls[0].1.get("resume").map(String::as_str), Some("true"));
  }

  #[tokio::test]
  async fn disabled_config_never_triggers_execute() {
      let store = store();
      let sid = SessionKey::main("main");
      seed_interrupted_run(&store, &sid).await;

      let adapter = Arc::new(RecordingAdapter::new());
      let calls = adapter.calls.clone();
      let registry = registry_with_agent(sid.agent_id()).await;

      // `resume_interrupted_runs` self-guards on `config.enabled`: even
      // when called directly it must scan nothing and trigger nothing.
      let cfg = ResumeConfig {
          enabled: false,
          ..ResumeConfig::default()
      };
      let coordinator = ResumeCoordinator::new(
          store.clone(),
          cfg,
          adapter as Arc<dyn ExecutionAdapter>,
          registry,
      );
      let report = coordinator.resume_interrupted_runs().await;

      assert_eq!(report, alephcore::gateway::ResumeReport::default());
      assert!(
          calls.lock().await.is_empty(),
          "disabled coordinator must never call execute"
      );
  }

  #[tokio::test]
  async fn crash_loop_cap_abandons_instead_of_retriggering() {
      let store = store();
      let sid = SessionKey::main("main");
      let at = now_ms();
      // 3 consecutive RunStarted with no RunFinished == default max_attempts.
      for (i, ev) in [
          SessionEvent::RunStarted { run_id: "r1".into(), at },
          SessionEvent::RunStarted { run_id: "r2".into(), at: at + 1 },
          SessionEvent::RunStarted { run_id: "r3".into(), at: at + 2 },
      ]
      .into_iter()
      .enumerate()
      {
          store
              .append(&sid, (i as u64) + 1, &ev, now_ms())
              .await
              .unwrap();
      }

      let adapter = Arc::new(RecordingAdapter::new());
      let calls = adapter.calls.clone();
      let registry = registry_with_agent(sid.agent_id()).await;

      let coordinator = ResumeCoordinator::new(
          store.clone(),
          ResumeConfig::default(),
          adapter as Arc<dyn ExecutionAdapter>,
          registry,
      );
      let report = coordinator.resume_interrupted_runs().await;

      assert_eq!(report.scanned, 1);
      assert_eq!(report.resumed, 0);
      assert_eq!(report.abandoned, 1);
      assert!(calls.lock().await.is_empty(), "capped run must not re-trigger");

      // An `Abandoned` marker was appended so the run is not re-scanned.
      let all = store.load_all_events(&sid).await.unwrap();
      let abandoned = all.iter().any(|r| {
          matches!(
              &r.event,
              SessionEvent::RunFinished {
                  outcome: RunOutcome::Abandoned,
                  ..
              }
          )
      });
      assert!(abandoned, "expected a RunFinished{{Abandoned}} marker");
  }
  ```
  > Implementation note: `AgentRegistry::register` and `AgentRegistry::new` exist (`agent_instance.rs:527+`). If `register`'s exact signature differs (e.g. it is not `async` or takes a different argument), adjust `registry_with_agent` to match — the goal is "a registry that resolves `sid.agent_id()`". Check `agent_instance.rs` around line 527-636.
- [ ] Run the integration tests (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test --test resume_coordinator_integration
  ```
  Expected: `interrupted_run_is_repaired_and_retriggered`, `disabled_config_never_triggers_execute`, `crash_loop_cap_abandons_instead_of_retriggering` all pass.
- [ ] Run the full library test suite for touched modules (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo test -p alephcore --lib session orchestrator resume_coordinator config::types::resume
  ```
  Expected: all green. Compare any failures against the known baseline (`MEMORY.md`: main has ~19-20 pre-existing `cargo test --lib` failures + 1 deadlocking concurrency test) — only NEW failures matter.
- [ ] Run clippy on the touched crate (concurrency gate, background):
  ```bash
  until [ "$(ps -A -o command | grep -E '^/[^ ]+/cargo (check|build|test|clippy)' | grep -v grep | wc -l | tr -d ' ')" -lt 3 ]; do sleep 15; done && cargo clippy -p alephcore --lib 2>&1 | grep -E 'resume_coordinator|session/(events|state|store)|flow_spec|harness_bridge|config/types/resume' || echo "no new clippy warnings in touched files"
  ```
  Expected: `no new clippy warnings in touched files` (the project baseline is not clippy-clean; only warnings in the files this plan touched matter).
- [ ] Audit pass — spec §8 scope coverage. Verify each spec item is implemented:
  - §3.1 `RunStarted`/`RunFinished`/`RunOutcome` + `state.rs` projection + `store.rs` tags → Task 1.
  - §3.2 orchestrator emits markers → Task 3.
  - §3.3 `load_run_markers` + scan + recency + cap + boundary repair → Tasks 4, 5.
  - §3.3 step 5 re-trigger + semaphore → Task 6.
  - §3.4 `FlowInput::Resume` + `seed_session` no-op + `last_user_query` `""` + `metadata["resume"]` conversion → Task 2.
  - §3.5 boot wiring + `[resume]` config → Tasks 5, 6.
  - §7 unit + integration tests → Tasks 1-7.
- [ ] Audit pass — placeholder scan: grep the new/modified files for `TODO`, `unimplemented!`, `todo!`, `unreachable!` introduced by this plan. The only intentional stub (Task 5's `retrigger`) is replaced in Task 6 — confirm no stub survives:
  ```bash
  grep -rn 'TODO\|unimplemented!\|todo!' src/gateway/resume_coordinator.rs
  ```
  Expected: no output (the Task-5 stub comment said "Task 6:" but that comment is removed when Task 6 replaces the method).
- [ ] **STOP. Do NOT merge to `main`.** Verify the branch state and report:
  ```bash
  git log --oneline main..HEAD && git status
  ```
  Expected: 7 commits (one per task), clean working tree. Report to the user: "Mid-Run Trajectory Resume implementation complete — 7 tasks, all tests green, branch ready. Awaiting explicit merge instruction." Do not run `git merge`, `git checkout main`, or any worktree-removal command.

---

## Notes for the implementer

- **`SessionEvent` has no `PartialEq`** (intentional, `events.rs:81-83`). Never write `assert_eq!(event_a, event_b)`. Compare on `serde_json::to_string(&ev)` or use `matches!`.
- **`AgentRegistry` ambiguity:** there are two — `crate::gateway::agent_instance::AgentRegistry` (gateway, what cron / heartbeat / `ResumeCoordinator` use, `get` is `async`) and `crate::agents::AgentRegistry` (orchestrator's AgentDef catalogue). This plan uses the **gateway** one throughout. Do not confuse them.
- **`RunRequest` has ~18 construction sites** — do NOT add a struct field. The resume signal rides `metadata["resume"]`. This plan adds exactly one new `RunRequest` construction (in `retrigger`, Task 6) and reads `metadata["resume"]` at exactly one site (`run_loop.rs`, Task 2).
- **Deferred (spec §9), not in scope:** user notification on abandonment; explicit manual resume tool/RPC; the session-split (Cycle 5) `perform_session_split` balanced-marker follow-up.
- **Semaphore is intentionally belt-and-suspenders.** `resume_interrupted_runs` drives `handle_interrupted` → `retrigger` sequentially, so the `max_concurrent` semaphore in `retrigger` does not currently contend (resumes are serial — which is fine: interrupted runs are typically 0–1, and serial resume cannot cause a boot storm). The semaphore is kept so a future change that makes resumes concurrent (`tokio::spawn` per candidate) is already bounded. A code-quality reviewer may note this — it is deliberate, not an oversight; do not "fix" it by deleting the semaphore.
