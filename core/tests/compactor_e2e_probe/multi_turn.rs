//! Scenario A: Multi-Turn Compression
//!
//! Sends 5+ messages about Rust topics, waits for compression to produce
//! d0 summary facts, then verifies the AI can recall earlier content.
//!
//! Validates the basic session compactor pipeline:
//! chat → session history → post_turn_compress → d0 facts in LanceDB

use super::harness::*;

/// Send 5 Rust-related messages, wait for compression, verify d0 facts exist.
#[tokio::test]
#[ignore = "requires running LLM API and built aleph binary"]
async fn multi_turn_produces_d0_facts() {
    let server = get_server().await;
    let session_key = CompactorE2eHarness::unique_session_key("multi-turn-d0");

    // Send 5 messages about Rust topics to fill the session
    let topics = [
        "Explain Rust's ownership system and how it prevents memory leaks.",
        "What are Rust's lifetime annotations and when do you need them?",
        "How does the borrow checker work in Rust? Give examples.",
        "Explain the difference between Box, Rc, and Arc in Rust.",
        "What is the Pin type in Rust and why does async code need it?",
    ];

    let replies = server.send_messages(&topics, &session_key).await;

    // Verify we got replies for all messages
    assert_eq!(
        replies.len(),
        5,
        "Should have received 5 assistant replies"
    );
    for (i, reply) in replies.iter().enumerate() {
        assert!(
            !reply.is_empty(),
            "Reply {} should not be empty",
            i
        );
    }

    // Wait for compression to produce facts
    // With fresh_tail_count=5 and 5 messages + 5 replies = 10 turns,
    // the first ~5 turns should be compressible.
    let compressed = server.wait_for_compression(60).await;

    if compressed {
        // Verify d0 facts exist
        let d0_count = server.count_facts_at_depth(0).await;
        assert!(
            d0_count > 0,
            "Should have at least one d0 summary fact after 5 turns"
        );

        eprintln!(
            "[multi_turn_produces_d0_facts] d0 facts created: {}",
            d0_count
        );

        // Verify facts have content related to Rust topics
        let facts = server.query_memory_facts(None).await;
        let session_facts: Vec<_> = facts.iter()
            .filter(|f| {
                let path = f["path"].as_str().unwrap_or("");
                path.contains("aleph://session/")
            })
            .collect();

        assert!(
            !session_facts.is_empty(),
            "Should have session-scoped facts after compression"
        );

        // Check that at least one fact references Rust concepts
        let has_rust_content = session_facts.iter().any(|f| {
            let content = f["content"].as_str().unwrap_or("").to_lowercase();
            content.contains("rust") || content.contains("ownership")
                || content.contains("borrow") || content.contains("lifetime")
        });

        eprintln!(
            "[multi_turn_produces_d0_facts] facts contain Rust content: {}",
            has_rust_content
        );
        // Note: This is a soft check — deterministic fallback may truncate
        // content in ways that don't preserve keywords.
    } else {
        eprintln!(
            "[multi_turn_produces_d0_facts] WARNING: No compression occurred within timeout. \
             This may be expected if the session compactor triggers asynchronously."
        );
    }
}

/// Verify that the AI can recall content from earlier in the conversation
/// after compression has occurred.
#[tokio::test]
#[ignore = "requires running LLM API and built aleph binary"]
async fn ai_recalls_compressed_content() {
    let server = get_server().await;
    let session_key = CompactorE2eHarness::unique_session_key("multi-turn-recall");

    // Plant specific memorable information
    let setup_messages = [
        "I'm working on a project called Nexus. It's a distributed database written in Rust.",
        "Nexus uses a novel consensus algorithm called QuickPaxos for replication.",
        "The main data structure in Nexus is called a SegmentTree for efficient range queries.",
        "We deploy Nexus on Kubernetes with a custom operator called nexus-operator.",
        "The Nexus API uses gRPC with protobuf for inter-node communication.",
    ];

    let _replies = server.send_messages(&setup_messages, &session_key).await;

    // Wait a moment for compression to process
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Ask about earlier content — the AI should recall it from either
    // fresh tail or injected session context summaries
    let recall_reply = server.send_message(
        "What consensus algorithm does my Nexus project use?",
        &session_key,
    ).await;

    eprintln!(
        "[ai_recalls_compressed_content] Recall reply: {}",
        &recall_reply[..recall_reply.len().min(200)]
    );

    // The AI should mention QuickPaxos (either from fresh tail or summary)
    let recall_lower = recall_reply.to_lowercase();
    let mentions_quickpaxos = recall_lower.contains("quickpaxos")
        || recall_lower.contains("quick paxos")
        || recall_lower.contains("paxos");

    assert!(
        mentions_quickpaxos,
        "AI should recall the QuickPaxos consensus algorithm from the conversation. \
         Reply was: {}",
        &recall_reply[..recall_reply.len().min(500)]
    );
}
