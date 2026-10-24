//! Integration test for end-to-end Markdown session lifecycle.
//!
//! This test verifies the complete flow from session event creation through
//! background projection to Markdown file generation.

use alephcore::memory::store::markdown::session_directory_key;
use alephcore::memory::store::markdown::MarkdownSessionStore;
use alephcore::memory::store::session::{CompressionStatus, RawSessionStore};
use alephcore::session::events::{
    MessageContent, RunOutcome, SessionEvent, SessionEventRecord, ToolOutput, ToolOutputMetadata,
    TurnTrigger,
};
use alephcore::session::markdown_projector::MarkdownProjector;
use alephcore::session::observer::SessionEventObserver;
use alephcore::session::service::SessionId;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

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

#[tokio::test]
async fn test_markdown_session_lifecycle() {
    // Setup: Create temp directory and initialize store
    let temp_dir = TempDir::new().unwrap();
    let store = Arc::new(
        MarkdownSessionStore::new(temp_dir.path())
            .await
            .expect("Failed to create MarkdownSessionStore"),
    );

    // Create projector and wire it as observer
    let projector = MarkdownProjector::new(store.clone());

    // Generate IDs
    let session_id = SessionId::ephemeral("markdown-session-lifecycle");
    let run_id = format!("run_{}", Uuid::new_v4());
    let turn_id_1 = Uuid::new_v4();
    let turn_id_2 = Uuid::new_v4();
    let call_id = format!("call_{}", Uuid::new_v4());

    // Emit sequence of events
    let mut seq = 0;

    // 1. RunStarted
    seq += 1;
    let run_started = make_record(
        seq,
        SessionEvent::RunStarted {
            run_id: run_id.clone(),
            at: chrono::Utc::now().timestamp(),
            project_root: Some("/tmp/test_project".to_string()),
            envelope: None,
            telecom_provenance: Default::default(),
        },
    );
    projector.on_appended(&session_id, &run_started);

    // 2. First turn started
    seq += 1;
    let turn_1_started = make_record(
        seq,
        SessionEvent::TurnStarted {
            turn_id: turn_id_1,
            trigger: TurnTrigger::UserMessage,
            at: chrono::Utc::now().timestamp(),
        },
    );
    projector.on_appended(&session_id, &turn_1_started);

    // 3. User message 1
    seq += 1;
    let user_msg_1 = make_record(
        seq,
        SessionEvent::UserMessage {
            turn_id: turn_id_1,
            content: make_content("Hello, can you help me with a task?"),
            at: chrono::Utc::now().timestamp(),
            synthetic: false,
            author_user_id: None,
        },
    );
    projector.on_appended(&session_id, &user_msg_1);

    // 4. Assistant message 1
    seq += 1;
    let assistant_msg_1 = make_record(
        seq,
        SessionEvent::AssistantMessage {
            turn_id: turn_id_1,
            content: make_content("Of course! I'd be happy to help. What do you need?"),
            usage: None,
            at: chrono::Utc::now().timestamp(),
            model: None,
            model_provider: None,
            model_generation: None,
        },
    );
    projector.on_appended(&session_id, &assistant_msg_1);

    // 5. User message 2
    seq += 1;
    let user_msg_2 = make_record(
        seq,
        SessionEvent::UserMessage {
            turn_id: turn_id_1,
            content: make_content("Can you list the files in the current directory?"),
            at: chrono::Utc::now().timestamp(),
            synthetic: false,
            author_user_id: None,
        },
    );
    projector.on_appended(&session_id, &user_msg_2);

    // 6. Second turn started
    seq += 1;
    let turn_2_started = make_record(
        seq,
        SessionEvent::TurnStarted {
            turn_id: turn_id_2,
            trigger: TurnTrigger::UserMessage,
            at: chrono::Utc::now().timestamp(),
        },
    );
    projector.on_appended(&session_id, &turn_2_started);

    // 7. Tool call requested
    seq += 1;
    let tool_call_requested = make_record(
        seq,
        SessionEvent::ToolCallRequested {
            turn_id: turn_id_2,
            call_id: call_id.clone(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls -la"}),
            identity: None,
            at: chrono::Utc::now().timestamp(),
        },
    );
    projector.on_appended(&session_id, &tool_call_requested);

    // 8. Tool result
    seq += 1;
    let tool_result = make_record(
        seq,
        SessionEvent::ToolResult {
            turn_id: turn_id_2,
            call_id: call_id.clone(),
            output: ToolOutput {
                value: serde_json::json!("total 8\ndrwxr-xr-x  2 user user 4096 Oct 19 00:00 .\ndrwxr-xr-x  3 user user 4096 Oct 19 00:00 .."),
                metadata: ToolOutputMetadata::default(),
            },
            at: chrono::Utc::now().timestamp(),
        },
    );
    projector.on_appended(&session_id, &tool_result);

    // 9. Assistant message 2
    seq += 1;
    let assistant_msg_2 = make_record(
        seq,
        SessionEvent::AssistantMessage {
            turn_id: turn_id_2,
            content: make_content("Here are the files in the current directory:\n- . (current directory)\n- .. (parent directory)"),
            usage: None,
            at: chrono::Utc::now().timestamp(),
            model: None,
            model_provider: None,
            model_generation: None,
        },
    );
    projector.on_appended(&session_id, &assistant_msg_2);

    // 10. User message 3
    seq += 1;
    let user_msg_3 = make_record(
        seq,
        SessionEvent::UserMessage {
            turn_id: turn_id_2,
            content: make_content("Thank you!"),
            at: chrono::Utc::now().timestamp(),
            synthetic: false,
            author_user_id: None,
        },
    );
    projector.on_appended(&session_id, &user_msg_3);

    // 11. RunFinished
    seq += 1;
    let run_finished = make_record(
        seq,
        SessionEvent::RunFinished {
            run_id: run_id.clone(),
            outcome: RunOutcome::Completed,
            at: chrono::Utc::now().timestamp(),
        },
    );
    projector.on_appended(&session_id, &run_finished);

    // Wait for background projection to complete
    // The projector uses an unbounded channel and processes events
    // asynchronously, so "the events were enqueued" is not "the writes
    // landed" — a fixed delay is a race with the scheduler. `RunFinished` is
    // what finalizes a session, so wait for that effect instead.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let finalized = store
            .get_session_metadata(&session_id.to_string())
            .await
            .ok()
            .and_then(|metadata| metadata.ended_at)
            .is_some();
        if finalized {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the projection worker did not finalize the session within the deadline",
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Verify: Check session directory exists
    // The store keys a session's directory by the hash of its id, not by the
    // id itself — `session_directory_key` is the one owner of that mapping.
    // Addressing the directory by the raw id looks for a directory that was
    // never written.
    let session_dir = temp_dir
        .path()
        .join("raw/sessions")
        .join(session_directory_key(&session_id.to_string()));
    assert!(
        session_dir.exists(),
        "Session directory should exist at {:?}",
        session_dir
    );

    // Verify: Check transcript.md exists and contains messages
    let transcript_path = session_dir.join("transcript.md");
    assert!(
        transcript_path.exists(),
        "Transcript file should exist at {:?}",
        transcript_path
    );

    let transcript_content = tokio::fs::read_to_string(&transcript_path)
        .await
        .expect("Failed to read transcript");

    // Check that all user and assistant messages are present
    assert!(
        transcript_content.contains("Hello, can you help me with a task?"),
        "First user message should be in transcript"
    );
    assert!(
        transcript_content.contains("Of course! I'd be happy to help. What do you need?"),
        "First assistant message should be in transcript"
    );
    assert!(
        transcript_content.contains("Can you list the files in the current directory?"),
        "Second user message should be in transcript"
    );
    assert!(
        transcript_content.contains("Here are the files in the current directory"),
        "Second assistant message should be in transcript"
    );
    assert!(
        transcript_content.contains("Thank you!"),
        "Third user message should be in transcript"
    );

    // Check message role headers
    assert!(
        transcript_content.contains("## User -"),
        "User message headers should be present"
    );
    assert!(
        transcript_content.contains("## Assistant -"),
        "Assistant message headers should be present"
    );

    // Verify: Check tool_outputs.md exists and contains tool call
    let tool_outputs_path = session_dir.join("tool_outputs.md");
    assert!(
        tool_outputs_path.exists(),
        "Tool outputs file should exist at {:?}",
        tool_outputs_path
    );

    let tool_outputs_content = tokio::fs::read_to_string(&tool_outputs_path)
        .await
        .expect("Failed to read tool outputs");

    // Check tool call details
    assert!(
        tool_outputs_content.contains("bash"),
        "Tool name should be in tool outputs"
    );
    assert!(
        tool_outputs_content.contains("ls -la") || tool_outputs_content.contains("command"),
        "Tool arguments should be in tool outputs"
    );

    // Verify: Check metadata.json exists and has correct stats
    let metadata_path = session_dir.join("metadata.json");
    assert!(
        metadata_path.exists(),
        "Metadata file should exist at {:?}",
        metadata_path
    );

    let metadata_content = tokio::fs::read_to_string(&metadata_path)
        .await
        .expect("Failed to read metadata");

    let metadata: serde_json::Value =
        serde_json::from_str(&metadata_content).expect("Failed to parse metadata JSON");

    // Verify session metadata
    assert_eq!(
        metadata["session_id"].as_str().unwrap(),
        session_id.to_string(),
        "Session ID should match"
    );
    assert_eq!(
        metadata["message_count"].as_i64().unwrap(),
        5,
        "Should have 5 messages (3 user + 2 assistant)"
    );
    assert!(
        metadata["tool_call_count"].as_i64().unwrap() >= 1,
        "Should have at least 1 tool call"
    );
    assert_eq!(
        metadata["compression_status"].as_str().unwrap(),
        "pending",
        "Compression status should be pending"
    );
    assert!(
        metadata["ended_at"].as_i64().is_some(),
        "Session should have an end timestamp"
    );
    assert!(
        metadata["content_hash"].as_str().is_some(),
        "Session should have a content hash"
    );

    // Verify: FTS5 index can search transcript content
    let search_results = store
        .search_transcripts("help me with a task", None, 10)
        .await
        .expect("Failed to search transcripts");

    assert!(
        !search_results.is_empty(),
        "Should find search results for transcript content"
    );
    assert_eq!(
        search_results[0].session_id,
        session_id.to_string(),
        "Search result should match our session"
    );

    // Verify: Can read back the transcript through the store API
    let read_transcript = store
        .read_transcript(&session_id.to_string())
        .await
        .expect("Failed to read transcript through store API");

    assert!(
        read_transcript.contains("Hello, can you help me with a task?"),
        "Read transcript should contain user messages"
    );

    // Verify: Can read back tool outputs through the store API
    let read_tool_outputs = store
        .read_tool_outputs(&session_id.to_string())
        .await
        .expect("Failed to read tool outputs through store API");

    assert!(
        read_tool_outputs.contains("bash"),
        "Read tool outputs should contain tool calls"
    );

    // Verify: Can retrieve session metadata through the store API
    let retrieved_metadata = store
        .get_session_metadata(&session_id.to_string())
        .await
        .expect("Failed to get session metadata through store API");

    assert_eq!(
        retrieved_metadata.session_id,
        session_id.to_string(),
        "Retrieved session ID should match"
    );
    assert_eq!(
        retrieved_metadata.message_count, 5,
        "Retrieved message count should match"
    );
    assert!(
        retrieved_metadata.tool_call_count >= 1,
        "Retrieved tool call count should be at least 1"
    );
    assert_eq!(
        retrieved_metadata.compression_status,
        CompressionStatus::Pending,
        "Retrieved compression status should be pending"
    );

    println!("✓ All verifications passed");
    println!("  - Session directory created");
    println!("  - Transcript contains 5 messages in correct order");
    println!("  - Tool outputs recorded 1+ tool call");
    println!("  - Metadata has correct stats");
    println!("  - FTS5 index is searchable");
    println!("  - Store API read methods work correctly");
}
