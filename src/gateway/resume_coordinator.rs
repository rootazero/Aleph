//! `ResumeCoordinator` — boot-scan auto-resume of interrupted agent runs.
//!
//! Cycle 6 of the long-task hardening directive. See
//! `docs/superpowers/specs/2026-05-21-mid-run-trajectory-resume-design.md`.
//!
//! A run is **interrupted** iff its run-marker sequence (every `RunStarted`
//! and `RunFinished` in the log, in `seq` order — see
//! `session::reduction::reduce_disposition`) ends with one or more
//! `RunStarted` events and no `RunFinished` after the last one. This module
//! scans for that shape, repairs the crash boundary (synthetic `ToolError`
//! for each dangling tool call), and re-triggers each surviving candidate.
//!
//! R10-safe: `src/harness/` is untouched. The harness already replays the
//! event log on every `run()`; resume only re-triggers it.

use crate::sync_primitives::Arc;

use std::collections::HashMap;

use tokio::sync::Semaphore;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::config::types::ResumeConfig;
use crate::gateway::agent_instance::{AgentInstance, AgentRegistry};
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{RunRequest, UNATTENDED_KEY};
use crate::session::events::{now_ms, RunOutcome, SessionEvent, SessionEventRecord};
use crate::session::reduction::{
    reduce_disposition, reduce_run, LogContradiction, RunDisposition,
};
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;

/// `FailsClosed`: `handlers/resume.rs` turns a missing handle into
/// `ResumeOutcome::Unavailable`. Nothing resumes and nothing is harmed — but
/// the setter's own doc below already names the cost in this round's exact
/// words: *"the only symptom would be a rejection, which is indistinguishable
/// from the feature not existing"*. That sentence was written about installing
/// the handle under too narrow a condition; it is equally true of not
/// installing it at all, and until now there was no way to ask which happened.
static GLOBAL_RESUME_COORDINATOR: CapabilitySlot<Arc<ResumeCoordinator>> =
    CapabilitySlot::new("gateway/resume-coordinator", MissingSemantics::FailsClosed);

/// Publish the process-wide coordinator so on-demand resume
/// ([`ResumeCoordinator::resume_session`]) can reach the same instance the boot
/// scan used — same config, same collaborators, same concurrency permit pool.
///
/// **Register this outside any `[resume] enabled` branch.** `enabled` gates the
/// automatic scan, not the explicit request; installing the handle under the
/// narrower condition would make `agent.resume` return "unavailable" on exactly
/// the deployments whose operators turned auto-resume off and therefore need the
/// manual verb most — and the only symptom would be a rejection, which is
/// indistinguishable from the feature not existing.
///
/// Idempotent: a second call is ignored (mirrors
/// [`crate::session::service::set_global_session_service`]).
pub fn set_global_resume_coordinator(coordinator: Arc<ResumeCoordinator>) {
    let _ = GLOBAL_RESUME_COORDINATOR.install(coordinator);
}

/// Record that boot reached this slot and had nothing to install.
///
/// The doc above explains why the handle must not be installed under the
/// narrower `[resume] enabled` condition. This is the other half of the same
/// argument: when the *wider* condition is also unmet, say so, because the only
/// other symptom is an `agent.resume` rejection that reads exactly like the
/// feature not existing. `because` is quoted verbatim to an operator.
pub fn decline_global_resume_coordinator(because: &'static str) {
    GLOBAL_RESUME_COORDINATOR.decline(because);
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_resume_coordinator_slot() -> &'static dyn SlotStatus {
    &GLOBAL_RESUME_COORDINATOR
}

/// The process-wide coordinator, if one has been installed. `None` in tests and
/// in any boot path that has no execution adapter to re-trigger runs with.
#[must_use]
pub fn global_resume_coordinator() -> Option<Arc<ResumeCoordinator>> {
    GLOBAL_RESUME_COORDINATOR.get().cloned()
}

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
    /// Interrupted sessions handed back to the scheduler that owns them
    /// (team dispatcher / cron / heartbeat) instead of being resumed here.
    /// Their dangling marker is closed on the way out — see
    /// [`has_own_scheduler`].
    pub delegated: usize,
    /// Sessions left alone because a resume for them was already in flight.
    ///
    /// Always 0 for the boot scan, which walks sessions sequentially. It exists
    /// for the on-demand face: two `agent.resume` calls for one session — or one
    /// racing the boot scan, which is spawned while the gateway is already
    /// serving — must not both run `repair_boundary`, because that is a
    /// read-then-append and two winners append **two** synthetic `ToolError`s
    /// for the same `call_id`. Since `harness::agent::prompt` learned to
    /// downgrade an orphaned/duplicate `tool_result` to a plain user note
    /// (7929bbda6) that is no longer a provider rejection — it is text noise:
    /// the model reads the same "outcome unknown" sentence twice, the second
    /// time as prose that no longer references the call it answers.
    pub busy: usize,
    /// Candidates this pass would not act on, and why. One entry per refusal,
    /// carrying the session so a multi-session boot report names which.
    ///
    /// Not a counter: "something was refused" and "this session's log
    /// contradicts itself at seq 41" are different answers, and the caller
    /// (`status_of`, the CLI receipt, the doctor) needs the second.
    pub refused: Vec<(SessionId, ResumeRefusal)>,
    /// Interrupted candidates left alone because their log's recency is
    /// unknown ([`LogContradiction::ClockAnomaly`]).
    ///
    /// Deliberately neither `resumed` nor `abandoned`: both are decisions
    /// taken on an age, and the age is exactly what this log does not support.
    pub skipped_unknown_age: usize,
    /// REPORT-kind contradictions seen across every candidate this pass
    /// reduced. A magnitude for the boot line — the kinds themselves are named
    /// per session by the `core/session-log` doctor check.
    pub contradictions: usize,
    /// Resumed runs that had to give something up on the way back: a model the
    /// catalog has retired since the crash, a `project_root` that no longer
    /// exists. The model is told in-band by the boundary repair; this is the
    /// operator's count of the same fact.
    ///
    /// The producer arrives with the ④ settings envelope; until a `RunStarted`
    /// carries one there is nothing that can degrade, and this reads 0 for the
    /// honest reason rather than because nobody looks.
    pub degraded: usize,
    /// Resumed runs whose `RunStarted` carried no settings envelope, so the
    /// re-triggered run follows today's session and global values instead of
    /// the ones the crashed run was executing under.
    ///
    /// Counted rather than assumed away: the first real boot after the
    /// envelope ships is what reports the true size of the pre-envelope
    /// backlog, and a "no-op that reports success" is exactly what a silent 0
    /// here would be.
    pub unsnapshotted: usize,
}

/// Why one candidate was not resumed.
///
/// Every arm is a refusal the coordinator *made*, not a state it found: a
/// clean session is `skipped`, a delegated one is `delegated`. This carries
/// only the cases where something was wrong enough to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeRefusal {
    /// The reducer refused the log ([`LogContradiction::rejects`]). "I do not
    /// know what state this run is in" — never read as clean.
    LogInconsistent(LogContradiction),
    /// The session's agent is not in the registry, so there is nothing to
    /// re-trigger the run on.
    AgentMissing,
    /// The log could not be read, or the repair events could not be appended.
    /// Resuming anyway would hand the model a `tool_use` with no result.
    BoundaryRepairFailed(String),
    /// The repair landed but the run could not be dispatched.
    RetriggerFailed(String),
}

impl ResumeRefusal {
    /// The stable word this refusal is reported under. Pinned by test to the
    /// variant list so a new arm cannot ship without a word of its own.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::LogInconsistent(_) => "log_inconsistent",
            Self::AgentMissing => "agent_missing",
            Self::BoundaryRepairFailed(_) => "boundary_repair_failed",
            Self::RetriggerFailed(_) => "retrigger_failed",
        }
    }

    /// The specifics behind [`Self::reason`], for an operator to act on.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::LogInconsistent(c) => c.to_string(),
            Self::AgentMissing => "the session's agent is not registered".to_string(),
            Self::BoundaryRepairFailed(e) | Self::RetriggerFailed(e) => e.clone(),
        }
    }
}

impl std::fmt::Display for ResumeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason(), self.detail())
    }
}

/// `task_type` of a cron-triggered run's session key.
///
/// Pinned to its producer by the source-level guard
/// `tests::the_delegated_task_types_match_their_producers`: the cron executor
/// builds its key from a literal and
/// exports no constant, so the only honest alternative to re-declaring it here
/// is a guard that reads that file. A silent drift here is a resume that
/// double-drives a cron job.
const CRON_TASK_TYPE: &str = "cron";

/// `task_type` of a heartbeat-triggered run's session key. Same provenance and
/// same guard as [`CRON_TASK_TYPE`].
const HEARTBEAT_TASK_TYPE: &str = "heartbeat";

/// True when `key` belongs to a unit that runs its **own** crash recovery.
///
/// The boot resume and these schedulers are two recovery projections of the
/// same state, and running both feeds one unit from two sources: the team
/// dispatcher's `reclaim_orphaned` + `abandon_orphaned_runs` already reclaim
/// every interrupted member run (and now bound how often they may), while cron
/// and heartbeat each decide at boot, by their own carryover rules, whether a
/// missed tick should be made up at all. A generic re-trigger on top of that is
/// not a safety net — it is a second, uncoordinated driver, and the two
/// disagree about *whether the run should happen again* rather than about how.
///
/// So those sessions are handed back, and (this is the part that is easy to
/// forget) their dangling `RunStarted` marker is closed on the way out. Left
/// open it would classify as `Interrupted` on every subsequent boot forever:
/// the scan would keep growing, and each pass would keep re-deciding the same
/// thing.
///
/// Team membership is asked of the teams subsystem itself
/// ([`crate::teams::run_mode::is_team_session`]) rather than re-derived — it
/// already owns "which sessions are team runs" and covers both team task types.
#[must_use]
pub fn has_own_scheduler(key: &SessionId) -> bool {
    if crate::teams::run_mode::is_team_session(key) {
        return true;
    }
    matches!(
        key,
        SessionId::Task { task_type, .. }
            if task_type == CRON_TASK_TYPE || task_type == HEARTBEAT_TASK_TYPE
    )
}

/// Extract `project_root` from the most recent `RunStarted` marker.
/// Returns `None` for legacy logs or when the original run was not
/// project-scoped, so the caller falls back to the agent's default
/// workspace — same shape as the in-memory `RunRequest.workspace_override`
/// field flows through the engine.
pub(crate) fn latest_project_root(markers: &[SessionEventRecord]) -> Option<std::path::PathBuf> {
    markers.iter().rev().find_map(|record| match &record.event {
        SessionEvent::RunStarted { project_root, .. } => {
            project_root.as_deref().map(std::path::PathBuf::from)
        }
        _ => None,
    })
}

/// Build a resumed run's metadata: the resume marker, the original working
/// directory, and the session's owner/scope attribution.
///
/// The scope is the half that used to be missing. `run_loop::with_request_scope`
/// reads this map and nothing else, and `scope_from_metadata` is fail-closed —
/// so a resume that carries only `project_root` runs UNSCOPED, and
/// `session_write_id` falls through to the base partition, which
/// `partition_visible` rules org-tier and shares with everyone. A resumed room's
/// memory landed where every user could read it, silently.
///
/// `from_persisted` requires both columns present and coherent, so a legacy
/// (pre-P1) session stamps nothing and resumes exactly as it did before — the
/// same zero-change carve-out `goal_wait::rehydrate_owner_scope` and cron's
/// executor take, from the same durable columns.
pub(crate) fn resume_metadata(
    workspace_override: Option<&std::path::Path>,
    session_meta: Option<&crate::gateway::session_store::types::SessionMetadata>,
) -> HashMap<String, String> {
    let mut metadata: HashMap<String, String> = HashMap::new();
    metadata.insert("resume".to_string(), "true".to_string());
    if let Some(p) = workspace_override {
        metadata.insert("project_root".to_string(), p.display().to_string());
    }
    if let Some(attr) = session_meta.and_then(|m| {
        crate::scope::ScopeAttribution::from_persisted(
            m.owner_user_id.as_deref(),
            m.scope_id.as_deref(),
        )
    }) {
        crate::scope::stamp_metadata(&mut metadata, &attr);
    }
    // The originating connection's role, same key and same source as
    // `handlers::agent::build_run_request`.
    //
    // `agent.resume` is member-open and KeyChecked, and `sessions.patch` lets a
    // member write `exec_tier` onto their OWN session — round 2 left that write
    // open precisely because the ceiling at resolution was supposed to bound
    // it. But the ceiling reads this key, and
    // `turn_context::role_is_operator(None)` is `true` ("absent role =
    // local/internal"), so a resumed run skipped both the clamp
    // (`ExecTier::most_restrictive(tier, global)`) and the operator-tool gate.
    // `stamp_origin_identity` below only reaches the writer of this key for
    // CHANNEL-origin sessions; a Panel session takes its early-return branch.
    //
    // Boot resume and the `/v1/admin` route have no caller scope, so this
    // writes nothing there and their behaviour is byte-identical.
    if let Some(role) = crate::gateway::caller_identity::current_caller_role() {
        metadata.insert("caller_role".to_string(), role);
    }
    metadata
}

/// When this candidate was last *alive*, in recording time.
///
/// Measured from the last thing that happened inside the run, not from the
/// marker that opened it. A long-running agent whose `RunStarted` is three days
/// old and whose last tool call landed a minute before the crash is the exact
/// candidate resume exists for; on the marker alone it was abandoned as "too
/// old", while a run that opened seconds before a crash and did nothing was
/// resumed. Whole classes of long agent runs were unresumable and the counter
/// said `abandoned`, which reads like a decision rather than a mismeasurement.
///
/// The marker still participates (`max`): a run that opened and recorded
/// nothing has no in-scope activity at all, and the marker's own recording time
/// is then the newest fact the log has about it. Pure, and separate from
/// [`ResumeCoordinator::handle_interrupted`], so the rule is falsifiable
/// without a coordinator, a store and an execution adapter.
fn last_alive_at(
    reduction: &crate::session::reduction::RunReduction,
    last_marker: &SessionEventRecord,
) -> crate::session::events::Timestamp {
    match reduction.progress.last_activity_at {
        Some(activity) => activity.max(last_marker.created_at_ms),
        None => last_marker.created_at_ms,
    }
}

/// Boot-scan coordinator. Constructed at boot with the durable event store,
/// the config, and the re-trigger collaborators (execution adapter + agent
/// registry). Mirrors the cron / heartbeat system-initiated-run precedent.
pub struct ResumeCoordinator {
    event_store: Arc<dyn SessionEventStore>,
    config: ResumeConfig,
    execution_adapter: Arc<dyn ExecutionAdapter>,
    agent_registry: Arc<AgentRegistry>,
    /// Source of the resumed session's persisted owner/scope. See
    /// [`ResumeCoordinator::retrigger`] for why a resume that carries the
    /// workspace but not the scope writes the room's memory to the org
    /// partition.
    session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
    /// Where a resumed run's live frames go. Required — a bus is one
    /// `GatewayEventBus::new()` away even in tests, so there is no `Option`
    /// escape hatch that could re-introduce the collect-and-drop shape.
    ///
    /// Without it a recovered run is *visibly running and provably
    /// unstoppable*: `SessionRunRegistry::try_claim` broadcasts
    /// `RunningSetChanged` unconditionally so the sidebar lights up, while the
    /// `run_id` minted below never reaches a client — and that run_id is the
    /// only key `chat.abort` and `agent.cancel` accept. See
    /// [`ResumeCoordinator::retrigger`].
    event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    /// Bounds the boot resume burst. `max_concurrent` permits.
    semaphore: Arc<Semaphore>,
    /// Session keys with a resume in flight, so one session is never resumed
    /// twice at once. See [`ResumeReport::busy`] for what the second winner
    /// would corrupt.
    ///
    /// A `std::sync::Mutex` around a `HashSet`, never held across an `.await` —
    /// the claim and the release are each a single lock/insert/drop, and the
    /// slot itself is an RAII guard so an early return or a panic mid-resume
    /// cannot leave a session permanently unresumable.
    in_flight: std::sync::Mutex<std::collections::HashSet<String>>,
}

/// RAII claim on one session's resume slot.
struct ResumeSlot<'a> {
    owner: &'a std::sync::Mutex<std::collections::HashSet<String>>,
    key: String,
}

impl Drop for ResumeSlot<'_> {
    fn drop(&mut self) {
        // Poison-safe (P7): recover the guard rather than leak the slot — a
        // panicked resume must not make the session unresumable forever.
        self.owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

impl ResumeCoordinator {
    /// Construct a coordinator.
    pub fn new(
        event_store: Arc<dyn SessionEventStore>,
        config: ResumeConfig,
        execution_adapter: Arc<dyn ExecutionAdapter>,
        agent_registry: Arc<AgentRegistry>,
        session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
        event_bus: Arc<crate::gateway::event_bus::GatewayEventBus>,
    ) -> Self {
        let permits = config.max_concurrent.max(1);
        Self {
            event_store,
            config,
            execution_adapter,
            agent_registry,
            session_store,
            event_bus,
            semaphore: Arc::new(Semaphore::new(permits)),
            in_flight: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Take this session's resume slot, or `None` if a resume is already in
    /// flight for it.
    fn try_claim_resume(&self, session_id: &SessionId) -> Option<ResumeSlot<'_>> {
        let key = session_id.to_key_string();
        let inserted = self
            .in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone());
        inserted.then(|| ResumeSlot {
            owner: &self.in_flight,
            key,
        })
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
            self.resume_from_markers(&session_id, &markers, &mut report)
                .await;
        }

        tracing::info!(
            scanned = report.scanned,
            resumed = report.resumed,
            abandoned = report.abandoned,
            skipped = report.skipped,
            delegated = report.delegated,
            // Expected to be 0 here — the scan is sequential. A non-zero value
            // means an on-demand `agent.resume` raced the boot scan, which is
            // the collision `in_flight` exists to make harmless and which is
            // worth seeing in the log rather than inferring.
            busy = report.busy,
            "resume scan complete"
        );
        report
    }

    /// Close the dangling run marker of a session this coordinator declined,
    /// so the scan does not re-classify it as interrupted on every later boot.
    ///
    /// Only the marker. Deliberately none of [`Self::abandon`]'s other three
    /// steps: this is not an abandonment. The owning scheduler decides whether
    /// the work is redone, so blocking the session's goal or telling the user
    /// "could not be resumed" would both be false — and the goal block in
    /// particular would be a wrong permanent verdict on a unit that is about
    /// to recover normally.
    async fn close_delegated_marker(&self, session_id: &SessionId) {
        let ev = SessionEvent::RunFinished {
            run_id: format!("delegated-{}", uuid::Uuid::new_v4()),
            outcome: RunOutcome::Abandoned,
            at: now_ms(),
        };
        match self.next_seq(session_id).await {
            Ok(seq) => {
                if let Err(e) = self
                    .event_store
                    .append(session_id, seq, &ev, now_ms())
                    .await
                {
                    tracing::warn!(session = ?session_id, error = %e, "resume: delegated marker close failed");
                }
            }
            // Same rule as `abandon`: never fabricate seq 1 on a read error —
            // it would overwrite the session's genuine first event. Skipping
            // costs one redundant re-classification on the next boot.
            Err(e) => {
                tracing::warn!(session = ?session_id, error = %e, "resume: delegated marker seq allocation failed; leaving it open");
            }
        }
    }

    /// Classify one session's run markers and act on the verdict.
    ///
    /// The single derivation shared by the boot scan and the on-demand
    /// [`resume_session`](Self::resume_session). A verb with two faces has to
    /// share its reasoning, not just its name: an on-demand resume that skipped
    /// the recency filter, the crash-loop cap or the boundary repair would be a
    /// second, weaker resume wearing the same word.
    async fn resume_from_markers(
        &self,
        session_id: &SessionId,
        markers: &[SessionEventRecord],
        report: &mut ResumeReport,
    ) {
        // Claimed before anything reads the log. `repair_boundary` is a
        // read-then-append: two concurrent resumes of one session both compute
        // the same repair set and both append it, leaving one `call_id` with
        // two `ToolError`s. `harness::agent::prompt` downgrades the second one
        // to a plain user note rather than sending an invalid pair, so the cost
        // is duplicated prose the model must reconcile, not an API rejection.
        // The boot scan never exposed this (it walks sessions in a sequential
        // loop); the on-demand face does, including against the boot scan
        // itself, which is spawned while the gateway is already accepting
        // requests.
        let Some(_slot) = self.try_claim_resume(session_id) else {
            tracing::info!(
                session = ?session_id,
                "resume: already in flight for this session; leaving it alone"
            );
            report.busy += 1;
            return;
        };
        report.scanned += 1;
        match reduce_disposition(markers) {
            Ok(RunDisposition::Clean) => {
                report.skipped += 1;
            }
            // Not ours to resume: the team dispatcher / cron / heartbeat
            // each recover their own interrupted work, and a second driver
            // on top of that is a duplicate run, not a safety net. Close
            // the dangling marker so the next boot does not re-decide this.
            Ok(RunDisposition::Interrupted { .. }) if has_own_scheduler(session_id) => {
                tracing::info!(
                    session = ?session_id,
                    "resume: session has its own scheduler; handing recovery back to it"
                );
                self.close_delegated_marker(session_id).await;
                report.delegated += 1;
            }
            Ok(RunDisposition::Interrupted { trailing_starts }) => {
                let project_root = latest_project_root(markers);
                self.handle_interrupted(session_id, markers, trailing_starts, project_root, report)
                    .await;
            }
            // A refused slice is "I do not know", not "clean": it is
            // deliberately NOT counted as `skipped` (which `status_of` renders
            // `already_finished`). It goes in the `refused` bucket, which
            // `status_of` reads BEFORE every counter that could be mistaken
            // for a verdict.
            Err(c) => {
                tracing::warn!(
                    session = ?session_id,
                    contradiction = %c,
                    "resume: session log refused by the reducer; not resuming"
                );
                report
                    .refused
                    .push((session_id.clone(), ResumeRefusal::LogInconsistent(c)));
            }
        }
    }

    /// Resume one session on demand.
    ///
    /// Boot is not the only moment a run can be found interrupted — the boot
    /// scan runs once, so a session that was interrupted while the daemon kept
    /// running, or one whose resume was skipped because a transient error ate
    /// its candidate, had no second chance and no way to ask for one. This is
    /// that way: `agent.resume` on the gateway and `aleph-server resume` on the
    /// CLI both land here.
    ///
    /// Deliberately does **not** consult `config.enabled`. That switch governs
    /// the *automatic* scan — whether the daemon resumes things nobody asked it
    /// to. An operator naming a session has already made the decision the switch
    /// exists to defer, and silently ignoring an explicit request is the kind of
    /// no-op that reads as a broken feature.
    ///
    /// Everything else is shared with boot via
    /// [`resume_from_markers`](Self::resume_from_markers): same recency filter,
    /// same crash-loop cap, same boundary repair, same concurrency permit.
    ///
    /// Reads the same cross-session marker query boot uses and picks this
    /// session out of it, rather than adding a narrower query. On-demand resume
    /// is an operator action measured in ones per hour, and one query with one
    /// grouping rule cannot drift from itself.
    ///
    /// A session with no run markers at all returns a zero report
    /// (`scanned == 0`), which the caller renders as "nothing to resume" — not
    /// an error, because "this session never ran anything" is a legitimate
    /// answer to the question.
    pub async fn resume_session(
        &self,
        session_id: &SessionId,
    ) -> Result<ResumeReport, crate::session::service::SessionError> {
        let mut report = ResumeReport::default();
        let groups = self.event_store.load_run_markers().await?;
        let Some((_, markers)) = groups.into_iter().find(|(sid, _)| sid == session_id) else {
            return Ok(report);
        };
        self.resume_from_markers(session_id, &markers, &mut report)
            .await;
        tracing::info!(
            session = ?session_id,
            resumed = report.resumed,
            abandoned = report.abandoned,
            skipped = report.skipped,
            "on-demand resume complete"
        );
        Ok(report)
    }

    /// Handle one interrupted candidate: **one** reduction over the log, then
    /// the recency filter, the cap check, the crash-boundary repair and the
    /// re-trigger — every one of them reading that same reduction.
    ///
    /// The repair used to re-read and re-reduce the log itself, so "what state
    /// is this candidate in" was answered twice per candidate, at two moments,
    /// with an append in between. Two derivations of one fact is the shape
    /// this round exists to remove.
    async fn handle_interrupted(
        &self,
        session_id: &SessionId,
        markers: &[SessionEventRecord],
        trailing_starts: usize,
        project_root: Option<std::path::PathBuf>,
        report: &mut ResumeReport,
    ) {
        // The dangling RunStarted is the last marker (reduce_disposition
        // guarantees `markers` is non-empty here).
        let Some(last) = markers.last() else {
            return;
        };

        let events = match self.event_store.load_all_events(session_id).await {
            Ok(events) => events,
            Err(e) => {
                tracing::warn!(
                    session = ?session_id,
                    error = %e,
                    "resume: candidate log unreadable; skipping candidate"
                );
                report.refused.push((
                    session_id.clone(),
                    ResumeRefusal::BoundaryRepairFailed(e.to_string()),
                ));
                return;
            }
        };
        let reduction = match reduce_run(&events) {
            Ok(reduction) => reduction,
            Err(c) => {
                tracing::warn!(
                    session = ?session_id,
                    contradiction = %c,
                    "resume: candidate log refused by the reducer; not resuming"
                );
                report
                    .refused
                    .push((session_id.clone(), ResumeRefusal::LogInconsistent(c)));
                return;
            }
        };
        report.contradictions += reduction.contradictions.len();

        // A clock anomaly makes the age unknown, and BOTH remaining verdicts
        // are decisions taken on an age: resuming says "recent enough",
        // abandoning says "too old". Neither is derivable, so this candidate
        // is left exactly as it is and counted under its own name.
        if reduction
            .contradictions
            .iter()
            .any(|c| matches!(c, LogContradiction::ClockAnomaly { .. }))
        {
            tracing::warn!(
                session = ?session_id,
                "resume: candidate log has a clock anomaly; its age is unknown, leaving it alone"
            );
            report.skipped_unknown_age += 1;
            return;
        }

        // Recency filter — abandon runs interrupted too long ago.
        let age_ms = now_ms().saturating_sub(last_alive_at(&reduction, last));
        if age_ms > (self.config.max_age_secs as i64).saturating_mul(1000) {
            tracing::info!(
                session = ?session_id,
                age_ms,
                "resume: candidate too old; abandoning"
            );
            self.abandon(
                session_id,
                "the interrupted run was too old to resume safely",
            )
            .await;
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
            self.abandon(session_id, "it kept crashing on every resume attempt")
                .await;
            report.abandoned += 1;
            return;
        }

        // Crash-boundary repair — append a synthetic ToolError for every
        // dangling call THIS reduction names, so the model sees each one
        // answered instead of silently dropped from the replay.
        if let Err(e) = crate::session::boundary_repair::repair_boundary(
            self.event_store.as_ref(),
            session_id,
            &reduction,
            None,
        )
        .await
        {
            tracing::warn!(
                session = ?session_id,
                error = %e,
                "resume: boundary repair failed; skipping candidate"
            );
            report.refused.push((
                session_id.clone(),
                ResumeRefusal::BoundaryRepairFailed(e.to_string()),
            ));
            return;
        }

        // Re-trigger. Task 6 implements `retrigger`. When the original
        // run carried a `project_root`, pre-validate it still exists so a
        // moved/deleted folder degrades to a default-workspace resume
        // (with a warn) instead of failing the run mid-tool-call.
        let resume_project_root = match project_root {
            Some(p) if p.is_dir() => Some(p),
            Some(p) => {
                tracing::warn!(
                    session = ?session_id,
                    project_root = %p.display(),
                    "resume: original project folder no longer exists; \
                     falling back to agent workspace"
                );
                None
            }
            None => None,
        };
        match self.retrigger(session_id, resume_project_root).await {
            Ok(()) => report.resumed += 1,
            Err(refusal) => {
                tracing::warn!(
                    session = ?session_id,
                    error = %refusal,
                    "resume: re-trigger failed; skipping candidate"
                );
                report.refused.push((session_id.clone(), refusal));
            }
        }
    }

    /// Terminate an abandoned candidate honestly: emit `RunFinished {
    /// Abandoned }` so the run is not re-scanned on the next boot, block any
    /// active goal in the session (its crash recovery hangs ENTIRELY on this
    /// coordinator's retrigger→post_run chain — abandoning severs it, so an
    /// Active goal would otherwise lie in `goal(list)` forever), and drop a
    /// one-line notice on the origin channel. Every step is best-effort and
    /// independent — a failed marker append must not silence the user notice.
    ///
    /// Deliberately does NOT touch loop state: loops are process-memory and
    /// the registry is empty at boot; "stopping" one here could only misfire
    /// against a loop the user started while the scan was still running.
    async fn abandon(&self, session_id: &SessionId, reason: &str) {
        let ev = SessionEvent::RunFinished {
            run_id: format!("abandoned-{}", uuid::Uuid::new_v4()),
            outcome: RunOutcome::Abandoned,
            at: now_ms(),
        };
        match self.next_seq(session_id).await {
            Ok(seq) => {
                if let Err(e) = self
                    .event_store
                    .append(session_id, seq, &ev, now_ms())
                    .await
                {
                    tracing::warn!(session = ?session_id, error = %e, "resume: abandon marker append failed");
                }
            }
            Err(e) => {
                // Don't fabricate seq 1 on a read error — that would append the
                // abandon marker at the head and overwrite the genuine first
                // event. Skip the best-effort marker; the next boot re-abandons.
                tracing::warn!(session = ?session_id, error = %e, "resume: abandon seq allocation failed; skipping marker");
            }
        }

        // Scope the block to goals whose recovery actually hung on THIS
        // crashed run: Active-pursuit, not parked on a task barrier (those are
        // woken by GoalWakeService, and a passive goal never depended on the
        // continuation chain). A healthy parked or interactive goal must not
        // be collateral-blocked by an unrelated abandoned run.
        let goal_blocked = crate::gateway::continuation_lifecycle::block_abandonable_session_goal(
            &session_id.to_key_string(),
            &format!(
                "Autonomous pursuit halted: its interrupted run was abandoned at daemon \
                 restart ({reason}). Re-set the goal to continue."
            ),
        );

        // One-line origin notice, mirroring `retrigger`'s fanout resolution.
        // Panel-only sessions (`gui:chat`) have no origin route and rely on
        // the stored blocked note; a missing agent cannot be routed for at
        // all (same documented limitation as the engine's agent-miss branch).
        if let Some(reg) = crate::gateway::event_emitter::origin_fanout::channel_registry() {
            if let Some(agent) = self.agent_registry.get(session_id.agent_id()).await {
                if let Some((channel, conversation)) = agent.origin_route(session_id).await {
                    let mut text = format!(
                        "⚠️ An interrupted run in this conversation could not be resumed \
                         after a restart ({reason}) and was abandoned."
                    );
                    if goal_blocked {
                        text.push_str(" Its standing goal was blocked — re-set it to continue.");
                    }
                    let msg = crate::gateway::channel::OutboundMessage::text(conversation, text);
                    if let Err(e) = reg
                        .send(&crate::gateway::channel::ChannelId::new(channel), msg)
                        .await
                    {
                        tracing::warn!(session = ?session_id, error = %e, "resume: abandon notice delivery failed");
                    }
                }
            }
        }
    }

    /// Allocate the next append seq for a session.
    ///
    /// Propagates read errors rather than defaulting to `1`: a transient
    /// `load_head_seq` failure is indistinguishable from an empty session, and
    /// guessing `1` for a non-empty session would collide with / overwrite its
    /// first event.
    async fn next_seq(
        &self,
        session_id: &SessionId,
    ) -> Result<u64, crate::session::service::SessionError> {
        Ok(self.event_store.load_head_seq(session_id).await? + 1)
    }

    /// Re-trigger an interrupted run. Resolves the agent from the session
    /// key, builds a `RunRequest` with `metadata["resume"] = "true"` (the
    /// engine→orchestrator boundary converts that into `FlowInput::Resume`,
    /// which skips re-seeding), and dispatches it through the same
    /// `ExecutionAdapter` cron / heartbeat use. A `max_concurrent`
    /// semaphore bounds the boot burst.
    async fn retrigger(
        &self,
        session_id: &SessionId,
        workspace_override: Option<std::path::PathBuf>,
    ) -> Result<(), ResumeRefusal> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ResumeRefusal::RetriggerFailed(format!("resume semaphore closed: {e}")))?;

        let agent_id = session_id.agent_id().to_string();
        // A missing agent is its own refusal, not a generic failure: the run
        // is intact and re-triggerable the moment that agent exists again,
        // which is a different thing for an operator to read than "dispatch
        // errored".
        let agent = self
            .agent_registry
            .get(&agent_id)
            .await
            .ok_or(ResumeRefusal::AgentMissing)?;

        let mut metadata = resume_metadata(
            workspace_override.as_deref(),
            self.persisted_session_meta(session_id).await.as_ref(),
        );
        self.stamp_origin_identity(&agent, session_id, &mut metadata)
            .await;

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
            workspace_override,
            max_iterations_override: None,
            model_override: None,
        };

        // Broadcast the recovered run live (Panel / CLI / `aleph watch`) on
        // the bus. Same two arms as `execute::spawn_continuation_run` and
        // `handlers::agent`.
        //
        // This is not cosmetic. `SessionRunRegistry::try_claim` broadcasts
        // `RunningSetChanged` unconditionally, so the sidebar shows the session
        // as running the moment the resume claims its slot — while a bare
        // collector emits no `RunAccepted`, and `RunAccepted` is both the seed
        // `event_visibility` needs to resolve every later frame of that run AND
        // the only carrier of the `run_id` that `chat.abort` / `agent.cancel`
        // require. A crash-recovered long run was therefore visibly running and
        // unstoppable from every UI until it finished or the daemon was killed
        // again. (That is also why the bus is a mandatory constructor
        // parameter: the collect-and-drop shape must not be constructible.)
        let base: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> = Arc::new(
            crate::gateway::event_emitter::GatewayEventEmitter::new(Arc::clone(&self.event_bus)),
        );
        // Fan the recovered run's final reply out to the session's bound
        // origin channel — the human who asked. Without this the resumed run
        // completed into a collect-and-drop emitter: the crash-recovered
        // answer existed only in the session log and the Telegram/Slack user
        // never heard back (R5). Mirrors `spawn_continuation_run`; a Panel-
        // only session (`gui:chat`, no origin route) has no channel to fan out
        // to and rides the bus alone.
        // Best-effort: the boot scan may outrun a slow channel connect — the
        // fanout decorator warns-and-drops on send failure, never fails the
        // resumed run itself.
        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            match crate::gateway::event_emitter::origin_fanout::channel_registry() {
                Some(reg) => match agent.origin_route(session_id).await {
                    Some((channel, conversation)) => Arc::new(
                        crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                            base,
                            reg,
                            channel,
                            conversation,
                        ),
                    ),
                    None => base,
                },
                None => base,
            };

        tracing::info!(session = ?session_id, agent_id, "resume: re-triggering interrupted run");

        let result = self
            .execution_adapter
            .execute(request, agent, emitter)
            .await
            .map_err(|e| ResumeRefusal::RetriggerFailed(format!("resume execute failed: {e}")));

        drop(permit);
        result
    }

    /// The resumed session's durable row, or `None` when it cannot be read.
    ///
    /// A store error is logged and swallowed: an unscoped resume is the
    /// pre-existing behaviour, and refusing to resume over it would turn a
    /// crash recovery into a lost conversation.
    async fn persisted_session_meta(
        &self,
        session_id: &SessionId,
    ) -> Option<crate::gateway::session_store::types::SessionMetadata> {
        match self.session_store.get_metadata(session_id).await {
            Ok(meta) => meta,
            Err(e) => {
                tracing::warn!(
                    session = ?session_id,
                    error = %e,
                    "resume: session metadata unreadable; resuming unscoped"
                );
                None
            }
        }
    }

    /// Re-derive the run identity the session's origin channel imposes.
    ///
    /// A resumed run used to be born with `{resume, project_root}` and nothing
    /// else, and both of the missing keys fail OPEN: `role_is_operator(None)`
    /// is `true` (`tools/turn_context.rs`), so the config-tool gate waves the
    /// run through, and an absent channel `ToolPermissionsConfig` merges no deny
    /// layer. A killed daemon restarting therefore resurrected a Chat-tier
    /// Telegram run as an unwatched **operator** with no deny layer — the exact
    /// bug class `execute::carry_policy_metadata` exists to prevent for the
    /// continuation path.
    ///
    /// The stamp is the shared `channel_policy::system_continuation_identity`:
    /// a `guest` role FLOOR (a boot resume is unattended — never silently
    /// operator, even for a `Config`-tier channel) PLUS the origin channel's
    /// live `tool_permissions` deny layer, read from the process-global
    /// channel-config snapshot. Historically this path threaded its own config
    /// map that was never wired, so it ran at guest with NO deny layer; the
    /// shared snapshot keeps the guest floor unchanged and adds the missing deny
    /// layer. An unknown / unconfigured channel (snapshot miss) resolves to
    /// guest + no deny — the same fail-closed default `channel_run_identity`
    /// pins for a live message.
    ///
    /// No routable origin (the Panel's `gui:chat`, or a session whose origin
    /// conversation was never captured) ⇒ mark the run `unattended`: nobody is
    /// there to answer an approval card raised by a run a boot scan re-triggered,
    /// so confirm-gated tools must fail closed instead of publishing into the
    /// void and parking on the 120 s approval timeout. A run that DOES carry a
    /// full origin route keeps it — its approval is genuinely deliverable and the
    /// human on the other end can `/approve`. Same rule, same reasons, as
    /// `tasks::cron::executor::build_cron_metadata`.
    async fn stamp_origin_identity(
        &self,
        agent: &Arc<AgentInstance>,
        session_id: &SessionId,
        metadata: &mut HashMap<String, String>,
    ) {
        let Some((channel, conversation)) = agent.origin_route(session_id).await else {
            metadata.insert(UNATTENDED_KEY.to_string(), "true".to_string());
            return;
        };
        // A boot resume is a system-initiated continuation: guest role floor +
        // the origin channel's tool_permissions deny layer, derived from the
        // process-global channel-config snapshot (published at the end of
        // `initialize_inbound_router`). Shared verbatim with the goal wake path
        // so both fail closed identically. Merges over `metadata` (its keys are
        // exactly the identity keys, overwriting any pre-stamped value).
        metadata.extend(
            crate::gateway::channel_policy::system_continuation_identity(&channel, &conversation),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{ToolOutput, TurnId};

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_resume_coordinator_slot();
        assert_eq!(slot.id(), "gateway/resume-coordinator");
        assert!(matches!(slot.missing(), MissingSemantics::FailsClosed));
    }

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
            project_root: None,
            envelope: None,
        }
    }

    fn run_started_with_project(at: i64, project: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: format!("r-{at}"),
            at,
            project_root: Some(project.to_string()),
            envelope: None,
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
        let markers = vec![rec(1, run_started(10), 10), rec(2, run_finished(20), 20)];
        assert_eq!(reduce_disposition(&markers), Ok(RunDisposition::Clean));
    }

    #[test]
    fn classify_interrupted_single_dangling_start() {
        let markers = vec![
            rec(1, run_started(10), 10),
            rec(2, run_finished(20), 20),
            rec(3, run_started(30), 30),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { trailing_starts: 1 })
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
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { trailing_starts: 3 })
        );
    }

    #[test]
    fn classify_interrupted_when_no_finish_at_all() {
        let markers = vec![rec(1, run_started(10), 10)];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { trailing_starts: 1 })
        );
    }


    /// ③-D8's falsification arm. A run whose `RunStarted` is ancient but whose
    /// last tool call landed a moment ago is alive, and measuring its age from
    /// the marker abandons exactly the long runs resume exists for.
    ///
    /// Goes red if `last_alive_at` is reverted to reading the marker alone.
    #[test]
    fn recency_is_measured_from_the_last_activity_not_the_marker() {
        let events = vec![
            rec(1, run_started(10), 1_000),
            rec(2, tool_requested("c1"), 900_000),
        ];
        let reduction = reduce_run(&events).expect("legal log");
        assert_eq!(
            last_alive_at(&reduction, &events[0]),
            900_000,
            "the dispatch is newer than the marker that opened the run"
        );
    }

    /// The other direction: a run that opened and recorded nothing has no
    /// in-scope activity, so the marker's own recording time is the newest
    /// fact there is. `None` here may not read as "epoch" — that would abandon
    /// every freshly-opened run.
    #[test]
    fn a_run_that_recorded_nothing_is_dated_by_its_marker() {
        let events = vec![rec(1, run_started(10), 5_000)];
        let reduction = reduce_run(&events).expect("legal log");
        assert_eq!(reduction.progress.last_activity_at, None);
        assert_eq!(last_alive_at(&reduction, &events[0]), 5_000);
    }

    /// An answered call is still activity — the run was alive when its result
    /// landed, whether or not anything is left dangling.
    #[test]
    fn an_answered_call_is_still_activity() {
        let events = vec![
            rec(1, run_started(10), 1_000),
            rec(2, tool_requested("c1"), 2_000),
            rec(3, tool_result("c1"), 3_000),
        ];
        let reduction = reduce_run(&events).expect("legal log");
        assert!(reduction.dangling.is_empty());
        assert_eq!(last_alive_at(&reduction, &events[0]), 3_000);
    }

    /// Every refusal carries a word of its own. A new variant that fans into
    /// an existing word would make two different answers read alike in the
    /// receipt, the CLI and the doctor at once.
    #[test]
    fn every_refusal_has_its_own_reason_word() {
        let all = [
            ResumeRefusal::LogInconsistent(LogContradiction::OutOfOrderSlice { at_seq: 7 }),
            ResumeRefusal::AgentMissing,
            ResumeRefusal::BoundaryRepairFailed("append failed".into()),
            ResumeRefusal::RetriggerFailed("adapter said no".into()),
        ];
        let words: std::collections::HashSet<&str> = all.iter().map(|r| r.reason()).collect();
        assert_eq!(words.len(), all.len(), "two refusals share one word");
        for refusal in &all {
            assert!(
                !refusal.detail().is_empty(),
                "{refusal:?} reports no detail an operator could act on"
            );
        }
        assert!(
            all[0].detail().contains("seq 7"),
            "a log contradiction must name where: {}",
            all[0].detail()
        );
    }

    /// `latest_project_root` walks the marker list from newest to oldest
    /// and returns the most recent persisted `project_root`, falling back
    /// to `None` (legacy log, or non-project run) for the resume default.
    #[test]
    fn latest_project_root_picks_newest_marker() {
        let markers = vec![
            rec(1, run_started_with_project(10, "/a"), 10),
            rec(2, run_finished(20), 20),
            rec(3, run_started_with_project(30, "/b"), 30),
        ];
        assert_eq!(
            latest_project_root(&markers),
            Some(std::path::PathBuf::from("/b"))
        );
    }

    #[test]
    fn latest_project_root_returns_none_for_legacy_runs() {
        let markers = vec![rec(1, run_started(10), 10)];
        assert_eq!(latest_project_root(&markers), None);
    }

    /// I2: a resumed run must carry the session's SCOPE, not just its folder.
    /// Without the stamp the run is unscoped and its memory writes land in the
    /// base partition — org-tier, readable by everyone — which for a project
    /// room means the room's memory leaks out of the room.
    #[test]
    fn a_resumed_room_run_carries_the_rooms_scope() {
        use crate::gateway::session_store::types::SessionMetadata;

        let room = SessionMetadata {
            owner_user_id: Some("u-alice".to_string()),
            scope_id: Some(crate::scope::ScopeId::Project("p-standup".into()).render()),
            ..Default::default()
        };
        let meta = resume_metadata(Some(std::path::Path::new("/srv/room")), Some(&room));

        assert_eq!(meta.get("resume").map(String::as_str), Some("true"));
        assert!(meta.contains_key("project_root"), "the folder still rides");
        // Assert through the consumer, not the raw keys: `with_request_scope`
        // reaches the run through exactly this call.
        let scope = crate::scope::scope_from_metadata(&meta)
            .expect("a project-scoped session must resolve a scope");
        assert_eq!(
            scope.scope,
            crate::scope::ScopeId::Project("p-standup".into())
        );
        assert_eq!(scope.owner_user_id, "u-alice");
    }

    /// A legacy (pre-P1) row, or no row at all, stamps nothing — the resume
    /// behaves exactly as it did before, rather than guessing an attribution.
    #[test]
    fn a_legacy_session_resumes_unscoped_exactly_as_before() {
        use crate::gateway::session_store::types::SessionMetadata;

        for meta in [
            resume_metadata(None, None),
            resume_metadata(None, Some(&SessionMetadata::default())),
        ] {
            assert!(crate::scope::scope_from_metadata(&meta).is_none());
            assert_eq!(meta.get("resume").map(String::as_str), Some("true"));
            assert!(!meta.contains_key("project_root"));
        }
    }

    /// The units that recover themselves must be excluded, and everything a
    /// human talks to must not be. Asserted through the predicate the scan
    /// loop actually calls, and (for teams) through the constructor the
    /// dispatcher actually uses.
    #[test]
    fn sessions_with_their_own_scheduler_are_not_resumed_here() {
        use crate::routing::session_key::DmScope;

        for key in [
            SessionId::task("main", CRON_TASK_TYPE, "daily-summary"),
            SessionId::task("main", HEARTBEAT_TASK_TYPE, "hb-1"),
            SessionId::task(
                "worker",
                crate::teams::run_mode::TEAM_TASK_TASK_TYPE,
                "task-1",
            ),
            SessionId::task(
                "worker",
                crate::teams::run_mode::TEAM_CHAT_TASK_TYPE,
                "squad",
            ),
        ] {
            assert!(
                has_own_scheduler(&key),
                "{} owns its recovery and must not be double-driven",
                key.to_key_string()
            );
        }

        for key in [
            SessionId::main("alice"),
            SessionId::dm("alice", "telegram", "u1", DmScope::PerPeer),
            SessionId::task("main", "a2a", "job-1"),
            SessionId::task("main", "webhook", "hook-1"),
        ] {
            assert!(
                !has_own_scheduler(&key),
                "{} has no other recovery path — excluding it loses the run",
                key.to_key_string()
            );
        }
    }

    /// `CRON_TASK_TYPE` / `HEARTBEAT_TASK_TYPE` are re-declared here because
    /// their producers export no constant. A re-declaration that drifts is
    /// silent (the exclusion simply stops matching and the double-drive
    /// returns), so pin them to the producers' source. Source-level on
    /// purpose: at runtime a key built from a drifted literal is
    /// indistinguishable from a correct one.
    #[test]
    fn the_delegated_task_types_match_their_producers() {
        for (path, task_type) in [
            ("src/tasks/cron/executor.rs", CRON_TASK_TYPE),
            ("src/tasks/heartbeat/executor.rs", HEARTBEAT_TASK_TYPE),
        ] {
            let src = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path),
            )
            .unwrap_or_else(|e| panic!("{path} must be readable: {e}"));
            // The producer shape is `SessionKey::task(agent, "<type>", id)`,
            // so the literal always sits between two commas.
            assert!(
                src.contains(&format!(", \"{task_type}\", ")),
                "{path} no longer builds its session key with \"{task_type}\" — \
                 `has_own_scheduler` has drifted from the producer and the boot \
                 resume is double-driving that scheduler again"
            );
        }
    }

}
