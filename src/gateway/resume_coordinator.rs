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

use tokio::sync::Semaphore;

use crate::config::types::ResumeConfig;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_emitter::CollectingEventEmitter;
use crate::gateway::execution_adapter::ExecutionAdapter;
use crate::gateway::execution_engine::RunRequest;
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

    /// Emit `RunFinished { Abandoned }` so a terminal run is not re-scanned
    /// on the next boot. Best-effort.
    async fn abandon(&self, session_id: &SessionId) {
        let ev = SessionEvent::RunFinished {
            run_id: format!("abandoned-{}", uuid::Uuid::new_v4()),
            outcome: RunOutcome::Abandoned,
            at: now_ms(),
        };
        let seq = match self.next_seq(session_id).await {
            Ok(seq) => seq,
            Err(e) => {
                // Don't fabricate seq 1 on a read error — that would append the
                // abandon marker at the head and overwrite the genuine first
                // event. Skip the best-effort marker instead.
                tracing::warn!(session = ?session_id, error = %e, "resume: abandon seq allocation failed; skipping marker");
                return;
            }
        };
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
        use std::collections::HashMap;

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
