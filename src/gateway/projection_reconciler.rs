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
//! This is that somebody, and for the projection it is a driver and nothing
//! else (the one non-projection duty it carries is the split-epoch heal, last
//! section below): it picks the sessions worth asking about and calls
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
//!   (`sub-bg-*`). They never appear in `load_run_markers`, so no amount of
//!   marker reduction could reach them. (Cron and heartbeat sessions DO emit
//!   markers: the bridge writes `RunStarted` unconditionally,
//!   `harness_bridge/runner_impl.rs`, and `resume_coordinator::has_own_scheduler`
//!   is what closes theirs on the way out of the resume pass.)
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
//!
//! # The one thing it does besides driving: heal split epochs
//!
//! Since 2026-09-13 the scan also carries [`ProjectionReconciler::heal_split_epochs`]:
//! a compaction split writes its routing epoch AFTER its two log batches (a
//! different connection), so a crash in between leaves a forked child the
//! routing table never learned. The heal registers it at boot, before the
//! disposition loop and therefore before the resume pass that follows this
//! scan in the same task. It lives here rather than in `ResumeCoordinator`
//! because it must run even when `[resume] enabled = false` — routing being
//! wrong is not a resume concern — and this scan is the one boot pass that
//! runs unconditionally over the marker groups.

use std::collections::HashSet;

use crate::gateway::session_projector::MessageProjector;
use crate::gateway::session_store::types::SessionFilter;
use crate::gateway::session_store::SessionStore;
use crate::session::epoch_registrar::SessionEpochRegistrar;
use crate::session::events::SessionEvent;
use crate::session::reduction::{reduce_marker_slice, RunDisposition};
use crate::session::service::SessionId;
use crate::session::store::{MarkerSlice, SessionEventStore};
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
    /// Stamps synthesized for a finished run whose `AssistantRunMeta` never
    /// reached the log — an errored or cancelled run, a slash-command
    /// fast-path turn, a run the resume coordinator closed as `Abandoned`, a
    /// split parent, or a crash between `RunFinished` and the meta (the list
    /// lives on `RepairReport::stamps_synthesized`; at boot the routine
    /// shapes outnumber the crash, so this number is not a crash count): the
    /// `run_id` join alone, billed from the run's own messages. Idempotent
    /// through the stamp, like a re-applied one.
    pub stamps_synthesized: usize,
    /// Of the stamps above (re-applied or synthesized), how many also
    /// accumulated the run's spend.
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
    /// Forked children (a `SessionForked` at seq 1, in the activity window)
    /// whose epoch the routing table had never learned — the split committed
    /// its two batches and the process died before `register_epoch` — and
    /// are now registered. Logged by name at boot.
    pub epochs_healed: usize,
    /// Such children found on a deployment with no epoch registrar (the file
    /// backend). Counted, not faked as healed and not folded into `errored`:
    /// the heal was not possible here, which is a different sentence from "it
    /// failed". Logged by name at boot.
    pub epoch_heal_skipped: usize,
}

/// Boot-time driver. Constructed with the durable event store, the projection
/// target, the projector that owns the write path, the activity horizon, and
/// — where the deployment has one — the epoch registrar the split heal writes
/// through.
pub struct ProjectionReconciler {
    event_store: Arc<dyn SessionEventStore>,
    session_store: Arc<dyn SessionStore>,
    projector: Arc<MessageProjector>,
    /// The activity window, in seconds — `[resume] max_age_secs`. Shared with
    /// resume on purpose: "recent enough that a crashed run would still be
    /// resumed" is exactly "recent enough that a lost row still matters".
    max_age_secs: u64,
    /// `None` on the file backend, which has no registrar; the split heal
    /// then counts `epoch_heal_skipped` instead of registering.
    epoch_registrar: Option<Arc<dyn SessionEpochRegistrar>>,
}

impl ProjectionReconciler {
    pub fn new(
        event_store: Arc<dyn SessionEventStore>,
        session_store: Arc<dyn SessionStore>,
        projector: Arc<MessageProjector>,
        max_age_secs: u64,
        epoch_registrar: Option<Arc<dyn SessionEpochRegistrar>>,
    ) -> Self {
        Self {
            event_store,
            session_store,
            projector,
            max_age_secs,
            epoch_registrar,
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
            report.stamps_synthesized += repair.stamps_synthesized;
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
            stamps_synthesized = report.stamps_synthesized,
            usage_rebilled = report.usage_rebilled,
            skipped_up_to_date = report.skipped_up_to_date,
            skipped_legacy = report.skipped_legacy,
            errored = report.errored,
            epochs_healed = report.epochs_healed,
            epoch_heal_skipped = report.epoch_heal_skipped,
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
                // Routing first, then dispositions: a forked child the
                // routing table never learned must be registered before
                // anything reads its `Interrupted` as the run to resume —
                // the resume pass follows this scan in the same boot task.
                self.heal_split_epochs(&groups, report).await;
                for (session_id, slice) in groups {
                    match reduce_marker_slice(&slice) {
                        // A seed no run answered is a candidate too: its
                        // projection may be a hole exactly like an interrupted
                        // run's (unreachable from a marker slice today, but a
                        // wider slice must not fall through to `continue`).
                        Ok(
                            RunDisposition::Interrupted { .. } | RunDisposition::Unanswered { .. },
                        ) => {}
                        Ok(RunDisposition::Clean) => continue,
                        Err(c) => {
                            // A refused slice — the reducer's, or a marker row
                            // this build could not decode — is "I cannot tell
                            // you whether this session was interrupted", which
                            // is not "it was fine". Repairing it is idempotent,
                            // so ask anyway, and count the refusal so the boot
                            // log does not read as a clean scan.
                            tracing::warn!(
                                session = ?session_id,
                                kind = c.tag(),
                                contradiction = %c,
                                "projection reconcile: marker slice refused; repairing anyway"
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
        let active_minutes =
            u32::try_from(self.max_age_secs.div_ceil(60).max(1)).unwrap_or(u32::MAX);
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

    /// Register every forked child the routing table never learned.
    ///
    /// A compaction split commits the parent's closer, then the child's seed
    /// (`SessionForked` at seq 1 … `RunStarted` last), and only THEN writes
    /// the child's epoch to the routing table — a different connection, so it
    /// cannot ride either transaction (`session_split.rs`, steps 3–5). A crash
    /// between the child batch and that write leaves the log saying "split"
    /// while `get_current_epoch` still answers the parent's epoch: inbound
    /// messages land on a session whose run is closed, and the child's open
    /// run is the one resume should pick up. This is the other half of that
    /// contract: the log leads, routing follows at the next boot.
    ///
    /// Detection reads the marker groups because the child's seed always ends
    /// in a `RunStarted` marker, so every forked child is in
    /// `load_run_markers`; `SessionForked` itself is not a marker and is read
    /// with one targeted range read of seq 1. A child whose seq 1 has since
    /// been retired reads as "not `SessionForked`" — unhealable, counted in
    /// `errored` and named in the warn — rather than guessed at. Only sessions
    /// inside the activity window are considered: the same `max_age_secs` the
    /// resume pass uses, but dated differently — here by the LAST marker's
    /// `created_at_ms`, `ResumeAttempted` stamps included, where resume dates a
    /// run by `last_alive_at` (its opening marker or in-run activity, stamps
    /// excluded). A split whose only recent marker is a resume stamp is
    /// therefore in scope here and out of scope there; sharing the predicate
    /// would need the full `RunReduction` this loop does not build.
    ///
    /// Runs BEFORE the disposition loop so the resume pass (which follows this
    /// scan in the same boot task) sees routing and log agree.
    async fn heal_split_epochs(
        &self,
        groups: &[(SessionId, MarkerSlice)],
        report: &mut ReconcileReport,
    ) {
        let horizon = crate::session::events::now_ms().saturating_sub(
            i64::try_from(self.max_age_secs.saturating_mul(1000)).unwrap_or(i64::MAX),
        );
        for (id, slice) in groups {
            // A slice this build could not decode is not "no markers": no
            // heal is derived from it. The caller's disposition loop, which
            // walks these same groups next, is where it is counted and named.
            let Ok(markers) = slice else {
                continue;
            };
            // Epoch 0 is never a fork; a fork whose last marker is older than
            // the window is out of scope, as it is for resume.
            if id.epoch() == 0 || markers.last().is_none_or(|m| m.created_at_ms < horizon) {
                continue;
            }
            let current = match self
                .session_store
                .get_current_epoch(&id.base_key_pattern())
                .await
            {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        session = ?id,
                        error = %e,
                        "epoch heal: get_current_epoch failed"
                    );
                    report.errored += 1;
                    continue;
                }
            };
            if current >= id.epoch() {
                continue;
            }
            // Routing is behind the log. The one shape this heal knows is a
            // split's child: seq 1 is `SessionForked`. Anything else at a
            // higher epoch than routing is "I cannot tell you what this is",
            // which is an error, not a skip.
            let head = match self
                .event_store
                .load_events_range(id, Some(1), Some(2))
                .await
            {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(session = ?id, error = %e, "epoch heal: could not read seq 1");
                    report.errored += 1;
                    continue;
                }
            };
            let Some(SessionEvent::SessionForked {
                parent_session_id, ..
            }) = head.first().map(|r| &r.event)
            else {
                tracing::warn!(
                    session = ?id,
                    current,
                    "epoch heal: log at a higher epoch than routing, but seq 1 is not \
                     SessionForked; left alone"
                );
                report.errored += 1;
                continue;
            };
            // The file backend has no registrar: count that the heal could
            // not run here rather than pretend it did or fold it into errors.
            // Says nothing about where routing ends up — on the file backend
            // `get_current_epoch` reads the directory tree, and the projection
            // repair later in this same scan may create the child's directory
            // (pinned in `without_a_registrar_the_heal_is_counted_not_faked`).
            let Some(registrar) = &self.epoch_registrar else {
                tracing::warn!(
                    session = ?id,
                    current,
                    "epoch heal: forked child unregistered, and this deployment has no \
                     epoch registrar to register it through"
                );
                report.epoch_heal_skipped += 1;
                continue;
            };
            // This is the boot-side replay of the split's step 5
            // (`session_split.rs`), so it does BOTH halves of that step, the
            // same way and in the same order: register, then retire what
            // belonged to the superseded epoch.
            //
            // Register under the PARENT's persisted attribution. The in-process
            // split registers inside the harness task, where the scope
            // task-local is live and `get_or_create` stamps the child row from
            // it; a boot task has no such scope, so without this the two
            // producers of the same row disagree — and the resume pass that
            // follows reads the child's own columns to scope the resumed run
            // (`resume_coordinator::resume_metadata`), so an unstamped child
            // would resume UNSCOPED and write its memory to the base partition.
            // A legacy (pre-P1) parent has no attribution and stamps nothing,
            // the same carve-out resume takes.
            let parent = self.forked_parent(id, parent_session_id);
            let inherited = match &parent {
                Some(parent) => self.persisted_attribution(id, parent).await,
                None => None,
            };
            match crate::scope::with_scope(inherited, registrar.register_epoch(id)).await {
                Ok(()) => {
                    tracing::info!(
                        session = ?id,
                        from = current,
                        "epoch heal: registered a forked child the routing table never learned"
                    );
                    report.epochs_healed += 1;
                    // Routing now resolves past the parent's epoch, so the
                    // `/btw` side session keyed to it is no longer derivable
                    // by any surface — retire it, exactly as the in-process
                    // split does after ITS registration. Only after a
                    // successful one: a failed registration leaves the
                    // parent live, and its side session with it.
                    if let Some(parent) = &parent {
                        registrar.retire_superseded(parent).await;
                    }
                }
                Err(e) => {
                    tracing::warn!(session = ?id, error = %e, "epoch heal: register_epoch failed");
                    report.errored += 1;
                }
            }
        }
    }

    /// The parent a child's `SessionForked` names, parsed. `None` — logged —
    /// when the string does not parse as a session key: the child is still
    /// registered (the log says it exists), but nothing can be inherited from
    /// or retired on a parent this process cannot name.
    fn forked_parent(&self, child: &SessionId, parent_key: &str) -> Option<SessionId> {
        let parsed = SessionId::from_key_string(parent_key);
        if parsed.is_none() {
            tracing::warn!(
                session = ?child,
                parent = %parent_key,
                "epoch heal: SessionForked names an unparseable parent; child registered \
                 without attribution and nothing retired"
            );
        }
        parsed
    }

    /// The owner/scope the parent's session row carries, for the child to
    /// inherit — `None` when the row cannot be read or is a legacy row with no
    /// attribution (both columns are required; same fail-closed derivation as
    /// `ScopeAttribution::from_persisted` everywhere else). Logged when the
    /// parent should have been there and was not, so a child that ends up
    /// unattributed is a child somebody can find in the boot log.
    async fn persisted_attribution(
        &self,
        child: &SessionId,
        parent: &SessionId,
    ) -> Option<crate::scope::ScopeAttribution> {
        match self.session_store.get_metadata(parent).await {
            Ok(Some(meta)) => crate::scope::ScopeAttribution::from_persisted(
                meta.owner_user_id.as_deref(),
                meta.scope_id.as_deref(),
            ),
            Ok(None) => {
                tracing::warn!(
                    session = ?child,
                    parent = ?parent,
                    "epoch heal: parent row not found; child registered without attribution"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    session = ?child,
                    parent = ?parent,
                    error = %e,
                    "epoch heal: parent row unreadable; child registered without attribution"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
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

    /// An assistant message whose provider reported no usage — absent, not
    /// zero.
    fn assistant_unpriced(tid: TurnId, at: i64) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: mc("hello"),
            usage: None,
            at,
        }
    }

    /// A finished run — `RunStarted`, one turn, the given assistant message,
    /// `RunFinished` — and NO `AssistantRunMeta`: the log a crash between the
    /// closer and the meta leaves behind.
    fn finished_run_without_meta(
        tid: TurnId,
        assistant_message: SessionEvent,
    ) -> Vec<(u64, SessionEvent)> {
        vec![
            (
                1,
                SessionEvent::RunStarted {
                    run_id: "r1".into(),
                    at: 1,
                    project_root: None,
                    envelope: None,
                },
            ),
            (
                2,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: TurnTrigger::UserMessage,
                    at: 2,
                },
            ),
            (3, assistant_message),
            (
                4,
                SessionEvent::RunFinished {
                    run_id: "r1".into(),
                    outcome: RunOutcome::Completed,
                    at: 4,
                },
            ),
        ]
    }

    /// The SQLite backend as the projection target — the one whose
    /// `stamp_assistant_metadata_in_range` is a `source_seq`-ranged query.
    fn temp_sqlite_store(name: &str) -> (Arc<dyn SessionStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: dir.path().join(name),
            ..Default::default()
        })
        .unwrap();
        (Arc::new(manager), dir)
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

    /// `append_all` dated at `now`. The projected row carries the record's
    /// `created_at_ms`, and `sessions.last_active_at` follows the row — so a
    /// log dated at `seq` (1970) drops its session out of the activity window
    /// the moment the first pass writes a row, and a second
    /// `reconcile_candidates` scans nothing. A test whose second pass must
    /// reach the projector dates its log here.
    async fn append_all_now(
        store: &Arc<dyn SessionEventStore>,
        id: &SessionId,
        evs: &[(u64, SessionEvent)],
    ) {
        let now = crate::session::events::now_ms();
        for (seq, ev) in evs {
            store.append(id, *seq, ev, now).await.unwrap();
        }
    }

    /// A reconciler with NO epoch registrar — the file backend's shape. The
    /// heal tests below build their own so they can hand in the SQLite
    /// `SessionManager` as both store and registrar.
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
            None,
        )
    }

    /// The child log a split leaves behind when the process dies between the
    /// child batch and `register_epoch`: seq 1 `SessionForked`, the summary,
    /// and the open `RunStarted` — and NO row for it in the routing table.
    async fn seed_unregistered_fork(
        event_store: &Arc<dyn SessionEventStore>,
        parent: &SessionKey,
    ) -> SessionKey {
        let child = parent.with_next_epoch();
        let now = crate::session::events::now_ms();
        for (seq, ev) in [
            (
                1,
                SessionEvent::SessionForked {
                    parent_session_id: parent.to_key_string(),
                    at: now,
                },
            ),
            (
                2,
                SessionEvent::SystemMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: "[Context Summary]".into(),
                    at: now,
                },
            ),
            (
                3,
                SessionEvent::RunStarted {
                    run_id: "split".into(),
                    at: now,
                    project_root: None,
                    envelope: None,
                },
            ),
        ] {
            event_store.append(&child, seq, &ev, now).await.unwrap();
        }
        child
    }

    /// Torn state (ii) of the split: both batches committed, the epoch never
    /// registered. Routing still says "epoch 0" while the log says "split".
    /// The boot heal registers the child BEFORE the resume pass, so resume
    /// finds one interrupted session (the child) and routing agrees with it.
    #[tokio::test]
    async fn a_forked_child_the_routing_table_never_learned_is_registered_at_boot() {
        let event_store = own_event_store();
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("heal.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let parent = SessionKey::Main {
            agent_id: "a".into(),
            main_key: "k".into(),
            epoch: 0,
        };
        manager.get_or_create(&parent).await.unwrap();
        let child = seed_unregistered_fork(&event_store, &parent).await;
        assert_eq!(child.epoch(), 1);

        let session_store: Arc<dyn SessionStore> = manager.clone();
        assert_eq!(
            session_store
                .get_current_epoch(&parent.base_key_pattern())
                .await
                .unwrap(),
            0,
            "precondition: routing still says epoch 0"
        );

        let reconciler = ProjectionReconciler::new(
            event_store.clone(),
            session_store.clone(),
            MessageProjector::with_event_store(
                session_store.clone(),
                None,
                Some(event_store.clone()),
            ),
            86_400,
            Some(manager.clone() as Arc<dyn SessionEpochRegistrar>),
        );

        let report = reconciler.reconcile_candidates().await;
        assert_eq!(report.epochs_healed, 1, "{report:?}");
        assert_eq!(report.epoch_heal_skipped, 0, "{report:?}");
        assert_eq!(
            session_store
                .get_current_epoch(&parent.base_key_pattern())
                .await
                .unwrap(),
            1,
            "routing now resolves to the child"
        );

        // Idempotent: a second boot has nothing to heal and does not pretend
        // otherwise.
        let again = reconciler.reconcile_candidates().await;
        assert_eq!(again.epochs_healed, 0, "idempotent: {again:?}");
        assert_eq!(again.epoch_heal_skipped, 0, "{again:?}");
    }

    /// The same torn split, plus a marker row of the child's that this build
    /// cannot decode. The child's slice is then `Err`, and `Err` is "I cannot
    /// tell you what this session's markers say" — so no epoch is healed from
    /// it (a heal derived from an unreadable slice would register a routing
    /// change nobody can vouch for), it is NOT read as a session with no
    /// markers (which would fall silently out of the scan), and the boot
    /// report counts it under `errored` by name.
    #[tokio::test]
    async fn a_child_whose_marker_slice_did_not_decode_gets_no_epoch_heal_and_is_counted() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let concrete = Arc::new(SqliteEventStore::new(conn));
        let event_store: Arc<dyn SessionEventStore> = concrete.clone();
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("heal-undecodable.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let parent = SessionKey::Main {
            agent_id: "a".into(),
            main_key: "k".into(),
            epoch: 0,
        };
        manager.get_or_create(&parent).await.unwrap();
        let child = seed_unregistered_fork(&event_store, &parent).await;
        concrete
            .insert_raw_row_for_test(
                &child,
                4,
                "run_finished",
                r#"{"type":"run_finished","run_id":"split","outcome":"from_the_future","at":1}"#,
            )
            .await;
        let session_store: Arc<dyn SessionStore> = manager.clone();

        let reconciler = ProjectionReconciler::new(
            event_store.clone(),
            session_store.clone(),
            MessageProjector::with_event_store(
                session_store.clone(),
                None,
                Some(event_store.clone()),
            ),
            86_400,
            Some(manager.clone() as Arc<dyn SessionEpochRegistrar>),
        );

        let report = reconciler.reconcile_candidates().await;
        assert_eq!(
            (report.epochs_healed, report.epoch_heal_skipped),
            (0, 0),
            "no heal is derived from a slice this build could not read: {report:?}"
        );
        assert!(report.errored >= 1, "counted, not dropped: {report:?}");
        assert_eq!(
            session_store
                .get_current_epoch(&parent.base_key_pattern())
                .await
                .unwrap(),
            0,
            "routing was left exactly as it was"
        );
    }

    /// The heal is the boot-side replay of the split's step 5, so it must do
    /// both halves the in-process split does after a registration:
    ///
    /// * the healed child's row carries the PARENT's owner/scope — the
    ///   in-process split registers inside the harness task where the scope
    ///   task-local is live; the boot task has none, and the resume pass that
    ///   follows scopes the resumed run from the child's own columns, so an
    ///   unattributed child resumes unscoped;
    /// * the parent's `/btw` side session is retired — routing now resolves
    ///   past the parent's epoch, so that side session is unaddressable and
    ///   nothing else would ever delete it.
    #[tokio::test]
    async fn the_healed_child_inherits_attribution_and_the_parents_side_session_is_retired() {
        use crate::scope::{with_scope, ScopeAttribution};

        let event_store = own_event_store();
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("heal-attr.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let parent = SessionKey::Main {
            agent_id: "a".into(),
            main_key: "owned".into(),
            epoch: 0,
        };
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            manager.get_or_create(&parent),
        )
        .await
        .unwrap();
        let parent_meta = manager.get_metadata(&parent).await.unwrap().unwrap();
        assert_eq!(
            parent_meta.owner_user_id.as_deref(),
            Some("u-alice"),
            "precondition: the parent row is attributed"
        );
        // The parent asked a `/btw` side question at some point: its side
        // session row exists and is keyed to the parent's exact epoch.
        let side = crate::gateway::btw::side_session_of(&parent).expect("a main key has a side");
        manager.get_or_create(&side).await.unwrap();
        assert!(
            manager.get_metadata(&side).await.unwrap().is_some(),
            "precondition: the side session row exists"
        );
        let child = seed_unregistered_fork(&event_store, &parent).await;

        let session_store: Arc<dyn SessionStore> = manager.clone();
        let reconciler = ProjectionReconciler::new(
            event_store.clone(),
            session_store.clone(),
            MessageProjector::with_event_store(
                session_store.clone(),
                None,
                Some(event_store.clone()),
            ),
            86_400,
            Some(manager.clone() as Arc<dyn SessionEpochRegistrar>),
        );
        let report = reconciler.reconcile_candidates().await;
        assert_eq!(report.epochs_healed, 1, "{report:?}");

        let child_meta = manager
            .get_metadata(&child)
            .await
            .unwrap()
            .expect("the heal created the child row");
        assert_eq!(
            child_meta.owner_user_id, parent_meta.owner_user_id,
            "the child is owned by whoever owned the parent"
        );
        assert_eq!(
            child_meta.scope_id, parent_meta.scope_id,
            "the child lives in the parent's scope"
        );
        assert!(
            child_meta.owner_user_id.is_some(),
            "and that is a real attribution, not two Nones agreeing"
        );
        assert!(
            manager.get_metadata(&side).await.unwrap().is_none(),
            "the parent's side session was retired with the registration, as the \
             in-process split retires it"
        );
        assert!(
            manager.get_metadata(&parent).await.unwrap().is_some(),
            "the parent itself is NOT deleted — retirement is the side session only"
        );
    }

    /// The same torn log on the FILE backend, where no registrar exists. The
    /// heal must say it could not run — `epoch_heal_skipped` — rather than
    /// count a registration that never happened or stay silent.
    #[tokio::test]
    async fn without_a_registrar_the_heal_is_counted_not_faked() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_file_store();
        let parent = SessionKey::Main {
            agent_id: "a".into(),
            main_key: "k".into(),
            epoch: 0,
        };
        session_store.get_or_create(&parent).await.unwrap();
        let child = seed_unregistered_fork(&event_store, &parent).await;
        assert_eq!(
            session_store
                .get_current_epoch(&parent.base_key_pattern())
                .await
                .unwrap(),
            0,
            "precondition: routing still says epoch 0"
        );

        let report = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;

        assert_eq!(report.epoch_heal_skipped, 1, "{report:?}");
        assert_eq!(report.epochs_healed, 0, "{report:?}");
        // Observed, not contracted — and named so nobody reads the counter
        // above as "routing is still wrong": on the file backend
        // `get_current_epoch` reads the directory tree, and the projection
        // repair in the SAME scan created the child's directory while
        // back-filling its summary row. Routing resolves to the child here by
        // that side effect of the other boot pass, not by this heal, which
        // truthfully reports it could not run.
        assert_eq!(
            session_store
                .get_current_epoch(&parent.base_key_pattern())
                .await
                .unwrap(),
            1,
            "file backend: the projection repair's directory for {child:?} is what \
             routing reads; the heal itself registered nothing"
        );
    }

    /// A child whose epoch routing already knows is not "healed" again, and a
    /// higher-epoch session whose seq 1 is NOT a fork marker is left alone and
    /// counted as an error — the heal only knows one shape and says so.
    #[tokio::test]
    async fn the_heal_touches_only_an_unregistered_fork() {
        let event_store = own_event_store();
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("heal2.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let session_store: Arc<dyn SessionStore> = manager.clone();

        // (a) A registered fork: routing already at epoch 1.
        let known = SessionKey::Main {
            agent_id: "a".into(),
            main_key: "known".into(),
            epoch: 0,
        };
        manager.get_or_create(&known).await.unwrap();
        let known_child = seed_unregistered_fork(&event_store, &known).await;
        manager.get_or_create(&known_child).await.unwrap();

        // (b) Epoch 2 in the log, routing at 0, but seq 1 is a plain run —
        // not the shape a split leaves. Stamped NOW so it is inside the
        // activity window (`append_all` stamps `created_at_ms = seq`, which
        // is 1970 and would make this session invisible to the heal).
        let odd = SessionKey::Main {
            agent_id: "a".into(),
            main_key: "odd".into(),
            epoch: 0,
        };
        manager.get_or_create(&odd).await.unwrap();
        let odd_child = odd.with_epoch(2);
        let now = crate::session::events::now_ms();
        for (seq, ev) in interrupted_turn(uuid::Uuid::new_v4(), 1, 1) {
            event_store.append(&odd_child, seq, &ev, now).await.unwrap();
        }

        let reconciler = ProjectionReconciler::new(
            event_store.clone(),
            session_store.clone(),
            MessageProjector::with_event_store(
                session_store.clone(),
                None,
                Some(event_store.clone()),
            ),
            86_400,
            Some(manager.clone() as Arc<dyn SessionEpochRegistrar>),
        );
        let report = reconciler.reconcile_candidates().await;

        assert_eq!(report.epochs_healed, 0, "{report:?}");
        assert_eq!(report.epoch_heal_skipped, 0, "{report:?}");
        assert!(
            report.errored >= 1,
            "the not-a-fork mismatch is an error, not a skip: {report:?}"
        );
        assert_eq!(
            session_store
                .get_current_epoch(&odd.base_key_pattern())
                .await
                .unwrap(),
            0,
            "a log the heal does not recognise is left alone"
        );
        assert_eq!(
            session_store
                .get_current_epoch(&known.base_key_pattern())
                .await
                .unwrap(),
            1
        );
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
                reduce_marker_slice(
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
        // This fixture is also a finished run with no meta, on the FILE
        // backend — the cheapest pin that the synthesized stamp lands there
        // too (the dedicated synthesized-stamp tests in this module are all
        // SQLite).
        assert_eq!(
            report.stamps_synthesized, 1,
            "a finished run with a row and no meta is stamped: {report:?}"
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

        let report = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;
        assert_eq!(
            report.stamps_synthesized, 0,
            "the run is still open (no RunFinished): its meta may still come"
        );

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 5, "user + assistant + 2 tool rows + assistant");
        let asst: Vec<_> = hist.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(asst.len(), 2, "one row per LLM call");
        assert_eq!((asst[0].input_tokens, asst[0].output_tokens), (10, 20));
        assert_eq!((asst[1].input_tokens, asst[1].output_tokens), (5, 7));
    }

    /// #11: the run finished, the process died before `AssistantRunMeta` — the
    /// session's counters under-counted forever. Boot folds the run's own messages.
    #[tokio::test]
    async fn a_finished_run_with_no_meta_is_billed_from_its_messages_at_boot() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_sqlite_store("nometa.db");
        let id = SessionKey::ephemeral("nometa");
        session_store.get_or_create(&id).await.unwrap();
        let tid = uuid::Uuid::new_v4();
        append_all_now(
            &event_store,
            &id,
            &finished_run_without_meta(tid, assistant(tid, 300, 40, 3)),
        )
        .await;
        let r = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;
        assert_eq!((r.stamps_synthesized, r.usage_rebilled), (1, 1), "{r:?}");
        let meta = session_store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (300, 40));
        let row = session_store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .unwrap();
        let stamp = row.metadata.unwrap();
        assert_eq!(
            stamp.get("run_id").and_then(|v| v.as_str()),
            Some("r1"),
            "the run_id join is what the crash cost the row"
        );
        assert_eq!(
            stamp.as_object().map(serde_json::Map::len),
            Some(1),
            "the run_id alone: the gauge is unknown and must not be written as zeros"
        );
        let again = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;
        // The second pass must have ASKED: with no candidate, the counters
        // below are zero for the wrong reason and prove nothing.
        assert_eq!(again.scanned, 1, "{again:?}");
        assert_eq!(
            (again.stamps_synthesized, again.usage_rebilled),
            (0, 0),
            "the stamp is the idempotence guard"
        );
        assert_eq!(
            session_store
                .get_metadata(&id)
                .await
                .unwrap()
                .unwrap()
                .input_tokens,
            300
        );
    }

    /// The same crash on a run whose provider reported no usage: the row's
    /// `run_id` join is still owed, but there is nothing to bill — `usage: None`
    /// is absent, not zero, and the fold gives `input == output == 0`.
    #[tokio::test]
    async fn a_finished_run_with_no_usage_is_stamped_but_not_billed() {
        let event_store = own_event_store();
        let (session_store, _dir) = temp_sqlite_store("nousage.db");
        let id = SessionKey::ephemeral("nousage");
        session_store.get_or_create(&id).await.unwrap();
        let tid = uuid::Uuid::new_v4();
        append_all_now(
            &event_store,
            &id,
            &finished_run_without_meta(tid, assistant_unpriced(tid, 3)),
        )
        .await;
        let r = reconciler(&event_store, &session_store)
            .reconcile_candidates()
            .await;
        assert_eq!(
            (r.stamps_synthesized, r.usage_rebilled, r.errored),
            (1, 0, 0),
            "stamped, not billed, not an error: {r:?}"
        );
        let meta = session_store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (0, 0));
        let row = session_store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .unwrap();
        assert_eq!(
            row.metadata
                .as_ref()
                .and_then(|m| m.get("run_id"))
                .and_then(|v| v.as_str()),
            Some("r1")
        );
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
