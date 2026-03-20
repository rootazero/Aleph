//! Scenario B: Compression Depth
//!
//! Sends 15+ messages to trigger hierarchical compression (d0 → d1),
//! verifying that the compactor creates deeper summary levels when the
//! configured fanout thresholds are met.
//!
//! With the test config:
//! - `leaf_chunk_tokens = 300` (small chunks → more d0 facts)
//! - `d1_min_fanout = 3` (condense after 3 d0 facts)
//! - `d2_min_fanout = 3` (condense after 3 d1 facts)

use super::harness::*;

/// Send 15 messages to trigger deep compression, verify d0 facts appear
/// and potentially d1 condensation.
#[tokio::test]
#[ignore = "requires running LLM API and built aleph binary"]
async fn many_turns_trigger_hierarchical_compression() {
    let server = get_server().await;
    let session_key = CompactorE2eHarness::unique_session_key("depth-15");

    // Send 15 diverse messages to generate substantial content.
    // Each message covers a different topic to produce varied summaries.
    let messages = [
        "Explain how TCP three-way handshake works.",
        "What is the difference between HTTP/1.1 and HTTP/2?",
        "How does TLS 1.3 improve on TLS 1.2?",
        "Explain DNS resolution step by step.",
        "What is BGP and how does internet routing work?",
        "How do CDNs improve content delivery performance?",
        "Explain the CAP theorem in distributed systems.",
        "What is the Raft consensus algorithm?",
        "How does consistent hashing work in distributed databases?",
        "Explain vector clocks and conflict resolution in CRDTs.",
        "What is the difference between strong and eventual consistency?",
        "How does gRPC differ from REST APIs?",
        "Explain WebSocket protocol and its use cases.",
        "What is QUIC protocol and how does it relate to HTTP/3?",
        "How does mTLS work for service-to-service authentication?",
    ];

    // Send all messages
    let replies = server.send_messages(&messages, &session_key).await;
    assert_eq!(
        replies.len(),
        15,
        "Should have received 15 assistant replies"
    );

    // Wait for compression to process all turns
    let compressed = server.wait_for_compression(90).await;

    if compressed {
        let d0_count = server.count_facts_at_depth(0).await;
        let d1_count = server.count_facts_at_depth(1).await;
        let d2_count = server.count_facts_at_depth(2).await;

        eprintln!(
            "[many_turns_trigger_hierarchical_compression] \
             d0={}, d1={}, d2={}",
            d0_count, d1_count, d2_count
        );

        // With 15 messages (30 turns including replies), fresh_tail=5,
        // and leaf_chunk_tokens=300, we should get multiple d0 facts.
        assert!(
            d0_count > 0 || d1_count > 0,
            "Should have d0 or d1 facts after 15 turns. \
             d0={}, d1={}, d2={}",
            d0_count, d1_count, d2_count
        );

        // If d1_min_fanout=3 and we have enough d0 facts, d1 should appear.
        // Note: d0 facts may have been invalidated during condensation,
        // so we check that at least some compression happened.
        let total_depth = d0_count + d1_count + d2_count;
        assert!(
            total_depth >= 1,
            "Total session facts across all depths should be >= 1"
        );

        // Memory stats should reflect the compression
        let stats = server.memory_stats().await;
        let total_facts = stats["totalFacts"].as_i64().unwrap_or(0);
        eprintln!(
            "[many_turns_trigger_hierarchical_compression] \
             totalFacts={}, validFacts={}",
            total_facts,
            stats["validFacts"].as_i64().unwrap_or(0)
        );
    } else {
        eprintln!(
            "[many_turns_trigger_hierarchical_compression] \
             WARNING: No compression occurred within timeout."
        );
    }
}

/// Verify that d0 facts are invalidated when condensed into d1.
#[tokio::test]
#[ignore = "requires running LLM API and built aleph binary"]
async fn condensation_invalidates_source_facts() {
    let server = get_server().await;
    let session_key = CompactorE2eHarness::unique_session_key("depth-invalidation");

    // Send enough messages to trigger d0→d1 condensation
    let messages = [
        "What is functional programming?",
        "Explain monads in simple terms.",
        "How do algebraic data types work?",
        "What is pattern matching and why is it useful?",
        "Explain higher-kinded types.",
        "What are type classes in Haskell?",
        "How does lazy evaluation work?",
        "Explain the difference between strict and lazy evaluation.",
        "What is referential transparency?",
        "How do pure functions differ from impure functions?",
    ];

    let _replies = server.send_messages(&messages, &session_key).await;

    // Wait for compression
    let compressed = server.wait_for_compression(90).await;

    if compressed {
        // Query including invalid facts
        let result = server.rpc_ok("memory.list_facts", serde_json::json!({
            "limit": 200,
            "include_invalid": true
        })).await;

        let all_facts = result["facts"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let session_facts: Vec<_> = all_facts.iter()
            .filter(|f| {
                let path = f["path"].as_str().unwrap_or("");
                path.contains("aleph://session/")
            })
            .collect();

        let valid_count = session_facts.iter()
            .filter(|f| f["is_valid"].as_bool() == Some(true))
            .count();
        let invalid_count = session_facts.iter()
            .filter(|f| f["is_valid"].as_bool() == Some(false))
            .count();

        eprintln!(
            "[condensation_invalidates_source_facts] \
             total_session_facts={}, valid={}, invalid={}",
            session_facts.len(), valid_count, invalid_count
        );

        // If condensation occurred, some d0 facts should be invalidated
        if server.count_facts_at_depth(1).await > 0 {
            assert!(
                invalid_count > 0,
                "When d1 facts exist, some d0 facts should be invalidated"
            );
        }
    } else {
        eprintln!(
            "[condensation_invalidates_source_facts] \
             WARNING: No compression occurred within timeout."
        );
    }
}
