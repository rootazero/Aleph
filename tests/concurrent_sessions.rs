//! Concurrent sessions stress test.
//!
//! This test verifies that the Markdown session store can handle multiple
//! concurrent sessions without race conditions, data corruption, or cross-contamination.

use alephcore::memory::store::markdown::MarkdownSessionStore;
use alephcore::memory::store::session::{CompressionStatus, MessageRole, RawSessionStore};
use alephcore::session::events::{
    MessageContent, RunOutcome, SessionEvent, SessionEventRecord, ToolOutput, ToolOutputMetadata,
    TurnTrigger,
};
use alephcore::session::markdown_projector::MarkdownProjector;
use alephcore::session::observer::SessionEventObserver;
use alephcore::session::service::SessionId;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::task::JoinSet;
use uuid::Uuid;

/// Number of concurrent sessions to create.
const NUM_SESSIONS: usize = 10;

/// Number of events per session (must be even for balanced message pairs).
const EVENTS_PER_SESSION: usize = 100;

/// Helper to create a message content structure.
fn make_content(text: &str) -> MessageContent {
    MessageContent {
        text: text.to_string(),
        blocks: Vec::new(),
        thinking: None,
        thinking_signature: None,
    }
}

/// Helper to create a session event record.
fn make_record(seq: u64, event: SessionEvent) -> SessionEventRecord {
    SessionEventRecord {
        seq,
        event,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    }
}

/// Emit a complete session with the specified number of events.
async fn emit_session(
    session_id: SessionId,
    projector: Arc<MarkdownProjector>,
    num_events: usize,
) -> SessionId {
    let run_id = format!("run_{}", Uuid::new_v4());
    let mut seq = 0u64;

    // 1. RunStarted
    seq += 1;
    let run_started = make_record(
        seq,
        SessionEvent::RunStarted {
            run_id: run_id.clone(),
            at: chrono::Utc::now().timestamp(),
            project_root: Some(format!("/tmp/test_project_{}", session_id)),
            envelope: None,
            telecom_provenance: Default::default(),
        },
    );
    projector.on_appended(&session_id, &run_started);

    // Calculate how many message pairs and tool calls we need
    // Reserve 2 events for RunStarted/RunFinished
    let remaining = num_events - 2;

    // Each turn needs: TurnStarted (1) + UserMessage (1) + AssistantMessage (1) = 3 events minimum
    // With tool call: TurnStarted (1) + UserMessage (1) + ToolCallRequested (1) + ToolResult (1) + AssistantMessage (1) = 5 events
    // Alternate between simple turns (3 events) and tool turns (5 events)

    let mut turn_count = 0;
    let mut events_emitted = 1; // RunStarted already emitted

    while events_emitted + 3 <= remaining {
        turn_count += 1;
        let turn_id = Uuid::new_v4();

        // TurnStarted
        seq += 1;
        let turn_started = make_record(
            seq,
            SessionEvent::TurnStarted {
                turn_id,
                trigger: TurnTrigger::UserMessage,
                at: chrono::Utc::now().timestamp(),
            },
        );
        projector.on_appended(&session_id, &turn_started);
        events_emitted += 1;

        // UserMessage
        seq += 1;
        let user_msg = make_record(
            seq,
            SessionEvent::UserMessage {
                turn_id,
                content: make_content(&format!(
                    "Session {} Turn {} User message",
                    session_id, turn_count
                )),
                at: chrono::Utc::now().timestamp(),
                synthetic: false,
                author_user_id: None,
            },
        );
        projector.on_appended(&session_id, &user_msg);
        events_emitted += 1;

        // Decide if we have room for a tool call (needs 3 more events)
        let use_tool = turn_count % 3 == 0 && events_emitted + 3 <= remaining;

        if use_tool {
            let call_id = format!("call_{}_{}", session_id, turn_count);

            // ToolCallRequested
            seq += 1;
            let tool_call = make_record(
                seq,
                SessionEvent::ToolCallRequested {
                    turn_id,
                    call_id: call_id.clone(),
                    name: "bash".to_string(),
                    input: serde_json::json!({
                        "command": format!("echo 'Session {} Turn {}'", session_id, turn_count)
                    }),
                    identity: None,
                    at: chrono::Utc::now().timestamp(),
                },
            );
            projector.on_appended(&session_id, &tool_call);
            events_emitted += 1;

            // ToolResult
            seq += 1;
            let tool_result = make_record(
                seq,
                SessionEvent::ToolResult {
                    turn_id,
                    call_id: call_id.clone(),
                    output: ToolOutput {
                        value: serde_json::json!(format!(
                            "Session {} Turn {}",
                            session_id, turn_count
                        )),
                        metadata: ToolOutputMetadata::default(),
                    },
                    at: chrono::Utc::now().timestamp(),
                },
            );
            projector.on_appended(&session_id, &tool_result);
            events_emitted += 1;
        }

        // AssistantMessage
        seq += 1;
        let assistant_msg = make_record(
            seq,
            SessionEvent::AssistantMessage {
                turn_id,
                content: make_content(&format!(
                    "Session {} Turn {} Assistant response{}",
                    session_id,
                    turn_count,
                    if use_tool { " with tool" } else { "" }
                )),
                usage: None,
                at: chrono::Utc::now().timestamp(),
            },
        );
        projector.on_appended(&session_id, &assistant_msg);
        events_emitted += 1;
    }

    // RunFinished
    seq += 1;
    let run_finished = make_record(
        seq,
        SessionEvent::RunFinished {
            run_id,
            outcome: RunOutcome::Completed,
            at: chrono::Utc::now().timestamp(),
        },
    );
    projector.on_appended(&session_id, &run_finished);

    session_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_sessions_stress() {
    // Setup: Create temp directory and initialize store
    let temp_dir = TempDir::new().unwrap();
    let store = Arc::new(
        MarkdownSessionStore::new(temp_dir.path())
            .await
            .expect("Failed to create MarkdownSessionStore"),
    );

    // Create projector shared across all sessions
    let projector = Arc::new(MarkdownProjector::new(store.clone()));

    // Spawn NUM_SESSIONS concurrent sessions
    let mut join_set = JoinSet::new();
    let session_ids: Vec<SessionId> = (0..NUM_SESSIONS)
        .map(|idx| SessionId::ephemeral(format!("concurrent-stress-{idx}")))
        .collect();

    println!(
        "Starting {} concurrent sessions with {} events each...",
        NUM_SESSIONS, EVENTS_PER_SESSION
    );

    for session_id in session_ids.iter().cloned() {
        let projector_clone = projector.clone();
        join_set.spawn(async move {
            emit_session(session_id, projector_clone, EVENTS_PER_SESSION).await
        });
    }

    // Wait for all sessions to complete
    let mut completed_sessions = Vec::new();
    while let Some(result) = join_set.join_next().await {
        let session_id = result.expect("Task should not panic");
        completed_sessions.push(session_id);
    }

    // Wait for background projection to complete
    // Longer wait time for concurrent operations
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    println!(
        "All {} sessions completed. Verifying results...",
        NUM_SESSIONS
    );

    // Verify: All session directories exist
    let sessions_dir = temp_dir.path().join("raw/sessions");
    assert!(sessions_dir.exists(), "Sessions directory should exist");

    let mut total_messages = 0;
    let mut total_tool_calls = 0;

    for (idx, session_id) in completed_sessions.iter().enumerate() {
        let session_dir = sessions_dir.join(session_id.to_string());
        assert!(
            session_dir.exists(),
            "Session {} directory should exist at {:?}",
            idx,
            session_dir
        );

        // Verify: transcript.md exists
        let transcript_path = session_dir.join("transcript.md");
        assert!(
            transcript_path.exists(),
            "Session {} transcript should exist",
            idx
        );

        let transcript_content = tokio::fs::read_to_string(&transcript_path)
            .await
            .unwrap_or_else(|_| panic!("Failed to read session {} transcript", idx));

        // Verify: Transcript contains session-specific content (no cross-contamination)
        assert!(
            transcript_content.contains(&session_id.to_string()),
            "Session {} transcript should contain its own session ID",
            idx
        );

        // Verify: Transcript does NOT contain other session IDs
        for other_session_id in completed_sessions.iter() {
            if other_session_id != session_id {
                assert!(
                    !transcript_content.contains(&other_session_id.to_string()),
                    "Session {} transcript should not contain session {} ID",
                    idx,
                    other_session_id
                );
            }
        }

        // Verify: tool_outputs.md exists
        let tool_outputs_path = session_dir.join("tool_outputs.md");
        assert!(
            tool_outputs_path.exists(),
            "Session {} tool outputs should exist",
            idx
        );

        let tool_outputs_content = tokio::fs::read_to_string(&tool_outputs_path)
            .await
            .unwrap_or_else(|_| panic!("Failed to read session {} tool outputs", idx));

        // Verify: Tool outputs contain session-specific content
        assert!(
            tool_outputs_content.contains(&session_id.to_string()),
            "Session {} tool outputs should contain its own session ID",
            idx
        );

        // Verify: metadata.json exists and has correct stats
        let metadata_path = session_dir.join("metadata.json");
        assert!(
            metadata_path.exists(),
            "Session {} metadata should exist",
            idx
        );

        let metadata_content = tokio::fs::read_to_string(&metadata_path)
            .await
            .unwrap_or_else(|_| panic!("Failed to read session {} metadata", idx));

        let metadata: serde_json::Value = serde_json::from_str(&metadata_content)
            .unwrap_or_else(|_| panic!("Failed to parse session {} metadata JSON", idx));

        // Verify session metadata
        assert_eq!(
            metadata["session_id"].as_str().unwrap(),
            session_id.to_string(),
            "Session {} metadata should have correct session ID",
            idx
        );

        let message_count = metadata["message_count"].as_i64().unwrap();
        assert!(message_count > 0, "Session {} should have messages", idx);
        total_messages += message_count;

        let tool_call_count = metadata["tool_call_count"].as_i64().unwrap();
        total_tool_calls += tool_call_count;

        assert_eq!(
            metadata["compression_status"].as_str().unwrap(),
            "pending",
            "Session {} compression status should be pending",
            idx
        );

        assert!(
            metadata["ended_at"].as_i64().is_some(),
            "Session {} should have an end timestamp",
            idx
        );

        assert!(
            metadata["content_hash"].as_str().is_some(),
            "Session {} should have a content hash",
            idx
        );
    }

    println!("✓ Session isolation verified: no cross-contamination");

    // Verify: FTS5 index integrity
    // Search for content from each session and ensure correct results
    for (idx, session_id) in completed_sessions.iter().enumerate() {
        let search_query = format!("Session {} Turn", session_id);
        let search_results = store
            .search_transcripts(&search_query, None, 10)
            .await
            .expect("Failed to search transcripts");

        assert!(
            !search_results.is_empty(),
            "Session {} should have searchable content",
            idx
        );

        // All results should be from this session only
        for result in search_results.iter() {
            assert_eq!(
                result.session_id,
                session_id.to_string(),
                "Search for session {} should only return results from that session",
                idx
            );
        }
    }

    println!("✓ FTS5 index integrity verified: searches return correct sessions");

    // Verify: Store API methods work for all sessions
    for (idx, session_id) in completed_sessions.iter().enumerate() {
        // Read transcript
        let transcript = store
            .read_transcript(&session_id.to_string())
            .await
            .unwrap_or_else(|_| panic!("Failed to read session {} transcript via API", idx));

        assert!(
            transcript.contains(&session_id.to_string()),
            "Session {} transcript via API should contain session ID",
            idx
        );

        // Read tool outputs
        let tool_outputs = store
            .read_tool_outputs(&session_id.to_string())
            .await
            .unwrap_or_else(|_| panic!("Failed to read session {} tool outputs via API", idx));

        assert!(
            !tool_outputs.is_empty(),
            "Session {} tool outputs via API should not be empty",
            idx
        );

        // Get metadata
        let metadata = store
            .get_session_metadata(&session_id.to_string())
            .await
            .unwrap_or_else(|_| panic!("Failed to get session {} metadata via API", idx));

        assert_eq!(
            metadata.session_id,
            session_id.to_string(),
            "Session {} metadata via API should have correct ID",
            idx
        );

        assert_eq!(
            metadata.compression_status,
            CompressionStatus::Pending,
            "Session {} compression status should be pending",
            idx
        );
    }

    println!("✓ Store API methods work correctly for all sessions");

    // Final statistics
    println!("\n=== Stress Test Results ===");
    println!("Sessions created:       {}", NUM_SESSIONS);
    println!("Events per session:     {}", EVENTS_PER_SESSION);
    println!(
        "Total events emitted:   {}",
        NUM_SESSIONS * EVENTS_PER_SESSION
    );
    println!("Total messages stored:  {}", total_messages);
    println!("Total tool calls:       {}", total_tool_calls);
    println!(
        "\n✓ All {} sessions passed concurrent stress test",
        NUM_SESSIONS
    );
    println!("✓ No race conditions detected");
    println!("✓ No data corruption detected");
    println!("✓ No cross-session contamination detected");
    println!("✓ FTS5 index maintains integrity under concurrent load");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_sessions_repeated() {
    // Run the stress test 10 times to catch intermittent race conditions
    const ITERATIONS: usize = 10;

    for iteration in 0..ITERATIONS {
        println!("\n--- Iteration {}/{} ---", iteration + 1, ITERATIONS);

        // Create a fresh temp directory for each iteration
        let temp_dir = TempDir::new().unwrap();
        let store = Arc::new(
            MarkdownSessionStore::new(temp_dir.path())
                .await
                .expect("Failed to create MarkdownSessionStore"),
        );

        let projector = Arc::new(MarkdownProjector::new(store.clone()));

        // Run 3 sessions concurrently (smaller scale for repeated testing)
        let mut join_set = JoinSet::new();
        let session_ids: Vec<SessionId> = (0..3)
            .map(|idx| SessionId::ephemeral(format!("concurrent-repeated-{iteration}-{idx}")))
            .collect();

        for session_id in session_ids.iter().cloned() {
            let projector_clone = projector.clone();
            join_set.spawn(async move { emit_session(session_id, projector_clone, 30).await });
        }

        // Wait for completion
        while let Some(result) = join_set.join_next().await {
            result.expect("Task should not panic");
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Quick verification: all sessions have metadata
        for session_id in session_ids.iter() {
            let metadata = store
                .get_session_metadata(&session_id.to_string())
                .await
                .expect("Metadata should exist");

            assert_eq!(metadata.session_id, session_id.to_string());
        }

        println!("✓ Iteration {}/{} passed", iteration + 1, ITERATIONS);
    }

    println!("\n✓ All {} iterations passed consistently", ITERATIONS);
    println!("✓ No intermittent race conditions detected");
}
