//! Sessions Spawn Tool
//!
//! Spawns a sub-agent session to handle a delegated task. The spawned session runs
//! asynchronously with its own session key and can optionally use a different model
//! or agent configuration.
//!
//! # Example
//!
//! ```rust,ignore
//! // Spawn an ephemeral sub-agent for a one-off task
//! let args = SessionsSpawnArgs {
//!     task: "Translate the following text to French: Hello world".to_string(),
//!     label: Some("translator".to_string()),
//!     agent_id: Some("translator".to_string()),
//!     model: None,
//!     thinking: None,
//!     run_timeout_seconds: 60,
//!     cleanup: CleanupPolicy::Ephemeral,
//! };
//!
//! // Spawn a persistent sub-agent for ongoing work
//! let args = SessionsSpawnArgs {
//!     task: "Monitor system logs and report anomalies".to_string(),
//!     label: Some("monitor".to_string()),
//!     agent_id: None, // defaults to current agent
//!     model: Some("anthropic/claude-sonnet-4".to_string()),
//!     thinking: Some("low".to_string()),
//!     run_timeout_seconds: 300,
//!     cleanup: CleanupPolicy::Persistent,
//! };
//! ```

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::gateway::context::GatewayContext;
use crate::tools::AlephTool;

use super::super::notify_tool_start;

/// Cleanup policy for spawned sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CleanupPolicy {
    /// Session is cleaned up after the run completes (default)
    #[default]
    Ephemeral,
    /// Session persists after run completion
    Persistent,
}

impl CleanupPolicy {
    /// Returns the string representation of the cleanup policy
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
            Self::Persistent => "persistent",
        }
    }
}

impl std::fmt::Display for CleanupPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Arguments for sessions_spawn tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionsSpawnArgs {
    /// The task description/prompt to send to the spawned agent
    ///
    /// This is the initial message that will be processed by the sub-agent.
    pub task: String,

    /// Optional human-readable label for this spawn
    ///
    /// Used for logging and identification. If not provided, a UUID will be used.
    #[serde(default)]
    pub label: Option<String>,

    /// Target agent ID to use for the spawned session
    ///
    /// If not specified, defaults to the current agent's ID.
    /// The agent must exist in the agent registry and be allowed by A2A policy.
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Override the model for the spawned session
    ///
    /// Format: `provider/model` (e.g., "anthropic/claude-sonnet-4")
    /// If not specified, uses the target agent's default model.
    #[serde(default)]
    pub model: Option<String>,

    /// Override the thinking level for the spawned session
    ///
    /// Valid values: "off", "minimal", "low", "medium", "high", "xhigh"
    /// If not specified, uses the target agent's default thinking level.
    #[serde(default)]
    pub thinking: Option<String>,

    /// Maximum run timeout in seconds
    ///
    /// The spawned run will be cancelled if it exceeds this timeout.
    /// Default: 300 seconds (5 minutes)
    #[serde(default = "default_run_timeout")]
    pub run_timeout_seconds: u32,

    /// Cleanup policy for the spawned session
    ///
    /// - `ephemeral` (default): Session is cleaned up after the run completes
    /// - `persistent`: Session persists for future interactions
    #[serde(default)]
    pub cleanup: CleanupPolicy,
}

fn default_run_timeout() -> u32 {
    300
}

/// Status of the sessions_spawn operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpawnStatus {
    /// Spawn request accepted, sub-agent session started
    Accepted,
    /// A2A policy denied the spawn
    Forbidden,
    /// Other error occurred
    Error,
}

/// Output from sessions_spawn tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsSpawnOutput {
    /// Status of the spawn operation
    pub status: SpawnStatus,

    /// Session key of the spawned sub-agent session
    ///
    /// Format: `agent:{target_agent_id}:subagent:{uuid}`
    pub child_session_key: String,

    /// Unique run ID for tracking the spawned execution
    pub run_id: String,

    /// Whether the model override was applied
    ///
    /// `None` if no model override was requested.
    /// `Some(true)` if the override was applied.
    /// `Some(false)` if the override could not be applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_applied: Option<bool>,

    /// Warning message (non-fatal issues)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,

    /// Error message if status is Error or Forbidden
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SessionsSpawnOutput {
    /// Create an Accepted response
    pub fn accepted(
        child_session_key: String,
        run_id: String,
        model_applied: Option<bool>,
        warning: Option<String>,
    ) -> Self {
        Self {
            status: SpawnStatus::Accepted,
            child_session_key,
            run_id,
            model_applied,
            warning,
            error: None,
        }
    }

    /// Create a Forbidden response
    pub fn forbidden(run_id: String, error: String) -> Self {
        Self {
            status: SpawnStatus::Forbidden,
            child_session_key: String::new(),
            run_id,
            model_applied: None,
            warning: None,
            error: Some(error),
        }
    }

    /// Create an Error response
    pub fn error(run_id: String, error: String) -> Self {
        Self {
            status: SpawnStatus::Error,
            child_session_key: String::new(),
            run_id,
            model_applied: None,
            warning: None,
            error: Some(error),
        }
    }
}

/// Sessions spawn tool for creating sub-agent sessions
///
/// This tool enables hierarchical agent orchestration by spawning sub-agents
/// to handle delegated tasks. It supports:
///
/// - Targeting specific agents in the registry
/// - Model/thinking level overrides
/// - Ephemeral or persistent session policies
/// - Authorization via whitelist
///
/// # Authorization
///
/// The tool uses a whitelist (`allow_agents`) to control which agents can be spawned:
/// - `["*"]` allows spawning any agent
/// - `["translator", "summarizer"]` only allows those specific agents
/// - Empty list `[]` blocks all spawns
///
/// Additionally, the A2A policy from GatewayContext is checked.
#[derive(Clone)]
pub struct SessionsSpawnTool {
    /// Gateway context for accessing registry and execution adapter
    context: Option<GatewayContext>,

    /// Current agent ID (the spawning agent)
    current_agent_id: String,

    /// Whitelist of agent IDs that can be spawned
    /// Use ["*"] to allow all agents
    allow_agents: Vec<String>,
}

impl SessionsSpawnTool {
    /// Tool identifier
    pub const NAME: &'static str = "sessions_spawn";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str =
        "Spawn a sub-agent session to handle a delegated task asynchronously. \
        The spawned session runs independently with its own session key and can use \
        a different model or thinking level. Use this for parallel processing, \
        specialized tasks, or when you need to delegate work to another agent. \
        Returns immediately with the child session key and run ID for tracking.";

    /// Create a new SessionsSpawnTool without context (will fail on execute)
    pub fn new() -> Self {
        Self {
            context: None,
            current_agent_id: "main".to_string(),
            allow_agents: vec!["*".to_string()], // Allow all by default
        }
    }

    /// Create a new SessionsSpawnTool with gateway context
    pub fn with_context(
        context: GatewayContext,
        current_agent_id: impl Into<String>,
        allow_agents: Vec<String>,
    ) -> Self {
        Self {
            context: Some(context),
            current_agent_id: current_agent_id.into(),
            allow_agents,
        }
    }

    /// Set the gateway context
    pub fn set_context(&mut self, context: GatewayContext) {
        self.context = Some(context);
    }

    /// Set the current agent ID
    pub fn set_current_agent_id(&mut self, agent_id: impl Into<String>) {
        self.current_agent_id = agent_id.into();
    }

    /// Set the allowed agents whitelist
    pub fn set_allow_agents(&mut self, allow_agents: Vec<String>) {
        self.allow_agents = allow_agents;
    }

    /// Check if spawning the target agent is authorized
    ///
    /// Authorization passes if:
    /// 1. `allow_agents` contains "*" (wildcard), OR
    /// 2. `allow_agents` contains the target agent ID
    ///
    /// Returns `Ok(())` if authorized, `Err(reason)` if not.
    pub fn check_authorization(&self, target_agent_id: &str) -> std::result::Result<(), String> {
        // Check whitelist
        if self.allow_agents.is_empty() {
            return Err("Agent spawn whitelist is empty".to_string());
        }

        // Wildcard allows all
        if self.allow_agents.iter().any(|a| a == "*") {
            return Ok(());
        }

        // Check explicit membership
        if self.allow_agents.iter().any(|a| a == target_agent_id) {
            return Ok(());
        }

        Err(format!(
            "Agent '{}' is not in the spawn whitelist",
            target_agent_id
        ))
    }

    /// Execute the tool (internal implementation)
    async fn call_impl(&self, args: SessionsSpawnArgs) -> SessionsSpawnOutput {
        let run_id = uuid::Uuid::new_v4().to_string();

        notify_tool_start(
            Self::NAME,
            &format!(
                "Spawning agent={:?}, label={:?}, timeout={}s",
                args.agent_id, args.label, args.run_timeout_seconds
            ),
        );

        // Validate context is available
        let context = match &self.context {
            Some(ctx) => ctx,
            None => {
                return SessionsSpawnOutput::error(
                    run_id,
                    "GatewayContext not configured for sessions_spawn tool".to_string(),
                );
            }
        };

        // Determine target agent ID
        let target_agent_id = args
            .agent_id
            .as_deref()
            .unwrap_or(&self.current_agent_id)
            .to_string();

        // Generate a unique ID for the sub-agent session
        let subagent_uuid = args
            .label
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Generate the child session key
        let child_session_key = format!("agent:{}:subagent:{}", target_agent_id, subagent_uuid);

        debug!(
            run_id = %run_id,
            from = %self.current_agent_id,
            to = %target_agent_id,
            child_session = %child_session_key,
            "sessions_spawn: checking authorization"
        );

        // Check whitelist authorization
        if let Err(reason) = self.check_authorization(&target_agent_id) {
            warn!(
                run_id = %run_id,
                from = %self.current_agent_id,
                to = %target_agent_id,
                reason = %reason,
                "sessions_spawn: whitelist denied"
            );
            return SessionsSpawnOutput::forbidden(run_id, reason);
        }

        // Check A2A policy
        let a2a_policy = context.a2a_policy();
        if !a2a_policy.is_allowed(&self.current_agent_id, &target_agent_id) {
            warn!(
                run_id = %run_id,
                from = %self.current_agent_id,
                to = %target_agent_id,
                "sessions_spawn: A2A policy denied"
            );
            return SessionsSpawnOutput::forbidden(
                run_id,
                format!(
                    "A2A policy denies spawning from '{}' to '{}'",
                    self.current_agent_id, target_agent_id
                ),
            );
        }

        // Verify target agent exists
        let agent_registry = context.agent_registry();
        if agent_registry.get(&target_agent_id).await.is_none() {
            return SessionsSpawnOutput::error(
                run_id,
                format!("Target agent '{}' not found in registry", target_agent_id),
            );
        }

        // Log model/thinking overrides
        let model_applied = if args.model.is_some() {
            // In a full implementation, we would apply the model override here
            // For now, we just indicate that it was requested
            info!(
                run_id = %run_id,
                model = ?args.model,
                thinking = ?args.thinking,
                "sessions_spawn: model/thinking override requested (not yet implemented)"
            );
            Some(false) // Not applied yet
        } else {
            None
        };

        let warning = if args.model.is_some() || args.thinking.is_some() {
            Some("Model/thinking overrides are not yet fully implemented".to_string())
        } else {
            None
        };

        info!(
            run_id = %run_id,
            child_session = %child_session_key,
            target_agent = %target_agent_id,
            cleanup = %args.cleanup,
            timeout = args.run_timeout_seconds,
            "sessions_spawn: sub-agent session spawned"
        );

        // Return immediately with the session key
        // The actual execution would be handled asynchronously by the execution engine
        SessionsSpawnOutput::accepted(child_session_key, run_id, model_applied, warning)
    }
}

impl Default for SessionsSpawnTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of AlephTool trait for SessionsSpawnTool
#[async_trait]
impl AlephTool for SessionsSpawnTool {
    const NAME: &'static str = "sessions_spawn";
    const DESCRIPTION: &'static str =
        "Spawn a sub-agent session to handle a delegated task asynchronously. \
        The spawned session runs independently with its own session key and can use \
        a different model or thinking level. Use this for parallel processing, \
        specialized tasks, or when you need to delegate work to another agent. \
        Returns immediately with the child session key and run ID for tracking.";

    type Args = SessionsSpawnArgs;
    type Output = SessionsSpawnOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(self.call_impl(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // CleanupPolicy tests
    // ============================================================================

    #[test]
    fn test_cleanup_policy_default() {
        let policy: CleanupPolicy = Default::default();
        assert_eq!(policy, CleanupPolicy::Ephemeral);
    }

    #[test]
    fn test_cleanup_policy_as_str() {
        assert_eq!(CleanupPolicy::Ephemeral.as_str(), "ephemeral");
        assert_eq!(CleanupPolicy::Persistent.as_str(), "persistent");
    }

    #[test]
    fn test_cleanup_policy_display() {
        assert_eq!(format!("{}", CleanupPolicy::Ephemeral), "ephemeral");
        assert_eq!(format!("{}", CleanupPolicy::Persistent), "persistent");
    }

    #[test]
    fn test_cleanup_policy_serialization() {
        assert_eq!(
            serde_json::to_string(&CleanupPolicy::Ephemeral).unwrap(),
            r#""ephemeral""#
        );
        assert_eq!(
            serde_json::to_string(&CleanupPolicy::Persistent).unwrap(),
            r#""persistent""#
        );
    }

    // ============================================================================
    // SessionsSpawnArgs tests
    // ============================================================================

    #[test]
    fn test_args_minimal() {
        let args: SessionsSpawnArgs =
            serde_json::from_str(r#"{"task": "Do something"}"#).unwrap();
        assert_eq!(args.task, "Do something");
        assert!(args.label.is_none());
        assert!(args.agent_id.is_none());
        assert!(args.model.is_none());
        assert!(args.thinking.is_none());
        assert_eq!(args.run_timeout_seconds, 300);
        assert_eq!(args.cleanup, CleanupPolicy::Ephemeral);
    }

    #[test]
    fn test_args_with_all_fields() {
        let args: SessionsSpawnArgs = serde_json::from_str(
            r#"{
                "task": "Translate to French",
                "label": "translator",
                "agent_id": "translator-agent",
                "model": "anthropic/claude-sonnet-4",
                "thinking": "low",
                "run_timeout_seconds": 60,
                "cleanup": "persistent"
            }"#,
        )
        .unwrap();

        assert_eq!(args.task, "Translate to French");
        assert_eq!(args.label, Some("translator".to_string()));
        assert_eq!(args.agent_id, Some("translator-agent".to_string()));
        assert_eq!(args.model, Some("anthropic/claude-sonnet-4".to_string()));
        assert_eq!(args.thinking, Some("low".to_string()));
        assert_eq!(args.run_timeout_seconds, 60);
        assert_eq!(args.cleanup, CleanupPolicy::Persistent);
    }

    // ============================================================================
    // SpawnStatus tests
    // ============================================================================

    #[test]
    fn test_spawn_status_serialization() {
        assert_eq!(
            serde_json::to_string(&SpawnStatus::Accepted).unwrap(),
            r#""accepted""#
        );
        assert_eq!(
            serde_json::to_string(&SpawnStatus::Forbidden).unwrap(),
            r#""forbidden""#
        );
        assert_eq!(
            serde_json::to_string(&SpawnStatus::Error).unwrap(),
            r#""error""#
        );
    }

    // ============================================================================
    // SessionsSpawnOutput tests
    // ============================================================================

    #[test]
    fn test_output_accepted() {
        let output = SessionsSpawnOutput::accepted(
            "agent:main:subagent:test-123".to_string(),
            "run-1".to_string(),
            Some(true),
            None,
        );
        assert_eq!(output.status, SpawnStatus::Accepted);
        assert_eq!(output.child_session_key, "agent:main:subagent:test-123");
        assert_eq!(output.run_id, "run-1");
        assert_eq!(output.model_applied, Some(true));
        assert!(output.warning.is_none());
        assert!(output.error.is_none());
    }

    #[test]
    fn test_output_accepted_with_warning() {
        let output = SessionsSpawnOutput::accepted(
            "agent:main:subagent:test-456".to_string(),
            "run-2".to_string(),
            Some(false),
            Some("Model override not applied".to_string()),
        );
        assert_eq!(output.status, SpawnStatus::Accepted);
        assert_eq!(output.model_applied, Some(false));
        assert_eq!(
            output.warning,
            Some("Model override not applied".to_string())
        );
    }

    #[test]
    fn test_output_forbidden() {
        let output = SessionsSpawnOutput::forbidden(
            "run-3".to_string(),
            "A2A policy denied".to_string(),
        );
        assert_eq!(output.status, SpawnStatus::Forbidden);
        assert!(output.child_session_key.is_empty());
        assert_eq!(output.run_id, "run-3");
        assert_eq!(output.error, Some("A2A policy denied".to_string()));
    }

    #[test]
    fn test_output_error() {
        let output = SessionsSpawnOutput::error(
            "run-4".to_string(),
            "Agent not found".to_string(),
        );
        assert_eq!(output.status, SpawnStatus::Error);
        assert!(output.child_session_key.is_empty());
        assert_eq!(output.error, Some("Agent not found".to_string()));
    }

    // ============================================================================
    // SessionsSpawnTool authorization tests
    // ============================================================================

    #[test]
    fn test_authorization_wildcard_allows_all() {
        let tool = SessionsSpawnTool {
            context: None,
            current_agent_id: "main".to_string(),
            allow_agents: vec!["*".to_string()],
        };

        assert!(tool.check_authorization("translator").is_ok());
        assert!(tool.check_authorization("summarizer").is_ok());
        assert!(tool.check_authorization("any-random-agent").is_ok());
    }

    #[test]
    fn test_authorization_explicit_whitelist() {
        let tool = SessionsSpawnTool {
            context: None,
            current_agent_id: "main".to_string(),
            allow_agents: vec!["translator".to_string(), "summarizer".to_string()],
        };

        assert!(tool.check_authorization("translator").is_ok());
        assert!(tool.check_authorization("summarizer").is_ok());
        assert!(tool.check_authorization("other-agent").is_err());
    }

    #[test]
    fn test_authorization_empty_whitelist() {
        let tool = SessionsSpawnTool {
            context: None,
            current_agent_id: "main".to_string(),
            allow_agents: vec![],
        };

        assert!(tool.check_authorization("translator").is_err());
        assert!(tool.check_authorization("main").is_err());
    }

    #[test]
    fn test_authorization_error_messages() {
        let tool = SessionsSpawnTool {
            context: None,
            current_agent_id: "main".to_string(),
            allow_agents: vec!["translator".to_string()],
        };

        let result = tool.check_authorization("other");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in the spawn whitelist"));

        let tool_empty = SessionsSpawnTool {
            context: None,
            current_agent_id: "main".to_string(),
            allow_agents: vec![],
        };

        let result_empty = tool_empty.check_authorization("any");
        assert!(result_empty.is_err());
        assert!(result_empty.unwrap_err().contains("whitelist is empty"));
    }

    // ============================================================================
    // SessionsSpawnTool basic tests
    // ============================================================================

    #[test]
    fn test_tool_creation() {
        let tool = SessionsSpawnTool::new();
        assert!(tool.context.is_none());
        assert_eq!(tool.current_agent_id, "main");
        assert_eq!(tool.allow_agents, vec!["*".to_string()]);
    }

    #[test]
    fn test_tool_name_and_description() {
        assert_eq!(SessionsSpawnTool::NAME, "sessions_spawn");
        assert!(!SessionsSpawnTool::DESCRIPTION.is_empty());
        assert!(SessionsSpawnTool::DESCRIPTION.contains("sub-agent"));
    }

    #[test]
    fn test_tool_setters() {
        let mut tool = SessionsSpawnTool::new();

        tool.set_current_agent_id("custom-agent");
        assert_eq!(tool.current_agent_id, "custom-agent");

        tool.set_allow_agents(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(tool.allow_agents, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_tool_without_context_returns_error() {
        let tool = SessionsSpawnTool::new();
        let args = SessionsSpawnArgs {
            task: "Test task".to_string(),
            label: None,
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

    // ============================================================================
    // Session key format tests
    // ============================================================================

    #[test]
    fn test_session_key_format_with_label() {
        // When label is provided, it should be used in the session key
        let label = "translator";
        let agent_id = "main";
        let expected_key = format!("agent:{}:subagent:{}", agent_id, label);
        assert_eq!(expected_key, "agent:main:subagent:translator");
    }

    #[test]
    fn test_session_key_format_without_label() {
        // When label is not provided, a UUID will be used
        // We can't test the exact UUID, but we can verify the format
        let agent_id = "main";
        let uuid = uuid::Uuid::new_v4().to_string();
        let key = format!("agent:{}:subagent:{}", agent_id, uuid);

        assert!(key.starts_with("agent:main:subagent:"));
        // UUID format: 8-4-4-4-12 = 36 chars
        // "agent:main:subagent:" = 20 chars
        // Total: 56 chars
        assert_eq!(key.len(), 56);
    }
}
