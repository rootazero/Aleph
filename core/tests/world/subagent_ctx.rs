//! Subagent Context for BDD tests
//!
//! Provides shared state for testing sub-agent orchestration components:
//! - RunEventBus lifecycle
//! - AuthProfileManager
//! - SessionsSpawnTool
//! - SubAgentRegistry lifecycle

use std::sync::Arc;

use tempfile::TempDir;
use tokio::sync::broadcast;

use alephcore::agents::sub_agents::{SubAgentRegistry, SubAgentRun};
use alephcore::gateway::run_event_bus::{
    ActiveRunHandle, RunEndResult, RunEvent, RunStatus,
};
use alephcore::gateway::router::SessionKey;
use alephcore::providers::profile_manager::AuthProfileManager;
use alephcore::routing::SessionKey as RoutingSessionKey;

#[cfg(feature = "gateway")]
use alephcore::builtin_tools::sessions::{
    CleanupPolicy, SessionsSpawnArgs, SessionsSpawnTool, SessionsSpawnOutput,
};

/// Subagent test context
#[derive(Default)]
pub struct SubagentContext {
    // RunEventBus state
    /// Active run handle
    pub run_handle: Option<Arc<ActiveRunHandle>>,
    /// Event receiver for subscribing
    pub event_rx: Option<broadcast::Receiver<RunEvent>>,
    /// Second event receiver for multi-subscriber tests
    pub event_rx2: Option<broadcast::Receiver<RunEvent>>,
    /// Input sender
    pub input_tx: Option<tokio::sync::mpsc::Sender<String>>,
    /// Input receiver
    pub input_rx: Option<tokio::sync::mpsc::Receiver<String>>,
    /// Cancel receiver (oneshot)
    pub cancel_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    /// Received event
    pub received_event: Option<RunEvent>,
    /// Received event 2 (for multi-subscriber)
    pub received_event2: Option<RunEvent>,
    /// Run end result
    pub run_end_result: Option<Result<RunEndResult, String>>,
    /// Sequence values collected
    pub seq_values: Vec<u64>,

    // AuthProfileManager state
    /// Profile manager
    pub profile_manager: Option<AuthProfileManager>,
    /// Retrieved profile ID
    pub profile_id: Option<String>,
    /// Profile error
    pub profile_error: Option<String>,
    /// Temp directory
    pub temp_dir: Option<TempDir>,

    // SessionsSpawnTool state (gateway feature)
    #[cfg(feature = "gateway")]
    pub spawn_tool: Option<SessionsSpawnTool>,
    #[cfg(feature = "gateway")]
    pub spawn_output: Option<SessionsSpawnOutput>,
    #[cfg(feature = "gateway")]
    pub spawn_args: Option<SessionsSpawnArgs>,
    #[cfg(feature = "gateway")]
    pub cleanup_policy: Option<CleanupPolicy>,
    /// Authorization result
    pub auth_result: Option<Result<(), String>>,
    /// Session key prefix
    pub session_key_prefix: Option<String>,

    // SubAgentRegistry state
    /// Registry for sub-agent runs
    pub registry: Option<SubAgentRegistry>,
    /// Registered run ID for tracking
    pub registered_run_id: Option<String>,
    /// Retrieved run for assertions
    pub retrieved_run: Option<SubAgentRun>,
}

impl std::fmt::Debug for SubagentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentContext")
            .field("run_handle", &self.run_handle.as_ref().map(|_| "ActiveRunHandle"))
            .field("event_rx", &self.event_rx.as_ref().map(|_| "Receiver"))
            .field("run_end_result", &self.run_end_result)
            .field("profile_manager", &self.profile_manager.as_ref().map(|_| "AuthProfileManager"))
            .field("profile_id", &self.profile_id)
            .field("profile_error", &self.profile_error)
            .field("registry", &self.registry.as_ref().map(|_| "SubAgentRegistry"))
            .field("registered_run_id", &self.registered_run_id)
            .field("retrieved_run", &self.retrieved_run.as_ref().map(|r| &r.run_id))
            .finish()
    }
}

impl SubagentContext {
    /// Create a new run handle
    pub fn create_run_handle(&mut self, run_id: &str) {
        let (handle, input_rx, cancel_rx) = ActiveRunHandle::new(
            run_id.to_string(),
            SessionKey::main("main"),
        );
        self.run_handle = Some(Arc::new(handle));
        self.input_rx = Some(input_rx);
        self.cancel_rx = Some(cancel_rx);
    }

    /// Create profile manager with a test profile
    pub fn create_profile_manager(&mut self, profile_id: &str, provider: &str) {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("profiles.toml");
        let agents_dir = temp_dir.path().join("agents");

        std::fs::write(
            &config_path,
            format!(
                r#"
[profiles.{}]
provider = "{}"
api_key = "sk-test-key-{}"
tier = "primary"
"#,
                profile_id, provider, profile_id
            ),
        )
        .unwrap();

        self.profile_manager = Some(AuthProfileManager::with_paths(config_path, agents_dir).unwrap());
        self.temp_dir = Some(temp_dir);
    }

    /// Create profile manager with primary and backup profiles
    pub fn create_profile_manager_with_backup(
        &mut self,
        primary_id: &str,
        backup_id: &str,
        provider: &str,
    ) {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("profiles.toml");
        let agents_dir = temp_dir.path().join("agents");

        std::fs::write(
            &config_path,
            format!(
                r#"
[profiles.{}]
provider = "{}"
api_key = "sk-{}"
tier = "primary"

[profiles.{}]
provider = "{}"
api_key = "sk-{}"
tier = "backup"
"#,
                primary_id, provider, primary_id, backup_id, provider, backup_id
            ),
        )
        .unwrap();

        self.profile_manager = Some(AuthProfileManager::with_paths(config_path, agents_dir).unwrap());
        self.temp_dir = Some(temp_dir);
    }

    /// Create empty profile manager
    pub fn create_empty_profile_manager(&mut self) {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("profiles.toml");
        let agents_dir = temp_dir.path().join("agents");

        std::fs::write(&config_path, "").unwrap();

        self.profile_manager = Some(AuthProfileManager::with_paths(config_path, agents_dir).unwrap());
        self.temp_dir = Some(temp_dir);
    }

    /// Create a fresh SubAgentRegistry
    pub fn create_registry(&mut self) {
        self.registry = Some(SubAgentRegistry::new_in_memory());
    }
}
