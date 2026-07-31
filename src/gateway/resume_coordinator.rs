//! `ResumeCoordinator` — boot-scan auto-resume of interrupted agent runs.
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

use crate::sync_primitives::Arc;

use std::collections::HashMap;

use tokio::sync::Semaphore;

use crate::config::types::ResumeConfig;
use crate::gateway::agent_instance::{AgentInstance, AgentRegistry};
use crate::gateway::event_emitter::CollectingEventEmitter;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::{RunRequest, UNATTENDED_KEY};
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

/// Walk a full session event log and return a synthetic `ToolError` for
/// every `ToolCallRequested` whose `call_id` has no matching `ToolResult`
/// or `ToolError`. The returned events are ready to append to the log; the
/// caller emits them in order. An already-answered call yields nothing.
pub(crate) fn compute_boundary_repairs(events: &[SessionEventRecord]) -> Vec<SessionEvent> {
    use std::collections::HashSet;

    let mut answered: HashSet<&str> = HashSet::new();
    for record in events {
        match &record.event {
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
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
            } if !answered.contains(call_id.as_str()) => Some(SessionEvent::ToolError {
                turn_id: *turn_id,
                call_id: call_id.clone(),
                error: "interrupted by server restart".to_string(),
                at,
            }),
            _ => None,
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
                    let project_root = latest_project_root(&markers);
                    self.handle_interrupted(
                        &session_id,
                        &markers,
                        trailing_starts,
                        project_root,
                        &mut report,
                    )
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
        project_root: Option<std::path::PathBuf>,
        report: &mut ResumeReport,
    ) {
        // The dangling RunStarted is the last marker (classify_markers
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

        let mut metadata: HashMap<String, String> = HashMap::new();
        metadata.insert("resume".to_string(), "true".to_string());
        if let Some(p) = workspace_override.as_ref() {
            metadata.insert("project_root".to_string(), p.display().to_string());
        }
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

        let collector = Arc::new(CollectingEventEmitter::new());
        let base: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            Arc::clone(&collector) as _;
        // Fan the recovered run's final reply out to the session's bound
        // origin channel — the human who asked. Without this the resumed run
        // completed into a collect-and-drop emitter: the crash-recovered
        // answer existed only in the session log and the Telegram/Slack user
        // never heard back (R5). Mirrors `spawn_continuation_run`; a Panel-
        // only session (`gui:chat`, no origin route) keeps the bare collector.
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
        }
    }

    fn run_started_with_project(at: i64, project: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: format!("r-{at}"),
            at,
            project_root: Some(project.to_string()),
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
