//! Borrowed terminal observation over the existing PTY and agent stores.
//!
//! This module is deliberately a borrowed adapter: it owns no session registry,
//! agent table, clock, or sampler. The gateway and tool faces provide their
//! caller kind explicitly, so actorless admission cannot accidentally widen
//! when one face is routed through another.

use std::time::Duration;

use aleph_protocol::pty::{PtyAttachResponse, PtyListResponse};
use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState, RuntimeAgentsListResponse};
use aleph_protocol::terminal::{
    TerminalExplainInputs, TerminalExplainResponse, TerminalExplainRule, TerminalReadResponse,
    TerminalWaitOutcome, TerminalWaitParams, TerminalWaitResponse,
};
use tokio_util::sync::CancellationToken;

use crate::gateway::runtime::RuntimeAgents;
use crate::tools::service::ToolError;

use super::{owner_admits, PtyManager, SessionOwner};

const WAIT_DEFAULT_TIMEOUT_MS: u64 = 60_000;
const WAIT_MAX_TIMEOUT_MS: u64 = 150_000;
const EXPLAIN_SCREEN_TAIL_LINES: usize = 12;
const WAIT_DEFAULT_UNTIL: [RuntimeAgentState; 2] =
    [RuntimeAgentState::Blocked, RuntimeAgentState::Idle];

/// The face that supplied an observation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObservationCaller {
    Gateway { actor: Option<String> },
    Tool { actor: Option<String> },
}

impl ObservationCaller {
    /// Gateway's actorless RPC remains the existing unrestricted gateway
    /// admission; the tool's actorless path is intentionally fail-closed to
    /// sessions with no recorded owner.
    fn admits(&self, created_by: Option<&str>) -> bool {
        match self {
            Self::Gateway { actor } => owner_admits(created_by, actor.as_deref()),
            Self::Tool { actor: None } => created_by.is_none(),
            Self::Tool { actor: Some(actor) } => owner_admits(created_by, Some(actor)),
        }
    }
}

pub(crate) fn caller_admits(caller: &ObservationCaller, owner: &SessionOwner) -> bool {
    match owner {
        SessionOwner::Known(created_by) => caller.admits(created_by.as_deref()),
        SessionOwner::Unknown => matches!(caller, ObservationCaller::Gateway { actor: None }),
    }
}

pub(crate) fn terminal_admits(created_by: Option<&str>, actor: Option<&str>) -> bool {
    ObservationCaller::Tool {
        actor: actor.map(str::to_owned),
    }
    .admits(created_by)
}

/// Borrowed runtime over an existing PTY manager and agent table.
pub(crate) struct TerminalRuntime<'a> {
    pty: &'a PtyManager,
    agents: &'a RuntimeAgents,
}

fn execution(cause: String) -> ToolError {
    ToolError::Execution {
        name: "terminal".to_owned(),
        cause,
    }
}

impl<'a> TerminalRuntime<'a> {
    pub(crate) fn new(pty: &'a PtyManager, agents: &'a RuntimeAgents) -> Self {
        Self { pty, agents }
    }

    fn owned(&self, caller: &ObservationCaller, session_id: &str) -> Result<(), ToolError> {
        if session_id.trim().is_empty() || !caller_admits(caller, &self.pty.owner_of(session_id)) {
            return Err(execution(super::no_such_session(session_id)));
        }
        Ok(())
    }

    pub(crate) fn list(&self, caller: &ObservationCaller) -> PtyListResponse {
        PtyListResponse {
            sessions: self
                .pty
                .list()
                .into_iter()
                .filter(|session| caller.admits(session.created_by.as_deref()))
                .map(|session| aleph_protocol::pty::PtySessionInfo::from(&session))
                .collect(),
        }
    }

    pub(crate) fn status(&self, caller: &ObservationCaller) -> RuntimeAgentsListResponse {
        RuntimeAgentsListResponse {
            agents: self
                .agents
                .snapshot()
                .into_iter()
                .filter(|entry| caller_admits(caller, &self.pty.owner_of(&entry.session_id)))
                .collect(),
        }
    }

    pub(crate) fn attach(
        &self,
        caller: &ObservationCaller,
        session_id: &str,
    ) -> Result<PtyAttachResponse, ToolError> {
        self.owned(caller, session_id)?;
        self.pty.attach_snapshot(session_id).map_err(execution)
    }

    pub(crate) fn read(
        &self,
        caller: &ObservationCaller,
        session_id: &str,
    ) -> Result<TerminalReadResponse, ToolError> {
        self.owned(caller, session_id)?;
        self.pty
            .visible_text(session_id)
            .map(|text| TerminalReadResponse {
                session_id: session_id.to_owned(),
                text,
            })
            .map_err(execution)
    }

    pub(crate) async fn wait(
        &self,
        caller: &ObservationCaller,
        params: &TerminalWaitParams,
        cancel: CancellationToken,
    ) -> Result<TerminalWaitResponse, ToolError> {
        self.owned(caller, &params.session_id)?;
        let until = match params.until.as_deref() {
            Some([]) => return Err(ToolError::ValidationFailed {
                name: "terminal".to_owned(),
                cause: "wait requires at least one state in `until` (blocked / idle / working / unknown); omit it for [blocked, idle]".to_owned(),
            }),
            Some(states) => states,
            None => &WAIT_DEFAULT_UNTIL,
        };
        let outcome = wait_for_state(
            self.pty,
            self.agents,
            &params.session_id,
            until,
            wait_window(params.timeout_ms),
            cancel,
        )
        .await?;
        Ok(TerminalWaitResponse {
            session_id: params.session_id.clone(),
            outcome: outcome.wire_outcome(),
            agent: outcome.agent(),
        })
    }

    pub(crate) fn explain(
        &self,
        caller: &ObservationCaller,
        session_id: &str,
    ) -> Result<TerminalExplainResponse, ToolError> {
        self.owned(caller, session_id)?;
        let screen = self.pty.detection_inputs(session_id).map_err(execution)?;
        Ok(explain_response(
            session_id,
            self.agents.detected_agent(session_id),
            self.agents.entry(session_id).as_ref(),
            &screen,
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    Reached(RuntimeAgentEntry),
    Timeout(Option<RuntimeAgentEntry>),
    Gone,
}

impl WaitOutcome {
    fn wire_outcome(&self) -> TerminalWaitOutcome {
        match self {
            Self::Reached(_) => TerminalWaitOutcome::Reached,
            Self::Timeout(_) => TerminalWaitOutcome::Timeout,
            Self::Gone => TerminalWaitOutcome::Gone,
        }
    }

    fn agent(&self) -> Option<RuntimeAgentEntry> {
        match self {
            Self::Reached(entry) => Some(entry.clone()),
            Self::Timeout(entry) => entry.clone(),
            Self::Gone => None,
        }
    }
}

pub(crate) fn wait_window(requested: Option<u64>) -> Duration {
    Duration::from_millis(
        requested
            .unwrap_or(WAIT_DEFAULT_TIMEOUT_MS)
            .min(WAIT_MAX_TIMEOUT_MS),
    )
}

fn wait_verdict(
    pty: &PtyManager,
    agents: &RuntimeAgents,
    session_id: &str,
    until: &[RuntimeAgentState],
) -> Option<WaitOutcome> {
    let registered = pty.list().iter().any(|s| s.session_id == session_id);
    if !registered {
        return Some(WaitOutcome::Gone);
    }
    match agents.entry(session_id) {
        Some(entry) if until.contains(&entry.state) => Some(WaitOutcome::Reached(entry)),
        Some(_) | None => None,
    }
}

pub(crate) async fn wait_for_state(
    pty: &PtyManager,
    agents: &RuntimeAgents,
    session_id: &str,
    until: &[RuntimeAgentState],
    window: Duration,
    cancel: CancellationToken,
) -> Result<WaitOutcome, ToolError> {
    let mut changes = agents.subscribe();
    let deadline = tokio::time::Instant::now() + window;
    loop {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled {
                name: "terminal".to_owned(),
            });
        }
        if let Some(outcome) = wait_verdict(pty, agents, session_id, until) {
            return Ok(outcome);
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(ToolError::Cancelled { name: "terminal".to_owned() }),
            changed = changes.changed() => {
                if changed.is_err() {
                    return Ok(wait_verdict(pty, agents, session_id, until)
                        .unwrap_or_else(|| WaitOutcome::Timeout(agents.entry(session_id))));
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Ok(wait_verdict(pty, agents, session_id, until)
                    .unwrap_or_else(|| WaitOutcome::Timeout(agents.entry(session_id))));
            }
        }
    }
}

pub(crate) fn explain_response(
    session_id: &str,
    agent: Option<agent_detect::Agent>,
    sampled: Option<&RuntimeAgentEntry>,
    screen: &super::manager::DetectionInputs,
) -> TerminalExplainResponse {
    let inputs = TerminalExplainInputs {
        title: screen.title.clone(),
        osc_progress: screen.osc_progress.clone(),
        screen_tail: screen_tail(&screen.text),
    };
    let Some(agent) = agent else {
        return TerminalExplainResponse {
            session_id: session_id.to_owned(),
            agent: None,
            state: RuntimeAgentState::Unknown,
            matched_rule: None,
            source: None,
            manifest_version: None,
            reason: Some(match sampled {
                None => "this session has no row in the agent table yet — nothing has been sampled, which is not the same as nothing running".to_owned(),
                Some(entry) => format!(
                    "the foreground program ({}) is not an agent the bundled manifests know",
                    entry.program.as_deref().unwrap_or("not probed")
                ),
            }),
            inputs,
        };
    };
    let explained = agent_detect::manifest::explain_with_input(
        agent,
        agent_detect::screen_rules::detection_input(
            &screen.text,
            &screen.title,
            &screen.osc_progress,
        ),
    );
    TerminalExplainResponse {
        session_id: session_id.to_owned(),
        agent: explained.agent.clone(),
        state: crate::gateway::runtime::wire_state(explained.state),
        matched_rule: explained
            .matched_rule
            .as_ref()
            .map(|rule| TerminalExplainRule {
                id: rule.id.clone(),
                priority: rule.priority,
                region: rule.region.clone(),
                state: crate::gateway::runtime::wire_state(rule.state),
            }),
        source: explained
            .source
            .as_ref()
            .map(|source| source.kind().to_owned()),
        manifest_version: explained.manifest_version.clone(),
        reason: explained
            .warning
            .clone()
            .or_else(|| explained.fallback_reason.clone()),
        inputs,
    }
}

fn screen_tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(EXPLAIN_SCREEN_TAIL_LINES)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::pty::{manager, screen::Screen, SpawnOptions};
    use crate::gateway::runtime::{RuntimeAgents, SampleInput};
    use crate::tools::service::ToolError;
    use aleph_protocol::runtime::RuntimeAgentState;
    use aleph_protocol::terminal::{
        TerminalExplainResponse, TerminalReadResponse, TerminalWaitParams,
    };
    use std::future::Future;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::task::{Context, Poll, Wake, Waker};
    use tokio_util::sync::CancellationToken;

    struct Session<'a>(String, &'a PtyManager);
    impl<'a> Session<'a> {
        fn new(manager: &'a PtyManager, owner: Option<&str>) -> Self {
            Self(
                manager
                    .spawn(&SpawnOptions {
                        created_by: owner.map(str::to_owned),
                        ..Default::default()
                    })
                    .expect("spawn")
                    .session_id,
                manager,
            )
        }
    }
    impl Drop for Session<'_> {
        fn drop(&mut self) {
            let _ = self.1.close(&self.0);
        }
    }

    fn sample(table: &RuntimeAgents, id: &str, working: bool) {
        let mut screen = Screen::new(4, 40);
        if working {
            screen.feed(b"\x1b]9;4;1;-1\x07");
        }
        table.sample(SampleInput {
            session_id: id,
            shell: if working { "grok" } else { "zsh" },
            program: None,
            argv: &[],
            cwd: "",
            screen: &screen,
            process_exited: false,
            frame_produced: true,
            now: 0,
        });
    }

    fn assert_hidden<T: std::fmt::Debug>(result: Result<T, ToolError>, id: &str) {
        match result.expect_err("must not disclose another owner's session") {
            ToolError::Execution { cause, .. } => {
                assert_eq!(cause, super::super::no_such_session(id))
            }
            other => panic!("unexpected refusal: {other:?}"),
        }
    }

    fn params(id: &str, timeout: u64) -> TerminalWaitParams {
        TerminalWaitParams {
            session_id: id.to_owned(),
            until: None,
            timeout_ms: Some(timeout),
        }
    }

    #[test]
    fn explain_wire_uses_real_newlines_and_keeps_the_last_twelve_lines() {
        let text = (1..=14)
            .map(|line| format!("line-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let screen = super::super::manager::DetectionInputs {
            text,
            title: String::new(),
            osc_progress: String::new(),
        };
        let response = explain_response("s-tail", None, None, &screen);
        let wire = serde_json::to_value(response).expect("explain response serializes");
        let tail = wire["inputs"]["screen_tail"]
            .as_str()
            .expect("screen tail is a string");
        assert_eq!(
            tail,
            (3..=14)
                .map(|line| format!("line-{line}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(
            tail.contains('\n'),
            "wire tail must contain actual newlines"
        );
        assert!(
            !tail.contains("\\n"),
            "wire tail must not contain a literal backslash-n"
        );
    }

    #[test]
    #[serial_test::serial(pty_global_manager)]
    fn gateway_actorless_preserves_unknown_owner_admission() {
        let manager = manager();
        let table = RuntimeAgents::default();
        let orphan = "orphan-without-pty";
        sample(&table, orphan, false);
        let runtime = TerminalRuntime::new(manager, &table);

        let gateway = runtime.status(&ObservationCaller::Gateway { actor: None });
        assert!(gateway
            .agents
            .iter()
            .any(|entry| entry.session_id == orphan));
        let identified = runtime.status(&ObservationCaller::Gateway {
            actor: Some("alice".to_owned()),
        });
        assert!(!identified
            .agents
            .iter()
            .any(|entry| entry.session_id == orphan));
        let tool = runtime.status(&ObservationCaller::Tool { actor: None });
        assert!(!tool.agents.iter().any(|entry| entry.session_id == orphan));
        let identified_tool = runtime.status(&ObservationCaller::Tool {
            actor: Some("alice".to_owned()),
        });
        assert!(!identified_tool
            .agents
            .iter()
            .any(|entry| entry.session_id == orphan));
        table.remove(orphan);
    }

    #[tokio::test]
    #[serial_test::serial(pty_global_manager)]
    async fn tool_without_actor_does_not_inherit_gateway_admission() {
        let manager = manager();
        let owned = Session::new(manager, Some("alice"));
        let unowned = Session::new(manager, None);
        let table = RuntimeAgents::default();
        for id in [&owned.0, &unowned.0] {
            sample(&table, id, false);
        }
        let runtime = TerminalRuntime::new(manager, &table);
        let tool = ObservationCaller::Tool { actor: None };
        let gateway = ObservationCaller::Gateway { actor: None };
        for caller in [&tool, &gateway] {
            let ids: Vec<_> = runtime
                .list(caller)
                .sessions
                .into_iter()
                .map(|s| s.session_id)
                .collect();
            let status: Vec<_> = runtime
                .status(caller)
                .agents
                .into_iter()
                .map(|s| s.session_id)
                .collect();
            assert!(ids.contains(&unowned.0));
            assert!(status.contains(&unowned.0));
            assert_eq!(
                ids.contains(&owned.0),
                matches!(caller, ObservationCaller::Gateway { .. })
            );
            assert_eq!(
                status.contains(&owned.0),
                matches!(caller, ObservationCaller::Gateway { .. })
            );
            assert!(runtime.attach(caller, &unowned.0).is_ok());
            assert!(runtime.read(caller, &unowned.0).is_ok());
            assert!(runtime.explain(caller, &unowned.0).is_ok());
            assert_eq!(
                runtime
                    .wait(caller, &params(&unowned.0, 0), CancellationToken::new())
                    .await
                    .unwrap()
                    .outcome,
                TerminalWaitOutcome::Timeout
            );
        }
        assert_hidden(runtime.attach(&tool, &owned.0), &owned.0);
        assert_hidden(runtime.read(&tool, &owned.0), &owned.0);
        assert_hidden(runtime.explain(&tool, &owned.0), &owned.0);
        assert_hidden(
            runtime
                .wait(&tool, &params(&owned.0, 0), CancellationToken::new())
                .await,
            &owned.0,
        );
        assert!(runtime.attach(&gateway, &owned.0).is_ok());
        assert!(runtime.read(&gateway, &owned.0).is_ok());
        assert!(runtime.explain(&gateway, &owned.0).is_ok());
        assert!(runtime
            .wait(&gateway, &params(&owned.0, 0), CancellationToken::new())
            .await
            .is_ok());
    }

    #[tokio::test]
    #[serial_test::serial(pty_global_manager)]
    async fn alice_and_bob_ownership_reaches_every_observation_face() {
        let manager = manager();
        let alice = Session::new(manager, Some("alice"));
        let bob = Session::new(manager, Some("bob"));
        let table = RuntimeAgents::default();
        sample(&table, &alice.0, false);
        sample(&table, &bob.0, false);
        let runtime = TerminalRuntime::new(manager, &table);
        for (actor, own, other) in [("alice", &alice.0, &bob.0), ("bob", &bob.0, &alice.0)] {
            for caller in [
                ObservationCaller::Tool {
                    actor: Some(actor.into()),
                },
                ObservationCaller::Gateway {
                    actor: Some(actor.into()),
                },
            ] {
                let ids: Vec<_> = runtime
                    .list(&caller)
                    .sessions
                    .into_iter()
                    .map(|s| s.session_id)
                    .collect();
                assert!(ids.contains(own));
                assert!(!ids.contains(other));
                assert_eq!(
                    runtime
                        .status(&caller)
                        .agents
                        .iter()
                        .map(|s| &s.session_id)
                        .collect::<Vec<_>>(),
                    vec![own]
                );
                assert!(runtime.attach(&caller, own).is_ok());
                assert!(runtime.read(&caller, own).is_ok());
                assert!(runtime.explain(&caller, own).is_ok());
                assert!(runtime
                    .wait(&caller, &params(own, 0), CancellationToken::new())
                    .await
                    .is_ok());
                for id in [other.as_str(), "never-existed"] {
                    assert_hidden(runtime.attach(&caller, id), id);
                    assert_hidden(runtime.read(&caller, id), id);
                    assert_hidden(runtime.explain(&caller, id), id);
                    assert_hidden(
                        runtime
                            .wait(&caller, &params(id, 0), CancellationToken::new())
                            .await,
                        id,
                    );
                }
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial(pty_global_manager)]
    async fn omitted_until_reaches_blocked_and_idle_through_the_borrowed_runtime() {
        let manager = manager();
        let blocked = Session::new(manager, Some("alice"));
        let idle = Session::new(manager, Some("alice"));
        let table = RuntimeAgents::default();
        let mut blocked_screen = Screen::new(4, 40);
        blocked_screen.feed(b"\x1b]0;Action Required\x07");
        let mut idle_screen = Screen::new(4, 40);
        idle_screen.feed(b"\x1b]9;4;0;0\x07");
        for (session, screen) in [(&blocked, &blocked_screen), (&idle, &idle_screen)] {
            table.sample(SampleInput {
                session_id: &session.0,
                shell: "grok",
                program: None,
                argv: &[],
                cwd: "",
                screen,
                process_exited: false,
                frame_produced: true,
                now: 0,
            });
        }
        let runtime = TerminalRuntime::new(manager, &table);
        let caller = ObservationCaller::Tool {
            actor: Some("alice".into()),
        };
        for (session, expected) in [
            (&blocked, RuntimeAgentState::Blocked),
            (&idle, RuntimeAgentState::Idle),
        ] {
            let response = runtime
                .wait(
                    &caller,
                    &TerminalWaitParams {
                        session_id: session.0.clone(),
                        until: None,
                        timeout_ms: Some(0),
                    },
                    CancellationToken::new(),
                )
                .await
                .expect("omitted until should use the default set");
            assert_eq!(response.outcome, TerminalWaitOutcome::Reached);
            assert_eq!(
                response.agent.expect("reached returns the row").state,
                expected
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial(pty_global_manager)]
    async fn quiet_does_not_become_idle() {
        let manager = manager();
        let session = Session::new(manager, Some("alice"));
        let table = RuntimeAgents::default();
        sample(&table, &session.0, true);
        table.mark_quiet(crate::gateway::runtime::QUIET_AFTER_MS);
        let runtime = TerminalRuntime::new(manager, &table);
        let caller = ObservationCaller::Tool {
            actor: Some("alice".into()),
        };
        let entry = runtime.status(&caller).agents.pop().unwrap();
        assert!(entry.quiet_since.is_some());
        assert_eq!(entry.state, RuntimeAgentState::Working);
        let response = runtime
            .wait(&caller, &params(&session.0, 0), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(response.outcome, TerminalWaitOutcome::Timeout);
        assert_eq!(response.agent.unwrap(), entry);
    }

    #[derive(Default)]
    struct Wakes(AtomicUsize);
    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    #[serial_test::serial(pty_global_manager)]
    async fn cancellation_ends_wait_without_busy_polling_or_a_held_store_lock() {
        let manager = manager();
        let session = Session::new(manager, Some("alice"));
        let table = RuntimeAgents::default();
        let runtime = TerminalRuntime::new(manager, &table);
        let caller = ObservationCaller::Tool {
            actor: Some("alice".into()),
        };
        let params = params(&session.0, 60_000);
        let cancel = CancellationToken::new();
        let mut wait = Box::pin(runtime.wait(&caller, &params, cancel.clone()));
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut context = Context::from_waker(&waker);
        assert!(wait.as_mut().poll(&mut context).is_pending());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            wakes.0.load(Ordering::SeqCst),
            0,
            "no self-wake or polling timer"
        );
        let generation = table.generation();
        sample(&table, &session.0, true); // Must not deadlock on a lock held across await.
        assert!(table.generation() > generation);
        assert!(
            wakes.0.load(Ordering::SeqCst) > 0,
            "the existing generation watch wakes the waiter"
        );
        assert!(wait.as_mut().poll(&mut context).is_pending());
        let builds = table.visible_text_builds();
        cancel.cancel();
        assert!(
            matches!(wait.as_mut().poll(&mut context), Poll::Ready(Err(ToolError::Cancelled { name })) if name == "terminal")
        );
        assert_eq!(
            table.visible_text_builds(),
            builds,
            "waiting never resamples the screen"
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(
            matches!(runtime.wait(&caller, &params, cancelled).await, Err(ToolError::Cancelled { name }) if name == "terminal")
        );
    }

    #[tokio::test]
    #[serial_test::serial(pty_global_manager)]
    async fn closed_pty_wins_over_a_stale_matching_agent_row() {
        let manager = manager();
        let session = Session::new(manager, Some("alice"));
        let table = RuntimeAgents::default();
        sample(&table, &session.0, true);
        manager.close(&session.0).expect("close the live PTY");

        for state in [RuntimeAgentState::Working, RuntimeAgentState::Blocked] {
            let outcome = wait_for_state(
                manager,
                &table,
                &session.0,
                &[state],
                Duration::ZERO,
                CancellationToken::new(),
            )
            .await
            .expect("uncancelled wait");
            assert_eq!(outcome, WaitOutcome::Gone);
        }
        table.remove(&session.0);
    }

    #[tokio::test]
    #[serial_test::serial(pty_global_manager)]
    async fn actual_core_producers_preserve_the_legacy_wire() {
        let manager = manager();
        let session = Session::new(manager, Some("alice"));
        let table = RuntimeAgents::default();
        let runtime = TerminalRuntime::new(manager, &table);
        let caller = ObservationCaller::Tool {
            actor: Some("alice".into()),
        };
        let read = runtime.read(&caller, &session.0).unwrap();
        let read_wire = serde_json::to_value(&read).unwrap();
        assert_eq!(
            read_wire,
            serde_json::json!({"session_id":session.0,"text":read.text})
        );
        assert_eq!(
            serde_json::from_value::<TerminalReadResponse>(read_wire).unwrap(),
            read
        );
        let explain = runtime.explain(&caller, &session.0).unwrap();
        let wire = serde_json::to_value(&explain).unwrap();
        assert_eq!(wire["session_id"], session.0);
        assert_eq!(wire["state"], "unknown");
        for key in ["agent", "matched_rule", "source", "manifest_version"] {
            assert!(wire[key].is_null());
        }
        assert!(wire["reason"].as_str().unwrap().contains("no row"));
        assert_eq!(wire.as_object().unwrap().len(), 8);
        assert_eq!(wire["inputs"].as_object().unwrap().len(), 3);
        assert_eq!(
            serde_json::from_value::<TerminalExplainResponse>(wire).unwrap(),
            explain
        );
        sample(&table, &session.0, false);
        let timeout = runtime
            .wait(&caller, &params(&session.0, 0), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&timeout).unwrap(),
            serde_json::json!({"session_id":session.0,"outcome":"timeout","agent":table.entry(&session.0)})
        );
        let reached = runtime
            .wait(
                &caller,
                &TerminalWaitParams {
                    until: Some(vec![RuntimeAgentState::Unknown]),
                    ..params(&session.0, 0)
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&reached).unwrap(),
            serde_json::json!({"session_id":session.0,"outcome":"reached","agent":table.entry(&session.0)})
        );
        manager.close(&session.0).unwrap();
        table.remove(&session.0);
        let gone = runtime
            .wait(&caller, &params(&session.0, 0), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&gone).unwrap(),
            serde_json::json!({"session_id":session.0,"outcome":"gone","agent":null})
        );
    }
}
