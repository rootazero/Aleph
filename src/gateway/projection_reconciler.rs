//! `ProjectionReconciler` — the boot driver for the transcript projection's
//! self-heal.
//!
//! P1 made `session_events` the SSOT and materialised the `messages`
//! projection asynchronously via `MessageProjector`. A hard crash between an
//! event's durable append and its drain leaves the read projection —
//! `transcript.jsonl` on the file backend, the `messages` table on SQLite —
//! short a row, and the Panel display loses it. The in-process record of which
//! seqs went missing dies with the process, so somebody has to ASK at boot.
//!
//! This is that somebody, and it is now a driver and nothing else: it picks the
//! sessions worth asking about and calls
//! [`MessageProjector::request_repair`](crate::gateway::session_projector::MessageProjector::request_repair)
//! on each. The repair itself runs inside the projector's drain task, which is
//! the single writer for a session — so a boot repair cannot interleave with a
//! live run that has already started.
//!
//! # Why the candidate set is activity, not markers
//!
//! Until 2026-09-02 this scanned run markers and repaired only the sessions
//! whose markers reduced to `Interrupted`. Two whole classes were invisible to
//! that:
//!
//! * a session whose run then finished CLEANLY — the marker slice reduced to
//!   `Clean`, the pass skipped it, and the dropped row was gone from the
//!   display permanently. The trigger condition was "the run was interrupted";
//!   the failure condition is "the projection has a gap", and the two are not
//!   the same set.
//! * sessions that emit no run markers at all — background sub-agent sessions
//!   (`sub-bg-*`), cron and heartbeat sessions. They never appeared in
//!   `load_run_markers`, so no amount of marker reduction could reach them.
//!
//! The candidate set is therefore **the activity window** (`[resume]
//! max_age_secs`, the same horizon resume uses) UNION **every session whose
//! markers read as interrupted** — the latter because a run interrupted longer
//! ago than the window is still worth repairing, and because a session can be
//! interrupted without its row's `last_active_at` having been touched since.
//!
//! Anything older than the window is left to the unbounded sweep in the
//! `core/projection-holes` doctor check. No durable projection watermark is
//! written; see `docs/superpowers/specs/2026-09-02-crash-recovery-r2-design.md`
//! A6 for why (a persisted watermark is a second statement of a fact the row
//! ids already carry).

use std::collections::HashSet;

use crate::gateway::session_projector::MessageProjector;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;
use crate::session::reduction::{reduce_disposition, RunDisposition};
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;
use crate::sync_primitives::Arc;

/// Summary of one boot pass — for the boot log and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Candidate sessions the projector was asked about.
    pub scanned: usize,
    /// Transcript rows that were absent and are now written.
    pub holes_filled: usize,
    /// `AssistantRunMeta` stamps re-applied to a row that had none.
    pub stamps_reapplied: usize,
    /// Of those stamps, how many also accumulated the run's spend.
    pub usage_rebilled: usize,
    /// Candidates that turned out to be whole.
    pub skipped_up_to_date: usize,
    /// Candidates whose transcript carries no projector seq ids (foreign /
    /// pre-SSOT content). Never touched: without seqs a hole cannot be told
    /// from a row this projector never wrote.
    pub skipped_legacy: usize,
    /// Candidates the pass could not settle — a store read failed, the repair
    /// could not be delivered, or the reducer refused the marker slice. Counted
    /// apart from every "skipped" bucket on purpose: a refusal means "I do not
    /// know", and folding it into a skip would read it as "nothing to do".
    pub errored: usize,
}

/// Boot-time driver. Constructed with the durable event store, the projection
/// target, the projector that owns the write path, and the activity horizon.
pub struct ProjectionReconciler {
    event_store: Arc<dyn SessionEventStore>,
    session_store: Arc<dyn SessionStore>,
    projector: Arc<MessageProjector>,
    /// The activity window, in seconds — `[resume] max_age_secs`. Shared with
    /// resume on purpose: "recent enough that a crashed run would still be
    /// resumed" is exactly "recent enough that a lost row still matters".
    max_age_secs: u64,
}

impl ProjectionReconciler {
    pub fn new(
        event_store: Arc<dyn SessionEventStore>,
        session_store: Arc<dyn SessionStore>,
        projector: Arc<MessageProjector>,
        max_age_secs: u64,
    ) -> Self {
        Self {
            event_store,
            session_store,
            projector,
            max_age_secs,
        }
    }

    /// Repair every boot candidate. Best-effort: any failure is counted and
    /// skipped; never panics, never blocks boot.
    pub async fn reconcile_candidates(&self) -> ReconcileReport {
        let mut report = ReconcileReport::default();
        let candidates = self.candidates(&mut report).await;

        for id in candidates {
            report.scanned += 1;
            let repair = self.projector.request_repair(&id).await;
            report.holes_filled += repair.holes_filled;
            report.stamps_reapplied += repair.stamps_reapplied;
            report.usage_rebilled += repair.usage_rebilled;
            if repair.errored {
                report.errored += 1;
            } else if repair.legacy {
                report.skipped_legacy += 1;
            } else if repair.up_to_date {
                report.skipped_up_to_date += 1;
            }
        }

        tracing::info!(
            scanned = report.scanned,
            holes_filled = report.holes_filled,
            stamps_reapplied = report.stamps_reapplied,
            usage_rebilled = report.usage_rebilled,
            skipped_up_to_date = report.skipped_up_to_date,
            skipped_legacy = report.skipped_legacy,
            errored = report.errored,
            "projection reconcile scan complete"
        );
        report
    }

    /// The activity window UNION the interrupted-marker sessions, deduplicated
    /// and in a stable order (markers first).
    async fn candidates(&self, report: &mut ReconcileReport) -> Vec<SessionId> {
        let mut seen: HashSet<SessionId> = HashSet::new();
        let mut out: Vec<SessionId> = Vec::new();

        match self.event_store.load_run_markers().await {
            Ok(groups) => {
                for (session_id, markers) in groups {
                    match reduce_disposition(&markers) {
                        Ok(RunDisposition::Interrupted { .. }) => {}
                        Ok(RunDisposition::Clean) => continue,
                        Err(c) => {
                            // A refused slice is "I cannot tell you whether this
                            // session was interrupted" — which is not "it was
                            // fine". Repairing it is idempotent, so ask anyway,
                            // and count the refusal so the boot log does not
                            // read as a clean scan.
                            tracing::warn!(
                                session = ?session_id,
                                contradiction = %c,
                                "projection reconcile: reducer refused the marker slice; \
                                 repairing anyway"
                            );
                            report.errored += 1;
                        }
                    }
                    if seen.insert(session_id.clone()) {
                        out.push(session_id);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "projection reconcile: load_run_markers failed");
                report.errored += 1;
            }
        }

        // Round UP so a sub-minute horizon still admits something: the filter
        // is minute-granular and `0` would mean "nothing is recent".
        let active_minutes = u32::try_from(self.max_age_secs.div_ceil(60).max(1)).unwrap_or(u32::MAX);
        match self
            .session_store
            .list_sessions(SessionFilter {
                active_minutes: Some(active_minutes),
                ..SessionFilter::default()
            })
            .await
        {
            Ok(sessions) => {
                for meta in sessions {
                    let Some(id) = SessionId::from_key_string(&meta.key) else {
                        // A stored key this process cannot parse is not an
                        // empty candidate list — say so rather than dropping it.
                        tracing::warn!(
                            key = %meta.key,
                            "projection reconcile: unparseable session key; skipped"
                        );
                        report.errored += 1;
                        continue;
                    };
                    if seen.insert(id.clone()) {
                        out.push(id);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "projection reconcile: list_sessions failed");
                report.errored += 1;
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::types::MessageRecord;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{
        MessageContent, RunOutcome, SessionEvent, ToolOutput, TurnId, TurnTrigger,
    };
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};

    /// A log of this test's own. Pinned into the projector rather than
    /// installed process-wide: `load_run_markers` is a CROSS-SESSION scan, so
    /// two tests sharing one log would each see the other's sessions as
    /// candidates and repair them into the wrong store.
    fn own_event_store() -> Arc<dyn SessionEventStore> {
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

    /// An assistant message billed `tin`/`tout` — the shape `think.rs` emits,
    /// one per LLM call.
    fn assistant(tid: TurnId, tin: u32, tout: u32, at: i64) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: mc("hello"),
            usage: Some(crate::orchestrator::dispatch::TokenBreakdown {
                input: tin,
                output: tout,
                ..Default::default()
            }),
            at,
        }
    }

    /// A minimal interrupted-run log: TurnStarted, UserMessage, RunStarted,
    /// AssistantMessage(tin,tout) — and NO RunFinished.
    fn interrupted_turn(tid: TurnId, tin: u32, tout: u32) -> Vec<(u64, SessionEvent)> {
        vec![
            (
                1,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: TurnTrigger::UserMessage,
                    at: 1,
                },
            ),
            (
                2,
                SessionEvent::UserMessage {
                    turn_id: tid,
                    content: mc("hi"),
                    at: 2,
                    synthetic: false,
                    author_user_id: None,
                },
            ),
            (
                3,
                SessionEvent::RunStarted {
                    run_id: "r1".into(),
                    at: 3,
                    project_root: None,
                    envelope: None,
                },
            ),
            (6, assistant(tid, tin, tout, 6)),
        ]
    }

    async fn append_all(
        store: &Arc<dyn SessionEventStore>,
        id: &SessionId,
        evs: &[(u64, SessionEvent)],
    ) {
        for (seq, ev) in evs {
            store.append(id, *seq, ev, *seq as i64).await.unwrap();
        }
    }

    fn reconciler(
        event_store: &Arc<dyn SessionEventStore>,
        session_store: &Arc<dyn SessionStore>,
    ) -> ProjectionReconciler {
        ProjectionReconciler::new(
            event_store.clone(),
            session_store.clone(),
            MessageProjector::with_event_store(
                session_store.clone(),
                None,
                Some(event_store.clone()),
            ),
            86_400,
        )
    }

    #[tokio::test]
    async fn fills_missing_tail_into_empty_transcript() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-fill");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 10, 20),
        )
        .await;

        let report = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        assert!(report.scanned >= 1);
        assert_eq!(report.holes_filled, 2, "user + assistant");
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
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-idem");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 1, 1),
        )
        .await;

        let r = reconciler(&event_store, &session_store);
        let r1 = r.reconcile_candidates().await;
        assert_eq!(r1.holes_filled, 2);
        let r2 = r.reconcile_candidates().await;
        assert_eq!(r2.holes_filled, 0, "second pass fills nothing");
        assert_eq!(
            session_store.get_history(&id, None).await.unwrap().len(),
            2,
            "no duplicate rows"
        );
    }

    /// The class the marker-driven scan could not see: the run FINISHED, so its
    /// markers reduce to `Clean` — and a row was still dropped. The old pass
    /// skipped this session and the row was lost from the display for good.
    #[tokio::test]
    async fn clean_session_with_hole_is_repaired() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-clean-hole");
        session_store.get_or_create(&id).await.unwrap();
        let mut evs = interrupted_turn(uuid::Uuid::new_v4(), 1, 1);
        evs.push((
            7,
            SessionEvent::RunFinished {
                run_id: "r1".into(),
                outcome: RunOutcome::Completed,
                at: 7,
            },
        ));
        append_all(&event_store, &id, &evs).await;
        assert!(
            matches!(
                reduce_disposition(
                    &event_store
                        .load_run_markers()
                        .await
                        .unwrap()
                        .into_iter()
                        .find(|(s, _)| *s == id)
                        .expect("markers for this session")
                        .1
                ),
                Ok(RunDisposition::Clean)
            ),
            "the premise: this session's markers read as CLEAN"
        );

        let report = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        assert_eq!(
            report.holes_filled, 2,
            "a clean session's dropped rows must still be filled: {report:?}"
        );
        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 2);
    }

    /// A background sub-agent session emits no run markers at all, so it never
    /// appeared in the marker scan. The activity window is what reaches it.
    #[tokio::test]
    async fn a_markerless_background_child_in_the_window_is_repaired() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("sub-bg-abc123");
        session_store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        // No RunStarted / RunFinished anywhere — this is the whole point.
        append_all(
            &event_store,
            &id,
            &[
                (
                    1,
                    SessionEvent::TurnStarted {
                        turn_id: tid,
                        trigger: TurnTrigger::UserMessage,
                        at: 1,
                    },
                ),
                (
                    2,
                    SessionEvent::UserMessage {
                        turn_id: tid,
                        content: mc("do the thing"),
                        at: 2,
                        synthetic: false,
                        author_user_id: None,
                    },
                ),
                (3, assistant(tid, 5, 7, 3)),
            ],
        )
        .await;
        assert!(
            !event_store
                .load_run_markers()
                .await
                .unwrap()
                .iter()
                .any(|(s, _)| *s == id),
            "the premise: this session has NO run markers"
        );

        let report = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        assert_eq!(
            report.holes_filled, 2,
            "a marker-less child session must still be repaired: {report:?}"
        );
        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[1].role, "assistant");
    }

    #[tokio::test]
    async fn legacy_transcript_without_seq_ids_is_skipped() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-legacy");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 1, 1),
        )
        .await;
        // Pre-existing legacy row with a non-seq id.
        session_store
            .append_message(
                &id,
                MessageRecord {
                    id: "legacy-row-1".into(),
                    role: "user".into(),
                    content: "old".into(),
                    timestamp: 0,
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();

        let report = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        assert_eq!(report.skipped_legacy, 1);
        assert_eq!(report.holes_filled, 0);
        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 1, "legacy transcript untouched");
        assert_eq!(hist[0].id, "legacy-row-1");
    }

    /// A two-call turn (tool round-trip in the middle) produces TWO assistant
    /// rows, each carrying its own call's tokens.
    fn two_call_turn(tid: TurnId) -> Vec<(u64, SessionEvent)> {
        vec![
            (
                1,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: TurnTrigger::UserMessage,
                    at: 1,
                },
            ),
            (
                2,
                SessionEvent::UserMessage {
                    turn_id: tid,
                    content: mc("q"),
                    at: 2,
                    synthetic: false,
                    author_user_id: None,
                },
            ),
            (
                3,
                SessionEvent::RunStarted {
                    run_id: "r1".into(),
                    at: 3,
                    project_root: None,
                    envelope: None,
                },
            ),
            (4, assistant(tid, 10, 20, 4)),
            (
                5,
                SessionEvent::ToolCallRequested {
                    turn_id: tid,
                    call_id: "c1".into(),
                    name: "bash_exec".into(),
                    input: serde_json::json!({"cmd":"ls"}),
                    at: 5,
                },
            ),
            (
                6,
                SessionEvent::ToolResult {
                    turn_id: tid,
                    call_id: "c1".into(),
                    output: ToolOutput {
                        value: serde_json::json!("ok"),
                        metadata: Default::default(),
                    },
                    at: 6,
                },
            ),
            (7, assistant(tid, 5, 7, 7)),
        ]
    }

    #[tokio::test]
    async fn each_assistant_row_carries_its_own_calls_tokens() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-tokens");
        session_store.get_or_create(&id).await.unwrap();
        append_all(&event_store, &id, &two_call_turn(uuid::Uuid::new_v4())).await;

        reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 5, "user + assistant + 2 tool rows + assistant");
        let asst: Vec<_> = hist.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(asst.len(), 2, "one row per LLM call");
        assert_eq!((asst[0].input_tokens, asst[0].output_tokens), (10, 20));
        assert_eq!((asst[1].input_tokens, asst[1].output_tokens), (5, 7));
    }

    #[tokio::test]
    async fn filled_rows_precede_later_appends() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-order");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 1, 1),
        )
        .await;

        reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        // A later append (mirrors ResumeCoordinator's re-triggered reply) must
        // land AFTER the back-filled rows.
        session_store
            .append_message(
                &id,
                MessageRecord {
                    id: format!("{}:99", id.to_key_string()),
                    role: "assistant".into(),
                    content: "fresh reply".into(),
                    timestamp: 100,
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(
            hist.first().unwrap().role,
            "user",
            "back-filled prompt is first"
        );
        assert_eq!(
            hist.last().unwrap().content,
            "fresh reply",
            "later append is last"
        );
    }

    /// A crash mid-turn: the first assistant row and both tool rows were
    /// flushed (seqs 2,4,5,6); the SECOND assistant row (seq 7) was not, and —
    /// the part a watermark cannot express — neither was seq 2's neighbour if
    /// it had been missing. The back-fill must neither lose the tokens nor
    /// duplicate the rows already there.
    #[tokio::test]
    async fn a_partially_flushed_turn_is_completed_without_duplicates() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-straddle");
        session_store.get_or_create(&id).await.unwrap();
        let key = id.to_key_string();

        append_all(&event_store, &id, &two_call_turn(uuid::Uuid::new_v4())).await;

        for (seq, role, content, tin, tout, tool_name, tool_call_id) in [
            (2u64, "user", "q", 0i64, 0i64, None, None),
            (4, "assistant", "hello", 10, 20, None, None),
            (
                5,
                "tool",
                "ls",
                0,
                0,
                Some("bash_exec".to_string()),
                Some("c1".to_string()),
            ),
            (
                6,
                "tool",
                "ok",
                0,
                0,
                Some("bash_exec".to_string()),
                Some("c1".to_string()),
            ),
        ] {
            session_store
                .append_message(
                    &id,
                    MessageRecord {
                        id: format!("{key}:{seq}"),
                        role: role.into(),
                        content: content.into(),
                        timestamp: seq as i64,
                        metadata: None,
                        input_tokens: tin,
                        output_tokens: tout,
                        tool_call_id,
                        tool_name,
                    },
                )
                .await
                .unwrap();
        }

        reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(
            hist.len(),
            5,
            "4 pre-flushed + 1 back-filled, no duplicates"
        );
        let asst: Vec<_> = hist.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(asst.len(), 2);
        assert_eq!(
            (asst[0].input_tokens, asst[0].output_tokens),
            (10, 20),
            "an already-present row must be left exactly as it was"
        );
        assert_eq!(
            (asst[1].input_tokens, asst[1].output_tokens),
            (5, 7),
            "back-filled row must arrive with its own call's tokens"
        );
    }
}
