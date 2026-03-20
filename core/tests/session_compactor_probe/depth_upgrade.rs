//! Depth upgrade (condensation) scenario tests.
//!
//! Tests hierarchical summarization: d0->d1 and d1->d2 condensation
//! when fanout thresholds are met.

use std::sync::atomic::Ordering;
use tempfile::TempDir;

use alephcore::gateway::agent_instance::{AgentInstance, AgentInstanceConfig, MessageRole};
use alephcore::gateway::router::SessionKey;
use alephcore::memory::session_compactor::SessionCompactorConfig;

use super::harness::CompactorProbeHarness;

/// Helper: create a temporary AgentInstance and populate it with messages.
async fn make_agent_with_messages(
    temp_dir: &TempDir,
    messages: &[(String, String)],
    session_key: &SessionKey,
) -> AgentInstance {
    let config = AgentInstanceConfig {
        agent_id: "test-agent".to_string(),
        workspace: temp_dir.path().join("workspace"),
        agent_dir: temp_dir.path().join("agent"),
        ..Default::default()
    };
    let agent = AgentInstance::new(config).expect("Failed to create AgentInstance");
    agent.ensure_session(session_key).await;

    for (role, content) in messages {
        let role = match role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::User,
        };
        agent.add_message(session_key, role, content).await;
    }

    agent
}

#[tokio::test]
async fn p3_d0_condensed_to_d1() {
    // Use aggressive config: d1_min_fanout=3 so condensation triggers sooner
    let config = SessionCompactorConfig {
        enabled: true,
        fresh_tail_count: 2, // Keep very few messages fresh — more compressible
        context_threshold: 0.75,
        leaf_chunk_tokens: 50, // Very small chunks — more d0 facts per run
        d1_min_fanout: 3,     // Low threshold for d0→d1 condensation
        d2_min_fanout: 3,
        max_summary_depth: 2,
        token_estimate_ratio: 3.5,
        session_fact_retention_hours: 24,
        promote_confidence_threshold: 0.8,
    };
    let h = CompactorProbeHarness::with_config(config).await;

    let agent_temp = TempDir::new().unwrap();
    let session_key = SessionKey::main("test-agent");

    // Run post_turn_compress multiple times with growing message batches.
    // Each run adds more messages to the agent, generating new d0 summaries.
    // Once d0 count >= d1_min_fanout (3), condensation should trigger.
    for batch in 0..5 {
        let batch_messages =
            CompactorProbeHarness::make_messages(10, &format!("batch-{}", batch));
        let agent =
            make_agent_with_messages(&agent_temp, &batch_messages, &session_key).await;

        let _result = h
            .compactor
            .post_turn_compress(&agent, &session_key)
            .await
            .expect("post_turn_compress should not fail");
    }

    let session_id = session_key.to_key_string();

    // Assert: d1 facts appear
    let d1_count = h.count_facts_at_depth(&session_id, 1).await;
    assert!(
        d1_count > 0,
        "Expected d1 facts after condensation, found {}",
        d1_count
    );

    // Assert: some d0 facts are invalidated (is_valid=false)
    let all_facts = h.query_all_session_facts(&session_id).await;
    let valid_facts = h.query_session_facts(&session_id).await;
    let invalid_count = all_facts.len() - valid_facts.len();
    assert!(
        invalid_count > 0,
        "Expected some invalidated d0 facts after condensation, \
         all={} valid={}",
        all_facts.len(),
        valid_facts.len()
    );

    // Assert: metrics.d1_condensations > 0
    let d1_condensations = h
        .compactor
        .metrics()
        .d1_condensations
        .load(Ordering::Relaxed);
    assert!(
        d1_condensations > 0,
        "Expected d1_condensations > 0, got {}",
        d1_condensations
    );
}
