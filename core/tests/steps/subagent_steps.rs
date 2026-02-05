//! Step definitions for Sub-Agent orchestration features

use std::time::Duration;

use cucumber::{given, when, then};

use crate::world::{AlephWorld, SubagentContext};
use alephcore::gateway::run_event_bus::{
    wait_for_run_end, RunEvent, RunStatus, RunEndResult,
};
use alephcore::providers::auth_profiles::AuthProfileFailureReason;

#[cfg(feature = "gateway")]
use alephcore::builtin_tools::sessions::{
    CleanupPolicy, SessionsSpawnArgs, SessionsSpawnTool, SpawnStatus,
};
#[cfg(feature = "gateway")]
use alephcore::tools::AlephTool;

// =============================================================================
// RunEventBus: Given Steps
// =============================================================================

#[given(expr = "a new RunEventBus handle with run_id {string}")]
async fn given_run_event_bus_handle(w: &mut AlephWorld, run_id: String) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    ctx.create_run_handle(&run_id);
}

// =============================================================================
// RunEventBus: When Steps
// =============================================================================

#[when("I subscribe to the event bus")]
async fn when_subscribe_to_event_bus(w: &mut AlephWorld) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    ctx.event_rx = Some(handle.subscribe());
}

#[when("I create two subscribers")]
async fn when_create_two_subscribers(w: &mut AlephWorld) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    ctx.event_rx = Some(handle.subscribe());
    ctx.event_rx2 = Some(handle.subscribe());
}

#[when("I emit a status changed event to Running")]
async fn when_emit_status_changed_running(w: &mut AlephWorld) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    let _seq = handle.next_seq();
    handle.emit(RunEvent::StatusChanged {
        run_id: handle.run_id.clone(),
        status: RunStatus::Running,
        reason: None,
    });
}

#[when(expr = "I emit a run completed event with summary {string} and tokens {int}")]
async fn when_emit_run_completed(w: &mut AlephWorld, summary: String, tokens: i64) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    let seq = handle.next_seq();
    handle.emit(RunEvent::RunCompleted {
        run_id: handle.run_id.clone(),
        seq,
        summary: Some(summary),
        total_tokens: tokens as u64,
        tool_calls: 2,
        loops: 1,
        duration_ms: 1000,
    });
}

#[when(expr = "I emit a run failed event with error {string} and code {string}")]
async fn when_emit_run_failed(w: &mut AlephWorld, error: String, code: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    let seq = handle.next_seq();
    handle.emit(RunEvent::RunFailed {
        run_id: handle.run_id.clone(),
        seq,
        error,
        error_code: Some(code),
    });
}

#[when(expr = "I emit a run cancelled event with reason {string}")]
async fn when_emit_run_cancelled(w: &mut AlephWorld, reason: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    let seq = handle.next_seq();
    handle.emit(RunEvent::RunCancelled {
        run_id: handle.run_id.clone(),
        seq,
        reason: Some(reason),
    });
}

#[when("I get the input sender")]
async fn when_get_input_sender(w: &mut AlephWorld) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    ctx.input_tx = Some(handle.input_sender());
}

#[when(expr = "I send {string} through the input sender")]
async fn when_send_through_input_sender(w: &mut AlephWorld, message: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let input_tx = ctx.input_tx.as_ref().expect("Input sender not obtained");
    input_tx.send(message).await.unwrap();
}

// =============================================================================
// RunEventBus: Then Steps
// =============================================================================

#[then(expr = "waiting for run end should return Completed with summary {string} and tokens {int}")]
async fn then_wait_for_completed(w: &mut AlephWorld, expected_summary: String, expected_tokens: i64) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let rx = ctx.event_rx.as_mut().expect("Event receiver not initialized");
    let result = wait_for_run_end(rx, Duration::from_secs(5)).await;
    assert!(result.is_ok(), "Expected Ok result, got: {:?}", result);
    match result.unwrap() {
        RunEndResult::Completed { summary, total_tokens, .. } => {
            assert_eq!(summary, Some(expected_summary));
            assert_eq!(total_tokens, expected_tokens as u64);
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
}

#[then(expr = "waiting for run end should return Failed with error {string} and code {string}")]
async fn then_wait_for_failed(w: &mut AlephWorld, expected_error: String, expected_code: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let rx = ctx.event_rx.as_mut().expect("Event receiver not initialized");
    let result = wait_for_run_end(rx, Duration::from_secs(5)).await;
    assert!(result.is_ok(), "Expected Ok result, got: {:?}", result);
    match result.unwrap() {
        RunEndResult::Failed { error, error_code } => {
            assert_eq!(error, expected_error);
            assert_eq!(error_code, Some(expected_code));
        }
        other => panic!("Expected Failed, got {:?}", other),
    }
}

#[then(expr = "waiting for run end should return Cancelled with reason {string}")]
async fn then_wait_for_cancelled(w: &mut AlephWorld, expected_reason: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let rx = ctx.event_rx.as_mut().expect("Event receiver not initialized");
    let result = wait_for_run_end(rx, Duration::from_secs(5)).await;
    assert!(result.is_ok(), "Expected Ok result, got: {:?}", result);
    match result.unwrap() {
        RunEndResult::Cancelled { reason } => {
            assert_eq!(reason, Some(expected_reason));
        }
        other => panic!("Expected Cancelled, got {:?}", other),
    }
}

#[then("both subscribers should receive the Running status event")]
async fn then_both_receive_running(w: &mut AlephWorld) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");

    let rx1 = ctx.event_rx.as_mut().expect("Event receiver 1 not initialized");
    let rx2 = ctx.event_rx2.as_mut().expect("Event receiver 2 not initialized");

    let event1 = rx1.recv().await.unwrap();
    let event2 = rx2.recv().await.unwrap();

    assert!(
        matches!(event1, RunEvent::StatusChanged { status: RunStatus::Running, .. }),
        "Expected StatusChanged(Running) for subscriber 1, got {:?}", event1
    );
    assert!(
        matches!(event2, RunEvent::StatusChanged { status: RunStatus::Running, .. }),
        "Expected StatusChanged(Running) for subscriber 2, got {:?}", event2
    );
}

#[then("the sequence counter should start at 0")]
async fn then_seq_starts_at_0(w: &mut AlephWorld) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    // The next_seq() returns the current value and increments
    // So first call should return 0
    let seq = handle.next_seq();
    ctx.seq_values.push(seq);
    assert_eq!(seq, 0, "Expected sequence to start at 0");
}

#[then(expr = "incrementing the sequence should return {int}, {int}, {int} in order")]
async fn then_seq_increments(w: &mut AlephWorld, a: i64, b: i64, c: i64) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");

    // First call already happened in previous step
    let seq1 = handle.next_seq();
    let seq2 = handle.next_seq();

    // The first value was already captured (0), now we get 1 and 2
    assert_eq!(ctx.seq_values[0], a as u64, "First seq mismatch");
    assert_eq!(seq1, b as u64, "Second seq mismatch");
    assert_eq!(seq2, c as u64, "Third seq mismatch");
}

#[then(expr = "the current sequence should be {int}")]
async fn then_current_seq(w: &mut AlephWorld, expected: i64) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let handle = ctx.run_handle.as_ref().expect("Run handle not created");
    assert_eq!(handle.current_seq(), expected as u64);
}

#[then(expr = "the input receiver should receive {string}")]
async fn then_input_received(w: &mut AlephWorld, expected: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let rx = ctx.input_rx.as_mut().expect("Input receiver not initialized");
    let received = rx.recv().await.unwrap();
    assert_eq!(received, expected);
}

// =============================================================================
// AuthProfileManager: Given Steps
// =============================================================================

#[given(expr = "a temp profiles config with profile {string} for provider {string}")]
async fn given_temp_profiles_config(w: &mut AlephWorld, profile_id: String, provider: String) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    ctx.create_profile_manager(&profile_id, &provider);
}

#[given(expr = "a temp profiles config with primary {string} and backup {string} for provider {string}")]
async fn given_temp_profiles_primary_backup(w: &mut AlephWorld, primary: String, backup: String, provider: String) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    ctx.create_profile_manager_with_backup(&primary, &backup, &provider);
}

#[given("an empty profiles config")]
async fn given_empty_profiles_config(w: &mut AlephWorld) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    ctx.create_empty_profile_manager();
}

// =============================================================================
// AuthProfileManager: When Steps
// =============================================================================

#[when(expr = "I get available profile for provider {string} and agent {string}")]
async fn when_get_available_profile(w: &mut AlephWorld, provider: String, agent: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let manager = ctx.profile_manager.as_ref().expect("Profile manager not initialized");
    match manager.get_available_profile(&provider, &agent) {
        Ok(profile) => {
            ctx.profile_id = Some(profile.id.clone());
            ctx.profile_error = None;
        }
        Err(e) => {
            ctx.profile_id = None;
            ctx.profile_error = Some(e.to_string());
        }
    }
}

#[when(expr = "I try to get available profile for provider {string} and agent {string}")]
async fn when_try_get_available_profile(w: &mut AlephWorld, provider: String, agent: String) {
    // Same as above, just different step text
    when_get_available_profile(w, provider, agent).await;
}

#[when(expr = "I mark profile {string} as failed due to rate limit")]
async fn when_mark_profile_failed(w: &mut AlephWorld, profile_id: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let manager = ctx.profile_manager.as_ref().expect("Profile manager not initialized");
    manager.mark_failure(&profile_id, AuthProfileFailureReason::RateLimit).unwrap();
}

#[when(expr = "I mark profile {string} as success")]
async fn when_mark_profile_success(w: &mut AlephWorld, profile_id: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let manager = ctx.profile_manager.as_ref().expect("Profile manager not initialized");
    manager.mark_success(&profile_id).unwrap();
}

#[when(expr = "I record usage for agent {string} profile {string} with {int} input tokens and {int} output tokens and cost {float}")]
async fn when_record_usage(
    w: &mut AlephWorld,
    agent: String,
    profile: String,
    input_tokens: i64,
    output_tokens: i64,
    cost: f64,
) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let manager = ctx.profile_manager.as_ref().expect("Profile manager not initialized");
    manager
        .record_usage(&agent, &profile, input_tokens as u64, output_tokens as u64, cost)
        .unwrap();
}

// =============================================================================
// AuthProfileManager: Then Steps
// =============================================================================

#[then(expr = "the profile id should be {string}")]
async fn then_profile_id_should_be(w: &mut AlephWorld, expected: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let profile_id = ctx.profile_id.as_ref().expect("Profile ID not set");
    assert_eq!(profile_id, &expected);
}

#[then(expr = "profile {string} should be in cooldown")]
async fn then_profile_in_cooldown(w: &mut AlephWorld, profile_id: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let manager = ctx.profile_manager.as_ref().expect("Profile manager not initialized");
    let profiles = manager.list_profiles();
    let profile = profiles.iter().find(|p| p.id == profile_id).expect("Profile not found");
    assert!(profile.in_cooldown, "Profile {} should be in cooldown", profile_id);
}

#[then(expr = "profile {string} should not be in cooldown")]
async fn then_profile_not_in_cooldown(w: &mut AlephWorld, profile_id: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let manager = ctx.profile_manager.as_ref().expect("Profile manager not initialized");
    let profiles = manager.list_profiles();
    let profile = profiles.iter().find(|p| p.id == profile_id).expect("Profile not found");
    assert!(!profile.in_cooldown, "Profile {} should not be in cooldown", profile_id);
}

#[then(expr = "profile {string} should have failure count {int}")]
async fn then_profile_failure_count(w: &mut AlephWorld, profile_id: String, expected: i32) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let manager = ctx.profile_manager.as_ref().expect("Profile manager not initialized");
    let profiles = manager.list_profiles();
    let profile = profiles.iter().find(|p| p.id == profile_id).expect("Profile not found");
    assert_eq!(profile.failure_count, expected as u32, "Failure count mismatch");
}

#[then("the usage state file should exist")]
async fn then_usage_state_file_exists(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp dir not set");
    let state_path = temp_dir.path().join("agents").join("main").join("state.json");
    assert!(state_path.exists(), "Usage state file should exist: {:?}", state_path);
}

#[then(expr = "the usage state file should contain {string}")]
async fn then_usage_state_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let temp_dir = ctx.temp_dir.as_ref().expect("Temp dir not set");
    let state_path = temp_dir.path().join("agents").join("main").join("state.json");
    let content = std::fs::read_to_string(&state_path).unwrap();
    assert!(content.contains(&expected), "State file should contain '{}': {}", expected, content);
}

#[then(expr = "it should return an error containing {string}")]
async fn then_error_containing(w: &mut AlephWorld, expected: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let error = ctx.profile_error.as_ref().expect("Expected an error");
    assert!(error.contains(&expected), "Error should contain '{}': {}", expected, error);
}

// =============================================================================
// SessionsSpawnTool: Given Steps (gateway feature)
// =============================================================================

#[cfg(feature = "gateway")]
#[given(expr = "an agent id {string} and label {string}")]
async fn given_agent_id_and_label(w: &mut AlephWorld, agent_id: String, label: String) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    let expected_prefix = format!("agent:{}:subagent:{}", agent_id, label);
    ctx.session_key_prefix = Some(expected_prefix);
}

#[cfg(feature = "gateway")]
#[given("a sessions spawn tool with default wildcard authorization")]
async fn given_spawn_tool_default(w: &mut AlephWorld) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    ctx.spawn_tool = Some(SessionsSpawnTool::new());
}

#[cfg(feature = "gateway")]
#[given(expr = "a sessions spawn tool with allowed agents {string} and {string}")]
async fn given_spawn_tool_allowed_agents(w: &mut AlephWorld, agent1: String, agent2: String) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    let mut tool = SessionsSpawnTool::new();
    tool.set_allow_agents(vec![agent1, agent2]);
    ctx.spawn_tool = Some(tool);
}

#[cfg(feature = "gateway")]
#[given("a sessions spawn tool with empty allowed agents list")]
async fn given_spawn_tool_empty_allowed(w: &mut AlephWorld) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    let mut tool = SessionsSpawnTool::new();
    tool.set_allow_agents(vec![]);
    ctx.spawn_tool = Some(tool);
}

#[cfg(feature = "gateway")]
#[given("a sessions spawn tool without gateway context")]
async fn given_spawn_tool_no_context(w: &mut AlephWorld) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    ctx.spawn_tool = Some(SessionsSpawnTool::new());
}

// =============================================================================
// SessionsSpawnTool: When Steps (gateway feature)
// =============================================================================

#[cfg(feature = "gateway")]
#[when(expr = "I call spawn with task {string}")]
async fn when_call_spawn(w: &mut AlephWorld, task: String) {
    let ctx = w.subagent.as_mut().expect("Subagent context not initialized");
    let tool = ctx.spawn_tool.as_ref().expect("Spawn tool not initialized");
    let args = SessionsSpawnArgs {
        task,
        label: Some("test".to_string()),
        agent_id: None,
        model: None,
        thinking: None,
        run_timeout_seconds: 60,
        cleanup: CleanupPolicy::Ephemeral,
    };
    ctx.spawn_output = Some(AlephTool::call(tool, args).await.unwrap());
}

#[cfg(feature = "gateway")]
#[when("I get the default cleanup policy")]
async fn when_get_default_cleanup_policy(w: &mut AlephWorld) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    ctx.cleanup_policy = Some(CleanupPolicy::default());
}

#[cfg(feature = "gateway")]
#[when(expr = "I parse spawn args from JSON with only task {string}")]
async fn when_parse_spawn_args(w: &mut AlephWorld, task: String) {
    let ctx = w.subagent.get_or_insert_with(SubagentContext::default);
    let json = format!(r#"{{"task": "{}"}}"#, task);
    ctx.spawn_args = Some(serde_json::from_str(&json).unwrap());
}

// =============================================================================
// SessionsSpawnTool: Then Steps (gateway feature)
// =============================================================================

#[cfg(feature = "gateway")]
#[then(expr = "the subagent session key prefix should be {string}")]
async fn then_session_key_prefix(w: &mut AlephWorld, expected: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let prefix = ctx.session_key_prefix.as_ref().expect("Session key prefix not set");
    assert_eq!(prefix, &expected);
}

#[cfg(feature = "gateway")]
#[then(expr = "authorization for {string} should succeed")]
async fn then_auth_succeeds(w: &mut AlephWorld, agent: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let tool = ctx.spawn_tool.as_ref().expect("Spawn tool not initialized");
    let result = tool.check_authorization(&agent);
    assert!(result.is_ok(), "Expected authorization to succeed for '{}', got: {:?}", agent, result);
}

#[cfg(feature = "gateway")]
#[then(expr = "authorization for {string} should fail")]
async fn then_auth_fails(w: &mut AlephWorld, agent: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let tool = ctx.spawn_tool.as_ref().expect("Spawn tool not initialized");
    let result = tool.check_authorization(&agent);
    assert!(result.is_err(), "Expected authorization to fail for '{}'", agent);
}

#[cfg(feature = "gateway")]
#[then("the spawn status should be Error")]
async fn then_spawn_status_error(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let output = ctx.spawn_output.as_ref().expect("Spawn output not set");
    assert_eq!(output.status, SpawnStatus::Error, "Expected Error status");
}

#[cfg(feature = "gateway")]
#[then(expr = "the spawn error should contain {string}")]
async fn then_spawn_error_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let output = ctx.spawn_output.as_ref().expect("Spawn output not set");
    let error = output.error.as_ref().expect("Expected error in spawn output");
    assert!(error.contains(&expected), "Expected error to contain '{}': {}", expected, error);
}

#[cfg(feature = "gateway")]
#[then("the cleanup policy should be Ephemeral")]
async fn then_cleanup_ephemeral(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let policy = ctx.cleanup_policy.as_ref().expect("Cleanup policy not set");
    assert_eq!(*policy, CleanupPolicy::Ephemeral);
}

#[cfg(feature = "gateway")]
#[then(expr = "the spawn args task should be {string}")]
async fn then_spawn_args_task(w: &mut AlephWorld, expected: String) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let args = ctx.spawn_args.as_ref().expect("Spawn args not set");
    assert_eq!(args.task, expected);
}

#[cfg(feature = "gateway")]
#[then("the spawn args label should be none")]
async fn then_spawn_args_label_none(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let args = ctx.spawn_args.as_ref().expect("Spawn args not set");
    assert!(args.label.is_none());
}

#[cfg(feature = "gateway")]
#[then("the spawn args agent_id should be none")]
async fn then_spawn_args_agent_id_none(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let args = ctx.spawn_args.as_ref().expect("Spawn args not set");
    assert!(args.agent_id.is_none());
}

#[cfg(feature = "gateway")]
#[then("the spawn args model should be none")]
async fn then_spawn_args_model_none(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let args = ctx.spawn_args.as_ref().expect("Spawn args not set");
    assert!(args.model.is_none());
}

#[cfg(feature = "gateway")]
#[then("the spawn args thinking should be none")]
async fn then_spawn_args_thinking_none(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let args = ctx.spawn_args.as_ref().expect("Spawn args not set");
    assert!(args.thinking.is_none());
}

#[cfg(feature = "gateway")]
#[then(expr = "the spawn args timeout should be {int}")]
async fn then_spawn_args_timeout(w: &mut AlephWorld, expected: i64) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let args = ctx.spawn_args.as_ref().expect("Spawn args not set");
    assert_eq!(args.run_timeout_seconds, expected as u32);
}

#[cfg(feature = "gateway")]
#[then("the spawn args cleanup should be Ephemeral")]
async fn then_spawn_args_cleanup_ephemeral(w: &mut AlephWorld) {
    let ctx = w.subagent.as_ref().expect("Subagent context not initialized");
    let args = ctx.spawn_args.as_ref().expect("Spawn args not set");
    assert_eq!(args.cleanup, CleanupPolicy::Ephemeral);
}
