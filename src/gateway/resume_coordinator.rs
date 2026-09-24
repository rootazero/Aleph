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
//! A seed is **unanswered** (§5.2) iff a real `UserMessage` sits after the
//! last `RunFinished` with no `RunStarted` and no `AssistantMessage` after
//! it — the crash landed before the run's own marker. Markers cannot show
//! that shape, so the scan reads the message tail of every Clean candidate
//! and of every marker-less session in the activity window, stamps the
//! message and re-triggers it with no boundary repair (nothing dangled).
//! A message is **lost** (§8.2(b)) iff the engine's `agent_tasks` row for it
//! exists and its session log holds no `UserMessage` since — the crash
//! landed before the seed itself. Nothing can be re-run; the user is told
//! once, in-band, to re-send it ([`ResumeCoordinator::adjudicate_orphaned_tasks`]).
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
use crate::session::events::{
    now_ms, EventSeq, RunOutcome, SessionEvent, SessionEventRecord, Timestamp,
};
use crate::session::reduction::{
    is_disposition_bearing, reduce_disposition, reduce_run, LogContradiction, RunDisposition,
};
use crate::session::service::{SessionError, SessionId};
use crate::session::store::{MarkerSlice, SessionEventStore};

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
    /// Sessions inspected that had at least one run marker, plus the
    /// marker-less sessions in the activity window about which something was
    /// decided: an unanswered message found (§5.2), or a tail the pass could
    /// not read or the reducer refused (both filed under `refused`). A
    /// marker-less session whose tail was answered is not counted, because
    /// nothing about it was decided.
    pub scanned: usize,
    /// Interrupted runs successfully re-triggered.
    pub resumed: usize,
    /// Runs marked `Abandoned` (too old or crash-loop cap reached).
    pub abandoned: usize,
    /// Sessions skipped (clean — newest marker is `RunFinished`, and the
    /// message tail after it holds no user message left unanswered).
    pub skipped: usize,
    /// Interrupted sessions handed back to the scheduler that owns them
    /// (team dispatcher / cron / heartbeat) instead of being resumed here.
    /// Their crash boundary is repaired and their dangling marker closed on the
    /// way out — see [`has_own_scheduler`].
    pub delegated: usize,
    /// Sessions left alone because somebody else is writing them right now.
    ///
    /// Two producers, one fact. **A resume already in flight**: two
    /// `agent.resume` calls for one session — or one racing the boot scan,
    /// which is spawned while the gateway is already serving — must not both
    /// run `repair_boundary`, because that is a
    /// read-then-append and two winners append **two** synthetic `ToolError`s
    /// for the same `call_id`. Since `harness::agent::prompt` learned to
    /// downgrade an orphaned/duplicate `tool_result` to a plain user note
    /// (7929bbda6) that is no longer a provider rejection — it is text noise:
    /// the model reads the same "outcome unknown" sentence twice, the second
    /// time as prose that no longer references the call it answers.
    ///
    /// **The engine is running it**: a session the engine has a live turn on
    /// gets no append from any arm (`defer_if_running`). For a delegated
    /// session that means neither the repair nor the marker close, because a
    /// `RunFinished` written into the middle of a running turn makes the real
    /// finish land as a `FinishWithoutStart` on a session that is then
    /// permanently mis-read. For an interrupted or unanswered one it also
    /// means no stamp and no retrigger: the live run would otherwise be read
    /// as the open one and resumed a second time. Unlike the first producer
    /// this one is reachable from the boot scan: the dispatcher's own tick, or
    /// a new inbound message, can start a run while the scan is still walking
    /// the list.
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
    /// §8.2(b): lost-input notices written this pass — one per interrupted
    /// Main-lane task row whose seed never reached its session log, so the
    /// user was told to re-send. Rendered by the boot log line only: the
    /// adjudication runs in the boot scan's [`ResumeLaunch::settle`], never
    /// on the per-session `agent.resume` face, whose receipt therefore does
    /// not carry it — a wire field that is 0 on every receipt that route can
    /// produce would read as "checked, none found".
    pub notified: usize,
}

impl ResumeReport {
    /// Fold one candidate's part into this total — the boot scan runs its
    /// candidates as separate tasks and sums their reports here.
    ///
    /// Exhaustive destructure, no `..`: a counter added to the report is a
    /// compile error on this line rather than a silent undercount on the boot
    /// log line. `refused` is appended, not summed — it names sessions.
    pub fn absorb(&mut self, other: Self) {
        let Self {
            scanned,
            resumed,
            abandoned,
            skipped,
            delegated,
            busy,
            refused,
            skipped_unknown_age,
            contradictions,
            degraded,
            unsnapshotted,
            notified,
        } = other;
        self.scanned += scanned;
        self.resumed += resumed;
        self.abandoned += abandoned;
        self.skipped += skipped;
        self.delegated += delegated;
        self.busy += busy;
        self.refused.extend(refused);
        self.skipped_unknown_age += skipped_unknown_age;
        self.contradictions += contradictions;
        self.degraded += degraded;
        self.unsnapshotted += unsnapshotted;
        self.notified += notified;
    }
}

/// A launched boot scan — see [`ResumeCoordinator::launch_resume`].
///
/// `pending` is every session the scan will VISIT (a marker group, or an
/// activity-window row asked the §5.2 question), so a caller can hold that
/// session's queued input back until [`settle`](Self::settle); everything
/// else is safe to re-deliver at once. Deliberately OVER-inclusive rather
/// than "the ones a marker-only slice already calls Interrupted": an
/// `Unanswered` verdict is invisible to a marker slice (it is read from a
/// bounded tail inside the task), so a narrower set would release a survivor
/// into a session the scan is about to act on. Scheduler-owned sessions are
/// left out — their queued input is never re-delivered by the boot path.
#[must_use = "dropping a ResumeLaunch aborts every resume it launched; call settle()"]
pub struct ResumeLaunch {
    pub pending: std::collections::HashSet<String>,
    /// Whether the marker scan was actually walked. `false` when resume is
    /// disabled or the marker load failed — then neither `settle` nor the
    /// caller may report a completion, because a "scan finished, scanned = 0"
    /// line after a "scan failed" line reads the failure as an empty result.
    /// Read it BEFORE `settle` consumes the launch.
    pub walked: bool,
    /// One task per candidate, tagged with its candidate ordinal so the
    /// settled report reads in scan order whatever order the tasks finish in.
    tasks: tokio::task::JoinSet<(usize, ResumeReport)>,
    /// Which session each task is deciding, by task id, so a task that did
    /// not complete can be named — its return value (and ordinal) is gone.
    sessions: HashMap<tokio::task::Id, SessionId>,
    /// The coordinator that launched this scan, for the pass `settle` runs
    /// after every candidate has been decided (see
    /// [`ResumeCoordinator::adjudicate_orphaned_tasks`]).
    me: Arc<ResumeCoordinator>,
    report: ResumeReport,
}

impl ResumeLaunch {
    /// Wait for every candidate task and fold its part into one report, then
    /// adjudicate the task rows the scan could not see (§8.2(b)).
    pub async fn settle(mut self) -> ResumeReport {
        let mut parts = Vec::new();
        while let Some(joined) = self.tasks.join_next().await {
            match joined {
                Ok(part) => parts.push(part),
                // A candidate whose task panicked (or was cancelled) has an
                // UNKNOWN verdict: its session is left exactly as the crash
                // left it, and it is counted nowhere — the report has no arm
                // for "the scan itself failed on this one", and inventing a
                // count here would read the panic as a decision.
                Err(e) => tracing::warn!(
                    session = ?self.sessions.get(&e.id()),
                    error = %e,
                    "resume: candidate task did not complete; its verdict is unknown"
                ),
            }
        }
        // Candidate order, so `refused` reads like the scan walked.
        parts.sort_by_key(|(ordinal, _)| *ordinal);
        for (_, part) in parts {
            self.report.absorb(part);
        }
        // After every candidate, on purpose: a row whose session the scan
        // just resumed or abandoned reads as "open" or "seeded" here, so the
        // resume arm's verdict is the one the user hears and this pass only
        // stamps the row. Not gated on `walked` — this pass reads each row's
        // own log, not the marker slice the scan may have failed to load.
        self.me.adjudicate_orphaned_tasks(&mut self.report).await;
        if self.walked {
            tracing::info!(
                scanned = self.report.scanned,
                resumed = self.report.resumed,
                abandoned = self.report.abandoned,
                skipped = self.report.skipped,
                delegated = self.report.delegated,
                // The scan fans out but claims one slot per session, so two
                // candidates never collide; a non-zero value is an on-demand
                // `agent.resume` racing the boot scan, which is the collision
                // `in_flight` exists to make harmless and which is worth
                // seeing in the log rather than inferring.
                busy = self.report.busy,
                notified = self.report.notified,
                "resume scan complete"
            );
        }
        self.report
    }
}

/// Why one candidate was not resumed.
///
/// Every arm is a refusal the coordinator *made*, not a state it found: a
/// clean session is `skipped`, a delegated one is `delegated`. This carries
/// only the cases where something was wrong enough to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeRefusal {
    /// The log was refused ([`LogContradiction::rejects`]) — by the reducer,
    /// or by the store's decoder before the reducer ever saw it
    /// (`UndecodableRecord`). "I do not know what state this run is in" —
    /// never read as clean.
    LogInconsistent(LogContradiction),
    /// The session's agent is not in the registry, so there is nothing to
    /// re-trigger the run on.
    AgentMissing,
    /// The log could not be read, or the repair events could not be appended.
    /// Resuming anyway would hand the model a `tool_use` with no result.
    BoundaryRepairFailed(String),
    /// The repair landed but the run could not be dispatched.
    RetriggerFailed(String),
    /// The stamp did not land, so the retrigger must not happen: a resume
    /// without its intent stamp is exactly the unbounded loop §5.1 closes.
    IntentStampFailed(String),
    /// The message tail past the last `RunFinished` could not be read, so
    /// whether the last user message was answered is unknown (§5.2). Not
    /// [`Self::BoundaryRepairFailed`]: no repair was attempted, and a label
    /// that says one was is the expensive kind of wrong. The reason word is a
    /// pass-through string on the wire — no client switches on reason words —
    /// so it needs no renderer of its own.
    TailReadFailed(String),
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
            Self::IntentStampFailed(_) => "intent_stamp_failed",
            Self::TailReadFailed(_) => "tail_read_failed",
        }
    }

    /// The specifics behind [`Self::reason`], for an operator to act on.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::LogInconsistent(c) => c.to_string(),
            Self::AgentMissing => "the session's agent is not registered".to_string(),
            Self::BoundaryRepairFailed(e)
            | Self::RetriggerFailed(e)
            | Self::IntentStampFailed(e)
            | Self::TailReadFailed(e) => e.clone(),
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

/// Sessions the Unanswered arm may retrigger: not a scheduler-owned unit
/// (they re-run by their own rule) and not a sub-agent child or an ephemeral
/// side session (A7: children are reported, never re-driven; an ephemeral
/// session has no user waiting on it).
#[must_use]
pub(crate) fn unanswered_eligible(key: &SessionId) -> bool {
    // Exhaustive on purpose: a retrigger is an LLM run nobody asked for, so a
    // new session kind must be classified here before it can be eligible —
    // a `!matches!` would admit it by default.
    match key {
        SessionId::Subagent { .. } | SessionId::Ephemeral { .. } => false,
        SessionId::Main { .. }
        | SessionId::DirectMessage { .. }
        | SessionId::Group { .. }
        | SessionId::Task { .. } => !has_own_scheduler(key),
    }
}

/// What [`ResumeCoordinator::abandon`] is giving up on — the subject of the
/// sentence the user reads. The closer is the same `RunFinished { Abandoned }`
/// either way; the sentence is not, and a notice that says "an interrupted
/// run" about a session in which no run ever existed is the expensive kind
/// of wrong (criterion #17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Abandoned {
    /// A `RunStarted` with no finish: the run existed and was cut off.
    InterruptedRun,
    /// A seeded message no run ever picked up (§5.2): no run existed.
    UnansweredMessage,
}

impl Abandoned {
    /// The one-line sentence the user reads on BOTH faces — the in-band
    /// `SystemMessage` note and the origin-channel notice — derived once in
    /// [`ResumeCoordinator::abandon`].
    fn notice(self, reason: &str) -> String {
        match self {
            Self::InterruptedRun => format!(
                "⚠️ An interrupted run in this conversation could not be resumed \
                 after a restart ({reason}) and was abandoned."
            ),
            Self::UnansweredMessage => format!(
                "⚠️ Your last message in this conversation was never picked up by a \
                 run, and after a restart it could not be retried ({reason}); it was \
                 abandoned — send it again if you still need it."
            ),
        }
    }

    /// The note stored on a blocked goal.
    fn goal_note(self, reason: &str) -> String {
        match self {
            Self::InterruptedRun => format!(
                "Autonomous pursuit halted: its interrupted run was abandoned at daemon \
                 restart ({reason}). Re-set the goal to continue."
            ),
            Self::UnansweredMessage => format!(
                "Autonomous pursuit halted: the message that would have driven it was \
                 never picked up by a run and was abandoned at daemon restart ({reason}). \
                 Re-set the goal to continue."
            ),
        }
    }
}

/// What a resume can still do with the model the crashed run was bound to.
///
/// The pin is read off a `RunStarted` envelope that may be days old, so it is
/// **validated before it is replayed**, never after: an id the vendor retired
/// while the daemon was down would otherwise come back as an opaque provider
/// 400 on the first Think step of every recovered run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SnapshotModel {
    /// Replay the pair as recorded.
    Keep {
        provider: Option<String>,
        model: String,
    },
    /// The vendor retired it and named what to use instead.
    Successor {
        from: String,
        to: String,
        why: String,
    },
    /// Nothing to replay: resume on the agent's own default chain — which is
    /// exactly what this session did before the envelope existed.
    Drop { from: String, why: String },
}

/// Decide the above.
///
/// `pinnable` is the published set of provider keys a pin can name
/// ([`crate::providers::session_model_handle::pinnable_providers`]), passed in
/// rather than read here so this stays a pure function with no global to
/// install in a test. `None` means **no set was published**, which is
/// "unvalidated", never "nothing is pinnable" — the same reading
/// `select_model::refuse_unpinnable_provider` takes of the same handle, because
/// one verb with two faces has to share its derivation (判据 #9).
///
/// Lifecycle comes from [`crate::providers::model_catalog::lifecycle_for`] —
/// the table `select_model`, the picker and the drift guard already read. A
/// second retirement list here is the shape this round exists to delete.
pub(crate) fn validate_snapshot_model(
    pinnable: Option<&std::collections::BTreeSet<String>>,
    provider: Option<&str>,
    model: &str,
) -> SnapshotModel {
    use crate::providers::model_catalog::lifecycle::{lifecycle_for, ModelStatus};

    let model = model.trim();
    let provider = provider.map(str::trim).filter(|p| !p.is_empty());

    // A pin naming a provider this server does not have cannot be honoured:
    // the run would fall through to the default chain anyway, and the
    // mis-attributed pair would be recorded as if it had served the run.
    if let (Some(p), Some(known)) = (provider, pinnable) {
        if !known.contains(p) {
            return SnapshotModel::Drop {
                from: model.to_string(),
                why: format!("provider `{p}` is no longer configured on this server"),
            };
        }
    }

    let life = lifecycle_for(provider, model);
    if life.status != ModelStatus::Deprecated {
        return SnapshotModel::Keep {
            provider: provider.map(str::to_string),
            model: model.to_string(),
        };
    }
    let why = life.note.map_or_else(
        || "it has been retired since this run started".to_string(),
        |n| n.into_owned(),
    );
    match life.successor {
        Some(to) => SnapshotModel::Successor {
            from: model.to_string(),
            to: to.into_owned(),
            why,
        },
        None => SnapshotModel::Drop {
            from: model.to_string(),
            why,
        },
    }
}

/// Everything a resume replays, derived once from the crashed run's
/// `RunStarted` marker.
///
/// Built by [`plan_resume`] from the reduction's `open_run` — the single
/// anchor, so the workspace, the knobs and the model cannot come from three
/// different markers.
#[derive(Debug, Default)]
pub(crate) struct ResumePlan {
    /// The project folder to resume in, `None` for the agent's own workspace.
    pub(crate) workspace: Option<std::path::PathBuf>,
    /// The model pin to replay, after validation.
    pub(crate) model_override: Option<crate::gateway::model_override::ModelOverride>,
    /// Request metadata the resumed run carries: the three replayable knobs
    /// plus the tier CEILING (never the tier request rung — see
    /// [`crate::gateway::execution_engine::RESUME_TIER_CEILING_KEY`]), plus
    /// the two per-run facts (skill scope, `/btw` stamp) under the keys their
    /// owning modules spell.
    pub(crate) knobs: HashMap<String, String>,
    /// What the model is told it lost, if anything.
    pub(crate) degrade: Option<crate::session::boundary_repair::DegradeNote>,
    /// Whether this resume gave something up (one or more sentences above).
    pub(crate) degraded: bool,
    /// Whether the crashed run's marker carried no envelope at all, so the
    /// resume follows today's session and global values.
    pub(crate) unsnapshotted: bool,
}

/// The per-run FACTS a `RunStarted` envelope freezes alongside the knobs —
/// the marker's own field names, spelled so the two facts cannot drift from
/// the metadata keys they are copied from and replayed to: the `/btw` stamp's
/// field IS the stamp's key (`btw::BTW_METADATA_KEY`), and the skill scope is
/// named literally because its request-metadata spelling belongs to
/// `slash_skill_scope` (`SLASH_SKILL_ALLOWED_TOOLS_KEY`) and is deliberately
/// not the marker's word.
///
/// Not knobs: no `custom` twin, no session / global rung, so [`plan_resume`]
/// below — their only production reader — replays them from the snapshot or
/// not at all. That is why the array lives here rather than beside
/// [`crate::gateway::session_snapshot::RUN_ENVELOPE_KNOB_KEYS`]:
/// `session_snapshot.rs` is the knob decoder, and `btw` must never appear in
/// it in any spelling (`btw::guard_tests::btw_is_not_filed_with_the_five_session_knobs`
/// reads that file for the word). And not in `session::events` beside the
/// struct, so `session` keeps depending on nothing in `gateway`. The census
/// `session::events::tests::the_envelope_carries_exactly_the_published_knob_keys`
/// asserts the envelope's key set == KNOB ∪ FACT.
pub const RUN_ENVELOPE_FACT_KEYS: [&str; 2] =
    ["allowed_tools", crate::gateway::btw::BTW_METADATA_KEY];

/// Derive the plan above.
///
/// `dir_exists` is injected so the project-root arm is testable without
/// touching a filesystem; production passes `|p| p.is_dir()`.
///
/// A missing `open_run` is the ③-D2 writer-side shape: the `RunStarted` append
/// failed and the run executed anyway. There is nothing to replay, and saying
/// so (`unsnapshotted`) is the honest answer — the resume still happens, on
/// today's values, exactly as it did before this field existed.
pub(crate) fn plan_resume(
    open_run: Option<&crate::session::reduction::RunStartFacts>,
    pinnable: Option<&std::collections::BTreeSet<String>>,
    dir_exists: &dyn Fn(&std::path::Path) -> bool,
) -> ResumePlan {
    use crate::gateway::execution_engine::RESUME_TIER_CEILING_KEY;
    use crate::gateway::model_override::ModelOverride;

    let mut plan = ResumePlan::default();
    let Some(facts) = open_run else {
        plan.unsnapshotted = true;
        return plan;
    };
    let mut sentences: Vec<String> = Vec::new();

    // Workspace. A folder that has since been deleted or moved falls back to
    // the agent's workspace rather than failing the run mid-tool-call — and
    // says so, because a silent fallback means the recovered run writes its
    // files somewhere the user is not looking (ruling A9).
    if let Some(root) = facts.project_root.as_deref() {
        let path = std::path::PathBuf::from(root);
        if dir_exists(&path) {
            plan.workspace = Some(path);
        } else {
            sentences.push(format!(
                "This run was working in `{root}`; it resumes in this agent's default \
                 workspace because that folder no longer exists."
            ));
            plan.degraded = true;
        }
    }

    let Some(env) = facts.envelope.as_ref() else {
        plan.unsnapshotted = true;
        plan.degrade = degrade_note(sentences);
        return plan;
    };

    for (key, value) in [
        (
            crate::config::types::policies::MODE_SESSION_KEY,
            env.session_mode.as_deref(),
        ),
        (
            crate::agents::thinking::THINK_LEVEL_SESSION_KEY,
            env.think_level.as_deref(),
        ),
        (
            crate::memory::session_memory_mode::MEMORY_MODE_SESSION_KEY,
            env.memory_mode.as_deref(),
        ),
        (RESUME_TIER_CEILING_KEY, env.exec_tier.as_deref()),
    ] {
        if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
            plan.knobs.insert(key.to_string(), v.to_string());
        }
    }

    // §6.3 per-run facts: snapshot rung only. Absent = "declared nothing" —
    // not a degrade, and never a session/global fallback.
    if let Some(tools) = env.allowed_tools.as_deref() {
        crate::gateway::execution_engine::slash_skill_scope::stamp_list(&mut plan.knobs, tools);
    }
    match env.btw.as_deref() {
        Some(stamp) if stamp == crate::gateway::btw::PROMOTE_STAMP => {
            // A promote is served before `admit_run` and is not a run, so a
            // marker carrying its sentinel is a writer-side oddity. Replaying
            // it would NOT reach the promote arm — that arm is gated on a
            // redirect from a MAIN key, and every stamped marker sits on the
            // side session the resume is addressed to — it would run a full
            // side-session turn under the read-only ceiling with `promote`
            // in its metadata: pointless, and not what the sentinel means.
            tracing::warn!(
                run_id = %facts.run_id,
                "resume: marker carries a promote stamp; a promote is not a run and is not replayed"
            );
        }
        Some(stamp) => {
            plan.knobs.insert(
                crate::gateway::btw::BTW_METADATA_KEY.to_string(),
                stamp.to_string(),
            );
        }
        None => {}
    }

    if let Some(model) = env
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        match validate_snapshot_model(pinnable, env.model_provider.as_deref(), model) {
            SnapshotModel::Keep { provider, model } => {
                plan.model_override =
                    ModelOverride::from_voice(provider.as_deref().unwrap_or(""), &model);
            }
            SnapshotModel::Successor { from, to, why } => {
                plan.model_override =
                    ModelOverride::from_voice(env.model_provider.as_deref().unwrap_or(""), &to);
                sentences.push(format!(
                    "This run was served by `{from}`; it resumes on `{to}` because {why}."
                ));
                plan.degraded = true;
            }
            SnapshotModel::Drop { from, why } => {
                sentences.push(format!(
                    "This run was served by `{from}`; it resumes on this agent's default \
                     model because {why}."
                ));
                plan.degraded = true;
            }
        }
    } else {
        // The envelope is here and the model half is not. Two origins, and
        // the sentence has to be true for both: a full run whose writer could
        // not name what served it (a dynamic route whose provider chain had
        // no `serving_model_hint`, or a marker from before that fallback
        // existed), and a slash-command fast path, which makes no LLM call and
        // so served on no model at all. The resume proceeds on today's chain —
        // but that chain is free to have moved, and this is the ONLY place
        // that knows it might have.
        //
        // `unsnapshotted` cannot carry this: it means "no envelope was
        // captured", and one was. `degraded` is the field whose definition
        // already fits — "this resume gave something up" — so the fact travels
        // on the existing wire instead of a new counter that four faces would
        // then have to learn to render (判据 #9).
        sentences.push(
            "This run recorded no model — a slash-command fast path serves on none, and a \
             full run may have failed to name what served it; it resumes on this session's \
             current model."
                .to_string(),
        );
        plan.degraded = true;
    }

    plan.degrade = degrade_note(sentences);
    plan
}

/// One note out of however many sentences the plan collected.
fn degrade_note(sentences: Vec<String>) -> Option<crate::session::boundary_repair::DegradeNote> {
    (!sentences.is_empty())
        .then(|| crate::session::boundary_repair::DegradeNote::new(sentences.join(" ")))
}

/// The §8.2(b) sentence: a message the engine accepted (its task row was
/// written at `created_at_secs`) but never recorded, so the only honest
/// answer is to ask for it again. Quotes the head of the prompt so the user
/// can tell which message — by `char`, not byte, so a multibyte prompt cannot
/// split a code point (P7).
fn lost_input_notice(created_at_secs: i64, prompt: &str) -> String {
    let when = chrono::DateTime::from_timestamp(created_at_secs, 0)
        .map_or_else(|| created_at_secs.to_string(), |t| t.to_rfc3339());
    let head: String = prompt.chars().take(80).collect();
    format!("A message you sent at {when} was lost before it was recorded: «{head}». Please re-send it.")
}

/// The emitter a re-triggered run reports through.
///
/// Live frames go on the bus (`base`). The final reply additionally fans out
/// to the session's bound origin channel — the human who asked. Without that
/// the resumed run completed into a collect-and-drop emitter: the
/// crash-recovered answer existed only in the session log and the
/// Telegram/Slack user never heard back (R5). Mirrors `spawn_continuation_run`;
/// a Panel-only session (`gui:chat`, no origin route) has no channel to fan out
/// to and rides the bus alone. Best-effort: the boot scan may outrun a slow
/// channel connect — the fanout decorator warns-and-drops on send failure,
/// never fails the resumed run itself.
///
/// A resume that replays the `/btw` stamp (`is_side_question`) rides the bus
/// alone whatever the registry and route say — the same rule
/// `busy_queue/durable.rs` applies to a reinjected side question: a
/// re-delivered side answer must not land on the origin conversation unmarked
/// (`btw::format_side_answer`'s doc carries the census of fan-out sites).
///
/// `route` is a future, not a value, so the origin-route lookup is only paid
/// when it can matter: a stamped resume and a boot with no channel registry
/// (a Panel-only server) return before polling it.
///
/// A free function so the choice is testable with a registry and a route in
/// hand; `retrigger` is the only production caller.
async fn retrigger_emitter(
    base: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync>,
    registry: Option<Arc<crate::gateway::channel_registry::ChannelRegistry>>,
    route: impl std::future::Future<Output = Option<(String, String)>>,
    is_side_question: bool,
) -> Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> {
    if is_side_question {
        return base;
    }
    let Some(reg) = registry else {
        return base;
    };
    match route.await {
        Some((channel, conversation)) => Arc::new(
            crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter::new(
                base,
                reg,
                channel,
                conversation,
            ),
        ),
        None => base,
    }
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
///
/// `run_started` is the record that OPENED the run (`reduction.run_anchor`'s),
/// never "the last marker": since §5.1 the last marker is usually the newest
/// `ResumeAttempted`, and a coordinator's intent stamp is not the run being
/// alive — measured from it, `max_age_secs` would count from the last attempt
/// instead of the interruption. `progress.last_activity_at` excludes the stamp
/// for the same reason.
fn last_alive_at(
    reduction: &crate::session::reduction::RunReduction,
    run_started: &SessionEventRecord,
) -> crate::session::events::Timestamp {
    match reduction.progress.last_activity_at {
        Some(activity) => activity.max(run_started.created_at_ms),
        None => run_started.created_at_ms,
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
    /// Bounds resumes in flight: `[resume] max_concurrent` permits, shared by
    /// the boot scan's fan-out ([`launch_resume`](Self::launch_resume)) and
    /// on-demand resumes ([`resume_session`](Self::resume_session)).
    ///
    /// One permit spans a candidate's whole resume — boundary repair AND
    /// re-trigger — and is taken by those two entry points only. `retrigger`
    /// takes none of its own: nested inside a held permit, a second acquire
    /// deadlocks as soon as `max_concurrent` candidates each hold a permit
    /// and wait for a second one (three candidates under a cap of 2 is
    /// enough; a cap of 1 is only the smallest case).
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
    /// The `agent_tasks` table, for the §8.2(b) pass: a Main-lane row the
    /// engine wrote before it seeded the session is the only trace of a
    /// message that died between the two writes. `None` on a deployment
    /// without the resilience database — then no such row exists to
    /// adjudicate, and the pass is a no-op for the honest reason.
    state_database: Option<Arc<crate::resilience::StateDatabase>>,
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
            state_database: None,
        }
    }

    /// Hand the coordinator the `agent_tasks` table so the boot scan can
    /// adjudicate the rows whose seed never reached the log (§8.2(b)).
    #[must_use]
    pub fn with_state_database(mut self, db: Arc<crate::resilience::StateDatabase>) -> Self {
        self.state_database = Some(db);
        self
    }

    /// Take this session's resume slot, or `None` if a resume is already in
    /// flight for it.
    /// Does the engine have a turn in flight on this session right now?
    ///
    /// The authoritative in-memory admission gate, asked of the adapter rather
    /// than re-derived: [`ExecutionAdapter::running_sessions`] is the same set
    /// `gateway.metrics.run_concurrency` publishes and the same one the queue
    /// admits against. An adapter with no run registry answers with an empty
    /// set, which is honest for it (a `SimpleExecutionEngine` runs nothing
    /// concurrently) and is why this is not a fail-closed predicate the way
    /// `marker_balance::retire_from_and_close_run`'s is: that one closes the
    /// marker of a run the *user* just cut, this one only ever declines to
    /// touch a log.
    fn is_running(&self, session_id: &SessionId) -> bool {
        let key = session_id.to_key_string();
        self.execution_adapter.running_sessions().contains(&key)
    }

    /// Leave a session alone, counted as `busy`, while the engine has a turn
    /// in flight on it. Returns `true` when the caller must stop.
    ///
    /// The one gate every arm that appends to a candidate's log asks before
    /// its first append: the delegated hand-back, the interrupted repair and
    /// the unanswered stamp. The scan runs while the gateway is already
    /// accepting requests, so a candidate that reduced as open or unanswered
    /// when its markers were loaded may have a live run by the time its turn
    /// comes. That run's own records are then what the arm reads, and every
    /// append it makes — a `ToolError` for an in-flight call, a closer, a
    /// stamp, a duplicate retrigger — lands in the middle of it.
    ///
    /// Check-then-act: a run admitted between this read and the arm's first
    /// append still races it. Closing that window needs the coordinator to
    /// hold the engine's claim on the session for the repair's span, which it
    /// does not have (FOLLOW-UP F28 / F30).
    fn defer_if_running(&self, session_id: &SessionId, report: &mut ResumeReport) -> bool {
        if !self.is_running(session_id) {
            return false;
        }
        tracing::info!(
            session = ?session_id,
            "resume: the engine is running this session right now; leaving the log alone"
        );
        report.busy += 1;
        true
    }

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

    /// Start the boot scan: classify every candidate and act on it in its own
    /// task, `[resume] max_concurrent` at a time, without waiting for any of
    /// them. The caller reads [`ResumeLaunch::pending`] to hold back queued
    /// input for the sessions the scan will visit, then
    /// [`ResumeLaunch::settle`]s for the report — or calls
    /// [`resume_interrupted_runs`](Self::resume_interrupted_runs) for both at
    /// once.
    ///
    /// Fanned out because one slow candidate (a resumed run's `execute` is the
    /// whole run) used to hold every later candidate behind it. Each task
    /// holds one permit across its candidate's repair AND re-trigger, so the
    /// cap bounds runs in flight, not just dispatches.
    ///
    /// Best-effort: any failure is logged and skipped; never panics, never
    /// blocks boot. When `config.enabled` is false the scan itself is
    /// skipped — no candidate is visited, `walked` stays false — but the
    /// launch is still returned, because [`ResumeLaunch::settle`] has one
    /// job that flag does not govern: a message lost before it was recorded
    /// (§8.2(b)) is not a run to resume, so boot settles a disabled launch
    /// too.
    pub async fn launch_resume(self: &Arc<Self>) -> ResumeLaunch {
        let mut launch = ResumeLaunch {
            pending: std::collections::HashSet::new(),
            walked: false,
            tasks: tokio::task::JoinSet::new(),
            sessions: HashMap::new(),
            me: Arc::clone(self),
            report: ResumeReport::default(),
        };

        if !self.config.enabled {
            tracing::debug!("resume disabled ([resume] enabled = false); skipping scan");
            return launch;
        }

        let marker_groups = match self.event_store.load_run_markers().await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, "resume scan failed; skipping resume");
                return launch;
            }
        };
        launch.walked = true;

        // `pending` is deliberately OVER-inclusive: every session the scan will
        // VISIT (not only the ones a marker-only slice already calls
        // Interrupted). An `Unanswered` verdict is invisible to a marker slice
        // (the task reads it from a bounded tail), so a narrower set would
        // release a survivor into a session the scan is about to act on.
        let mut seen: std::collections::HashSet<SessionId> = std::collections::HashSet::new();
        let group_count = marker_groups.len();
        for (ordinal, (session_id, slice)) in marker_groups.into_iter().enumerate() {
            seen.insert(session_id.clone());
            if !has_own_scheduler(&session_id) {
                launch.pending.insert(session_id.to_key_string());
            }
            let me = Arc::clone(self);
            let semaphore = Arc::clone(&self.semaphore);
            let named = session_id.clone();
            let task = launch.tasks.spawn(async move {
                let mut part = ResumeReport::default();
                match semaphore.acquire_owned().await {
                    Ok(_permit) => {
                        me.resume_from_markers(&session_id, &slice, &mut part).await;
                    }
                    Err(e) => part.refused.push((
                        session_id,
                        ResumeRefusal::RetriggerFailed(format!("resume semaphore closed: {e}")),
                    )),
                }
                (ordinal, part)
            });
            launch.sessions.insert(task.id(), named);
        }

        // §5.2: a session that crashed between its seed and its first
        // `RunStarted` has NO marker, so the tasks above never visit it. The
        // activity window (the same one `ProjectionReconciler::candidates`
        // walks) is where such a session shows up: `execute()` stamps the row's
        // `last_active_at` before seeding. Sessions the marker scan already
        // visited are skipped — their Clean arm asks this question itself —
        // and so are sessions the Unanswered arm may never retrigger, so
        // `pending` does not hold their queued input for nothing.
        //
        // Round UP so a sub-minute horizon still admits something: the filter
        // is minute-granular and `0` would mean "nothing is recent".
        let active_minutes =
            u32::try_from(self.config.max_age_secs.div_ceil(60).max(1)).unwrap_or(u32::MAX);
        match self
            .session_store
            .list_sessions(crate::gateway::session_store::types::SessionFilter {
                active_minutes: Some(active_minutes),
                ..Default::default()
            })
            .await
        {
            Ok(rows) => {
                for (offset, meta) in rows.into_iter().enumerate() {
                    let Some(id) = SessionId::from_key_string(&meta.key) else {
                        // A stored key this process cannot parse is a session
                        // nothing can be asked of — say so rather than dropping
                        // it in silence (the reconciler twin does the same).
                        // Not a report counter: every counter on this report
                        // reaches the wire, and this row has no session to
                        // name there.
                        tracing::warn!(
                            key = %meta.key,
                            "resume: unparseable session key in the activity window; not scanned for an unanswered message"
                        );
                        continue;
                    };
                    if seen.contains(&id) || !unanswered_eligible(&id) {
                        continue;
                    }
                    launch.pending.insert(id.to_key_string());
                    let me = Arc::clone(self);
                    let semaphore = Arc::clone(&self.semaphore);
                    // After every marker group, so the settled report lists
                    // marker candidates first and window candidates after.
                    let ordinal = group_count + offset;
                    let named = id.clone();
                    let task = launch.tasks.spawn(async move {
                        let mut part = ResumeReport::default();
                        match semaphore.acquire_owned().await {
                            Ok(_permit) => {
                                let Some(_slot) = me.try_claim_resume(&id) else {
                                    part.busy += 1;
                                    return (ordinal, part);
                                };
                                if me.check_unanswered(&id, &[], &mut part).await {
                                    part.scanned += 1;
                                }
                            }
                            Err(e) => part.refused.push((
                                id,
                                ResumeRefusal::RetriggerFailed(format!(
                                    "resume semaphore closed: {e}"
                                )),
                            )),
                        }
                        (ordinal, part)
                    });
                    launch.sessions.insert(task.id(), named);
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                "resume: activity-window listing failed; marker-less unanswered sessions not scanned"
            ),
        }

        launch
    }

    /// Scan for interrupted runs and re-trigger each, waiting for all of them:
    /// [`launch_resume`](Self::launch_resume) + [`ResumeLaunch::settle`].
    pub async fn resume_interrupted_runs(self: &Arc<Self>) -> ResumeReport {
        self.launch_resume().await.settle().await
    }

    /// Hand a session back to the scheduler that owns it: answer the calls its
    /// crash left dangling, then close the run marker so the scan does not
    /// re-classify it as interrupted on every later boot.
    ///
    /// Still none of [`Self::abandon`]'s other three steps: this is not an
    /// abandonment. The owning scheduler decides whether the *work* is redone,
    /// so blocking the session's goal or telling the user "could not be
    /// resumed" would both be false — the goal block in particular would be a
    /// wrong permanent verdict on a unit that is about to recover normally.
    ///
    /// But the crash boundary is not the scheduler's decision, it is a fact
    /// about the log, and leaving it unrepaired meant this arm produced a
    /// session whose next run replays a `tool_use` with no `tool_result` — the
    /// exact silent drop [`crate::session::boundary_repair`] exists for. The
    /// team dispatcher repairs its own member sessions in `reclaim_orphaned`;
    /// cron and heartbeat have no such pass, so this arm is their only repair.
    ///
    /// Returns whether anything was written, so the caller can tell "handed
    /// back" from "could not read its log".
    async fn hand_back_to_scheduler(
        &self,
        session_id: &SessionId,
    ) -> Result<(), crate::session::SessionError> {
        let report = crate::session::boundary_repair::repair_and_close_abandoned(
            self.event_store.as_ref(),
            session_id,
        )
        .await?;
        tracing::info!(
            session = ?session_id,
            repaired = report.appended,
            closed = ?report.closed_run_id,
            "resume: crash boundary repaired and marker closed for the owning scheduler"
        );
        Ok(())
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
        slice: &MarkerSlice,
        report: &mut ResumeReport,
    ) {
        // Claimed before anything reads the log. `repair_boundary` is a
        // read-then-append: two concurrent resumes of one session both compute
        // the same repair set and both append it, leaving one `call_id` with
        // two `ToolError`s. `harness::agent::prompt` downgrades the second one
        // to a plain user note rather than sending an invalid pair, so the cost
        // is duplicated prose the model must reconcile, not an API rejection.
        // The boot scan fans out but claims one slot per session, so two of
        // its candidates never collide; `busy` still counts an on-demand
        // resume racing the scan, which is spawned while the gateway is
        // already accepting requests.
        let Some(_slot) = self.try_claim_resume(session_id) else {
            tracing::info!(
                session = ?session_id,
                "resume: already in flight for this session; leaving it alone"
            );
            report.busy += 1;
            return;
        };
        report.scanned += 1;
        // The slice before the markers: one the store could not decode is
        // refused here, under its own kind, BEFORE any arm reads it as a
        // list. `Err` is "I do not know what this session holds" — never
        // "it holds no markers" (criterion #8).
        let markers = match slice {
            Ok(markers) => markers,
            Err(undecodable) => {
                self.refuse_log(session_id, LogContradiction::from(undecodable), report);
                return;
            }
        };
        match reduce_disposition(markers) {
            // No run is open. The markers cannot say whether the LAST user
            // message was ever answered — that lives in the message tail
            // (§5.2) — so `Clean` is only half the verdict here, and the
            // other half is asked before the session is filed as skipped.
            //
            // `Unanswered` is unreachable from a marker-only slice (the
            // list face's honest ceiling). It shares the arm rather than
            // being filed under `skipped`: if a caller ever hands a wider
            // slice, the answer is re-derived from the tail read and acted
            // on, so the word cannot be wrong in the confident direction.
            Ok(RunDisposition::Clean | RunDisposition::Unanswered { .. }) => {
                if !self.check_unanswered(session_id, markers, report).await {
                    report.skipped += 1;
                }
            }
            // Not ours to resume: the team dispatcher / cron / heartbeat
            // each recover their own interrupted work, and a second driver
            // on top of that is a duplicate run, not a safety net. Close
            // the dangling marker so the next boot does not re-decide this.
            Ok(RunDisposition::Interrupted { .. }) if has_own_scheduler(session_id) => {
                // Handing recovery back does not mean handing the log back:
                // the marker close and the boundary repair are facts about
                // this session that only a reader of its log can write. But
                // both are appends, so they may only happen while nobody else
                // is writing — a session the engine is running RIGHT NOW is
                // mid-turn, and a `RunFinished` appended into the middle of a
                // live run is the `FinishWithoutStart` this round exists to
                // stop producing.
                if self.defer_if_running(session_id, report) {
                    return;
                }
                tracing::info!(
                    session = ?session_id,
                    "resume: session has its own scheduler; handing recovery back to it"
                );
                if let Err(e) = self.hand_back_to_scheduler(session_id).await {
                    // Not `delegated`: nothing was handed back. A refusal here
                    // reads as "I could not repair this log", which is exactly
                    // what the `refused` bucket says and exactly what a
                    // `delegated` counter would hide.
                    tracing::warn!(
                        session = ?session_id,
                        error = %e,
                        "resume: delegated hand-back failed; leaving the marker open"
                    );
                    report.refused.push((
                        session_id.clone(),
                        ResumeRefusal::BoundaryRepairFailed(e.to_string()),
                    ));
                    return;
                }
                report.delegated += 1;
            }
            Ok(RunDisposition::Interrupted { attempts }) => {
                self.handle_interrupted(session_id, attempts, report).await;
            }
            Err(c) => self.refuse_log(session_id, c, report),
        }
    }

    /// File a session under `refused` for a log the reducer (or the store's
    /// decoder) would not read.
    ///
    /// A refused log is "I do not know", not "clean": it is deliberately NOT
    /// counted as `skipped` (which `status_of` renders `already_finished`).
    /// It goes in the `refused` bucket, which `status_of` reads BEFORE every
    /// counter that could be mistaken for a verdict.
    fn refuse_log(
        &self,
        session_id: &SessionId,
        contradiction: LogContradiction,
        report: &mut ResumeReport,
    ) {
        tracing::warn!(
            session = ?session_id,
            kind = contradiction.tag(),
            contradiction = %contradiction,
            "resume: session log refused; not resuming"
        );
        report.refused.push((
            session_id.clone(),
            ResumeRefusal::LogInconsistent(contradiction),
        ));
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
    /// A session with no run markers at all is still asked the §5.2 question
    /// — did its last user message go unanswered? — under a claimed slot.
    /// When the answer is no, the zero report (`scanned == 0`) is what the
    /// caller renders as "nothing to resume" — not an error, because "this
    /// session never ran anything" is a legitimate answer to the question.
    pub async fn resume_session(
        &self,
        session_id: &SessionId,
    ) -> Result<ResumeReport, crate::session::service::SessionError> {
        let mut report = ResumeReport::default();
        let groups = self.event_store.load_run_markers().await?;
        // The same `max_concurrent` permit the boot scan's tasks hold, taken
        // here for the same span: repair + re-trigger, on either arm below.
        // `retrigger` takes none of its own (see `semaphore`).
        let _permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| SessionError::Other(format!("resume semaphore closed: {e}")))?;
        let Some((_, slice)) = groups.into_iter().find(|(sid, _)| sid == session_id) else {
            let Some(_slot) = self.try_claim_resume(session_id) else {
                report.busy += 1;
                return Ok(report);
            };
            if self.check_unanswered(session_id, &[], &mut report).await {
                report.scanned += 1;
            }
            return Ok(report);
        };
        self.resume_from_markers(session_id, &slice, &mut report)
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
    /// the recency filter, the cap check, the crash-boundary repair, the
    /// intent stamp and the re-trigger — every one of them reading that same
    /// reduction.
    ///
    /// The repair used to re-read and re-reduce the log itself, so "what state
    /// is this candidate in" was answered twice per candidate, at two moments,
    /// with an append in between. Two derivations of one fact is the shape
    /// this round exists to remove.
    ///
    /// `attempts` is the number of `ResumeAttempted` stamps this coordinator
    /// (or a previous boot's) already wrote for the open run — the §5.1
    /// ratchet. It is compared against `max_attempts` BEFORE this boot writes
    /// its own stamp, so the cap is "this many tries have been made", not
    /// "this many tries plus the one about to happen".
    async fn handle_interrupted(
        &self,
        session_id: &SessionId,
        attempts: u32,
        report: &mut ResumeReport,
    ) {
        if self.defer_if_running(session_id, report) {
            return;
        }
        let events = match self.event_store.load_all_events(session_id).await {
            Ok(events) => events,
            // A row this build cannot decode is the log refusing to be read,
            // under the same kind the marker scan names — not a failed repair,
            // which is what the arm below says.
            Err(SessionError::UndecodableRecord(u)) => {
                self.refuse_log(session_id, LogContradiction::from(&u), report);
                return;
            }
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
                self.refuse_log(session_id, c, report);
                return;
            }
        };
        report.contradictions += reduction.contradictions.len();

        // The run being resumed: the last `RunStarted`, read off the SAME
        // reduction everything below reads — not "the last marker", which
        // since §5.1 is usually the newest `ResumeAttempted` stamp. Its record
        // dates the run (`last_alive_at`) and its seq is the stamp's target.
        // The disposition guarantees a `RunStarted` after the last finish, so
        // this cannot miss; if the two reads of the log ever disagree, refuse
        // rather than resume a run whose anchor nobody can name.
        let Some(run_started) = reduction
            .run_anchor
            .and_then(|seq| events.iter().find(|r| r.seq == seq))
        else {
            report.refused.push((
                session_id.clone(),
                ResumeRefusal::IntentStampFailed("interrupted run has no RunStarted anchor".into()),
            ));
            return;
        };

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

        // Recency filter — abandon runs interrupted too long ago. Dated by the
        // run's own activity and its `RunStarted`; this boot's predecessors'
        // stamps do not keep it young.
        let age_ms = now_ms().saturating_sub(last_alive_at(&reduction, run_started));
        if age_ms > (self.config.max_age_secs as i64).saturating_mul(1000) {
            tracing::info!(
                session = ?session_id,
                age_ms,
                "resume: candidate too old; abandoning"
            );
            self.abandon(
                session_id,
                Abandoned::InterruptedRun,
                reduction.open_run.as_ref().map(|o| o.run_id.as_str()),
                "the interrupted run was too old to resume safely",
            )
            .await;
            report.abandoned += 1;
            return;
        }

        // Cap check — abandon crash-looped runs. Counted off the intent
        // stamps, so a resume that died before its run's own `RunStarted`
        // still spent one of these.
        if attempts >= self.config.max_attempts {
            tracing::warn!(
                session = ?session_id,
                attempts,
                max_attempts = self.config.max_attempts,
                "resume: crash-loop cap reached; abandoning"
            );
            self.abandon(
                session_id,
                Abandoned::InterruptedRun,
                reduction.open_run.as_ref().map(|o| o.run_id.as_str()),
                "it kept crashing on every resume attempt",
            )
            .await;
            report.abandoned += 1;
            return;
        }

        // ④ Everything the resume replays, derived from the SAME `open_run`
        // the repair is about to answer against: the project folder, the three
        // replayable knobs, the tier ceiling and the validated model pin.
        let plan = plan_resume(
            reduction.open_run.as_ref(),
            crate::providers::session_model_handle::pinnable_providers(),
            &|p| p.is_dir(),
        );
        if plan.degraded {
            report.degraded += 1;
        }
        if plan.unsnapshotted {
            report.unsnapshotted += 1;
        }

        // Crash-boundary repair — append a synthetic ToolError for every
        // dangling call THIS reduction names, so the model sees each one
        // answered instead of silently dropped from the replay. The degrade
        // note rides on the first of them.
        match crate::session::boundary_repair::repair_boundary(
            self.event_store.as_ref(),
            session_id,
            &reduction,
            plan.degrade.as_ref(),
        )
        .await
        {
            // A degrade with no dangling call has no repair to ride on. It is
            // still a fact the model needs — the alternative is a run that
            // silently comes back on a different model — so it gets its own
            // carrier rather than being dropped for want of one.
            Ok(repair) => {
                if repair.appended == 0 {
                    if let Some(note) = plan.degrade.as_ref() {
                        self.announce_degrade(session_id, note).await;
                    }
                }
            }
            Err(e) => {
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
        }

        // §5.1: stamp the intent BEFORE the retrigger. The stamp is what the
        // next boot counts, so a crash anywhere between here and the resumed
        // run's own `RunStarted` (admit / hook / seed) still moves the
        // ratchet. A stamp that did not land is a refusal, not a warning: a
        // retrigger without its stamp is the unbounded loop this closes.
        // `saturating_add`: the ordinal must not depend on the cap check
        // above having run first.
        if let Err(e) = self
            .stamp_resume_attempt(session_id, run_started.seq, attempts.saturating_add(1))
            .await
        {
            tracing::warn!(
                session = ?session_id,
                error = %e,
                "resume: intent stamp failed; not retriggering"
            );
            report.refused.push((
                session_id.clone(),
                ResumeRefusal::IntentStampFailed(e.to_string()),
            ));
            return;
        }

        // Re-trigger, carrying the plan. `RunRequest.model_override` is the
        // carrier for the model because it never writes back to the session
        // row — the crash-time pin governs THIS run and the `select_model`
        // pick the user made after the crash still governs the next one,
        // which is exactly the promise `select_model` prints to the model.
        match self.retrigger(session_id, &plan).await {
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

    /// The Clean arm's second question, and the whole question for a session
    /// with no markers: is there a user message nobody answered (§5.2)?
    ///
    /// One bounded read past the last `RunFinished` the markers carry (the
    /// whole log when they carry none) — past the last FINISH, not the last
    /// marker: a previous boot's stamp is a marker too, and a read that began
    /// after it would never see the message it stamped, so the seed would
    /// hide behind its own stamp on every later boot. The markers before that
    /// finish plus the tail, filtered to what `reduce_disposition` accepts,
    /// are handed to the reducer so the derivation stays its own.
    ///
    /// Returns whether an unanswered tail was found (and acted on, or refused
    /// for a reason the report names) — `false` means "answered, or nothing
    /// there", which the caller files under `skipped` or not at all.
    async fn check_unanswered(
        &self,
        session_id: &SessionId,
        markers: &[SessionEventRecord],
        report: &mut ResumeReport,
    ) -> bool {
        if !unanswered_eligible(session_id) {
            return false;
        }
        let from = markers
            .iter()
            .rev()
            .find(|m| matches!(m.event, SessionEvent::RunFinished { .. }))
            .map(|m| m.seq + 1);
        let tail = match self
            .event_store
            .load_events_range(session_id, from, None)
            .await
        {
            Ok(tail) => tail,
            // A row this build cannot decode: the log refused to be read,
            // under the same kind every other face names for it.
            Err(SessionError::UndecodableRecord(u)) => {
                self.refuse_log(session_id, LogContradiction::from(&u), report);
                return true;
            }
            // Not `BoundaryRepairFailed`: no repair was attempted. The answer
            // is "I cannot tell whether the last message was answered", and
            // the refusal says exactly that.
            Err(e) => {
                tracing::warn!(
                    session = ?session_id,
                    error = %e,
                    "resume: tail read failed; cannot tell whether the last message was answered"
                );
                report.refused.push((
                    session_id.clone(),
                    ResumeRefusal::TailReadFailed(e.to_string()),
                ));
                return true;
            }
        };
        // The markers before the re-read window, then the window itself:
        // disjoint by seq, so no record is counted twice. With no finish in
        // the markers the window is the whole log and the prefix is empty.
        let mut bearing: Vec<SessionEventRecord> = markers
            .iter()
            .filter(|m| from.is_some_and(|f| m.seq < f))
            .cloned()
            .collect();
        bearing.extend(
            tail.into_iter()
                .filter(|r| is_disposition_bearing(&r.event)),
        );
        match reduce_disposition(&bearing) {
            Ok(RunDisposition::Unanswered { user_seq, attempts }) => {
                // The message's own recording time dates it. A record this
                // reducer named but the slice does not hold, or one recorded
                // at 0, is an unknown age — not "now" and not "forever ago".
                let user_at = bearing
                    .iter()
                    .find(|r| r.seq == user_seq)
                    .map(|r| r.created_at_ms)
                    .filter(|&at| at > 0);
                self.handle_unanswered(session_id, user_seq, user_at, attempts, report)
                    .await;
                true
            }
            Ok(RunDisposition::Clean) => false,
            // Unreachable by construction: every caller reaches here with a
            // marker slice that reduced `Clean`, and the tail past the last
            // finish holds no `RunStarted`. Spelled out rather than folded
            // into "answered": a slice that says otherwise is worth a line.
            Ok(RunDisposition::Interrupted { .. }) => {
                tracing::warn!(
                    session = ?session_id,
                    "resume: the message tail reads as interrupted after the marker slice read clean; leaving it to the marker scan"
                );
                false
            }
            Err(c) => {
                self.refuse_log(session_id, c, report);
                true
            }
        }
    }

    /// `Interrupted` minus the boundary repair: nothing dangled, so nothing is
    /// owed a receipt; recency is the message's own recording time; the plan
    /// is empty because no `RunStarted` ever froze an envelope — which is NOT
    /// `unsnapshotted` (that counter is about markers that exist). The stamp
    /// names the message and is written BEFORE the retrigger, and the cap
    /// reads the stamps written after the message, exactly as the interrupted
    /// arm reads its own.
    async fn handle_unanswered(
        &self,
        session_id: &SessionId,
        user_seq: EventSeq,
        user_at: Option<Timestamp>,
        attempts: u32,
        report: &mut ResumeReport,
    ) {
        if self.defer_if_running(session_id, report) {
            return;
        }
        let Some(user_at) = user_at else {
            tracing::warn!(
                session = ?session_id,
                user_seq,
                "resume: unanswered message has no usable recording time; its age is unknown, leaving it alone"
            );
            report.skipped_unknown_age += 1;
            return;
        };
        let age_ms = now_ms().saturating_sub(user_at);
        if age_ms > (self.config.max_age_secs as i64).saturating_mul(1000) {
            tracing::info!(
                session = ?session_id,
                age_ms,
                "resume: unanswered message too old; abandoning"
            );
            self.abandon(
                session_id,
                Abandoned::UnansweredMessage,
                None,
                "the unanswered message was too old to retry safely",
            )
            .await;
            report.abandoned += 1;
            return;
        }
        if attempts >= self.config.max_attempts {
            tracing::warn!(
                session = ?session_id,
                attempts,
                max_attempts = self.config.max_attempts,
                "resume: unanswered message hit the crash-loop cap; abandoning"
            );
            self.abandon(
                session_id,
                Abandoned::UnansweredMessage,
                None,
                "it kept crashing before the run could start",
            )
            .await;
            report.abandoned += 1;
            return;
        }
        if let Err(e) = self
            .stamp_resume_attempt(session_id, user_seq, attempts.saturating_add(1))
            .await
        {
            tracing::warn!(
                session = ?session_id,
                error = %e,
                "resume: intent stamp failed; not retriggering the unanswered message"
            );
            report.refused.push((
                session_id.clone(),
                ResumeRefusal::IntentStampFailed(e.to_string()),
            ));
            return;
        }
        match self.retrigger(session_id, &ResumePlan::default()).await {
            Ok(()) => report.resumed += 1,
            Err(refusal) => {
                tracing::warn!(
                    session = ?session_id,
                    error = %refusal,
                    "resume: re-trigger of the unanswered message failed"
                );
                report.refused.push((session_id.clone(), refusal));
            }
        }
    }

    /// Terminate an abandoned candidate honestly: emit `RunFinished {
    /// Abandoned }` so the run is not re-scanned on the next boot, block any
    /// active goal in the session (its crash recovery hangs ENTIRELY on this
    /// coordinator's retrigger→post_run chain — abandoning severs it, so an
    /// Active goal would otherwise lie in `goal(list)` forever), and say so
    /// to the user — in-band, after the closer, and on the origin channel.
    /// Every step is best-effort and independent — except the in-band note,
    /// which waits for its closer (below); a failed marker append must not
    /// silence the channel notice.
    ///
    /// Deliberately does NOT touch loop state: loops are process-memory and
    /// the registry is empty at boot; "stopping" one here could only misfire
    /// against a loop the user started while the scan was still running.
    ///
    /// `what` names the thing being given up on — the two arms write the
    /// same closer but must not say the same sentence to the user.
    ///
    /// `closes` is the open run's own id when there is one — the same
    /// derivation `marker_balance` and `boundary_repair` use — so a by-id
    /// reader pairs the closer with its opener. The unanswered arm has no open
    /// run (it gives up on a user message nobody answered), so its closer keeps
    /// a synthesized id that pairs with nothing, by design.
    async fn abandon(
        &self,
        session_id: &SessionId,
        what: Abandoned,
        closes: Option<&str>,
        reason: &str,
    ) {
        let ev = SessionEvent::RunFinished {
            run_id: closes.map_or_else(
                || format!("abandoned-{}", uuid::Uuid::new_v4()),
                str::to_string,
            ),
            outcome: RunOutcome::Abandoned,
            at: now_ms(),
        };
        let closed = match self.next_seq(session_id).await {
            Ok(seq) => match self
                .event_store
                .append(session_id, seq, &ev, now_ms())
                .await
            {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(session = ?session_id, error = %e, "resume: abandon marker append failed");
                    false
                }
            },
            Err(e) => {
                // Don't fabricate seq 1 on a read error — that would append the
                // abandon marker at the head and overwrite the genuine first
                // event. Skip the best-effort marker; the next boot re-abandons.
                tracing::warn!(session = ?session_id, error = %e, "resume: abandon seq allocation failed; skipping marker");
                false
            }
        };

        // Scope the block to goals whose recovery actually hung on THIS
        // crashed run: Active-pursuit, not parked on a task barrier (those are
        // woken by GoalWakeService, and a passive goal never depended on the
        // continuation chain). A healthy parked or interactive goal must not
        // be collateral-blocked by an unrelated abandoned run.
        let goal_blocked = crate::gateway::continuation_lifecycle::block_abandonable_session_goal(
            &session_id.to_key_string(),
            &what.goal_note(reason),
        );

        // One sentence, derived once, for both faces it reaches.
        let mut text = what.notice(reason);
        if goal_blocked {
            text.push_str(" Its standing goal was blocked — re-set it to continue.");
        }

        // In-band, and only behind a closer that landed: the note must read
        // AFTER the verdict it is about, and a note written without its
        // closer would be written again by the next boot's re-abandon.
        if closed {
            if let Err(e) = self.system_note(session_id, text.clone()).await {
                tracing::warn!(session = ?session_id, error = %e, "resume: abandon note append failed");
            }
        }

        // One-line origin notice, mirroring `retrigger`'s fanout resolution.
        // Panel-only sessions (`gui:chat`) have no origin route and rely on
        // the in-band note and the stored blocked note; a missing agent cannot
        // be routed for at all (same documented limitation as the engine's
        // agent-miss branch).
        if let Some(reg) = crate::gateway::event_emitter::origin_fanout::channel_registry() {
            if let Some(agent) = self.agent_registry.get(session_id.agent_id()).await {
                if let Some((channel, conversation)) = agent.origin_route(session_id).await {
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

    /// §5.1: write the intent BEFORE the action. `append` (A1) makes this a
    /// Barrier commit, so a crash one instruction later still counts.
    ///
    /// `target` is the seq of the `RunStarted` being resumed, or of the
    /// unanswered `UserMessage` being retried (§5.2); `attempt` is this
    /// stamp's ordinal for that target (the reducer counts the stamps, not
    /// this number — the number is for the operator reading the log).
    async fn stamp_resume_attempt(
        &self,
        session_id: &SessionId,
        target: EventSeq,
        attempt: u32,
    ) -> Result<(), crate::session::service::SessionError> {
        let seq = self.next_seq(session_id).await?;
        self.event_store
            .append(
                session_id,
                seq,
                &SessionEvent::ResumeAttempted { target, attempt },
                now_ms(),
            )
            .await
    }

    /// Tell the model what this resume gave up, when there was no dangling
    /// call for the note to ride on.
    ///
    /// Best-effort: a failed append leaves the run resumable, and refusing to
    /// resume because a *notice* could not be written would trade a whole
    /// recovered conversation for a sentence.
    async fn announce_degrade(
        &self,
        session_id: &SessionId,
        note: &crate::session::boundary_repair::DegradeNote,
    ) {
        if let Err(e) = self.system_note(session_id, note.sentence.clone()).await {
            tracing::warn!(session = ?session_id, error = %e, "resume: degrade notice append failed");
        }
    }

    /// The one way this coordinator says something in-band: a `SystemMessage`
    /// appended at the head of the session's log, then the projector asked to
    /// paint it now rather than at the next boot's reconcile. Never a direct
    /// write into `messages` — that table has one writer, the projector, and
    /// a second one is how a boot notice used to reach the transcript without
    /// ever reaching the log.
    ///
    /// A fresh turn id: whatever turn was open when the process died is over,
    /// and this sentence is about what happens next, not about that turn.
    ///
    /// `Err` means the event did not land — the caller decides what that
    /// costs. A repaint that could not be delivered is only logged: the event
    /// is durable, and the transcript catches up at the next reconcile.
    async fn system_note(
        &self,
        session_id: &SessionId,
        content: String,
    ) -> Result<(), SessionError> {
        let seq = self.next_seq(session_id).await?;
        let ev = SessionEvent::SystemMessage {
            turn_id: crate::session::events::TurnId::new_v4(),
            content,
            at: now_ms(),
        };
        self.event_store
            .append(session_id, seq, &ev, now_ms())
            .await?;
        if let Some(projector) = crate::gateway::session_projector::global_message_projector() {
            let repaint = projector.request_repair(session_id).await;
            // A whole-session pass: it also stamps and bills every finished
            // run on this session whose meta never landed (a run this
            // coordinator closes as `Abandoned` is one), and that happens
            // after the reconciler's boot line was printed, so it is said
            // here.
            if repaint.stamps_synthesized > 0 {
                tracing::info!(
                    session = ?session_id,
                    seq,
                    stamps_synthesized = repaint.stamps_synthesized,
                    usage_rebilled = repaint.usage_rebilled,
                    "resume: repaint stamped finished runs whose meta never landed"
                );
            }
            if repaint.errored || repaint.legacy {
                tracing::warn!(
                    session = ?session_id,
                    seq,
                    errored = repaint.errored,
                    legacy = repaint.legacy,
                    stamps_synthesized = repaint.stamps_synthesized,
                    "resume: system note appended but not painted into the transcript yet"
                );
            }
        }
        Ok(())
    }

    /// §8.2(b): examine every Main-lane task row the engine wrote before a
    /// seed that may never have reached the log, and stamp each one so it is
    /// examined exactly once.
    ///
    /// The row's `id` is the gateway `run_id`, which is NOT the `RunStarted`
    /// run id, so "no events for this task" is derived by TIME: a
    /// `UserMessage` recorded at or after the row's `created_at` means the
    /// seed landed. Three arms, one stamp:
    ///
    /// * the session has an open run, or the seed landed — the resume arm
    ///   owns it; nothing to say;
    /// * neither, and the row carries a prompt — the message is gone, and the
    ///   user is told to re-send it, once;
    /// * neither, and the prompt is empty — a resume re-trigger's own row
    ///   (`retrigger` sends `input: ""`); there is no message to re-send.
    ///
    /// A log this build cannot read or the reducer refuses is NOT stamped:
    /// the doctor is its exit, and the age window bounds how often it is
    /// re-asked.
    async fn adjudicate_orphaned_tasks(&self, report: &mut ResumeReport) {
        let Some(db) = self.state_database.as_ref() else {
            return;
        };
        let window = i64::try_from(self.config.max_age_secs).unwrap_or(i64::MAX);
        let since = (now_ms() / 1000).saturating_sub(window);
        let rows = match db.unadjudicated_interrupted_tasks(since).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "resume: orphaned task rows unreadable");
                return;
            }
        };
        for task in rows {
            // `from_key_string`, not `parse`: this is a key somebody else
            // PERSISTED, so it must accept every spelling the store ever
            // wrote — `parse` alone rejects the legacy form, and the oldest
            // conversations are exactly the ones that carry it.
            let Some(key) = SessionId::from_key_string(&task.parent_session_id) else {
                tracing::warn!(
                    task_id = %task.id,
                    session = %task.parent_session_id,
                    "resume: orphaned task has an unparseable session key"
                );
                continue;
            };
            let Ok(events) = self.event_store.load_all_events(&key).await else {
                continue;
            };
            let Ok(reduction) = reduce_run(&events) else {
                continue;
            };
            let open = !matches!(reduction.disposition, RunDisposition::Clean);
            let seed_floor_ms = task.created_at.saturating_mul(1000);
            let seeded = events.iter().any(|r| {
                matches!(r.event, SessionEvent::UserMessage { .. })
                    && r.created_at_ms >= seed_floor_ms
            });
            if !open && !seeded && !task.task_prompt.trim().is_empty() {
                match self
                    .system_note(&key, lost_input_notice(task.created_at, &task.task_prompt))
                    .await
                {
                    Ok(()) => {
                        tracing::info!(
                            task_id = %task.id,
                            session = ?key,
                            "resume: a message was lost before it was recorded; the user was asked to re-send it"
                        );
                        report.notified += 1;
                    }
                    Err(e) => {
                        // Not stamped: the next boot tries again, within the window.
                        tracing::warn!(task_id = %task.id, session = ?key, error = %e, "resume: lost-input notice append failed");
                        continue;
                    }
                }
            }
            if let Err(e) = db.mark_task_adjudicated(&task.id, now_ms()).await {
                tracing::warn!(task_id = %task.id, error = %e, "resume: adjudicated stamp failed");
            }
        }
    }

    /// Re-trigger an interrupted run. Resolves the agent from the session
    /// key, builds a `RunRequest` with `metadata["resume"] = "true"` (the
    /// engine→orchestrator boundary converts that into `FlowInput::Resume`,
    /// which skips re-seeding), and dispatches it through the same
    /// `ExecutionAdapter` cron / heartbeat use.
    ///
    /// Takes no `max_concurrent` permit of its own: the caller already holds
    /// one for the whole resume (see `semaphore`), and a nested acquire here
    /// deadlocks as soon as `max_concurrent` candidates each hold a permit
    /// and wait for a second.
    async fn retrigger(
        &self,
        session_id: &SessionId,
        plan: &ResumePlan,
    ) -> Result<(), ResumeRefusal> {
        let workspace_override = plan.workspace.clone();

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
        // ④ The crashed run's knobs. `extend` after the identity stamp so a
        // replayed knob can never overwrite the caller-role / scope keys the
        // stamp exists to restore — the knob keys are disjoint from those, and
        // this ordering keeps that true by construction rather than by
        // inspection.
        metadata.extend(plan.knobs.iter().map(|(k, v)| (k.clone(), v.clone())));
        // A replayed `/btw` stamp makes this resume a side question. Read
        // here, before `metadata` moves into the request, because the
        // emitter choice below depends on it.
        let is_side_question = metadata.contains_key(crate::gateway::btw::BTW_METADATA_KEY);

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
            model_override: plan.model_override.clone(),
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
        let emitter = retrigger_emitter(
            base,
            crate::gateway::event_emitter::origin_fanout::channel_registry(),
            agent.origin_route(session_id),
            is_side_question,
        )
        .await;

        tracing::info!(session = ?session_id, agent_id, "resume: re-triggering interrupted run");

        self.execution_adapter
            .execute(request, agent, emitter)
            .await
            .map_err(|e| ResumeRefusal::RetriggerFailed(format!("resume execute failed: {e}")))
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

    /// Every counter the report carries is SUMMED by `absorb`, and `refused`
    /// is appended — a counter it forgot would undercount the boot line for
    /// every fanned-out scan and nothing else would notice (criterion #6).
    ///
    /// Every field is 1 so a forgotten one reads 0, not a default that
    /// happens to match; the second absorb tells a sum from an overwrite.
    #[test]
    fn absorb_sums_every_counter_and_appends_every_refusal() {
        let one = ResumeReport {
            scanned: 1,
            resumed: 1,
            abandoned: 1,
            skipped: 1,
            delegated: 1,
            busy: 1,
            refused: vec![(SessionId::main("absorbed"), ResumeRefusal::AgentMissing)],
            skipped_unknown_age: 1,
            contradictions: 1,
            degraded: 1,
            unsnapshotted: 1,
            notified: 1,
        };
        let mut total = ResumeReport::default();
        total.absorb(one.clone());
        assert_eq!(total, one, "absorbing into an empty report yields the part");
        total.absorb(one.clone());
        // Exhaustive destructure (no `..`): a new counter must be asserted here.
        let ResumeReport {
            scanned,
            resumed,
            abandoned,
            skipped,
            delegated,
            busy,
            refused,
            skipped_unknown_age,
            contradictions,
            degraded,
            unsnapshotted,
            notified,
        } = total;
        assert_eq!(
            [
                scanned,
                resumed,
                abandoned,
                skipped,
                delegated,
                busy,
                skipped_unknown_age,
                contradictions,
                degraded,
                unsnapshotted,
                notified,
            ],
            [2; 11],
            "every counter is a sum, not the last part's value"
        );
        assert_eq!(refused, [one.refused[0].clone(), one.refused[0].clone()]);
    }

    /// The notice quotes the head of the prompt by `char`, so a multibyte
    /// prompt longer than the head is cut between code points, not inside
    /// one — and it dates the loss from the row, not from now.
    #[test]
    fn the_lost_input_notice_quotes_a_char_bounded_head_and_the_rows_time() {
        let prompt: String = "买".repeat(100);
        let note = lost_input_notice(1_700_000_000, &prompt);
        let quoted = note
            .split('«')
            .nth(1)
            .and_then(|s| s.split('»').next())
            .expect("the quoted head");
        assert_eq!(quoted.chars().count(), 80);
        assert!(quoted.chars().all(|c| c == '买'), "{quoted}");
        assert!(note.contains("2023-11-14T22:13:20+00:00"), "{note}");
        assert!(note.contains("lost before it was recorded") && note.contains("re-send"));
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
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
    }

    /// §5.1: the cap counts the coordinator's own `ResumeAttempted` stamps
    /// since the last finish — what was TRIED — not the `RunStarted` markers
    /// those tries happened to leave behind. A stamp with no `RunStarted`
    /// after it is a resume that died before its run opened, and it counts.
    #[test]
    fn classify_counts_stamps_since_last_finish() {
        let stamp = |target: u64, attempt: u32| SessionEvent::ResumeAttempted { target, attempt };
        // Three boots that each stamped intent and crashed before RunStarted.
        let markers = vec![
            rec(1, run_finished(10), 10),
            rec(2, run_started(20), 20),
            rec(3, stamp(2, 1), 30),
            rec(4, stamp(2, 2), 40),
            rec(5, stamp(2, 3), 50),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 3 })
        );
        // Three bare RunStarted: three crashes, zero resumes tried.
        let markers = vec![
            rec(1, run_finished(10), 10),
            rec(2, run_started(20), 20),
            rec(3, run_started(30), 30),
            rec(4, run_started(40), 40),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
        // A finish resets the ratchet.
        let markers = vec![
            rec(1, run_started(10), 10),
            rec(2, stamp(1, 1), 20),
            rec(3, run_finished(30), 30),
            rec(4, run_started(40), 40),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
    }

    #[test]
    fn classify_interrupted_when_no_finish_at_all() {
        let markers = vec![rec(1, run_started(10), 10)];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 0 })
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

    /// The coordinator's own stamp is not activity: a run interrupted long ago
    /// and stamped a moment ago is still dated by its last real event. Read
    /// against the `RunStarted` record (what `handle_interrupted` passes), not
    /// against "the last marker" — since §5.1 the last marker is usually the
    /// newest stamp, which is exactly the record that must not count.
    #[test]
    fn recency_is_not_refreshed_by_an_intent_stamp() {
        let events = vec![
            rec(1, run_started(10), 1_000),
            rec(2, tool_requested("c1"), 2_000),
            rec(
                3,
                SessionEvent::ResumeAttempted {
                    target: 1,
                    attempt: 1,
                },
                900_000,
            ),
        ];
        let reduction = reduce_run(&events).expect("legal log");
        assert_eq!(last_alive_at(&reduction, &events[0]), 2_000);

        let only_stamped = vec![
            rec(1, run_started(10), 1_000),
            rec(
                2,
                SessionEvent::ResumeAttempted {
                    target: 1,
                    attempt: 1,
                },
                900_000,
            ),
        ];
        let reduction = reduce_run(&only_stamped).expect("legal log");
        assert_eq!(
            last_alive_at(&reduction, &only_stamped[0]),
            1_000,
            "nothing happened inside the run; the marker that opened it dates it"
        );
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
            ResumeRefusal::IntentStampFailed("stamp append failed".into()),
            ResumeRefusal::TailReadFailed("range read failed".into()),
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

    // ---- ④ the crash-time envelope --------------------------------------

    fn facts(
        project_root: Option<&str>,
        envelope: Option<crate::session::events::RunEnvelopeSnapshot>,
    ) -> crate::session::reduction::RunStartFacts {
        crate::session::reduction::RunStartFacts {
            seq: 1,
            run_id: "r".to_string(),
            project_root: project_root.map(str::to_string),
            envelope,
        }
    }

    fn envelope_with(
        model: Option<&str>,
        provider: Option<&str>,
        exec_tier: Option<&str>,
    ) -> crate::session::events::RunEnvelopeSnapshot {
        crate::session::events::RunEnvelopeSnapshot {
            exec_tier: exec_tier.map(str::to_string),
            session_mode: Some("code".to_string()),
            think_level: Some("high".to_string()),
            memory_mode: Some("off".to_string()),
            model: model.map(str::to_string),
            model_provider: provider.map(str::to_string),
            allowed_tools: None,
            btw: None,
        }
    }

    fn pinnable(names: &[&str]) -> std::collections::BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// An envelope that never recorded which model served is a DEGRADE, not a
    /// clean resume.
    ///
    /// This is the silent substitution the ruling closes: the resume walks
    /// today's chain, and if the session was re-pinned while the crashed run
    /// sat unresumed, that chain answers with a different model. `unsnapshotted`
    /// cannot report it — the envelope is right here — so before this the fact
    /// had no carrier at all and every face showed a clean recovery.
    ///
    /// The sentence has two origins and must be true for both: a full run
    /// whose chain could not name what served it, and a slash-command fast
    /// path that served on NO model at all. "The model … was not recorded"
    /// asserted a model existed and was lost, which is false for the second
    /// origin — so the wording is pinned here, positively and negatively.
    #[test]
    fn an_envelope_that_never_named_the_model_resumes_degraded_and_says_so() {
        let plan = plan_resume(
            Some(&facts(None, Some(envelope_with(None, None, Some("full"))))),
            None,
            &|_| true,
        );
        assert!(
            !plan.unsnapshotted,
            "the envelope is present; only its model half is missing"
        );
        assert!(
            plan.degraded,
            "an unrecoverable model is something given up"
        );
        let note = plan
            .degrade
            .as_ref()
            .expect("a degrade must carry the sentence the model reads");
        assert!(
            note.sentence.contains("This run recorded no model")
                && note.sentence.contains("fast path serves on none")
                && note
                    .sentence
                    .contains("resumes on this session's current model"),
            "the note must say no model was recorded, name both origins, and \
             say what it resumes on: {}",
            note.sentence
        );
        assert!(
            !note.sentence.contains("was not recorded"),
            "the old wording asserted a model existed and was lost, which is \
             false for a fast-path run: {}",
            note.sentence
        );
        assert!(
            plan.model_override.is_none(),
            "nothing was recorded, so there is nothing to pin"
        );
    }

    /// §6.3 the skill scope is a per-run FACT with one rung: the snapshot.
    /// Present ⇒ it reaches the resumed request under the same key the run
    /// loop decodes; absent ⇒ nothing is stamped (full surface) and nothing
    /// degrades — "declared nothing" is the normal case, not a loss.
    #[test]
    fn a_snapshot_scope_reaches_the_request_and_an_absent_one_stamps_nothing() {
        use crate::gateway::execution_engine::slash_skill_scope::from_metadata;
        let mut env = envelope_with(Some("gpt-5.6"), Some("openai"), Some("full"));
        env.allowed_tools = Some(vec!["file_read".to_string()]);
        let plan = plan_resume(
            Some(&facts(None, Some(env))),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert_eq!(
            from_metadata(&plan.knobs),
            Some(["file_read".to_string()].into_iter().collect())
        );
        assert!(!plan.degraded && !plan.unsnapshotted);

        let plan = plan_resume(
            Some(&facts(
                None,
                Some(envelope_with(Some("gpt-5.6"), Some("openai"), Some("full"))),
            )),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert_eq!(
            from_metadata(&plan.knobs),
            None,
            "no declaration ⇒ full surface"
        );
        assert!(!plan.degraded, "an optional fact never degrades a resume");
    }

    /// The `/btw` stamp is replayed verbatim so the resumed side question
    /// keeps its read-only ceiling — except the promote sentinel: a promote
    /// is not a run. A replayed sentinel would not reach the promote arm
    /// (the resume is addressed to the side session, and that arm only
    /// serves a redirect from a main key); it would run a pointless
    /// side-session turn, so it is refused instead.
    #[test]
    fn a_btw_question_is_replayed_but_a_promote_sentinel_is_not() {
        use crate::gateway::btw::{BTW_METADATA_KEY, PROMOTE_STAMP};
        let mut env = envelope_with(Some("m"), Some("openai"), Some("ask"));
        env.btw = Some("is it green?".into());
        let plan = plan_resume(
            Some(&facts(None, Some(env.clone()))),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert_eq!(
            plan.knobs.get(BTW_METADATA_KEY).map(String::as_str),
            Some("is it green?")
        );

        env.btw = Some(PROMOTE_STAMP.into());
        let plan = plan_resume(
            Some(&facts(None, Some(env))),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert_eq!(
            plan.knobs.get(BTW_METADATA_KEY),
            None,
            "a promote is not a run; replayed on the side session it would be a \
             pointless read-only turn, never the promote arm"
        );
    }

    /// The mirror of `busy_queue/durable.rs`'s rule: a re-triggered run that
    /// carries the side-question stamp rides the bus alone. With a channel
    /// registry AND a bound origin route in hand, the stamped run's final
    /// reply must still reach no channel — a re-delivered side answer must
    /// not land on the origin conversation unmarked — and the route lookup
    /// is never even polled for it; the unstamped twin, same registry, same
    /// route, fans out.
    #[tokio::test]
    async fn a_stamped_resume_skips_the_origin_fan_out_and_an_unstamped_one_takes_it() {
        use crate::gateway::channel::{
            Channel, ChannelCapabilities, ChannelId, ChannelInfo, ChannelResult, ChannelState,
            ChannelStatus, MessageId, OutboundMessage, SendResult,
        };
        use crate::gateway::channel_registry::ChannelRegistry;
        use crate::gateway::event_emitter::{CollectingEventEmitter, RunSummary, StreamEvent};

        struct Seen {
            info: ChannelInfo,
            state: ChannelState,
            seen: Arc<tokio::sync::Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl Channel for Seen {
            fn info(&self) -> &ChannelInfo {
                &self.info
            }
            fn state(&self) -> &ChannelState {
                &self.state
            }
            async fn start(&mut self) -> ChannelResult<()> {
                Ok(())
            }
            async fn stop(&mut self) -> ChannelResult<()> {
                Ok(())
            }
            async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
                self.seen.lock().await.push(message.text.clone());
                Ok(SendResult {
                    message_id: MessageId::new("ok"),
                    timestamp: chrono::Utc::now(),
                })
            }
        }

        let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let registry = Arc::new(ChannelRegistry::new());
        registry
            .register(Box::new(Seen {
                info: ChannelInfo {
                    id: ChannelId::new("origin"),
                    name: "origin".to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: ChannelCapabilities::default(),
                },
                state: ChannelState::new(8),
                seen: seen.clone(),
            }))
            .await;
        // The route lookup, instrumented: `looked_up` flips only if the
        // future is polled, which is the cost a stamped resume must not pay.
        let looked_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let route = |flag: Arc<std::sync::atomic::AtomicBool>| async move {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Some(("origin".to_string(), "conv-1".to_string()))
        };

        let reply = |text: &str| StreamEvent::RunComplete {
            run_id: "r".to_string(),
            seq: 0,
            summary: RunSummary {
                final_response: Some(text.to_string()),
                ..Default::default()
            },
            total_duration_ms: 0,
        };

        let bus = Arc::new(CollectingEventEmitter::new());
        retrigger_emitter(
            bus.clone(),
            Some(registry.clone()),
            route(looked_up.clone()),
            true,
        )
        .await
        .emit(reply("side answer"))
        .await
        .unwrap();
        assert_eq!(
            bus.events().await.len(),
            1,
            "the bus still carries the stamped run's frames"
        );
        assert!(
            seen.lock().await.is_empty(),
            "a stamped resume must not fan its answer out to the origin channel"
        );
        assert!(
            !looked_up.load(std::sync::atomic::Ordering::SeqCst),
            "a stamped resume must not pay for the origin-route lookup"
        );

        retrigger_emitter(bus, Some(registry), route(looked_up.clone()), false)
            .await
            .emit(reply("ordinary answer"))
            .await
            .unwrap();
        assert!(
            looked_up.load(std::sync::atomic::Ordering::SeqCst),
            "the unstamped twin looks the route up"
        );
        assert_eq!(
            seen.lock().await.as_slice(),
            ["ordinary answer".to_string()],
            "the unstamped twin, same registry and route, fans out"
        );
    }

    /// The other half of the same gate: a snapshot that DID name a model must
    /// not collect that sentence. Without this, the branch above could be made
    /// unconditional and every test here would stay green.
    #[test]
    fn a_recorded_model_resumes_without_the_unknown_model_sentence() {
        // A LIVE model, deliberately: `deepseek-chat` is deprecated in the
        // catalog (the Successor test below depends on that), and a degrade
        // from the lifecycle arm would have made this test pass for the wrong
        // reason — it would no longer separate "recorded" from "not recorded".
        let plan = plan_resume(
            Some(&facts(
                None,
                Some(envelope_with(Some("gpt-5.6"), Some("openai"), None)),
            )),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert!(
            !plan.degraded,
            "a model that is still valid gives nothing up"
        );
        assert!(plan.degrade.is_none());
        assert_eq!(
            plan.model_override
                .as_ref()
                .map(crate::gateway::model_override::ModelOverride::model),
            Some("gpt-5.6")
        );
    }

    /// The three replayable knobs ride on their own metadata keys, and the
    /// tier rides on the CEILING key — never on `exec_tier`, which is the
    /// request rung and would let a resume raise a tightened conversation.
    #[test]
    fn the_snapshot_knobs_reach_the_request_and_the_tier_rides_the_ceiling_key() {
        // The model half is filled in so this stays a test about KNOBS: an
        // envelope with no model is now a degrade in its own right, and the
        // `!plan.degraded` assertion at the bottom would be answering that
        // question instead of this one.
        let plan = plan_resume(
            Some(&facts(
                None,
                Some(envelope_with(Some("gpt-5.6"), Some("openai"), Some("full"))),
            )),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert_eq!(
            plan.knobs
                .get(crate::gateway::execution_engine::RESUME_TIER_CEILING_KEY)
                .map(String::as_str),
            Some("full")
        );
        assert_eq!(
            plan.knobs
                .get(crate::config::types::policies::EXEC_TIER_SESSION_KEY),
            None,
            "the tier must never arrive as the request rung"
        );
        assert_eq!(
            plan.knobs
                .get(crate::config::types::policies::MODE_SESSION_KEY)
                .map(String::as_str),
            Some("code")
        );
        assert_eq!(
            plan.knobs
                .get(crate::agents::thinking::THINK_LEVEL_SESSION_KEY)
                .map(String::as_str),
            Some("high")
        );
        assert_eq!(
            plan.knobs
                .get(crate::memory::session_memory_mode::MEMORY_MODE_SESSION_KEY)
                .map(String::as_str),
            Some("off")
        );
        assert!(!plan.degraded);
        assert!(!plan.unsnapshotted);
        assert!(plan.degrade.is_none());
    }

    /// A live pin is replayed verbatim, as a qualified override.
    #[test]
    fn a_live_snapshot_model_is_replayed_as_a_qualified_override() {
        let plan = plan_resume(
            Some(&facts(
                None,
                Some(envelope_with(Some("gpt-5.6"), Some("openai"), None)),
            )),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert_eq!(
            plan.model_override,
            Some(crate::gateway::model_override::ModelOverride::Qualified {
                provider: "openai".to_string(),
                model: "gpt-5.6".to_string(),
            })
        );
        assert!(!plan.degraded);
    }

    /// A model the catalog retired since the crash comes back on its
    /// successor, and the model is told which and why.
    #[test]
    fn a_retired_snapshot_model_resumes_on_its_successor_and_says_so() {
        let plan = plan_resume(
            Some(&facts(
                None,
                Some(envelope_with(Some("deepseek-chat"), Some("deepseek"), None)),
            )),
            Some(&pinnable(&["deepseek"])),
            &|_| true,
        );
        let note = plan.degrade.expect("a degraded resume carries a sentence");
        assert!(
            note.sentence.contains("deepseek-chat") && note.sentence.contains("resumes on"),
            "{}",
            note.sentence
        );
        assert!(plan.degraded);
        let over = plan.model_override.expect("a successor is still a pin");
        assert_ne!(over.model(), "deepseek-chat");
    }

    /// A pin naming a provider this server no longer has cannot be honoured:
    /// the run falls back to the default chain, degraded and stated.
    #[test]
    fn a_pin_on_an_unconfigured_provider_is_dropped_not_replayed() {
        let plan = plan_resume(
            Some(&facts(
                None,
                Some(envelope_with(Some("m-x"), Some("gone-inc"), None)),
            )),
            Some(&pinnable(&["openai"])),
            &|_| true,
        );
        assert_eq!(plan.model_override, None);
        assert!(plan.degraded);
        assert!(plan.degrade.expect("stated").sentence.contains("gone-inc"));
    }

    /// "No published pinnable set" is *unvalidated*, never "nothing is
    /// pinnable" — the same reading `select_model` takes of the same handle.
    #[test]
    fn an_unpublished_pinnable_set_does_not_drop_the_pin() {
        let plan = plan_resume(
            Some(&facts(
                None,
                Some(envelope_with(Some("m-x"), Some("whatever"), None)),
            )),
            None,
            &|_| true,
        );
        assert!(plan.model_override.is_some());
        assert!(!plan.degraded);
    }

    /// A project folder that has since gone away degrades to the agent
    /// workspace *and* says so (ruling A9) — a silent fallback would write
    /// the recovered run's files where nobody is looking.
    #[test]
    fn a_vanished_project_root_degrades_and_is_stated() {
        let plan = plan_resume(
            Some(&facts(Some("/gone"), Some(envelope_with(None, None, None)))),
            None,
            &|_| false,
        );
        assert_eq!(plan.workspace, None);
        assert!(plan.degraded);
        assert!(plan.degrade.expect("stated").sentence.contains("/gone"));
    }

    /// A marker written before the envelope existed is counted, not assumed
    /// away: the first boot after this ships is what reports the real size of
    /// the pre-envelope backlog.
    #[test]
    fn a_legacy_marker_is_unsnapshotted_and_replays_nothing() {
        let plan = plan_resume(Some(&facts(Some("/p"), None)), None, &|_| true);
        assert!(plan.unsnapshotted);
        assert!(plan.knobs.is_empty());
        assert_eq!(plan.model_override, None);
        assert_eq!(plan.workspace, Some(std::path::PathBuf::from("/p")));
    }

    /// No `open_run` at all — the ③-D2 shape where the `RunStarted` append
    /// failed and the run executed anyway. Nothing to replay; today's values
    /// apply, and the count says so.
    #[test]
    fn a_missing_open_run_is_unsnapshotted_rather_than_invented() {
        let plan = plan_resume(None, None, &|_| true);
        assert!(plan.unsnapshotted);
        assert!(plan.knobs.is_empty());
        assert_eq!(plan.workspace, None);
        assert!(plan.degrade.is_none());
    }

    /// Two degradations in one resume are one note, not one that wins.
    #[test]
    fn a_resume_that_loses_two_things_says_both() {
        let plan = plan_resume(
            Some(&facts(
                Some("/gone"),
                Some(envelope_with(Some("m-x"), Some("gone-inc"), None)),
            )),
            Some(&pinnable(&["openai"])),
            &|_| false,
        );
        let s = plan.degrade.expect("stated").sentence;
        assert!(s.contains("/gone"), "{s}");
        assert!(s.contains("m-x"), "{s}");
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

    /// The Unanswered arm retriggers a conversation somebody is waiting on:
    /// not a unit that re-runs by its own rule, and not a sub-agent child or
    /// an ephemeral side session (A7: children are reported, never re-driven;
    /// nobody waits on an ephemeral session).
    #[test]
    fn unanswered_eligibility_excludes_scheduled_child_and_ephemeral_sessions() {
        use crate::routing::session_key::DmScope;

        for key in [
            SessionId::main("alice"),
            SessionId::dm("alice", "telegram", "u1", DmScope::PerPeer),
            SessionId::task("main", "webhook", "hook-1"),
        ] {
            assert!(
                unanswered_eligible(&key),
                "{} is a conversation with a user waiting on it",
                key.to_key_string()
            );
        }
        for key in [
            SessionId::task("main", CRON_TASK_TYPE, "daily-summary"),
            SessionId::task("main", HEARTBEAT_TASK_TYPE, "hb-1"),
            SessionId::subagent(SessionId::main("alice"), "child-1"),
            SessionId::ephemeral("alice"),
        ] {
            assert!(
                !unanswered_eligible(&key),
                "{} must not be retriggered for an unanswered seed",
                key.to_key_string()
            );
        }
    }

    /// The two things `abandon` gives up on get two true sentences. The
    /// unanswered arm must never tell the user an "interrupted run" was
    /// abandoned in a session where no run existed (criterion #17: the wrong
    /// label is dearer than the missing one), and both must carry the reason.
    #[test]
    fn the_abandon_sentences_name_the_thing_that_was_abandoned() {
        let run = Abandoned::InterruptedRun.notice("too old");
        let message = Abandoned::UnansweredMessage.notice("too old");
        assert!(
            run.contains("interrupted run") && run.contains("too old"),
            "{run}"
        );
        assert!(
            !message.contains("interrupted run") && message.contains("never picked up"),
            "{message}"
        );
        assert!(message.contains("too old"), "{message}");
        assert_ne!(run, message);

        let run_goal = Abandoned::InterruptedRun.goal_note("capped");
        let message_goal = Abandoned::UnansweredMessage.goal_note("capped");
        assert!(run_goal.contains("interrupted run") && run_goal.contains("capped"));
        assert!(
            !message_goal.contains("interrupted run") && message_goal.contains("capped"),
            "{message_goal}"
        );
        assert!(
            run_goal.contains("Re-set the goal") && message_goal.contains("Re-set the goal"),
            "both tell the user what to do next"
        );
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
