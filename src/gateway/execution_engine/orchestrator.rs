use super::{ExecutionEngine, ExecutionError};

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    /// Phase 5 orchestrator-backed dispatch. Used by Task 13 integration tests
    /// and (eventually) direct Gateway callers. Returns a minimal `FlowOutcome`.
    /// Full Gateway sink streaming is NOT yet wired — events are consumed but
    /// not re-emitted. This is Phase 5's "plumbing only" landing.
    ///
    /// The orchestrator is read from the engine's `orchestrator` field, which
    /// is populated at boot by the `orchestrator_cell()` handle.
    pub async fn dispatch_via_orchestrator(
        &self,
        agent_id: String,
        input_text: String,
        session_key: String,
        channel: Option<String>,
    ) -> Result<crate::orchestrator::FlowOutcome, ExecutionError> {
        use crate::orchestrator::{FlowInput, FlowRequest, FlowStreamEvent};
        use tokio::sync::broadcast;

        let orchestrator = self
            .orchestrator
            .get()
            .ok_or_else(|| {
                ExecutionError::Orchestrator(
                    "orchestrator not yet initialised — boot ordering error".to_string(),
                )
            })?
            .clone();

        let req = FlowRequest {
            flow_id: None,
            agent_id,
            input: FlowInput::Prompt(input_text),
            channel,
            session_hint: Some(session_key),
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
            // Legacy `run_flow` helper has no channel handle to query;
            // the harness bridge falls back to Background paradigm.
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            // Legacy `run_flow` helper assembles no per-turn reminders.
            transient_context: None,
            // No per-run thinking directive on this path — the provider keeps its
            // own default, which is what every release before this field sent.
            think_level: None,
            // Legacy `run_flow` helper resolves no exec tier — no approval line.
            exec_tier: None,
        };

        let handle = orchestrator
            .dispatch(req)
            .await
            .map_err(|e| ExecutionError::Orchestrator(format!("dispatch: {e}")))?;

        // Drain events; sink wiring is Phase 6. Complete(outcome) event or
        // channel close both terminate the drain.
        let mut events = handle.events;
        loop {
            match events.recv().await {
                Ok(FlowStreamEvent::Complete(_outcome)) => break,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "orchestrator event stream lagged; dropping");
                }
            }
        }

        handle
            .completion
            .await
            .map_err(|e| ExecutionError::Orchestrator(format!("completion dropped: {e}")))?
            .map_err(|e| ExecutionError::Orchestrator(format!("flow: {e}")))
    }
}
