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
            scope: Default::default(),
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

/// An agent with no flow of its own must still run *in the caller's session*.
///
/// Replaces an `#[ignore]`d `unimplemented!()` shell that had waited on "Phase 6:
/// Child session strategy wiring" since 2026-04. The premise was wrong twice
/// over: the `researcher` preset it named is gone, and `Child` was never the
/// right strategy for a gateway-addressed agent in the first place — with the
/// only production producer passing `parent_session: None`, `resolve_session`
/// took Child's no-parent fallback, minted a UUID, and never looked at
/// `session_hint`. Every turn addressed to one of those six agents started from
/// an empty log.
///
/// What is actually worth pinning end to end is the fallback path: `researcher`
/// has no preset, so routing resolves a flow id the registry does not hold,
/// `resolve_spec` substitutes `default-agent` with the requested agent stamped
/// on, and the caller's session key survives. That path serves every
/// filesystem-, plugin-, and team-created agent too.
#[tokio::test]
async fn an_agent_without_its_own_flow_runs_in_the_callers_session() {
    let fx = common::OrchestratorFixture::new_with_scripted_response("researched.").await;

    let handle = fx
        .orchestrator
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "researcher".into(),
            input: FlowInput::Prompt("go look it up".into()),
            channel: None,
            session_hint: Some("e2e-session-fallback".into()),
            scope: Default::default(),
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

    assert_eq!(
        handle.session_key, "e2e-session-fallback",
        "the fallback flow minted a new session instead of reusing the caller's — \
         every turn for this agent would start from an empty transcript"
    );

    let outcome = handle
        .completion
        .await
        .expect("completion recv")
        .expect("flow ok");
    assert!(outcome.final_text.contains("researched"));
}
