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
    reduce_disposition, reduce_run, DanglingProvenance, RunDisposition, RunReduction,
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
    /// for the same `call_id`. A tool_use with two tool_results is a provider
    /// API error on every later turn of that session.
    pub busy: usize,
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

/// The sentence a dangling call is answered with.
///
/// Deliberately **not** a safety-level classifier. `ToolSafetyLevel` exists and
/// could sort read-only calls from destructive ones, but deciding "is this safe
/// to redo?" from a tool name and its arguments is exactly the reasoning R7
/// reserves for the model. State the fact; let it judge.
///
/// Two arms because there are two true sentences. Everything after the lead-in
/// is shared, so the five semantic points cannot drift apart between them.
fn boundary_repair_text(tool: &str, provenance: DanglingProvenance) -> String {
    let lead = match provenance {
        DanglingProvenance::ThisRestart => format!(
            "the server restarted after this `{tool}` call was dispatched but before its \
             result was recorded"
        ),
        DanglingProvenance::EarlierRun => format!(
            "an earlier run in this session ended without recording the result of this \
             `{tool}` call"
        ),
    };
    format!(
        "OUTCOME UNKNOWN — {lead}. This is NOT a report that the call failed: it may have \
         completed, and any side effects it has (file writes, commands, network calls, \
         external state) have already landed. Verify the current state before deciding \
         whether to repeat it."
    )
}

/// Turn a reduction's dangling set into appendable answer events.
///
/// **Both provenances get an event.** Leaving the older ones unanswered is not
/// the cheaper option: `build_prompt` drops an orphan `tool_use` whose result
/// never arrives, so the model stops seeing that the call ever happened — while
/// its side effects may still be on disk. A missing row reads as "there was no
/// value"; that is the reading this whole repair exists to prevent.
///
/// The answer is shaped as `ToolError` because there is no result to hand back:
/// a synthetic `ToolResult` would make an invented payload indistinguishable
/// from the tool's real output.
pub(crate) fn repairs_for(reduction: &RunReduction) -> Vec<SessionEvent> {
    let at = now_ms();
    reduction
        .dangling
        .iter()
        .map(|call| SessionEvent::ToolError {
            turn_id: call.turn_id,
            call_id: call.call_id.clone(),
            error: boundary_repair_text(&call.tool_name, call.provenance),
            at,
        })
        .collect()
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
        // two `ToolError`s — a tool_use with two tool_results, which the
        // provider rejects on every later turn. The boot scan never exposed
        // this (it walks sessions in a sequential loop); the on-demand face
        // does, including against the boot scan itself, which is spawned while
        // the gateway is already accepting requests.
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
            // deliberately NOT counted as `skipped` (documented as clean, and
            // rendered `already_finished` by `status_of`). Left uncounted it
            // lands on the `scanned > 0` → `not_resumed` arm, the honest
            // interim word until the `refused` bucket exists.
            Err(c) => {
                tracing::warn!(
                    session = ?session_id,
                    contradiction = %c,
                    "resume: session log refused by the reducer; not resuming"
                );
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

    /// Handle one interrupted candidate: recency filter, cap check,
    /// crash-boundary repair, then re-trigger.
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

        // Recency filter — abandon runs interrupted too long ago.
        let age_ms = now_ms().saturating_sub(last.created_at_ms);
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
            Err(e) => {
                tracing::warn!(
                    session = ?session_id,
                    error = %e,
                    "resume: re-trigger failed; skipping candidate"
                );
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

    /// Append synthetic `ToolError`s for any dangling tool calls.
    async fn repair_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), crate::session::service::SessionError> {
        let events = self.event_store.load_all_events(session_id).await?;
        // A refused log propagates as an error: repairing on top of a slice
        // the reducer could not read would append receipts to the wrong calls.
        let reduction = reduce_run(&events)
            .map_err(|c| crate::session::service::SessionError::Other(c.to_string()))?;
        let repairs = repairs_for(&reduction);
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
    ) -> Result<(), crate::session::service::SessionError> {
        use crate::session::service::SessionError;

        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| SessionError::Other(format!("resume semaphore closed: {e}")))?;

        let agent_id = session_id.agent_id().to_string();
        let agent = self.agent_registry.get(&agent_id).await.ok_or_else(|| {
            SessionError::Other(format!("resume: agent '{agent_id}' not registered"))
        })?;

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
            .map_err(|e| SessionError::Other(format!("resume execute failed: {e}")));

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

    /// G3 — both arms must carry all five semantic points. Asserting on
    /// MEANING, not bytes: `!contains("failed")` gets hit by the text's own
    /// negation sentence, which is how the first version of this guard went
    /// red for the wrong reason (§4.13a).
    fn assert_five_points(error: &str, tool: &str) {
        assert!(
            error.contains("OUTCOME UNKNOWN"),
            "must state the outcome is unknown, got: {error}"
        );
        assert!(
            error.contains("NOT a report that the call failed"),
            "must explicitly deny that the call failed, got: {error}"
        );
        assert!(
            error.contains(tool),
            "must name the tool so the model knows what to verify, got: {error}"
        );
        assert!(
            error.contains("side effects"),
            "must warn that side effects may have landed, got: {error}"
        );
        assert!(
            error.contains("Verify the current state before deciding"),
            "must tell the model to verify current state before redoing, got: {error}"
        );
    }

    #[test]
    fn repairs_speak_a_different_sentence_per_provenance() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, run_started(30), 30),
            rec(4, tool_requested("c2"), 40),
        ];
        let repairs = repairs_for(&reduce_run(&events).expect("legal log"));
        assert_eq!(repairs.len(), 2, "BOTH provenances get a repair event");

        let mut texts = Vec::new();
        for ev in &repairs {
            let SessionEvent::ToolError { call_id, error, .. } = ev else {
                panic!("expected ToolError, got {ev:?}");
            };
            assert_five_points(error, "bash_exec");
            texts.push((call_id.clone(), error.clone()));
        }
        assert_eq!(texts[0].0, "c1");
        assert!(
            texts[0].1.contains("an earlier run in this session"),
            "the older dangle must not be blamed on this restart, got: {}",
            texts[0].1
        );
        assert_eq!(texts[1].0, "c2");
        assert!(
            texts[1].1.contains("the server restarted"),
            "this run's dangle must say so, got: {}",
            texts[1].1
        );
        assert_ne!(texts[0].1, texts[1].1, "two provenances, two sentences");
    }

    #[test]
    fn repairs_are_empty_when_every_call_is_answered() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(3, tool_result("c1"), 30),
        ];
        assert!(repairs_for(&reduce_run(&events).expect("legal log")).is_empty());
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

    #[test]
    fn a_tool_error_counts_as_an_answer() {
        let events = vec![
            rec(1, run_started(10), 10),
            rec(2, tool_requested("c1"), 20),
            rec(
                3,
                SessionEvent::ToolError {
                    turn_id: TurnId::new_v4(),
                    call_id: "c1".into(),
                    error: "prior failure".into(),
                    at: 30,
                },
                30,
            ),
        ];
        assert!(repairs_for(&reduce_run(&events).expect("legal log")).is_empty());
    }
}
