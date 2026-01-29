//! Sessions Send Tool
//!
//! Enables cross-session communication by sending messages to other sessions
//! (same or different agents). Supports both fire-and-forget and wait-for-reply modes.
//!
//! # Example
//!
//! ```rust,ignore
//! // Fire-and-forget mode
//! let args = SessionsSendArgs {
//!     session_key: Some("agent:translator:main".to_string()),
//!     message: "Translate this to French".to_string(),
//!     timeout_seconds: 0, // fire-and-forget
//! };
//!
//! // Wait mode (default 30s)
//! let args = SessionsSendArgs {
//!     session_key: None, // defaults to main session
//!     message: "Hello world".to_string(),
//!     timeout_seconds: 30,
//! };
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;
use crate::tools::AetherTool;

use super::super::notify_tool_start;
use super::helpers::parse_session_key;

/// Arguments for sessions_send tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionsSendArgs {
    /// Target session key (defaults to main if not specified)
    ///
    /// Format: `agent:<agent_id>:<session_type>:<id>`
    /// Examples:
    /// - `agent:main:main` - main session
    /// - `agent:translator:main` - translator agent's main session
    /// - `agent:main:dm:user123` - DM session
    #[serde(default)]
    pub session_key: Option<String>,

    /// Message to send to the target session
    pub message: String,

    /// Timeout in seconds.
    /// - 0 = fire-and-forget (returns immediately with Accepted status)
    /// - >0 = wait for response (default: 30 seconds)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

fn default_timeout() -> u32 {
    30
}

/// Status of the sessions_send operation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionsSendStatus {
    /// Successfully sent and received response
    Ok,
    /// Fire-and-forget accepted (timeout_seconds was 0)
    Accepted,
    /// Timed out waiting for response
    Timeout,
    /// A2A policy denied the communication
    Forbidden,
    /// Other error occurred
    Error,
}

/// Output from sessions_send tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsSendOutput {
    /// Unique identifier for this run
    pub run_id: String,
    /// Status of the operation
    pub status: SessionsSendStatus,
    /// Response from target session (if waited and received)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// Target session key that was used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// Error message if status is Error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SessionsSendOutput {
    /// Create an Ok response with reply
    pub fn ok(run_id: String, session_key: String, reply: String) -> Self {
        Self {
            run_id,
            status: SessionsSendStatus::Ok,
            reply: Some(reply),
            session_key: Some(session_key),
            error: None,
        }
    }

    /// Create an Accepted response (fire-and-forget)
    pub fn accepted(run_id: String, session_key: String) -> Self {
        Self {
            run_id,
            status: SessionsSendStatus::Accepted,
            reply: None,
            session_key: Some(session_key),
            error: None,
        }
    }

    /// Create a Timeout response
    pub fn timeout(run_id: String, session_key: String) -> Self {
        Self {
            run_id,
            status: SessionsSendStatus::Timeout,
            reply: None,
            session_key: Some(session_key),
            error: None,
        }
    }

    /// Create a Forbidden response
    pub fn forbidden(run_id: String, error: String) -> Self {
        Self {
            run_id,
            status: SessionsSendStatus::Forbidden,
            reply: None,
            session_key: None,
            error: Some(error),
        }
    }

    /// Create an Error response
    pub fn error(run_id: String, error: String) -> Self {
        Self {
            run_id,
            status: SessionsSendStatus::Error,
            reply: None,
            session_key: None,
            error: Some(error),
        }
    }
}

/// Sessions send tool for cross-session communication
///
/// Requires GatewayContext to access:
/// - AgentRegistry: to find the target agent
/// - ExecutionAdapter: to execute the run
/// - AgentToAgentPolicy: to check communication permissions
/// - SessionManager: to fetch the reply from history
#[derive(Clone)]
pub struct SessionsSendTool {
    /// Gateway context containing required components
    context: Option<GatewayContext>,
    /// Current agent ID (requester)
    current_agent_id: String,
}

impl SessionsSendTool {
    /// Tool identifier
    pub const NAME: &'static str = "sessions_send";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str =
        "Send a message to another session (same or different agent). \
        Supports fire-and-forget (timeout_seconds=0) or wait-for-reply modes. \
        Use this to delegate tasks to other agents or communicate across sessions.";

    /// Create a new SessionsSendTool without context (will fail on execute)
    pub fn new() -> Self {
        Self {
            context: None,
            current_agent_id: "main".to_string(),
        }
    }

    /// Create a new SessionsSendTool with gateway context
    pub fn with_context(context: GatewayContext, current_agent_id: impl Into<String>) -> Self {
        Self {
            context: Some(context),
            current_agent_id: current_agent_id.into(),
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

    /// Execute the tool (internal implementation)
    async fn call_impl(&self, args: SessionsSendArgs) -> SessionsSendOutput {
        let run_id = uuid::Uuid::new_v4().to_string();

        notify_tool_start(
            Self::NAME,
            &format!(
                "Sending to {:?}, timeout={}s",
                args.session_key, args.timeout_seconds
            ),
        );

        // Validate context is available
        let context = match &self.context {
            Some(ctx) => ctx,
            None => {
                return SessionsSendOutput::error(
                    run_id,
                    "GatewayContext not configured for sessions_send tool".to_string(),
                );
            }
        };

        // Parse target session key
        let target_session_key = match &args.session_key {
            Some(key_str) => match parse_session_key(key_str) {
                Ok(key) => key,
                Err(e) => {
                    return SessionsSendOutput::error(
                        run_id,
                        format!("Invalid session key: {}", e),
                    );
                }
            },
            None => {
                // Default to main session
                crate::routing::session_key::SessionKey::main("main")
            }
        };

        let target_key_str = target_session_key.to_key_string();
        let target_agent_id = target_session_key.agent_id().to_string();

        debug!(
            run_id = %run_id,
            from = %self.current_agent_id,
            to = %target_agent_id,
            session = %target_key_str,
            "sessions_send: checking A2A policy"
        );

        // Check A2A policy
        let a2a_policy = context.a2a_policy();
        if !a2a_policy.is_allowed(&self.current_agent_id, &target_agent_id) {
            warn!(
                run_id = %run_id,
                from = %self.current_agent_id,
                to = %target_agent_id,
                "sessions_send: A2A policy denied"
            );
            return SessionsSendOutput::forbidden(
                run_id,
                format!(
                    "A2A policy denies communication from '{}' to '{}'",
                    self.current_agent_id, target_agent_id
                ),
            );
        }

        // Get target agent from registry
        let agent_registry = context.agent_registry();
        let target_agent = match agent_registry.get(&target_agent_id).await {
            Some(agent) => agent,
            None => {
                return SessionsSendOutput::error(
                    run_id,
                    format!("Target agent '{}' not found in registry", target_agent_id),
                );
            }
        };

        // Convert routing::SessionKey to gateway::router::SessionKey
        let gateway_session_key = session_key_to_gateway(&target_session_key);

        // Create run request
        let request = RunRequest {
            run_id: run_id.clone(),
            input: args.message.clone(),
            session_key: gateway_session_key,
            timeout_secs: if args.timeout_seconds > 0 {
                Some(args.timeout_seconds as u64)
            } else {
                Some(300) // Default 5 min for fire-and-forget
            },
            metadata: HashMap::new(),
        };

        // Get execution adapter
        let execution_adapter = context.execution_adapter();

        // Create a no-op emitter for this execution
        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            Arc::new(NoOpEventEmitter::new());

        // Fire-and-forget mode
        if args.timeout_seconds == 0 {
            info!(
                run_id = %run_id,
                target = %target_key_str,
                "sessions_send: fire-and-forget mode"
            );

            // Spawn execution in background
            let adapter = execution_adapter.clone();
            let agent = target_agent.clone();
            let emitter_clone = emitter.clone();
            let run_id_clone = run_id.clone();

            tokio::spawn(async move {
                if let Err(e) = adapter.execute(request, agent, emitter_clone).await {
                    warn!(
                        run_id = %run_id_clone,
                        error = %e,
                        "sessions_send: background execution failed"
                    );
                }
            });

            return SessionsSendOutput::accepted(run_id, target_key_str);
        }

        // Wait mode: execute and wait for completion
        info!(
            run_id = %run_id,
            target = %target_key_str,
            timeout = args.timeout_seconds,
            "sessions_send: wait mode"
        );

        let timeout_duration = std::time::Duration::from_secs(args.timeout_seconds as u64);

        // Execute with timeout
        let execution_result = tokio::time::timeout(
            timeout_duration,
            execution_adapter.execute(request, target_agent.clone(), emitter),
        )
        .await;

        match execution_result {
            Ok(Ok(())) => {
                // Execution completed successfully, fetch the last assistant message
                let history = target_agent
                    .get_history(
                        &session_key_to_gateway(&target_session_key),
                        Some(1),
                    )
                    .await;

                let reply = history
                    .last()
                    .filter(|msg| {
                        matches!(
                            msg.role,
                            crate::gateway::agent_instance::MessageRole::Assistant
                        )
                    })
                    .map(|msg| msg.content.clone());

                match reply {
                    Some(content) => {
                        info!(
                            run_id = %run_id,
                            target = %target_key_str,
                            reply_len = content.len(),
                            "sessions_send: got reply"
                        );
                        SessionsSendOutput::ok(run_id, target_key_str, content)
                    }
                    None => {
                        warn!(
                            run_id = %run_id,
                            target = %target_key_str,
                            "sessions_send: execution completed but no assistant reply found"
                        );
                        SessionsSendOutput::ok(
                            run_id,
                            target_key_str,
                            "(No reply content)".to_string(),
                        )
                    }
                }
            }
            Ok(Err(e)) => {
                warn!(
                    run_id = %run_id,
                    error = %e,
                    "sessions_send: execution failed"
                );
                SessionsSendOutput::error(run_id, format!("Execution failed: {}", e))
            }
            Err(_) => {
                warn!(
                    run_id = %run_id,
                    timeout = args.timeout_seconds,
                    "sessions_send: timed out"
                );
                SessionsSendOutput::timeout(run_id, target_key_str)
            }
        }
    }
}

impl Default for SessionsSendTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert routing::SessionKey to gateway::router::SessionKey
fn session_key_to_gateway(key: &crate::routing::session_key::SessionKey) -> SessionKey {
    match key {
        crate::routing::session_key::SessionKey::Main { agent_id, .. } => {
            SessionKey::main(agent_id.clone())
        }
        crate::routing::session_key::SessionKey::DirectMessage {
            agent_id, peer_id, ..
        } => SessionKey::peer(agent_id.clone(), peer_id.clone()),
        crate::routing::session_key::SessionKey::Group {
            agent_id, peer_id, ..
        } => SessionKey::peer(agent_id.clone(), peer_id.clone()),
        crate::routing::session_key::SessionKey::Task {
            agent_id,
            task_type,
            task_id,
        } => SessionKey::task(agent_id.clone(), task_type.clone(), task_id.clone()),
        crate::routing::session_key::SessionKey::Subagent { parent_key, .. } => {
            session_key_to_gateway(parent_key)
        }
        crate::routing::session_key::SessionKey::Ephemeral { agent_id, .. } => {
            SessionKey::ephemeral(agent_id.clone())
        }
    }
}

/// Implementation of AetherTool trait for SessionsSendTool
#[async_trait]
impl AetherTool for SessionsSendTool {
    const NAME: &'static str = "sessions_send";
    const DESCRIPTION: &'static str =
        "Send a message to another session (same or different agent). \
        Supports fire-and-forget (timeout_seconds=0) or wait-for-reply modes. \
        Use this to delegate tasks to other agents or communicate across sessions.";

    type Args = SessionsSendArgs;
    type Output = SessionsSendOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(self.call_impl(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // SessionsSendArgs tests
    // ============================================================================

    #[test]
    fn test_args_default_timeout() {
        let args: SessionsSendArgs =
            serde_json::from_str(r#"{"message": "hello"}"#).unwrap();
        assert_eq!(args.timeout_seconds, 30);
        assert!(args.session_key.is_none());
    }

    #[test]
    fn test_args_with_all_fields() {
        let args: SessionsSendArgs = serde_json::from_str(
            r#"{"session_key": "agent:translator:main", "message": "translate this", "timeout_seconds": 60}"#,
        )
        .unwrap();
        assert_eq!(args.session_key, Some("agent:translator:main".to_string()));
        assert_eq!(args.message, "translate this");
        assert_eq!(args.timeout_seconds, 60);
    }

    #[test]
    fn test_args_fire_and_forget() {
        let args: SessionsSendArgs =
            serde_json::from_str(r#"{"message": "async task", "timeout_seconds": 0}"#).unwrap();
        assert_eq!(args.timeout_seconds, 0);
    }

    // ============================================================================
    // SessionsSendStatus tests
    // ============================================================================

    #[test]
    fn test_status_serialization() {
        assert_eq!(
            serde_json::to_string(&SessionsSendStatus::Ok).unwrap(),
            r#""ok""#
        );
        assert_eq!(
            serde_json::to_string(&SessionsSendStatus::Accepted).unwrap(),
            r#""accepted""#
        );
        assert_eq!(
            serde_json::to_string(&SessionsSendStatus::Timeout).unwrap(),
            r#""timeout""#
        );
        assert_eq!(
            serde_json::to_string(&SessionsSendStatus::Forbidden).unwrap(),
            r#""forbidden""#
        );
        assert_eq!(
            serde_json::to_string(&SessionsSendStatus::Error).unwrap(),
            r#""error""#
        );
    }

    // ============================================================================
    // SessionsSendOutput tests
    // ============================================================================

    #[test]
    fn test_output_ok() {
        let output = SessionsSendOutput::ok(
            "run-1".to_string(),
            "agent:main:main".to_string(),
            "Hello!".to_string(),
        );
        assert_eq!(output.status, SessionsSendStatus::Ok);
        assert_eq!(output.reply, Some("Hello!".to_string()));
        assert_eq!(output.session_key, Some("agent:main:main".to_string()));
        assert!(output.error.is_none());
    }

    #[test]
    fn test_output_accepted() {
        let output = SessionsSendOutput::accepted(
            "run-2".to_string(),
            "agent:main:main".to_string(),
        );
        assert_eq!(output.status, SessionsSendStatus::Accepted);
        assert!(output.reply.is_none());
    }

    #[test]
    fn test_output_timeout() {
        let output = SessionsSendOutput::timeout(
            "run-3".to_string(),
            "agent:main:main".to_string(),
        );
        assert_eq!(output.status, SessionsSendStatus::Timeout);
    }

    #[test]
    fn test_output_forbidden() {
        let output = SessionsSendOutput::forbidden(
            "run-4".to_string(),
            "Policy denied".to_string(),
        );
        assert_eq!(output.status, SessionsSendStatus::Forbidden);
        assert_eq!(output.error, Some("Policy denied".to_string()));
    }

    #[test]
    fn test_output_error() {
        let output = SessionsSendOutput::error(
            "run-5".to_string(),
            "Something went wrong".to_string(),
        );
        assert_eq!(output.status, SessionsSendStatus::Error);
        assert_eq!(output.error, Some("Something went wrong".to_string()));
    }

    // ============================================================================
    // SessionsSendTool tests
    // ============================================================================

    #[test]
    fn test_tool_creation() {
        let tool = SessionsSendTool::new();
        assert!(tool.context.is_none());
        assert_eq!(tool.current_agent_id, "main");
    }

    #[test]
    fn test_tool_name_and_description() {
        assert_eq!(SessionsSendTool::NAME, "sessions_send");
        assert!(!SessionsSendTool::DESCRIPTION.is_empty());
    }

    #[tokio::test]
    async fn test_tool_without_context_returns_error() {
        let tool = SessionsSendTool::new();
        let args = SessionsSendArgs {
            session_key: None,
            message: "test".to_string(),
            timeout_seconds: 30,
        };

        let output = AetherTool::call(&tool, args).await.unwrap();
        assert_eq!(output.status, SessionsSendStatus::Error);
        assert!(output.error.is_some());
        assert!(output.error.unwrap().contains("GatewayContext not configured"));
    }

    // ============================================================================
    // session_key_to_gateway conversion tests
    // ============================================================================

    #[test]
    fn test_convert_main_key() {
        use crate::routing::session_key::SessionKey as RoutingKey;

        let routing_key = RoutingKey::main("test-agent");
        let gateway_key = session_key_to_gateway(&routing_key);

        assert!(matches!(gateway_key, SessionKey::Main { agent_id, .. } if agent_id == "test-agent"));
    }

    #[test]
    fn test_convert_dm_key() {
        use crate::routing::session_key::{DmScope, SessionKey as RoutingKey};

        let routing_key = RoutingKey::dm("main", "telegram", "user123", DmScope::PerPeer);
        let gateway_key = session_key_to_gateway(&routing_key);

        assert!(matches!(gateway_key, SessionKey::PerPeer { agent_id, peer_id } if agent_id == "main" && peer_id == "user123"));
    }

    #[test]
    fn test_convert_task_key() {
        use crate::routing::session_key::SessionKey as RoutingKey;

        let routing_key = RoutingKey::task("main", "cron", "daily");
        let gateway_key = session_key_to_gateway(&routing_key);

        assert!(matches!(gateway_key, SessionKey::Task { agent_id, task_type, task_id } if agent_id == "main" && task_type == "cron" && task_id == "daily"));
    }
}
