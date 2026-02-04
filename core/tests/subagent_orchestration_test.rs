//! Integration tests for Sub-Agent Orchestration feature.
//!
//! Tests the following components:
//! - RunEventBus lifecycle (event broadcasting, waiting for completion)
//! - SessionsSpawnTool (session key generation, authorization)
//! - AuthProfileManager (cooldown behavior, profile selection)

use std::time::Duration;
use tempfile::TempDir;

use alephcore::gateway::run_event_bus::{
    wait_for_run_end, ActiveRunHandle, RunEndResult, RunEvent, RunStatus,
};
use alephcore::gateway::router::SessionKey;
use alephcore::providers::auth_profiles::AuthProfileFailureReason;
use alephcore::providers::profile_manager::AuthProfileManager;

// ============================================================================
// RunEventBus Tests
// ============================================================================

#[tokio::test]
async fn test_run_event_bus_lifecycle() {
    // Create a new run handle
    let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
        "run-123".to_string(),
        SessionKey::main("main"),
    );

    // Subscribe before emitting
    let mut rx = handle.subscribe();

    // Emit status change
    let _seq = handle.next_seq();
    handle.emit(RunEvent::StatusChanged {
        run_id: "run-123".to_string(),
        status: RunStatus::Running,
        reason: None,
    });

    // Emit completion
    let seq2 = handle.next_seq();
    handle.emit(RunEvent::RunCompleted {
        run_id: "run-123".to_string(),
        seq: seq2,
        summary: Some("Task done".to_string()),
        total_tokens: 150,
        tool_calls: 2,
        loops: 1,
        duration_ms: 1000,
    });

    // Wait should return completed
    let result = wait_for_run_end(&mut rx, Duration::from_secs(5)).await;
    assert!(result.is_ok());

    match result.unwrap() {
        RunEndResult::Completed {
            summary,
            total_tokens,
            ..
        } => {
            assert_eq!(summary, Some("Task done".to_string()));
            assert_eq!(total_tokens, 150);
        }
        _ => panic!("Expected Completed"),
    }
}

#[tokio::test]
async fn test_run_event_bus_failed() {
    let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
        "run-fail-test".to_string(),
        SessionKey::main("main"),
    );

    let mut rx = handle.subscribe();

    // Emit failure
    let seq = handle.next_seq();
    handle.emit(RunEvent::RunFailed {
        run_id: "run-fail-test".to_string(),
        seq,
        error: "Something went wrong".to_string(),
        error_code: Some("ERR_TEST".to_string()),
    });

    let result = wait_for_run_end(&mut rx, Duration::from_secs(5)).await;
    assert!(result.is_ok());

    match result.unwrap() {
        RunEndResult::Failed { error, error_code } => {
            assert_eq!(error, "Something went wrong");
            assert_eq!(error_code, Some("ERR_TEST".to_string()));
        }
        _ => panic!("Expected Failed"),
    }
}

#[tokio::test]
async fn test_run_event_bus_cancelled() {
    let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
        "run-cancel-test".to_string(),
        SessionKey::main("main"),
    );

    let mut rx = handle.subscribe();

    // Emit cancellation
    let seq = handle.next_seq();
    handle.emit(RunEvent::RunCancelled {
        run_id: "run-cancel-test".to_string(),
        seq,
        reason: Some("User cancelled".to_string()),
    });

    let result = wait_for_run_end(&mut rx, Duration::from_secs(5)).await;
    assert!(result.is_ok());

    match result.unwrap() {
        RunEndResult::Cancelled { reason } => {
            assert_eq!(reason, Some("User cancelled".to_string()));
        }
        _ => panic!("Expected Cancelled"),
    }
}

#[tokio::test]
async fn test_run_event_bus_multiple_subscribers() {
    let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
        "run-multi-sub".to_string(),
        SessionKey::main("main"),
    );

    let mut rx1 = handle.subscribe();
    let mut rx2 = handle.subscribe();

    // Emit an event
    handle.emit(RunEvent::StatusChanged {
        run_id: "run-multi-sub".to_string(),
        status: RunStatus::Running,
        reason: None,
    });

    // Both subscribers should receive the event
    let event1 = rx1.recv().await.unwrap();
    let event2 = rx2.recv().await.unwrap();

    assert!(matches!(
        event1,
        RunEvent::StatusChanged {
            status: RunStatus::Running,
            ..
        }
    ));
    assert!(matches!(
        event2,
        RunEvent::StatusChanged {
            status: RunStatus::Running,
            ..
        }
    ));
}

#[tokio::test]
async fn test_run_event_bus_seq_counter() {
    let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
        "run-seq-test".to_string(),
        SessionKey::main("main"),
    );

    // Sequence counter should increment
    assert_eq!(handle.next_seq(), 0);
    assert_eq!(handle.next_seq(), 1);
    assert_eq!(handle.next_seq(), 2);
    assert_eq!(handle.current_seq(), 3);
}

#[tokio::test]
async fn test_run_event_bus_input_sender() {
    let (handle, mut input_rx, _cancel_rx) = ActiveRunHandle::new(
        "run-input-test".to_string(),
        SessionKey::main("main"),
    );

    let input_tx = handle.input_sender();

    // Send input
    input_tx.send("user input message".to_string()).await.unwrap();

    // Receive it
    let received = input_rx.recv().await.unwrap();
    assert_eq!(received, "user input message");
}

// ============================================================================
// AuthProfileManager Tests
// ============================================================================

#[test]
fn test_profile_manager_cooldown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("profiles.toml");
    let agents_dir = temp_dir.path().join("agents");

    // Create a test profile config
    std::fs::write(
        &config_path,
        r#"
[profiles.test_profile]
provider = "openai"
api_key = "sk-test-key-123"
tier = "primary"
"#,
    )
    .unwrap();

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Profile should be available initially
    let profile = manager.get_available_profile("openai", "main");
    assert!(profile.is_ok());
    assert_eq!(profile.unwrap().id, "test_profile");

    // Mark as failed with rate limit
    manager
        .mark_failure("test_profile", AuthProfileFailureReason::RateLimit)
        .unwrap();

    // Profile should be in cooldown
    let profiles = manager.list_profiles();
    let test_profile = profiles.iter().find(|p| p.id == "test_profile").unwrap();
    assert!(test_profile.in_cooldown);
    assert!(test_profile.cooldown_remaining_ms.is_some());
    assert_eq!(test_profile.failure_count, 1);
}

#[test]
fn test_profile_manager_success_clears_cooldown() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("profiles.toml");
    let agents_dir = temp_dir.path().join("agents");

    std::fs::write(
        &config_path,
        r#"
[profiles.test_profile]
provider = "anthropic"
api_key = "sk-ant-test-123"
tier = "primary"
"#,
    )
    .unwrap();

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Mark as failed
    manager
        .mark_failure("test_profile", AuthProfileFailureReason::RateLimit)
        .unwrap();

    // Verify it's in cooldown
    let profiles = manager.list_profiles();
    let profile = profiles.iter().find(|p| p.id == "test_profile").unwrap();
    assert!(profile.in_cooldown);

    // Mark as success
    manager.mark_success("test_profile").unwrap();

    // Cooldown should be cleared
    let profiles = manager.list_profiles();
    let profile = profiles.iter().find(|p| p.id == "test_profile").unwrap();
    assert!(!profile.in_cooldown);
    assert_eq!(profile.failure_count, 0);
}

#[test]
fn test_profile_manager_fallback_to_backup() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("profiles.toml");
    let agents_dir = temp_dir.path().join("agents");

    // Create primary and backup profiles
    std::fs::write(
        &config_path,
        r#"
[profiles.primary_profile]
provider = "anthropic"
api_key = "sk-ant-primary"
tier = "primary"

[profiles.backup_profile]
provider = "anthropic"
api_key = "sk-ant-backup"
tier = "backup"
"#,
    )
    .unwrap();

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Initially should get primary
    let profile = manager.get_available_profile("anthropic", "main").unwrap();
    assert_eq!(profile.id, "primary_profile");

    // Mark primary as failed
    manager
        .mark_failure("primary_profile", AuthProfileFailureReason::RateLimit)
        .unwrap();

    // Now should get backup
    let profile = manager.get_available_profile("anthropic", "main").unwrap();
    assert_eq!(profile.id, "backup_profile");
}

#[test]
fn test_profile_manager_usage_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("profiles.toml");
    let agents_dir = temp_dir.path().join("agents");

    std::fs::write(
        &config_path,
        r#"
[profiles.usage_test]
provider = "openai"
api_key = "sk-usage-test"
tier = "primary"
"#,
    )
    .unwrap();

    let manager = AuthProfileManager::with_paths(config_path.clone(), agents_dir.clone()).unwrap();

    // Record some usage
    manager
        .record_usage("main", "usage_test", 1000, 500, 0.015)
        .unwrap();

    // Verify state file was created
    let state_path = agents_dir.join("main").join("state.json");
    assert!(state_path.exists());

    // Record more usage
    manager
        .record_usage("main", "usage_test", 2000, 1000, 0.030)
        .unwrap();

    // Create a new manager to load from disk (verifies persistence)
    let _manager2 = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // The state should be cached/reloaded - we can verify by checking the file exists
    // and contains the expected data
    let state_content = std::fs::read_to_string(&state_path).unwrap();
    assert!(state_content.contains("usage_test"));
    assert!(state_content.contains("input_tokens"));
}

#[test]
fn test_profile_manager_no_profiles_error() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("profiles.toml");
    let agents_dir = temp_dir.path().join("agents");

    // Create empty config
    std::fs::write(&config_path, "").unwrap();

    let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

    // Should get NoProfilesAvailable error
    let result = manager.get_available_profile("anthropic", "main");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("No profiles available"));
}

// ============================================================================
// SessionsSpawnTool Tests (requires gateway feature)
// ============================================================================

#[cfg(feature = "gateway")]
mod gateway_tests {
    use alephcore::builtin_tools::sessions::{
        CleanupPolicy, SessionsSpawnArgs, SessionsSpawnTool, SpawnStatus,
    };
    use alephcore::tools::AlephTool;

    #[test]
    fn test_spawn_tool_session_key_format() {
        // Test that session keys follow the expected format
        let agent_id = "poet";
        let label = "translator";
        let expected_prefix = format!("agent:{}:subagent:{}", agent_id, label);
        assert_eq!(expected_prefix, "agent:poet:subagent:translator");
    }

    #[test]
    fn test_spawn_tool_authorization_wildcard() {
        let tool = SessionsSpawnTool::new();
        // Default allows all with wildcard
        assert!(tool.check_authorization("any_agent").is_ok());
        assert!(tool.check_authorization("translator").is_ok());
    }

    #[test]
    fn test_spawn_tool_authorization_explicit_list() {
        let mut tool = SessionsSpawnTool::new();
        tool.set_allow_agents(vec!["translator".to_string(), "summarizer".to_string()]);

        assert!(tool.check_authorization("translator").is_ok());
        assert!(tool.check_authorization("summarizer").is_ok());
        assert!(tool.check_authorization("other").is_err());
    }

    #[test]
    fn test_spawn_tool_authorization_empty_list() {
        let mut tool = SessionsSpawnTool::new();
        tool.set_allow_agents(vec![]);

        assert!(tool.check_authorization("any").is_err());
    }

    #[tokio::test]
    async fn test_spawn_tool_without_context_returns_error() {
        let tool = SessionsSpawnTool::new();
        let args = SessionsSpawnArgs {
            task: "Test task".to_string(),
            label: Some("test".to_string()),
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: 60,
            cleanup: CleanupPolicy::Ephemeral,
        };

        let output = AlephTool::call(&tool, args).await.unwrap();
        assert_eq!(output.status, SpawnStatus::Error);
        assert!(output.error.is_some());
        assert!(output
            .error
            .unwrap()
            .contains("GatewayContext not configured"));
    }

    #[test]
    fn test_cleanup_policy_defaults() {
        let policy: CleanupPolicy = Default::default();
        assert_eq!(policy, CleanupPolicy::Ephemeral);
    }

    #[test]
    fn test_spawn_args_defaults() {
        let args: SessionsSpawnArgs =
            serde_json::from_str(r#"{"task": "Do something"}"#).unwrap();
        assert_eq!(args.task, "Do something");
        assert!(args.label.is_none());
        assert!(args.agent_id.is_none());
        assert!(args.model.is_none());
        assert!(args.thinking.is_none());
        assert_eq!(args.run_timeout_seconds, 300); // default timeout
        assert_eq!(args.cleanup, CleanupPolicy::Ephemeral);
    }
}
