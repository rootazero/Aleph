//! Tests for types module

use crate::agent_loop::RequestContext;
use crate::components::types::*;

#[test]
fn test_knowledge_creation() {
    let knowledge = Knowledge::new("db_path", "./config/db.toml", "search_files");
    assert_eq!(knowledge.key, "db_path");
    assert_eq!(knowledge.value, "./config/db.toml");
    assert_eq!(knowledge.source, "search_files");
    assert!(knowledge.confidence >= 0.0 && knowledge.confidence <= 1.0);
}

#[test]
fn test_entity_creation() {
    let entity = Entity::new("project", "Aether");
    assert_eq!(entity.entity_type, "project");
    assert_eq!(entity.value, "Aether");
}

#[test]
fn test_user_intent_creation() {
    let intent = UserIntent::new("Help me deploy the project")
        .understood_as("Deploy current project to remote server")
        .with_entity(Entity::new("project", "Aether"))
        .with_expectation("Don't break existing service");

    assert_eq!(intent.raw_input, "Help me deploy the project");
    assert_eq!(
        intent.understood_as,
        Some("Deploy current project to remote server".to_string())
    );
    assert_eq!(intent.key_entities.len(), 1);
    assert_eq!(intent.implicit_expectations.len(), 1);
}

#[test]
fn test_goal_creation() {
    let goal = Goal::new("Find project config files")
        .with_success_criteria("Located Cargo.toml and verified build target")
        .with_parent("Deploy project");

    assert_eq!(goal.description, "Find project config files");
    assert!(goal.success_criteria.is_some());
    assert!(goal.parent_goal.is_some());
}

#[test]
fn test_execution_context_creation() {
    let intent = UserIntent::new("Deploy the project");
    let goal = Goal::new("Find configuration");

    let ctx = ExecutionContext::new(intent, goal);

    assert_eq!(ctx.original_intent.raw_input, "Deploy the project");
    assert_eq!(ctx.current_goal.description, "Find configuration");
    assert!(ctx.decision_trail.is_empty());
    assert!(ctx.acquired_knowledge.is_empty());
    assert_eq!(ctx.phase, ExecutionPhase::Understanding);
}

#[test]
fn test_execution_context_add_knowledge() {
    let intent = UserIntent::new("Test");
    let goal = Goal::new("Test goal");
    let mut ctx = ExecutionContext::new(intent, goal);

    ctx.add_knowledge(Knowledge::new("key", "value", "test_tool"));

    assert_eq!(ctx.acquired_knowledge.len(), 1);
    assert_eq!(ctx.acquired_knowledge[0].key, "key");
}

#[test]
fn test_execution_context_add_decision() {
    let intent = UserIntent::new("Test");
    let goal = Goal::new("Test goal");
    let mut ctx = ExecutionContext::new(intent, goal);

    ctx.add_decision(
        "Use search_files tool",
        "Need to find config location first",
        vec!["read_file".to_string(), "list_dir".to_string()],
    );

    assert_eq!(ctx.decision_trail.len(), 1);
    assert_eq!(ctx.decision_trail[0].choice, "Use search_files tool");
}

#[test]
fn test_context_verbosity_prompt_generation() {
    let intent = UserIntent::new("Deploy project").understood_as("Deploy to server");
    let goal = Goal::new("Find config");
    let mut ctx = ExecutionContext::new(intent, goal);
    ctx.add_knowledge(Knowledge::new("project_type", "rust", "analysis").with_confidence(0.95));
    ctx.add_decision("Analyze project first", "Need to understand structure", vec![]);

    let minimal = ctx.to_prompt(ContextVerbosity::Minimal);
    assert!(minimal.contains("Find config"));
    assert!(minimal.contains("project_type=rust"));

    let full = ctx.to_prompt(ContextVerbosity::Full);
    assert!(full.contains("Deploy project"));
    assert!(full.contains("Deploy to server"));
    assert!(full.contains("Decision History"));
}

#[test]
fn test_part_id_trait() {
    // Test ToolCallPart ID extraction
    let tool_call = SessionPart::ToolCall(ToolCallPart {
        id: "call-123".to_string(),
        tool_name: "search".to_string(),
        input: serde_json::json!({}),
        status: ToolCallStatus::Running,
        output: None,
        error: None,
        started_at: 1000,
        completed_at: None,
    });
    assert_eq!(tool_call.part_id(), "call-123");
    assert_eq!(tool_call.type_name(), "tool_call");

    // Test PlanPart ID extraction
    let plan = SessionPart::PlanCreated(PlanPart {
        plan_id: "plan-456".to_string(),
        steps: vec![crate::components::types::PlanStep {
            step_id: "step-1".to_string(),
            description: "Step 1".to_string(),
            status: crate::components::types::StepStatus::Pending,
            dependencies: vec![],
        }],
        requires_confirmation: false,
        created_at: 2000,
    });
    assert_eq!(plan.part_id(), "plan-456");
    assert_eq!(plan.type_name(), "plan_created");

    // Test UserInputPart ID (uses timestamp)
    let input = SessionPart::UserInput(UserInputPart {
        text: "Hello".to_string(),
        context: None,
        timestamp: 3000,
    });
    assert_eq!(input.part_id(), "user_input_3000");
    assert_eq!(input.type_name(), "user_input");
}

#[test]
fn test_part_update_data_creation() {
    let tool_call = SessionPart::ToolCall(ToolCallPart {
        id: "call-789".to_string(),
        tool_name: "web_fetch".to_string(),
        input: serde_json::json!({"url": "https://example.com"}),
        status: ToolCallStatus::Completed,
        output: Some("Page content".to_string()),
        error: None,
        started_at: 1000,
        completed_at: Some(2000),
    });

    // Test added event
    let added = PartUpdateData::added("session-1", &tool_call);
    assert_eq!(added.session_id, "session-1");
    assert_eq!(added.part_id, "call-789");
    assert_eq!(added.part_type, "tool_call");
    assert_eq!(added.event_type, PartEventType::Added);
    assert!(added.delta.is_none());
    assert!(!added.part_json.is_empty());

    // Test updated event with delta
    let updated = PartUpdateData::updated("session-1", &tool_call, Some("output chunk".to_string()));
    assert_eq!(updated.event_type, PartEventType::Updated);
    assert_eq!(updated.delta, Some("output chunk".to_string()));

    // Test text delta event
    let delta = PartUpdateData::text_delta("session-1", "resp-1", "ai_response", "Hello, ");
    assert_eq!(delta.part_id, "resp-1");
    assert_eq!(delta.part_type, "ai_response");
    assert_eq!(delta.event_type, PartEventType::Updated);
    assert_eq!(delta.delta, Some("Hello, ".to_string()));
    assert!(delta.part_json.is_empty()); // text_delta doesn't include full part

    // Test removed event
    let removed = PartUpdateData::removed("session-1", "call-789", "tool_call");
    assert_eq!(removed.part_id, "call-789");
    assert_eq!(removed.event_type, PartEventType::Removed);
    assert!(removed.part_json.is_empty());
}

#[test]
fn test_part_event_type_display() {
    assert_eq!(format!("{}", PartEventType::Added), "added");
    assert_eq!(format!("{}", PartEventType::Updated), "updated");
    assert_eq!(format!("{}", PartEventType::Removed), "removed");
}

#[test]
fn test_system_reminder_part() {
    let reminder = SessionPart::SystemReminder(SystemReminderPart {
        content: "Continue with your tasks".to_string(),
        reminder_type: ReminderType::ContinueTask,
        timestamp: 1000,
    });

    assert_eq!(reminder.type_name(), "system_reminder");
    assert!(reminder.part_id().starts_with("reminder_"));
}

#[test]
fn test_execution_session_with_request_context() {
    let ctx = RequestContext {
        current_app: Some("Terminal".to_string()),
        working_directory: Some("/tmp".to_string()),
        ..Default::default()
    };

    let session = ExecutionSession::new()
        .with_original_request("Find files")
        .with_context(ctx);

    assert_eq!(session.original_request, "Find files");
    assert!(session.context.is_some());
    assert_eq!(session.context.as_ref().unwrap().current_app, Some("Terminal".to_string()));
    assert!(!session.needs_compaction);
}

// =========================================================================
// Tests for new SessionPart types (step boundaries, snapshots, streaming)
// =========================================================================

#[test]
fn test_step_start_part() {
    let step = StepStartPart::new(1);
    assert_eq!(step.step_id, 1);
    assert!(step.timestamp > 0);
    assert!(step.snapshot_id.is_none());

    let step_with_snapshot = StepStartPart::with_snapshot(2, "snap-123".to_string());
    assert_eq!(step_with_snapshot.step_id, 2);
    assert_eq!(step_with_snapshot.snapshot_id, Some("snap-123".to_string()));
}

#[test]
fn test_step_finish_part() {
    let finish = StepFinishPart::new(1, StepFinishReason::Completed, 500);
    assert_eq!(finish.step_id, 1);
    assert_eq!(finish.reason, StepFinishReason::Completed);
    assert_eq!(finish.duration_ms, 500);
    assert!(finish.tokens.is_none());

    let finish_with_tokens = StepFinishPart::with_tokens(
        2,
        StepFinishReason::Failed,
        1000,
        StepTokenUsage::new(100, 50),
    );
    assert_eq!(finish_with_tokens.step_id, 2);
    assert_eq!(finish_with_tokens.reason, StepFinishReason::Failed);
    assert_eq!(finish_with_tokens.tokens.as_ref().unwrap().total(), 150);
}

#[test]
fn test_step_finish_reason_variants() {
    assert_eq!(StepFinishReason::default(), StepFinishReason::Completed);
    assert_ne!(StepFinishReason::Failed, StepFinishReason::Completed);
    assert_ne!(StepFinishReason::UserAborted, StepFinishReason::ToolError);
    assert_ne!(StepFinishReason::MaxStepsReached, StepFinishReason::Failed);
}

#[test]
fn test_step_token_usage() {
    let usage = StepTokenUsage::new(100, 50);
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.total(), 150);

    let default = StepTokenUsage::default();
    assert_eq!(default.total(), 0);
}

#[test]
fn test_file_snapshot() {
    let file = FileSnapshot::new("/src/main.rs", "abc123");
    assert_eq!(file.path, "/src/main.rs");
    assert_eq!(file.hash, "abc123");
}

#[test]
fn test_snapshot_part() {
    let mut snapshot = SnapshotPart::new("snap-001");
    assert_eq!(snapshot.snapshot_id, "snap-001");
    assert!(snapshot.files.is_empty());
    assert!(snapshot.timestamp > 0);

    snapshot.add_file("/src/main.rs", "hash1");
    snapshot.add_file("/Cargo.toml", "hash2");
    assert_eq!(snapshot.files.len(), 2);

    let files = vec![
        FileSnapshot::new("/a.rs", "h1"),
        FileSnapshot::new("/b.rs", "h2"),
    ];
    let snapshot2 = SnapshotPart::with_files("snap-002", files);
    assert_eq!(snapshot2.files.len(), 2);
}

#[test]
fn test_file_change() {
    let added = FileChange::added("/new.rs", "hash1");
    assert_eq!(added.change_type, FileChangeType::Added);
    assert_eq!(added.content_hash, Some("hash1".to_string()));

    let modified = FileChange::modified("/existing.rs", "hash2");
    assert_eq!(modified.change_type, FileChangeType::Modified);
    assert_eq!(modified.content_hash, Some("hash2".to_string()));

    let deleted = FileChange::deleted("/old.rs");
    assert_eq!(deleted.change_type, FileChangeType::Deleted);
    assert!(deleted.content_hash.is_none());
}

#[test]
fn test_patch_part() {
    let mut patch = PatchPart::new("patch-001", "snap-000");
    assert_eq!(patch.patch_id, "patch-001");
    assert_eq!(patch.base_snapshot_id, "snap-000");
    assert!(patch.changes.is_empty());

    patch.add_change(FileChange::added("/new.rs", "h1"));
    patch.add_change(FileChange::modified("/main.rs", "h2"));
    assert_eq!(patch.changes.len(), 2);

    let changes = vec![
        FileChange::added("/a.rs", "h1"),
        FileChange::deleted("/b.rs"),
    ];
    let patch2 = PatchPart::with_changes("patch-002", "snap-001", changes);
    assert_eq!(patch2.changes.len(), 2);
}

#[test]
fn test_streaming_text_part() {
    let mut stream = StreamingTextPart::new("stream-001");
    assert_eq!(stream.part_id, "stream-001");
    assert!(stream.content.is_empty());
    assert!(!stream.is_complete);
    assert!(stream.delta.is_none());

    stream.append("Hello, ");
    assert_eq!(stream.content, "Hello, ");
    assert_eq!(stream.delta, Some("Hello, ".to_string()));

    stream.append("World!");
    assert_eq!(stream.content, "Hello, World!");
    assert_eq!(stream.delta, Some("World!".to_string()));

    stream.complete();
    assert!(stream.is_complete);
    assert!(stream.delta.is_none());
}

#[test]
fn test_streaming_text_with_content() {
    let stream = StreamingTextPart::with_content("stream-002", "Initial content");
    assert_eq!(stream.content, "Initial content");
    assert!(!stream.is_complete);
}

#[test]
fn test_compaction_marker_constructors() {
    let marker = CompactionMarker::new(true);
    assert!(marker.auto);
    assert!(marker.timestamp > 0);
    assert!(marker.marker_id.is_none());
    assert!(marker.parts_compacted.is_none());
    assert!(marker.tokens_freed.is_none());

    let marker2 = CompactionMarker::with_timestamp(1000, false);
    assert_eq!(marker2.timestamp, 1000);
    assert!(!marker2.auto);

    let marker3 = CompactionMarker::with_details(true, "m-001".to_string(), 10, 5000);
    assert!(marker3.auto);
    assert_eq!(marker3.marker_id, Some("m-001".to_string()));
    assert_eq!(marker3.parts_compacted, Some(10));
    assert_eq!(marker3.tokens_freed, Some(5000));
}

#[test]
fn test_new_session_part_type_names() {
    let step_start = SessionPart::StepStart(StepStartPart::new(1));
    assert_eq!(step_start.type_name(), "step_start");

    let step_finish = SessionPart::StepFinish(StepFinishPart::new(1, StepFinishReason::Completed, 100));
    assert_eq!(step_finish.type_name(), "step_finish");

    let snapshot = SessionPart::Snapshot(SnapshotPart::new("s-001"));
    assert_eq!(snapshot.type_name(), "snapshot");

    let patch = SessionPart::Patch(PatchPart::new("p-001", "s-000"));
    assert_eq!(patch.type_name(), "patch");

    let streaming = SessionPart::StreamingText(StreamingTextPart::new("st-001"));
    assert_eq!(streaming.type_name(), "streaming_text");
}

#[test]
fn test_new_session_part_ids() {
    let step_start = SessionPart::StepStart(StepStartPart::new(5));
    assert_eq!(step_start.part_id(), "step_start_5");

    let step_finish = SessionPart::StepFinish(StepFinishPart::new(5, StepFinishReason::Completed, 100));
    assert_eq!(step_finish.part_id(), "step_finish_5");

    let snapshot = SessionPart::Snapshot(SnapshotPart::new("snap-123"));
    assert_eq!(snapshot.part_id(), "snap-123");

    let patch = SessionPart::Patch(PatchPart::new("patch-456", "snap-123"));
    assert_eq!(patch.part_id(), "patch-456");

    let streaming = SessionPart::StreamingText(StreamingTextPart::new("stream-789"));
    assert_eq!(streaming.part_id(), "stream-789");
}
