# P2 ProjectionReconciler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A boot-time, interrupted-run-scoped reconciler that fills the file-backend `transcript.jsonl` rows the async `MessageProjector` failed to flush before a mid-run crash, so the Panel display is complete after restart.

**Architecture:** New `src/gateway/projection_reconciler.rs`. It reuses `ResumeCoordinator`'s `load_run_markers` + `classify_markers` to find interrupted sessions, derives a materialisation watermark `w = max(seq embedded in existing transcript row ids)`, loads the un-projected event tail `[w+1..head]`, and replays it through the **same** projection logic as the live drain (extracted from `MessageProjector::project_one` into a shared `project_event`, suppressing writes for already-materialised seqs). Wired at boot to run before `ResumeCoordinator`'s re-trigger.

**Tech Stack:** Rust · tokio · rusqlite (event store) · file backend JSONL · async-trait.

**Spec:** `docs/superpowers/specs/2026-07-04-projection-reconciler-p2-design.md`

## Global Constraints

- **R10:** zero `src/harness/` change — all logic in `src/gateway/` + boot wiring.
- **Scope lock:** file backend only; interrupted-run-scoped detection (not a full sweep).
- **No schema change, no new persisted field.** The watermark is derived from the seq embedded in existing projector-written row ids (`id = "{key}:{seq}"`).
- **DRY:** the live drain and the reconciler share one `project_event` — do not duplicate projection logic.
- **Best-effort boot:** the reconciler never panics and never blocks boot; per-session failures are logged and skipped.
- **Type facts:** `SessionId = crate::routing::session_key::SessionKey` (same type as `SessionStore`'s `&SessionKey` — no conversion). `EventSeq = u64`. `TurnId = uuid::Uuid`. `MessageRecord.metadata: Option<serde_json::Value>`. `project_row` produces a row for `UserMessage`/`AssistantMessage`/`SystemMessage`/`ToolCallRequested`/`ToolResult`/`ToolError`; the `AssistantMessage` row is written by a dedicated arm (token-aggregating), not via `project_row`.
- **Cargo restraint (repo rule):** never run the full test suite. Scope every run with `-p alephcore --lib` + a name filter, prefixed with `CARGO_PROFILE_TEST_DEBUG=line-tables-only` to avoid lib-test OOM. The binary-crate wiring task uses `cargo check` only.

---

### Task 1: Extract shared `project_event` (DRY refactor of `MessageProjector`)

Extract the per-event projection body from `MessageProjector::project_one` into a reusable `pub(crate) async fn project_event(...)` with an `already: Option<&HashSet<u64>>` write-suppression parameter, so the reconciler (Task 2) can replay events through identical projection semantics. The live drain calls it with `None`.

**Files:**
- Modify: `src/gateway/session_projector.rs` (drain loop ~59-64; `project_one` ~68-169)
- Test: `src/gateway/session_projector.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: existing `TurnAccum`, `project_row`, `build_message_metadata`, `RunContextOccupancy`.
- Produces (used by Task 2):
  - `pub(crate) type TurnAccums = std::collections::HashMap<(String, TurnId), TurnAccum>;`
  - `pub(crate) async fn project_event(store: &Arc<dyn SessionStore>, accums: &mut TurnAccums, id: &SessionId, rec: &SessionEventRecord, already: Option<&std::collections::HashSet<u64>>)`
  - `pub(crate) struct TurnAccum` (fields stay private; `#[derive(Default)]`).

- [ ] **Step 1: Add the write-suppression test (fails to compile — `project_event`/`TurnAccums` don't exist yet)**

Add to the existing `#[cfg(test)] mod tests` in `session_projector.rs` (the module already has `rec`, `user_msg`, `SessionManager`, `SessionId`, `tempdir` in scope):

```rust
    #[tokio::test]
    async fn project_event_suppresses_already_materialised_seq() {
        use std::collections::HashSet;
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("suppress.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("suppress");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let tid = uuid::Uuid::new_v4();
        let mut accums = TurnAccums::new();
        let already: HashSet<u64> = [1u64].into_iter().collect();

        // seq 1 is marked already-materialised → its write is suppressed.
        project_event(&store, &mut accums, &id, &rec(1, user_msg(tid)), Some(&already)).await;
        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "a materialised seq must not be re-written"
        );

        // seq 2 is unseen → written.
        project_event(&store, &mut accums, &id, &rec(2, user_msg(tid)), Some(&already)).await;
        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 1, "an unseen seq must be written");
        assert_eq!(rows[0].role, "user");
    }
```

- [ ] **Step 2: Run it to confirm it fails to compile**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::session_projector -- project_event_suppresses`
Expected: compile error — `cannot find function project_event` / `cannot find type TurnAccums`.

- [ ] **Step 3: Add `HashSet` import + make `TurnAccum` shareable + add the `TurnAccums` alias**

At the top of `session_projector.rs`, change the collections import from:

```rust
use std::collections::HashMap;
```

to:

```rust
use std::collections::{HashMap, HashSet};
```

Change the `TurnAccum` declaration from `struct TurnAccum {` to `pub(crate) struct TurnAccum {` (leave `#[derive(Default)]` and the private fields unchanged), and add the alias right after it:

```rust
/// Per-turn token/model accumulator map, keyed by `(session_key, turn_id)`.
/// Shared by the live drain and the boot-time `ProjectionReconciler`.
pub(crate) type TurnAccums = HashMap<(String, TurnId), TurnAccum>;
```

- [ ] **Step 4: Replace `project_one` with the shared `project_event`, and point the drain loop at it**

Replace the drain loop inside `MessageProjector::new`:

```rust
        tokio::spawn(async move {
            let mut accums: HashMap<(String, TurnId), TurnAccum> = HashMap::new();
            while let Some((id, rec)) = rx.recv().await {
                Self::project_one(&store, &mut accums, &id, &rec).await;
            }
        });
```

with:

```rust
        tokio::spawn(async move {
            let mut accums: TurnAccums = TurnAccums::new();
            while let Some((id, rec)) = rx.recv().await {
                project_event(&store, &mut accums, &id, &rec, None).await;
            }
        });
```

Then replace the entire `async fn project_one(...) { ... }` method (the `impl MessageProjector` block's private method, ~68-169) with this free function (place it after the `impl MessageProjector` block, at module scope):

```rust
/// Project one session event into `store` — the single source of projection
/// truth shared by the live drain (`already = None`) and the boot-time
/// `ProjectionReconciler` (`already = Some(set of materialised seqs)`).
///
/// When `already` contains `rec.seq`, a row-producing event still advances the
/// accumulator but its WRITE is suppressed, so re-projecting an
/// already-materialised event is a no-op (reconcile idempotency).
pub(crate) async fn project_event(
    store: &Arc<dyn SessionStore>,
    accums: &mut TurnAccums,
    id: &SessionId,
    rec: &SessionEventRecord,
    already: Option<&HashSet<u64>>,
) {
    let key = id.to_key_string();
    let suppress = already.is_some_and(|s| s.contains(&rec.seq));
    match &rec.event {
        SessionEvent::LlmCallStarted {
            turn_id,
            provider,
            model,
            ..
        } => {
            let a = accums.entry((key, *turn_id)).or_default();
            a.model = Some(model.clone());
            a.provider = Some(provider.clone());
        }
        SessionEvent::LlmCallEnded {
            turn_id,
            tokens_in,
            tokens_out,
            ..
        } => {
            let a = accums.entry((key, *turn_id)).or_default();
            a.tin += *tokens_in as i64;
            a.tout += *tokens_out as i64;
        }
        SessionEvent::AssistantMessage {
            turn_id, content, ..
        } => {
            // Consume the turn's accumulator regardless of suppression so
            // accumulator state advances identically to the live drain.
            let a = accums.remove(&(key.clone(), *turn_id)).unwrap_or_default();
            if suppress {
                return;
            }
            if let Err(e) = store
                .append_message(
                    id,
                    MessageRecord {
                        id: format!("{key}:{}", rec.seq),
                        role: "assistant".into(),
                        content: content.text.clone(),
                        timestamp: rec.created_at_ms,
                        metadata: None,
                        input_tokens: a.tin,
                        output_tokens: a.tout,
                        model: a.model,
                        model_provider: a.provider,
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
            {
                tracing::warn!(error = %e, "projector assistant append failed");
            }
        }
        SessionEvent::AssistantRunMeta {
            run_id,
            context_tokens,
            context_window,
            total_tokens,
            ..
        } => {
            let occupancy = crate::gateway::execution_engine::helpers::RunContextOccupancy {
                context_tokens: *context_tokens,
                context_window: *context_window,
                total_tokens: *total_tokens,
            };
            if let Some(meta) = crate::gateway::agent_instance::build_message_metadata(
                Some(run_id),
                Some(occupancy),
            ) {
                if let Err(e) = store.stamp_last_assistant_metadata(id, &meta).await {
                    tracing::warn!(error = %e, "projector: stamp run-meta failed");
                }
            }
        }
        other => {
            if let Some(row) = project_row(other) {
                if suppress {
                    return;
                }
                if let Err(e) = store
                    .append_message(
                        id,
                        MessageRecord {
                            id: format!("{key}:{}", rec.seq),
                            role: row.role,
                            content: row.text,
                            timestamp: rec.created_at_ms,
                            metadata: None,
                            input_tokens: 0,
                            output_tokens: 0,
                            model: None,
                            model_provider: None,
                            tool_call_id: row.tool_call_id,
                            tool_name: row.tool_name,
                        },
                    )
                    .await
                {
                    tracing::warn!(error = %e, "projector append failed");
                }
            }
        }
    }
}
```

Note: `project_event` is `async fn` in the module, referenced without `Self::`. Confirm the module still imports `SessionEvent`, `SessionEventRecord`, `TurnId`, `SessionId`, `MessageRecord`, `project_row`, `SessionStore`, `Arc` (all already imported at the top of the file).

- [ ] **Step 5: Run the new test + the two existing projector tests (all must pass)**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::session_projector`
Expected: PASS — `project_event_suppresses_already_materialised_seq`, `projector_materializes_events_into_store_with_tokens`, `projector_stamps_run_meta_on_assistant_row` all green (the existing two are the behavior-preserving regression gate).

- [ ] **Step 6: Confirm the touched file is rustfmt-clean, then commit**

Run: `cargo fmt -p alephcore -- src/gateway/session_projector.rs` then `git add src/gateway/session_projector.rs && git commit -m "gateway: extract shared project_event from MessageProjector"`

---

### Task 2: `ProjectionReconciler` module

Create the reconciler: `parse_source_seq` (pure), `ReconcileReport`, and `ProjectionReconciler` with `reconcile_interrupted`, using Task 1's `project_event`. Wire the module into `gateway/mod.rs`.

**Files:**
- Create: `src/gateway/projection_reconciler.rs`
- Modify: `src/gateway/mod.rs` (add `pub mod projection_reconciler;` near line 106; add `pub use projection_reconciler::{ProjectionReconciler, ReconcileReport};` near line 175)
- Test: `src/gateway/projection_reconciler.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `crate::gateway::session_projector::{project_event, TurnAccums}`; `crate::gateway::resume_coordinator::{classify_markers, ScanVerdict}`; `SessionEventStore::{load_run_markers, load_events_range}`; `SessionStore::{get_history, append_message}`.
- Produces (used by Task 3):
  - `pub struct ProjectionReconciler` with `pub fn new(event_store: Arc<dyn SessionEventStore>, session_store: Arc<dyn SessionStore>) -> Self` and `pub async fn reconcile_interrupted(&self) -> ReconcileReport`.
  - `pub struct ReconcileReport { pub scanned, pub reconciled, pub rows_filled, pub skipped_clean, pub skipped_legacy: usize }` (all `usize`, `#[derive(Debug, Default, Clone, PartialEq, Eq)]`).

- [ ] **Step 1: Add `pub mod` + re-export to `gateway/mod.rs`**

Near the other `pub mod` lines (after `pub mod resume_coordinator;` at ~106):

```rust
pub mod projection_reconciler;
```

Near the re-exports (after `pub use resume_coordinator::{ResumeCoordinator, ResumeReport};` at ~175):

```rust
pub use projection_reconciler::{ProjectionReconciler, ReconcileReport};
```

- [ ] **Step 2: Write the module with the parser, report, reconciler, and full test suite (tests fail — impl is a stub)**

Create `src/gateway/projection_reconciler.rs`. First write the non-test body but leave `reconcile_interrupted` returning `ReconcileReport::default()` (a stub) so the test file compiles and the tests fail on assertions:

```rust
//! `ProjectionReconciler` — boot-time events→messages back-fill for the file
//! backend's transcript projection.
//!
//! P1 made `session_events` the SSOT and materialised the `messages`
//! projection asynchronously via `MessageProjector`. On a hard crash *during* a
//! run, events durably in `session_events` may not have been drained to
//! `transcript.jsonl` — the Panel display loses those rows. This reconciler
//! runs at boot, finds interrupted sessions (via `ResumeCoordinator`'s run
//! markers), and re-projects the un-materialised tail through the same
//! `project_event` the live drain uses.
//!
//! Scope (see `docs/superpowers/specs/2026-07-04-projection-reconciler-p2-design.md`):
//! file backend only, interrupted runs only, no schema change — the source seq
//! is recovered from the `"{key}:{seq}"` id embedded in each projector-written
//! transcript row.

use std::collections::HashSet;

use crate::gateway::resume_coordinator::{classify_markers, ScanVerdict};
use crate::gateway::session_projector::{project_event, TurnAccums};
use crate::gateway::session_store::SessionStore;
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;
use crate::sync_primitives::Arc;

/// Summary of one `reconcile_interrupted` pass — for the boot log and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Sessions with run markers inspected.
    pub scanned: usize,
    /// Interrupted sessions that had ≥1 row filled.
    pub reconciled: usize,
    /// Total transcript rows appended.
    pub rows_filled: usize,
    /// Sessions skipped because the newest marker is `RunFinished`.
    pub skipped_clean: usize,
    /// Sessions skipped because the transcript is non-empty but carries no
    /// parseable source seq (foreign / pre-P1 rows — never touched).
    pub skipped_legacy: usize,
}

/// Parse the source event seq embedded in a projector-written row id.
///
/// Projector ids have the form `"{session_key}:{seq}"`. Returns `seq` only when
/// the prefix equals `session_key` exactly — rejecting legacy / foreign ids
/// that carry no such suffix. `session_key` may itself contain `':'`; the split
/// is on the LAST `':'`, which is the separator the projector appended.
fn parse_source_seq(id: &str, session_key: &str) -> Option<u64> {
    let (prefix, suffix) = id.rsplit_once(':')?;
    if prefix != session_key {
        return None;
    }
    suffix.parse::<u64>().ok()
}

/// Boot-time reconciler. Constructed with the durable event store and the
/// projection target (the same `SessionStore` `MessageProjector` writes to).
pub struct ProjectionReconciler {
    event_store: Arc<dyn SessionEventStore>,
    session_store: Arc<dyn SessionStore>,
}

impl ProjectionReconciler {
    pub fn new(
        event_store: Arc<dyn SessionEventStore>,
        session_store: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            event_store,
            session_store,
        }
    }

    /// Scan run markers; for each interrupted session, fill the un-materialised
    /// transcript tail from the event log. Best-effort: any failure is logged
    /// and skipped; never panics, never blocks boot.
    pub async fn reconcile_interrupted(&self) -> ReconcileReport {
        let mut report = ReconcileReport::default();

        let groups = match self.event_store.load_run_markers().await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, "projection reconcile: load_run_markers failed; skipping");
                return report;
            }
        };

        for (session_id, markers) in groups {
            report.scanned += 1;
            match classify_markers(&markers) {
                ScanVerdict::Clean => report.skipped_clean += 1,
                ScanVerdict::Interrupted { .. } => {
                    self.reconcile_session(&session_id, &mut report).await;
                }
            }
        }

        tracing::info!(
            scanned = report.scanned,
            reconciled = report.reconciled,
            rows_filled = report.rows_filled,
            skipped_clean = report.skipped_clean,
            skipped_legacy = report.skipped_legacy,
            "projection reconcile scan complete"
        );
        report
    }

    /// Reconcile one interrupted session: derive the watermark from the
    /// transcript, load the un-projected tail, replay it (writes suppressed for
    /// already-materialised seqs).
    async fn reconcile_session(&self, session_id: &SessionId, report: &mut ReconcileReport) {
        let session_key = session_id.to_key_string();

        let transcript = match self.session_store.get_history(session_id, None).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(session = ?session_id, error = %e, "projection reconcile: get_history failed; skipping");
                return;
            }
        };

        let seqs: HashSet<u64> = transcript
            .iter()
            .filter_map(|m| parse_source_seq(&m.id, &session_key))
            .collect();

        // Legacy guard: a non-empty transcript with no parseable seq is
        // foreign / pre-P1 content — never touch it (would risk duplicates).
        if !transcript.is_empty() && seqs.is_empty() {
            report.skipped_legacy += 1;
            return;
        }

        let watermark = seqs.iter().copied().max().unwrap_or(0);

        let tail = match self
            .event_store
            .load_events_range(session_id, Some(watermark + 1), None)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(session = ?session_id, error = %e, "projection reconcile: load_events_range failed; skipping");
                return;
            }
        };
        if tail.is_empty() {
            return;
        }

        let before = transcript.len();
        let mut accums = TurnAccums::new();
        for rec in &tail {
            project_event(&self.session_store, &mut accums, session_id, rec, Some(&seqs)).await;
        }
        let after = self
            .session_store
            .get_history(session_id, None)
            .await
            .map(|t| t.len())
            .unwrap_or(before);

        let filled = after.saturating_sub(before);
        if filled > 0 {
            report.rows_filled += filled;
            report.reconciled += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::types::MessageRecord;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{MessageContent, RunOutcome, SessionEvent, ToolOutput, TurnTrigger};
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};

    fn mem_event_store() -> Arc<dyn SessionEventStore> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        Arc::new(SqliteEventStore::new(conn))
    }

    fn temp_file_store() -> (Arc<dyn SessionStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (Arc::new(FileSessionStore::new(config).unwrap()), dir)
    }

    fn mc(text: &str) -> MessageContent {
        MessageContent {
            text: text.into(),
            blocks: vec![],
            thinking: None,
            thinking_signature: None,
        }
    }

    /// A minimal interrupted-run log: TurnStarted, UserMessage, RunStarted,
    /// LlmCallStarted/Ended(tin,tout), AssistantMessage — and NO RunFinished.
    fn interrupted_turn(tid: TurnId, tin: u32, tout: u32) -> Vec<(u64, SessionEvent)> {
        vec![
            (1, SessionEvent::TurnStarted { turn_id: tid, trigger: TurnTrigger::UserMessage, at: 1 }),
            (2, SessionEvent::UserMessage { turn_id: tid, content: mc("hi"), at: 2, synthetic: false }),
            (3, SessionEvent::RunStarted { run_id: "r1".into(), at: 3, project_root: None }),
            (4, SessionEvent::LlmCallStarted { turn_id: tid, provider: "anthropic".into(), model: "claude".into(), at: 4 }),
            (5, SessionEvent::LlmCallEnded { turn_id: tid, tokens_in: tin, tokens_out: tout, finish_reason: "stop".into(), at: 5 }),
            (6, SessionEvent::AssistantMessage { turn_id: tid, content: mc("hello"), at: 6 }),
        ]
    }

    async fn append_all(store: &Arc<dyn SessionEventStore>, id: &SessionId, evs: &[(u64, SessionEvent)]) {
        for (seq, ev) in evs {
            store.append(id, *seq, ev, *seq as i64).await.unwrap();
        }
    }

    #[tokio::test]
    async fn fills_missing_tail_into_empty_transcript() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-fill");
        session_store.get_or_create(&id).await.unwrap();
        append_all(&event_store, &id, &interrupted_turn(uuid::Uuid::new_v4(), 10, 20)).await;

        let report = ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        assert_eq!(report.scanned, 1);
        assert_eq!(report.reconciled, 1);
        assert_eq!(report.rows_filled, 2, "user + assistant");
        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].role, "user");
        assert_eq!(hist[0].content, "hi");
        assert_eq!(hist[1].role, "assistant");
        assert_eq!(hist[1].content, "hello");
        assert_eq!(hist[1].input_tokens, 10);
        assert_eq!(hist[1].output_tokens, 20);
    }

    #[tokio::test]
    async fn reconcile_is_idempotent() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-idem");
        session_store.get_or_create(&id).await.unwrap();
        append_all(&event_store, &id, &interrupted_turn(uuid::Uuid::new_v4(), 1, 1)).await;

        let reconciler = ProjectionReconciler::new(event_store.clone(), session_store.clone());
        let r1 = reconciler.reconcile_interrupted().await;
        assert_eq!(r1.rows_filled, 2);
        let r2 = reconciler.reconcile_interrupted().await;
        assert_eq!(r2.rows_filled, 0, "second pass fills nothing");
        assert_eq!(r2.reconciled, 0);
        assert_eq!(session_store.get_history(&id, None).await.unwrap().len(), 2, "no duplicate rows");
    }

    #[tokio::test]
    async fn clean_session_is_skipped() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-clean");
        session_store.get_or_create(&id).await.unwrap();
        let mut evs = interrupted_turn(uuid::Uuid::new_v4(), 1, 1);
        evs.push((7, SessionEvent::RunFinished { run_id: "r1".into(), outcome: RunOutcome::Completed, at: 7 }));
        append_all(&event_store, &id, &evs).await;

        let report = ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        assert_eq!(report.skipped_clean, 1);
        assert_eq!(report.reconciled, 0);
        assert_eq!(report.rows_filled, 0);
        assert!(session_store.get_history(&id, None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn legacy_transcript_without_seq_ids_is_skipped() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-legacy");
        session_store.get_or_create(&id).await.unwrap();
        append_all(&event_store, &id, &interrupted_turn(uuid::Uuid::new_v4(), 1, 1)).await;
        // Pre-existing legacy row with a non-seq id.
        session_store
            .append_message(&id, MessageRecord {
                id: "legacy-row-1".into(),
                role: "user".into(),
                content: "old".into(),
                timestamp: 0,
                metadata: None,
                input_tokens: 0,
                output_tokens: 0,
                model: None,
                model_provider: None,
                tool_call_id: None,
                tool_name: None,
            })
            .await
            .unwrap();

        let report = ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        assert_eq!(report.skipped_legacy, 1);
        assert_eq!(report.reconciled, 0);
        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 1, "legacy transcript untouched");
        assert_eq!(hist[0].id, "legacy-row-1");
    }

    #[tokio::test]
    async fn assistant_row_aggregates_multi_call_tokens() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-tokens");
        session_store.get_or_create(&id).await.unwrap();
        let tid = uuid::Uuid::new_v4();
        let evs = vec![
            (1, SessionEvent::TurnStarted { turn_id: tid, trigger: TurnTrigger::UserMessage, at: 1 }),
            (2, SessionEvent::UserMessage { turn_id: tid, content: mc("q"), at: 2, synthetic: false }),
            (3, SessionEvent::RunStarted { run_id: "r1".into(), at: 3, project_root: None }),
            (4, SessionEvent::LlmCallStarted { turn_id: tid, provider: "anthropic".into(), model: "claude".into(), at: 4 }),
            (5, SessionEvent::LlmCallEnded { turn_id: tid, tokens_in: 10, tokens_out: 20, finish_reason: "tool_use".into(), at: 5 }),
            (6, SessionEvent::ToolCallRequested { turn_id: tid, call_id: "c1".into(), name: "bash_exec".into(), input: serde_json::json!({"cmd":"ls"}), at: 6 }),
            (7, SessionEvent::ToolResult { turn_id: tid, call_id: "c1".into(), output: ToolOutput { value: serde_json::json!("ok"), metadata: Default::default() }, at: 7 }),
            (8, SessionEvent::LlmCallStarted { turn_id: tid, provider: "anthropic".into(), model: "claude".into(), at: 8 }),
            (9, SessionEvent::LlmCallEnded { turn_id: tid, tokens_in: 5, tokens_out: 7, finish_reason: "stop".into(), at: 9 }),
            (10, SessionEvent::AssistantMessage { turn_id: tid, content: mc("final"), at: 10 }),
        ];
        append_all(&event_store, &id, &evs).await;

        ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        let hist = session_store.get_history(&id, None).await.unwrap();
        // rows: user, tool(req), tool(res), assistant
        assert_eq!(hist.len(), 4, "user + 2 tool rows + assistant");
        let asst = hist.iter().find(|m| m.role == "assistant").unwrap();
        assert_eq!(asst.input_tokens, 15, "10 + 5 across both LLM calls");
        assert_eq!(asst.output_tokens, 27, "20 + 7 across both LLM calls");
    }

    #[tokio::test]
    async fn filled_rows_precede_later_appends() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-order");
        session_store.get_or_create(&id).await.unwrap();
        append_all(&event_store, &id, &interrupted_turn(uuid::Uuid::new_v4(), 1, 1)).await;

        ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        // A later append (mirrors ResumeCoordinator's re-triggered reply) must
        // land AFTER the back-filled rows.
        session_store
            .append_message(&id, MessageRecord {
                id: format!("{}:99", id.to_key_string()),
                role: "assistant".into(),
                content: "fresh reply".into(),
                timestamp: 100,
                metadata: None,
                input_tokens: 0,
                output_tokens: 0,
                model: None,
                model_provider: None,
                tool_call_id: None,
                tool_name: None,
            })
            .await
            .unwrap();

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.first().unwrap().role, "user", "back-filled prompt is first");
        assert_eq!(hist.last().unwrap().content, "fresh reply", "later append is last");
    }

    #[test]
    fn parse_source_seq_accepts_projector_ids_only() {
        assert_eq!(parse_source_seq("agent:main:reflect:42", "agent:main:reflect"), Some(42));
        assert_eq!(parse_source_seq("m-user-5", "agent:main"), None, "no colon");
        assert_eq!(parse_source_seq("other:7", "agent:main"), None, "prefix mismatch");
        assert_eq!(parse_source_seq("agent:main:xyz", "agent:main"), None, "non-numeric suffix");
    }
}
```

Then implement `reconcile_interrupted` / `reconcile_session` bodies exactly as shown above (they are NOT stubs — write the full bodies in this step; the "stub" note only means: if you scaffold incrementally, the compiler-error checkpoint is Step 3).

- [ ] **Step 3: Run tests to confirm they fail before the bodies compile / pass**

If you scaffolded `reconcile_interrupted` as `ReconcileReport::default()` first: Run `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::projection_reconciler` → Expected: assertion failures (e.g. `rows_filled` 0 ≠ 2). Otherwise skip to Step 4.

- [ ] **Step 4: Run the full reconciler test module**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p alephcore --lib gateway::projection_reconciler`
Expected: PASS — all 6 tests (`fills_missing_tail_into_empty_transcript`, `reconcile_is_idempotent`, `clean_session_is_skipped`, `legacy_transcript_without_seq_ids_is_skipped`, `assistant_row_aggregates_multi_call_tokens`, `filled_rows_precede_later_appends`, `parse_source_seq_accepts_projector_ids_only`).

- [ ] **Step 5: rustfmt the new + touched files, then commit**

Run: `cargo fmt -p alephcore -- src/gateway/projection_reconciler.rs src/gateway/mod.rs` then `git add src/gateway/projection_reconciler.rs src/gateway/mod.rs && git commit -m "gateway: add ProjectionReconciler for boot-time transcript back-fill"`

---

### Task 3: Boot wiring

Run the reconciler at boot, unconditionally, before `ResumeCoordinator`'s re-trigger, in one ordered detached task.

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (add a `session_store` clone after ~374; restructure the resume spawn block ~2178-2215)

**Interfaces:**
- Consumes: `alephcore::gateway::ProjectionReconciler` (Task 2), `session_event_store_for_resume`, the new `session_store_for_reconcile` clone, `agent_result.{execution_adapter, agent_registry}`, `resume_cfg`.

- [ ] **Step 1: Clone `session_store` for the reconciler right after it is finalised**

After the `let session_store: Arc<dyn SessionStore> = if let Some(sm) = sqlite_sm { ... } else { session_store };` block (ends ~line 374), add:

```rust
    // Keep a clone reachable at the boot-scan wiring site (~2178) for the
    // ProjectionReconciler; `session_store` itself is moved into downstream
    // subsystems below.
    let session_store_for_reconcile = session_store.clone();
```

- [ ] **Step 2: Replace the resume spawn block with an ordered reconcile→resume task**

Replace the entire block at lines 2178-2215 (from the `// Spawn the boot-scan ResumeCoordinator` comment through its closing `}`) with:

```rust
    // Boot-scan: ProjectionReconciler (display back-fill) THEN ResumeCoordinator
    // (agent re-execution), in one ordered detached task so back-filled old
    // rows are appended before re-trigger appends new ones (the file backend's
    // get_history returns append order). The reconciler runs unconditionally;
    // only re-trigger is gated by [resume] enabled. Detached — boot is NOT
    // blocked on it.
    {
        let app_cfg = app_config_for_channels.read().await;
        let resume_cfg = app_cfg.resume.clone();
        drop(app_cfg);
        if let Some(event_store) = session_event_store_for_resume.clone() {
            let reconciler = alephcore::gateway::ProjectionReconciler::new(
                event_store.clone(),
                session_store_for_reconcile.clone(),
            );
            let resume_collaborators = (
                agent_result.execution_adapter.clone(),
                agent_result.agent_registry.clone(),
            );
            tokio::spawn(async move {
                let rr = reconciler.reconcile_interrupted().await;
                tracing::info!(
                    scanned = rr.scanned,
                    reconciled = rr.reconciled,
                    rows_filled = rr.rows_filled,
                    skipped_clean = rr.skipped_clean,
                    skipped_legacy = rr.skipped_legacy,
                    "ProjectionReconciler boot scan finished"
                );
                if resume_cfg.enabled {
                    if let (Some(exec_adapter), Some(registry)) = resume_collaborators {
                        let coordinator = alephcore::gateway::ResumeCoordinator::new(
                            event_store,
                            resume_cfg,
                            exec_adapter,
                            registry,
                        );
                        let report = coordinator.resume_interrupted_runs().await;
                        tracing::info!(
                            scanned = report.scanned,
                            resumed = report.resumed,
                            abandoned = report.abandoned,
                            skipped = report.skipped,
                            "ResumeCoordinator boot scan finished"
                        );
                    }
                } else {
                    tracing::debug!("Resume coordinator: disabled ([resume] enabled = false)");
                }
            });
            if !args.daemon {
                println!("Projection reconciler + resume: boot scan spawned");
            }
        } else if !args.daemon {
            println!("Projection reconciler + resume: skipped (no session event store)");
        }
    }
```

- [ ] **Step 3: Compile-check the binary crate**

Run: `cargo check --bin aleph-server`
Expected: clean compile (no errors). This is a wiring-only change with no unit test; correctness of the boot behaviour is verified by the user's manual E2E (mid-run crash → restart → the prompt reappears in the Panel above the re-run reply).

- [ ] **Step 4: rustfmt the touched file, then commit**

Run: `cargo fmt -p aleph-server -- src/bin/aleph-server/commands/start/mod.rs` then `git add src/bin/aleph-server/commands/start/mod.rs && git commit -m "server: run ProjectionReconciler before ResumeCoordinator at boot"`

---

## Self-Review

**Spec coverage:**
- §4 architecture (module in `src/gateway/`, before resume, unconditional) → Task 2 + Task 3. ✓
- §5 interfaces (`ProjectionReconciler`, `ReconcileReport`, shared `project_event`/`TurnAccums`) → Task 1 (extract) + Task 2 (reconciler). ✓
- §6 data flow (detect → build S → legacy guard → watermark → tail → replay+suppress) → `reconcile_session` in Task 2. ✓
- §7 correctness (watermark captures prompt, idempotency, token aggregation, legacy avoidance, ordering) → Task 2 tests (`fills_*`, `reconcile_is_idempotent`, `assistant_row_aggregates_multi_call_tokens`, `legacy_*`, `filled_rows_precede_later_appends`). ✓
- §8 error handling (best-effort, warn+skip, one info line) → `reconcile_interrupted` bodies. ✓
- §9 boot wiring (ordered task, unconditional reconcile) → Task 3. ✓
- §10 testing → all six behaviours mapped to named tests. ✓
- §11 YAGNI boundary → no sweep, no SQLite, no persisted field, no backfill/ResumeCoordinator change. ✓
- §12 R10 → no `src/harness/` file touched. ✓

**Placeholder scan:** No TBD/TODO. Every code step shows full code. The "stub" mention in Task 2 Step 2/3 is an optional TDD checkpoint, with the full body given in the same step. ✓

**Type consistency:** `ProjectionReconciler::new(event_store, session_store)`, `reconcile_interrupted() -> ReconcileReport`, `parse_source_seq(&str, &str) -> Option<u64>`, `project_event(store, accums, id, rec, already: Option<&HashSet<u64>>)`, `TurnAccums` used identically across Task 1↔2↔3. `SessionId`/`SessionKey` are the same type (Global Constraints). ✓
