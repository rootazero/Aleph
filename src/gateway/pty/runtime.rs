//! Shared read-only terminal observations. No second sampler or process lifecycle.

use std::time::Duration;

use aleph_protocol::pty::{PtyAttachResponse, PtyListResponse, PtySessionInfo};
use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState, RuntimeAgentsListResponse};
use aleph_protocol::terminal::{
    TerminalExplainInputs, TerminalExplainResponse, TerminalExplainRule, TerminalReadResponse,
    TerminalWaitOutcome, TerminalWaitResponse,
};
use tokio_util::sync::CancellationToken;

use super::{no_such_session, PtyManager, SessionOwner};
use crate::gateway::runtime::RuntimeAgents;

/// Gateway identity absence means unrestricted; tool identity absence does not.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ObservationCaller<'a> {
    Gateway { actor: Option<&'a str> },
    Tool { actor: Option<&'a str> },
}

impl ObservationCaller<'_> {
    pub(crate) fn admits(self, owner: &SessionOwner) -> bool {
        match self {
            Self::Gateway { actor } => owner.admits(actor),
            Self::Tool { actor: None } => matches!(owner, SessionOwner::Known(None)),
            Self::Tool { actor: Some(actor) } => owner.admits(Some(actor)),
        }
    }
}

pub(crate) const WAIT_DEFAULT_TIMEOUT_MS: u64 = 60_000;
pub(crate) const WAIT_MAX_TIMEOUT_MS: u64 = 150_000;
const WAIT_DEFAULT_UNTIL: [RuntimeAgentState; 2] =
    [RuntimeAgentState::Blocked, RuntimeAgentState::Idle];
const EXPLAIN_SCREEN_TAIL_LINES: usize = 12;

pub(crate) fn wait_window(requested: Option<u64>) -> Duration {
    Duration::from_millis(
        requested
            .unwrap_or(WAIT_DEFAULT_TIMEOUT_MS)
            .min(WAIT_MAX_TIMEOUT_MS),
    )
}

fn wait_states(until: Option<&[RuntimeAgentState]>) -> Result<&[RuntimeAgentState], String> {
    match until {
        Some([]) => Err("wait requires at least one state in `until` \
            (blocked / idle / working / unknown); omit it for [blocked, idle]"
            .to_string()),
        Some(states) => Ok(states),
        None => Ok(&WAIT_DEFAULT_UNTIL),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    Reached(RuntimeAgentEntry),
    Timeout(Option<RuntimeAgentEntry>),
    Gone,
}

impl WaitOutcome {
    fn into_response(self, session_id: &str) -> TerminalWaitResponse {
        let (outcome, agent) = match self {
            Self::Reached(entry) => (TerminalWaitOutcome::Reached, Some(entry)),
            Self::Timeout(entry) => (TerminalWaitOutcome::Timeout, entry),
            Self::Gone => (TerminalWaitOutcome::Gone, None),
        };
        TerminalWaitResponse {
            session_id: session_id.to_owned(),
            outcome,
            agent,
        }
    }
}

/// An internal cancellation error, not a fourth wire outcome or agent state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitCancelled;

/// Borrows the existing registry and agent table; owns no runtime state.
pub(crate) struct TerminalRuntime<'a> {
    manager: &'a PtyManager,
    agents: &'a RuntimeAgents,
}

impl<'a> TerminalRuntime<'a> {
    pub(crate) fn new(manager: &'a PtyManager, agents: &'a RuntimeAgents) -> Self {
        Self { manager, agents }
    }

    pub(crate) fn require_owned(
        &self,
        session_id: &str,
        caller: ObservationCaller<'_>,
    ) -> Result<(), String> {
        if caller.admits(&self.manager.owner_of(session_id)) {
            Ok(())
        } else {
            Err(no_such_session(session_id))
        }
    }

    pub(crate) fn list(&self, caller: ObservationCaller<'_>) -> PtyListResponse {
        PtyListResponse {
            sessions: self
                .manager
                .list()
                .iter()
                .filter(|session| caller.admits(&SessionOwner::Known(session.created_by.clone())))
                .map(PtySessionInfo::from)
                .collect(),
        }
    }

    pub(crate) fn status(&self, caller: ObservationCaller<'_>) -> RuntimeAgentsListResponse {
        RuntimeAgentsListResponse {
            agents: self
                .agents
                .snapshot()
                .into_iter()
                .filter(|entry| caller.admits(&self.manager.owner_of(&entry.session_id)))
                .collect(),
        }
    }

    pub(crate) fn read(
        &self,
        session_id: &str,
        caller: ObservationCaller<'_>,
    ) -> Result<TerminalReadResponse, String> {
        self.require_owned(session_id, caller)?;
        Ok(TerminalReadResponse {
            session_id: session_id.to_owned(),
            text: self.manager.visible_text(session_id)?,
        })
    }

    // Authorisation seam only; the production attach RPC stays unchanged in A2.
    #[allow(dead_code)]
    pub(crate) fn attach(
        &self,
        session_id: &str,
        caller: ObservationCaller<'_>,
    ) -> Result<PtyAttachResponse, String> {
        self.require_owned(session_id, caller)?;
        self.manager.attach_snapshot(session_id)
    }

    pub(crate) fn explain(
        &self,
        session_id: &str,
        caller: ObservationCaller<'_>,
    ) -> Result<TerminalExplainResponse, String> {
        self.require_owned(session_id, caller)?;
        let screen = self.manager.detection_inputs(session_id)?;
        Ok(explain_detection(
            session_id,
            self.agents.detected_agent(session_id),
            self.agents.entry(session_id).as_ref(),
            &screen,
        ))
    }

    pub(crate) async fn wait(
        &self,
        session_id: &str,
        caller: ObservationCaller<'_>,
        until: Option<&[RuntimeAgentState]>,
        timeout_ms: Option<u64>,
        cancel: &CancellationToken,
    ) -> Result<TerminalWaitResponse, String> {
        self.require_owned(session_id, caller)?;
        let states = wait_states(until)?;
        self.wait_for_state(session_id, states, wait_window(timeout_ms), cancel)
            .await
            .map(|outcome| outcome.into_response(session_id))
            .map_err(|_| "terminal wait cancelled".to_string())
    }

    pub(crate) fn session_is_registered(&self, session_id: &str) -> bool {
        self.manager
            .list()
            .iter()
            .any(|session| session.session_id == session_id)
    }

    /// Registry absence outranks EVERY agent row, including a matching stale row.
    fn wait_verdict(&self, session_id: &str, until: &[RuntimeAgentState]) -> Option<WaitOutcome> {
        if !self.session_is_registered(session_id) {
            return Some(WaitOutcome::Gone);
        }
        match self.agents.entry(session_id) {
            Some(entry) if until.contains(&entry.state) => Some(WaitOutcome::Reached(entry)),
            _ => None,
        }
    }

    /// Subscribe before reading, never hold a registry/table lock across an await.
    /// The existing generation watch is the only clock; no screen busy polling.
    pub(crate) async fn wait_for_state(
        &self,
        session_id: &str,
        until: &[RuntimeAgentState],
        window: Duration,
        cancel: &CancellationToken,
    ) -> Result<WaitOutcome, WaitCancelled> {
        let mut changes = self.agents.subscribe();
        let deadline = tokio::time::Instant::now() + window;
        loop {
            if cancel.is_cancelled() {
                return Err(WaitCancelled);
            }
            if let Some(outcome) = self.wait_verdict(session_id, until) {
                return Ok(outcome);
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(WaitCancelled),
                changed = tokio::time::timeout_at(deadline, changes.changed()) => {
                    if !matches!(changed, Ok(Ok(()))) {
                        // Use the same registry-first verdict at the deadline, too.
                        return Ok(self.wait_verdict(session_id, until)
                            .unwrap_or_else(|| WaitOutcome::Timeout(self.agents.entry(session_id))));
                    }
                }
            }
        }
    }
}

/// Raw screen explanation, not the sampler's damped state. Quiet time is not Idle.
pub(crate) fn explain_detection(
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
                None => "this session has no row in the agent table yet — nothing has been \
                         sampled, which is not the same as nothing running"
                    .to_string(),
                Some(entry) => format!(
                    "the foreground program ({}) is not an agent the bundled manifests know",
                    entry.program.as_deref().unwrap_or("not probed"),
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
    use crate::gateway::pty::{screen::Screen, SpawnOptions};
    use crate::gateway::runtime::SampleInput;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    // Keep singleton access in test bodies so the global-manager census can
    // attribute it. The guard closes only its own PTY, even after a panic.
    struct RealPty<'a> {
        manager: &'a PtyManager,
        id: String,
    }

    impl<'a> RealPty<'a> {
        fn spawn(manager: &'a PtyManager, owner: Option<&str>) -> Self {
            let id = manager
                .spawn(&SpawnOptions {
                    created_by: owner.map(str::to_owned),
                    ..Default::default()
                })
                .expect("spawn fixture")
                .session_id;
            Self { manager, id }
        }
    }

    impl Drop for RealPty<'_> {
        fn drop(&mut self) {
            let _ = self.manager.close(&self.id);
        }
    }

    // Same real sampling path and grok OSC signal as the existing wait tests;
    // the table is isolated from the process-global flush loop.
    fn sample_working(agents: &RuntimeAgents, session_id: &str) -> RuntimeAgentEntry {
        let mut screen = Screen::new(4, 40);
        screen.feed(b"\x1b]9;4;1;-1\x07");
        agents.sample(SampleInput {
            session_id,
            shell: "grok",
            program: None,
            argv: &[],
            cwd: "",
            screen: &screen,
            process_exited: false,
            frame_produced: true,
            now: 0,
        });
        let entry = agents.entry(session_id).expect("sampled row");
        assert_eq!(entry.state, RuntimeAgentState::Working, "fixture state");
        entry
    }

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn gateway_actorless_admits_owned_unowned_and_unknown() {
        let caller = ObservationCaller::Gateway { actor: None };
        assert!(caller.admits(&SessionOwner::Known(Some("alice".into()))));
        assert!(caller.admits(&SessionOwner::Known(None)));
        assert!(caller.admits(&SessionOwner::Unknown));
    }

    #[test]
    fn tool_actorless_admits_only_known_unowned() {
        let caller = ObservationCaller::Tool { actor: None };
        assert!(!caller.admits(&SessionOwner::Known(Some("alice".into()))));
        assert!(caller.admits(&SessionOwner::Known(None)));
        assert!(!caller.admits(&SessionOwner::Unknown));
    }

    #[test]
    fn identified_callers_admit_only_the_exact_owner() {
        for caller in [
            ObservationCaller::Gateway {
                actor: Some("alice"),
            },
            ObservationCaller::Tool {
                actor: Some("alice"),
            },
        ] {
            assert!(caller.admits(&SessionOwner::Known(Some("alice".into()))));
            for owner in [
                SessionOwner::Known(Some("Alice".into())),
                SessionOwner::Known(Some("alice-other".into())),
                SessionOwner::Known(Some("bob".into())),
                SessionOwner::Known(None),
                SessionOwner::Unknown,
            ] {
                assert!(!caller.admits(&owner), "{caller:?} admitted {owner:?}");
            }
        }
    }

    #[test]
    #[serial_test::parallel(pty_global_manager)]
    fn shared_runtime_checks_the_registry_owner_and_hides_refusals() {
        let manager = crate::gateway::pty::manager();
        let live = RealPty::spawn(manager, Some("alice"));
        let agents = RuntimeAgents::default();
        let runtime = TerminalRuntime::new(manager, &agents);
        for caller in [
            ObservationCaller::Gateway {
                actor: Some("alice"),
            },
            ObservationCaller::Tool {
                actor: Some("alice"),
            },
            ObservationCaller::Gateway { actor: None },
        ] {
            assert_eq!(runtime.require_owned(&live.id, caller), Ok(()));
        }
        for caller in [
            ObservationCaller::Gateway { actor: Some("bob") },
            ObservationCaller::Tool { actor: Some("bob") },
            ObservationCaller::Tool { actor: None },
        ] {
            assert_eq!(
                runtime.require_owned(&live.id, caller),
                Err(no_such_session(&live.id))
            );
        }
        let missing = uuid::Uuid::new_v4().to_string();
        for caller in [
            ObservationCaller::Gateway {
                actor: Some("alice"),
            },
            ObservationCaller::Tool {
                actor: Some("alice"),
            },
            ObservationCaller::Tool { actor: None },
        ] {
            assert_eq!(
                runtime.require_owned(&missing, caller),
                Err(no_such_session(&missing))
            );
        }
    }

    #[test]
    fn wait_rejects_empty_until_and_defaults_to_blocked_and_idle() {
        assert_eq!(
            wait_states(Some(&[])).unwrap_err(),
            "wait requires at least one state in `until` \
             (blocked / idle / working / unknown); omit it for [blocked, idle]"
        );
        assert_eq!(
            wait_states(None).unwrap(),
            &[RuntimeAgentState::Blocked, RuntimeAgentState::Idle]
        );
        let explicit = [RuntimeAgentState::Working, RuntimeAgentState::Unknown];
        assert_eq!(wait_states(Some(&explicit)).unwrap(), &explicit);
    }

    #[test]
    fn wait_window_defaults_clamps_and_preserves_zero() {
        assert_eq!(wait_window(None), Duration::from_millis(60_000));
        assert_eq!(wait_window(Some(0)), Duration::ZERO);
        assert_eq!(wait_window(Some(17)), Duration::from_millis(17));
        assert_eq!(wait_window(Some(150_000)), Duration::from_millis(150_000));
        assert_eq!(wait_window(Some(u64::MAX)), Duration::from_millis(150_000));
    }

    #[tokio::test(start_paused = true)]
    #[serial_test::parallel(pty_global_manager)]
    async fn cancellation_wakes_a_pending_wait_without_periodic_polling() {
        let manager = crate::gateway::pty::manager();
        let live = RealPty::spawn(manager, None);
        let agents = RuntimeAgents::default();
        let entry = sample_working(&agents, &live.id);
        let runtime = TerminalRuntime::new(manager, &agents);
        let cancel = CancellationToken::new();
        let wakes = Arc::new(WakeCounter::default());
        let waker = Waker::from(Arc::clone(&wakes));
        let mut context = Context::from_waker(&waker);
        let mut waiting = Box::pin(runtime.wait_for_state(
            &live.id,
            &[RuntimeAgentState::Blocked],
            Duration::from_secs(60),
            &cancel,
        ));
        assert_eq!(waiting.as_mut().poll(&mut context), Poll::Pending);
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(
            wakes.0.load(Ordering::SeqCst),
            0,
            "no table change or deadline: the waiter must not schedule polling"
        );
        assert_eq!(agents.entry(&live.id), Some(entry));
        cancel.cancel();
        assert!(
            wakes.0.load(Ordering::SeqCst) > 0,
            "cancel must wake the waiter"
        );
        assert_eq!(
            waiting.as_mut().poll(&mut context),
            Poll::Ready(Err(WaitCancelled))
        );
    }

    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn registry_removal_outranks_matching_and_nonmatching_stale_rows() {
        let manager = crate::gateway::pty::manager();
        let live = RealPty::spawn(manager, None);
        let agents = RuntimeAgents::default();
        let entry = sample_working(&agents, &live.id);
        let runtime = TerminalRuntime::new(manager, &agents);
        assert_eq!(
            runtime.wait_verdict(&live.id, &[RuntimeAgentState::Working]),
            Some(WaitOutcome::Reached(entry.clone()))
        );
        assert_eq!(
            runtime.wait_verdict(&live.id, &[RuntimeAgentState::Blocked]),
            None
        );
        manager.close(&live.id).expect("kill and remove fixture");
        assert!(!runtime.session_is_registered(&live.id));
        assert_eq!(agents.entry(&live.id), Some(entry));
        let cancel = CancellationToken::new();
        for state in [RuntimeAgentState::Working, RuntimeAgentState::Blocked] {
            assert_eq!(
                runtime.wait_verdict(&live.id, &[state]),
                Some(WaitOutcome::Gone)
            );
            assert_eq!(
                runtime
                    .wait_for_state(&live.id, &[state], Duration::ZERO, &cancel)
                    .await,
                Ok(WaitOutcome::Gone),
                "registry absence must win even at the deadline for {state:?}"
            );
        }
    }

    #[test]
    fn screen_tail_is_exactly_the_last_twelve_real_newline_lines() {
        let text = "discard-1\ndiscard-2\ndiscard-3\nline-04\nline-05\n\n\
                    line-07 literal\\n stays here\nline-08\nline-09\nline-10\n\
                    line-11\nline-12\nline-13\nline-14\nline-15\n";
        let tail = screen_tail(text);
        assert_eq!(
            tail,
            "line-04\nline-05\n\nline-07 literal\\n stays here\nline-08\nline-09\n\
             line-10\nline-11\nline-12\nline-13\nline-14\nline-15"
        );
        assert_eq!(tail.lines().count(), 12);
    }

    #[test]
    fn screen_tail_preserves_short_blank_and_unwrapped_lines() {
        assert_eq!(screen_tail(""), "");
        assert_eq!(screen_tail("one\n\ntwo\n"), "one\n\ntwo");
        assert_eq!(screen_tail("one\r\ntwo\r\n"), "one\ntwo");
        let long_line = "x".repeat(1_000);
        assert_eq!(screen_tail(&long_line), long_line);
        let twelve = (1..=12)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(screen_tail(&twelve), twelve);
    }
}
