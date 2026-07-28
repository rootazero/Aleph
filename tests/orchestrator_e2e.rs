//! Phase 5 Orchestrator end-to-end integration tests.
//!
//! Exercises the Gateway → Orchestrator → HarnessRunner → AgentHarness →
//! SessionService path with a real in-memory session store + a scripted
//! `AiProvider`. See `docs/superpowers/plans/2026-04-19-managed-agents-phase-5-orchestrator.md`
//! §Task 13.

mod common;

use alephcore::orchestrator::{FlowInput, FlowRequest};

#[tokio::test]
async fn default_agent_roundtrip() {
    let fx = common::OrchestratorFixture::new_with_scripted_response("The answer is 42.").await;

    let handle = fx
        .orchestrator
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("what is the answer?".into()),
            channel: Some("openai-api-client".into()),
            session_hint: Some("e2e-session-1".into()),
            parent_session: None,
            depth: 0,
            tool_service: None,
            trace_sink: None,
            interaction_manifest: None,
            sandbox_override: None,
            workspace_override: None,
            max_iterations_override: None,
            transient_context: None,
            think_level: None,
            envelope: alephcore::thinker::TurnEnvelope::none(),
            model_directive: None,
        })
        .await
        .expect("dispatch");

    let outcome = handle
        .completion
        .await
        .expect("completion recv")
        .expect("flow ok");

    assert!(
        outcome.final_text.contains("42"),
        "expected '42' in: {}",
        outcome.final_text
    );

    // ScriptedLlm reports 7 input + 11 output tokens per call; this flow
    // makes one LLM call, so the bridge surfaces 18 tokens.
    assert_eq!(
        outcome.total_tokens, 18,
        "FlowOutcome.total_tokens should reflect provider-reported usage",
    );
}

#[tokio::test]
#[ignore = "Phase 6: requires Child session strategy wiring + per-session sandbox"]
async fn researcher_child_dispatch() {
    // TODO(Phase 6): FlowSpec `researcher` uses `session_strategy: child`,
    // which requires per-session ToolService + Sandbox plumbing that Phase 5
    // Task 9 intentionally deferred.
    unimplemented!();
}

#[tokio::test]
#[ignore = "Phase 6: requires flow_run ToolService registration (Task 12 deferred adapter)"]
async fn flow_run_composition_main_to_researcher() {
    // TODO(Phase 6): depends on AlephTool adapter wiring `flow_run` into
    // ToolService. Task 12 deferred this adapter to Phase 6.
    unimplemented!();
}
