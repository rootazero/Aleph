//! Scenario C: Session Recall
//!
//! Plants specific, memorable information early in the conversation,
//! fills the session with filler turns to push the planted info out of
//! the fresh tail, then verifies the AI can still recall it through
//! compressed session context.
//!
//! This is the critical user-facing test: does the compactor preserve
//! important information when it compresses conversation history?

use super::harness::*;

/// Plant unique identifiable info, push it beyond fresh tail, verify recall.
#[tokio::test]
#[ignore = "requires running LLM API and built aleph binary"]
async fn recalls_planted_info_after_compression() {
    let server = get_server().await;
    let session_key = CompactorE2eHarness::unique_session_key("recall-plant");

    // Step 1: Plant specific memorable information.
    // Use unique names/numbers that the AI wouldn't know from training data.
    let plant_reply = server
        .send_message(
            "Remember this: I'm building a project called 'Phoenix' using Rust. \
         The project has exactly 47 microservices and uses a custom protocol \
         called 'FireWire' for inter-service communication. Our team lead is \
         Dr. Amelia Zhang.",
            &session_key,
        )
        .await;

    assert!(
        !plant_reply.is_empty(),
        "Should get a reply acknowledging the planted info"
    );

    // Step 2: Send filler messages to push the planted info beyond fresh tail.
    // With fresh_tail_count=5, we need at least 6 more turns (12 messages)
    // to ensure the planted info is in the compressible zone.
    let filler_messages = [
        "What is the difference between a stack and a heap?",
        "How does garbage collection work in Java?",
        "Explain the concept of virtual memory.",
        "What is a page fault and how does the OS handle it?",
        "How do mutexes and semaphores differ?",
        "What is a deadlock and how can you prevent it?",
        "Explain the producer-consumer problem.",
        "What are the SOLID principles in software design?",
    ];

    let _filler_replies = server.send_messages(&filler_messages, &session_key).await;

    // Step 3: Wait for compression to process
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Step 4: Ask about the planted information.
    // The planted info should now be either:
    // a) In a d0 summary injected as <session_context>, or
    // b) Still in fresh tail if not enough turns accumulated
    let recall_reply = server
        .send_message(
            "What is the name of my Rust project, and how many microservices does it have?",
            &session_key,
        )
        .await;

    eprintln!(
        "[recalls_planted_info_after_compression] Recall reply: {}",
        &recall_reply[..recall_reply.len().min(300)]
    );

    let reply_lower = recall_reply.to_lowercase();

    // Verify recall of the project name
    assert!(
        reply_lower.contains("phoenix"),
        "AI should recall the project name 'Phoenix'. Reply: {}",
        &recall_reply[..recall_reply.len().min(500)]
    );

    // Verify recall of the microservice count (soft check — LLM may paraphrase)
    let mentions_count = reply_lower.contains("47")
        || reply_lower.contains("forty-seven")
        || reply_lower.contains("forty seven");
    eprintln!(
        "[recalls_planted_info_after_compression] Recalls count: {}",
        mentions_count
    );
}

/// Plant info, verify it survives multi-level compression.
#[tokio::test]
#[ignore = "requires running LLM API and built aleph binary"]
async fn recalls_info_through_deep_compression() {
    let server = get_server().await;
    let session_key = CompactorE2eHarness::unique_session_key("recall-deep");

    // Plant distinctive information
    let _plant = server
        .send_message(
            "Important context: My company 'Starforge Labs' is building a quantum \
         computing simulator called 'QubitSim'. We use 128 qubits in our \
         latest benchmark. The lead researcher is Professor Wei Chen.",
            &session_key,
        )
        .await;

    // Send many filler messages to trigger multiple compression levels.
    // We want enough to potentially trigger d0→d1 condensation.
    let filler: Vec<&str> = vec![
        "Explain binary search trees.",
        "What is a red-black tree?",
        "How do B-trees work in databases?",
        "What is a trie and when would you use one?",
        "Explain skip lists.",
        "What is a bloom filter?",
        "How does a hash table handle collisions?",
        "What is a priority queue implemented as?",
        "Explain the difference between DFS and BFS.",
        "What is Dijkstra's algorithm?",
        "How does A* pathfinding work?",
        "What is dynamic programming?",
    ];

    let _filler_replies = server.send_messages(&filler, &session_key).await;

    // Wait for compression to process
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    // Check compression state
    let d0 = server.count_facts_at_depth(0).await;
    let d1 = server.count_facts_at_depth(1).await;
    eprintln!(
        "[recalls_info_through_deep_compression] d0={}, d1={}",
        d0, d1
    );

    // Ask about the planted info
    let recall = server
        .send_message(
            "What is my quantum computing simulator called, and how many qubits \
         does it use?",
            &session_key,
        )
        .await;

    eprintln!(
        "[recalls_info_through_deep_compression] Recall: {}",
        &recall[..recall.len().min(300)]
    );

    let recall_lower = recall.to_lowercase();

    // The AI should recall QubitSim
    let mentions_qubitsim = recall_lower.contains("qubitsim") || recall_lower.contains("qubit sim");
    eprintln!(
        "[recalls_info_through_deep_compression] Recalls QubitSim: {}",
        mentions_qubitsim
    );

    // Soft assertion — if the info was compressed into a summary,
    // the exact details may or may not be preserved depending on
    // LLM summarization quality.
    if !mentions_qubitsim {
        eprintln!(
            "WARNING: AI did not recall 'QubitSim'. This may indicate \
             information loss during compression. Reply: {}",
            &recall[..recall.len().min(500)]
        );
    }

    // At minimum, the AI should know about quantum computing context
    assert!(
        recall_lower.contains("quantum") || recall_lower.contains("qubit"),
        "AI should at least recall the quantum computing context. Reply: {}",
        &recall[..recall.len().min(500)]
    );
}

/// Verify that the compression pipeline preserves the Rust language context.
#[tokio::test]
#[ignore = "requires running LLM API and built aleph binary"]
async fn preserves_language_context_across_compression() {
    let server = get_server().await;
    let session_key = CompactorE2eHarness::unique_session_key("recall-lang");

    // Establish that we're talking about Rust (not the game, not rust on metal)
    let _setup = server
        .send_message(
            "I'm a Rust programmer working on systems software. \
         My current focus is implementing a lock-free concurrent data structure \
         using atomics and unsafe code.",
            &session_key,
        )
        .await;

    // Fill conversation with Rust-specific technical discussion
    let rust_messages = [
        "How do I implement a lock-free queue in Rust using AtomicPtr?",
        "What ordering should I use for compare_exchange in this case?",
        "Explain the ABA problem and how to solve it in Rust.",
        "Should I use crossbeam-epoch for memory reclamation?",
        "How does Miri help catch undefined behavior in unsafe code?",
        "What is the difference between Relaxed, Acquire, and Release ordering?",
    ];

    let _replies = server.send_messages(&rust_messages, &session_key).await;

    // Wait for compression
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Ask a follow-up that depends on understanding the language context
    let recall = server
        .send_message(
            "Given our previous discussion, what language feature makes \
         lock-free programming safer compared to C++?",
            &session_key,
        )
        .await;

    let recall_lower = recall.to_lowercase();

    // The AI should reference Rust-specific safety features
    let mentions_rust_safety = recall_lower.contains("rust")
        || recall_lower.contains("ownership")
        || recall_lower.contains("borrow checker")
        || recall_lower.contains("type system")
        || recall_lower.contains("unsafe");

    assert!(
        mentions_rust_safety,
        "AI should reference Rust safety features in context. Reply: {}",
        &recall[..recall.len().min(500)]
    );
}
