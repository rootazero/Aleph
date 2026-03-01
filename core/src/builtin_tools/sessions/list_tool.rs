//! Sessions list tool for querying available sessions.
//!
//! This tool allows agents to discover and query sessions in the system,
//! enabling agent-to-agent communication by listing accessible sessions.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::Arc;
use tracing::{debug, info};

use super::helpers::{classify_session_kind, derive_channel, SessionKind};
use crate::error::Result;
use crate::gateway::context::GatewayContext;
use crate::gateway::session_manager::{SessionMetadata, StoredMessage};
use crate::routing::session_key::SessionKey;
use crate::tools::AlephTool;

/// Arguments for the sessions_list tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionsListArgs {
    /// Filter by session kinds (e.g., ["main", "group", "task", "dm", "subagent", "ephemeral"])
    /// If not specified, all kinds are returned.
    #[serde(default)]
    pub kinds: Option<Vec<String>>,

    /// Maximum number of sessions to return (default: 50)
    #[serde(default = "default_limit")]
    pub limit: Option<u32>,

    /// Filter to sessions active within the last N minutes (default: no filter)
    #[serde(default)]
    pub active_minutes: Option<u32>,

    /// Include last N messages for each session (0-20, default: 0)
    /// Set to 0 to not include messages.
    #[serde(default)]
    pub message_limit: Option<u32>,
}

fn default_limit() -> Option<u32> {
    Some(50)
}

/// A row in the session list output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListRow {
    /// Session key string
    pub key: String,
    /// Session kind (main, dm, group, task, subagent, ephemeral)
    pub kind: String,
    /// Channel name (if applicable)
    pub channel: String,
    /// Last update timestamp (Unix epoch seconds)
    pub updated_at: Option<i64>,
    /// Number of messages in the session
    pub message_count: usize,
    /// Recent messages (if message_limit > 0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<StoredMessage>>,
}

/// Output from the sessions_list tool
#[derive(Debug, Clone, Serialize)]
pub struct SessionsListOutput {
    /// Total count of sessions returned
    pub count: usize,
    /// List of session rows
    pub sessions: Vec<SessionListRow>,
}

/// Tool for listing sessions accessible to the current agent.
///
/// This tool queries the session manager to find sessions that the
/// calling agent is allowed to access based on the A2A policy.
#[derive(Clone)]
pub struct SessionsListTool {
    /// Gateway context containing session manager and A2A policy
    context: Arc<GatewayContext>,
    /// The agent ID of the caller (for policy filtering)
    caller_agent_id: String,
}

impl SessionsListTool {
    /// Create a new sessions_list tool.
    ///
    /// # Arguments
    ///
    /// * `context` - Gateway context with session manager and A2A policy
    /// * `caller_agent_id` - The agent ID making the request (for policy checks)
    pub fn new(context: Arc<GatewayContext>, caller_agent_id: impl Into<String>) -> Self {
        Self {
            context,
            caller_agent_id: caller_agent_id.into(),
        }
    }

    /// Parse session kind from string
    fn parse_kind(s: &str) -> Option<SessionKind> {
        match s.to_lowercase().as_str() {
            "main" => Some(SessionKind::Main),
            "dm" | "direct_message" | "directmessage" => Some(SessionKind::DirectMessage),
            "group" => Some(SessionKind::Group),
            "task" => Some(SessionKind::Task),
            "subagent" => Some(SessionKind::Subagent),
            "ephemeral" => Some(SessionKind::Ephemeral),
            _ => None,
        }
    }

    /// Check if a session is accessible based on A2A policy
    fn is_accessible(&self, session_agent_id: &str) -> bool {
        self.context
            .a2a_policy()
            .is_allowed(&self.caller_agent_id, session_agent_id)
    }

    /// Convert session metadata to list row
    fn metadata_to_row(&self, meta: &SessionMetadata) -> SessionListRow {
        // Parse the session key to get kind and channel info
        let (kind, channel) = if let Some(parsed) = SessionKey::parse(&meta.key) {
            let kind = classify_session_kind(&parsed);
            let channel = derive_channel(&parsed);
            (kind.as_str().to_string(), channel)
        } else {
            // Fallback to session_type from metadata
            (meta.session_type.clone(), "unknown".to_string())
        };

        SessionListRow {
            key: meta.key.clone(),
            kind,
            channel,
            updated_at: Some(meta.last_active_at),
            message_count: meta.message_count as usize,
            messages: None,
        }
    }
}

#[async_trait]
impl AlephTool for SessionsListTool {
    const NAME: &'static str = "sessions_list";
    const DESCRIPTION: &'static str =
        "List sessions accessible to this agent. Use to discover other sessions for cross-session communication.";

    type Args = SessionsListArgs;
    type Output = SessionsListOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        use super::super::{notify_tool_result, notify_tool_start};

        let args_summary = format!(
            "Listing sessions (limit: {}, kinds: {:?})",
            args.limit.unwrap_or(50),
            args.kinds
        );
        notify_tool_start(Self::NAME, &args_summary);

        info!(
            caller = %self.caller_agent_id,
            limit = ?args.limit,
            kinds = ?args.kinds,
            active_minutes = ?args.active_minutes,
            "Listing sessions"
        );

        // 1. Query all sessions from session manager
        let all_sessions = self
            .context
            .session_manager()
            .list_sessions(None)
            .await
            .map_err(|e| {
                let msg = format!("Failed to list sessions: {}", e);
                notify_tool_result(Self::NAME, &msg, false);
                crate::error::AlephError::other(msg)
            })?;

        debug!(total_sessions = all_sessions.len(), "Raw session list retrieved");

        // 2. Apply A2A policy filtering
        let accessible_sessions: Vec<_> = all_sessions
            .into_iter()
            .filter(|meta| self.is_accessible(&meta.agent_id))
            .collect();

        debug!(
            accessible_count = accessible_sessions.len(),
            "Sessions after A2A filtering"
        );

        // 3. Convert to rows and parse session keys
        let mut rows: Vec<SessionListRow> = accessible_sessions
            .iter()
            .map(|meta| self.metadata_to_row(meta))
            .collect();

        // 4. Apply kind filter if specified
        if let Some(ref kinds) = args.kinds {
            let parsed_kinds: Vec<SessionKind> = kinds
                .iter()
                .filter_map(|s| Self::parse_kind(s))
                .collect();

            if !parsed_kinds.is_empty() {
                rows.retain(|row| {
                    if let Some(kind) = Self::parse_kind(&row.kind) {
                        parsed_kinds.contains(&kind)
                    } else {
                        false
                    }
                });
            }
        }

        debug!(after_kind_filter = rows.len(), "Sessions after kind filtering");

        // 5. Apply active_minutes filter if specified
        if let Some(active_mins) = args.active_minutes {
            let threshold = chrono::Utc::now().timestamp() - (active_mins as i64 * 60);
            rows.retain(|row| {
                row.updated_at
                    .map(|ts| ts >= threshold)
                    .unwrap_or(false)
            });
        }

        debug!(
            after_time_filter = rows.len(),
            "Sessions after activity filtering"
        );

        // 6. Apply limit
        let limit = args.limit.unwrap_or(50) as usize;
        rows.truncate(limit);

        // 7. Optionally fetch messages
        let message_limit = args.message_limit.unwrap_or(0).min(20) as usize;
        if message_limit > 0 {
            for row in &mut rows {
                // Parse the legacy session key format for message retrieval
                if let Some(legacy_key) =
                    crate::gateway::router::SessionKey::from_key_string(&row.key)
                {
                    match self
                        .context
                        .session_manager()
                        .get_history(&legacy_key, Some(message_limit))
                        .await
                    {
                        Ok(messages) => {
                            row.messages = Some(messages);
                        }
                        Err(e) => {
                            debug!(
                                session_key = %row.key,
                                error = %e,
                                "Failed to fetch messages for session"
                            );
                        }
                    }
                }
            }
        }

        let count = rows.len();
        let result_summary = format!("Found {} accessible sessions", count);
        notify_tool_result(Self::NAME, &result_summary, true);

        info!(
            count = count,
            caller = %self.caller_agent_id,
            "Sessions list completed"
        );

        Ok(SessionsListOutput {
            count,
            sessions: rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::a2a_policy::AgentToAgentPolicy;
    use crate::gateway::agent_instance::AgentRegistry;
    use crate::gateway::execution_adapter::ExecutionAdapter;
    use crate::gateway::execution_engine::{ExecutionError, RunRequest, RunState, RunStatus};
    use crate::gateway::event_emitter::EventEmitter;
    use crate::gateway::session_manager::SessionManagerConfig;
    use crate::gateway::{GatewayContext, SessionManager};
    use crate::gateway::agent_instance::AgentInstance;
    use tempfile::tempdir;

    /// Mock execution adapter for testing
    struct MockExecutionAdapter;

    #[async_trait]
    impl ExecutionAdapter for MockExecutionAdapter {
        async fn execute(
            &self,
            _request: RunRequest,
            _agent: Arc<AgentInstance>,
            _emitter: Arc<dyn EventEmitter + Send + Sync>,
        ) -> std::result::Result<(), ExecutionError> {
            Ok(())
        }

        async fn cancel(&self, run_id: &str) -> std::result::Result<(), ExecutionError> {
            Err(ExecutionError::RunNotFound(run_id.to_string()))
        }

        async fn get_status(&self, run_id: &str) -> Option<RunStatus> {
            Some(RunStatus {
                run_id: run_id.to_string(),
                state: RunState::Completed,
                started_at: Some(chrono::Utc::now()),
                completed_at: Some(chrono::Utc::now()),
                steps_completed: 0,
                current_tool: None,
            })
        }
    }

    fn create_test_context(temp_path: std::path::PathBuf) -> Arc<GatewayContext> {
        let session_config = SessionManagerConfig {
            db_path: temp_path.join("sessions.db"),
            ..Default::default()
        };
        let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
        let agent_registry = Arc::new(AgentRegistry::new());
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        let a2a_policy = Arc::new(AgentToAgentPolicy::permissive());

        Arc::new(GatewayContext::new(
            session_manager,
            agent_registry,
            execution_adapter,
            a2a_policy,
        ))
    }

    #[test]
    fn test_parse_kind() {
        assert_eq!(
            SessionsListTool::parse_kind("main"),
            Some(SessionKind::Main)
        );
        assert_eq!(
            SessionsListTool::parse_kind("dm"),
            Some(SessionKind::DirectMessage)
        );
        assert_eq!(
            SessionsListTool::parse_kind("direct_message"),
            Some(SessionKind::DirectMessage)
        );
        assert_eq!(
            SessionsListTool::parse_kind("group"),
            Some(SessionKind::Group)
        );
        assert_eq!(
            SessionsListTool::parse_kind("task"),
            Some(SessionKind::Task)
        );
        assert_eq!(
            SessionsListTool::parse_kind("subagent"),
            Some(SessionKind::Subagent)
        );
        assert_eq!(
            SessionsListTool::parse_kind("ephemeral"),
            Some(SessionKind::Ephemeral)
        );
        assert_eq!(SessionsListTool::parse_kind("invalid"), None);
    }

    #[test]
    fn test_args_default_values() {
        let args: SessionsListArgs = serde_json::from_str("{}").unwrap();
        assert!(args.kinds.is_none());
        assert_eq!(args.limit, Some(50));
        assert!(args.active_minutes.is_none());
        assert!(args.message_limit.is_none());
    }

    #[test]
    fn test_args_with_values() {
        let args: SessionsListArgs = serde_json::from_str(
            r#"{"kinds": ["main", "task"], "limit": 10, "active_minutes": 30, "message_limit": 5}"#,
        )
        .unwrap();
        assert_eq!(args.kinds, Some(vec!["main".to_string(), "task".to_string()]));
        assert_eq!(args.limit, Some(10));
        assert_eq!(args.active_minutes, Some(30));
        assert_eq!(args.message_limit, Some(5));
    }

    #[tokio::test]
    async fn test_list_empty_sessions() {
        let temp = tempdir().unwrap();
        let context = create_test_context(temp.path().to_path_buf());
        let tool = SessionsListTool::new(context, "main");

        let args = SessionsListArgs {
            kinds: None,
            limit: Some(50),
            active_minutes: None,
            message_limit: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert_eq!(result.count, 0);
        assert!(result.sessions.is_empty());
    }

    #[tokio::test]
    async fn test_list_with_sessions() {
        let temp = tempdir().unwrap();
        let context = create_test_context(temp.path().to_path_buf());

        // Create some sessions using the legacy SessionKey
        let session_manager = context.session_manager();
        let key1 = crate::gateway::router::SessionKey::main("main");
        let key2 = crate::gateway::router::SessionKey::task("main", "cron", "daily");

        session_manager.get_or_create(&key1).await.unwrap();
        session_manager.get_or_create(&key2).await.unwrap();

        let tool = SessionsListTool::new(context, "main");
        let args = SessionsListArgs {
            kinds: None,
            limit: Some(50),
            active_minutes: None,
            message_limit: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert_eq!(result.count, 2);
    }

    #[tokio::test]
    async fn test_list_with_kind_filter() {
        let temp = tempdir().unwrap();
        let context = create_test_context(temp.path().to_path_buf());

        // Create sessions of different kinds
        let session_manager = context.session_manager();
        let key1 = crate::gateway::router::SessionKey::main("main");
        let key2 = crate::gateway::router::SessionKey::task("main", "cron", "daily");

        session_manager.get_or_create(&key1).await.unwrap();
        session_manager.get_or_create(&key2).await.unwrap();

        let tool = SessionsListTool::new(context, "main");

        // Filter for only task sessions
        let args = SessionsListArgs {
            kinds: Some(vec!["task".to_string()]),
            limit: Some(50),
            active_minutes: None,
            message_limit: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert_eq!(result.count, 1);
        assert_eq!(result.sessions[0].kind, "task");
    }

    #[tokio::test]
    async fn test_list_with_limit() {
        let temp = tempdir().unwrap();
        let context = create_test_context(temp.path().to_path_buf());

        // Create multiple sessions
        let session_manager = context.session_manager();
        for i in 0..5 {
            let key = crate::gateway::router::SessionKey::task("main", "cron", format!("task-{}", i));
            session_manager.get_or_create(&key).await.unwrap();
        }

        let tool = SessionsListTool::new(context, "main");
        let args = SessionsListArgs {
            kinds: None,
            limit: Some(3),
            active_minutes: None,
            message_limit: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert_eq!(result.count, 3);
    }

    #[tokio::test]
    async fn test_a2a_policy_filtering() {
        let temp = tempdir().unwrap();

        // Create context with restrictive A2A policy
        let session_config = SessionManagerConfig {
            db_path: temp.path().join("sessions.db"),
            ..Default::default()
        };
        let session_manager = Arc::new(SessionManager::new(session_config).unwrap());
        let agent_registry = Arc::new(AgentRegistry::new());
        let execution_adapter: Arc<dyn ExecutionAdapter> = Arc::new(MockExecutionAdapter);
        // Policy that only allows communication to agents matching "main"
        let a2a_policy = Arc::new(AgentToAgentPolicy::new(true, vec!["main".to_string()]));

        let context = Arc::new(GatewayContext::new(
            session_manager.clone(),
            agent_registry,
            execution_adapter,
            a2a_policy,
        ));

        // Create sessions for different agents
        let key1 = crate::gateway::router::SessionKey::main("main");
        let key2 = crate::gateway::router::SessionKey::main("work");

        session_manager.get_or_create(&key1).await.unwrap();
        session_manager.get_or_create(&key2).await.unwrap();

        // Tool created for "other" agent which can only access "main" sessions
        let tool = SessionsListTool::new(context, "other");
        let args = SessionsListArgs {
            kinds: None,
            limit: Some(50),
            active_minutes: None,
            message_limit: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        // Should only see "main" sessions, not "work"
        assert_eq!(result.count, 1);
        assert!(result.sessions[0].key.contains("main"));
    }

    #[tokio::test]
    async fn test_list_with_messages() {
        let temp = tempdir().unwrap();
        let context = create_test_context(temp.path().to_path_buf());

        // Create a session with messages
        let session_manager = context.session_manager();
        let key = crate::gateway::router::SessionKey::main("main");
        session_manager.get_or_create(&key).await.unwrap();
        session_manager.add_message(&key, "user", "Hello").await.unwrap();
        session_manager.add_message(&key, "assistant", "Hi there!").await.unwrap();

        let tool = SessionsListTool::new(context, "main");
        let args = SessionsListArgs {
            kinds: None,
            limit: Some(50),
            active_minutes: None,
            message_limit: Some(10),
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert_eq!(result.count, 1);

        let session = &result.sessions[0];
        assert!(session.messages.is_some());
        let messages = session.messages.as_ref().unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn test_tool_definition() {
        let temp = tempdir().unwrap();
        let context = create_test_context(temp.path().to_path_buf());
        let tool = SessionsListTool::new(context, "main");

        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "sessions_list");
        assert!(!def.description.is_empty());
    }
}
