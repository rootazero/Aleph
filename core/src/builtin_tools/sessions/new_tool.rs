//! Session new tool — start a new conversation session.
//!
//! Closes the current session (optionally with a topic summary) and creates
//! a new session with the next epoch. This allows the LLM to handle "start
//! a new conversation" requests via natural language, complementing the
//! `/new` slash command.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::Result;
use crate::gateway::router::SessionKey as LegacySessionKey;
use crate::gateway::SessionManager;
use crate::routing::session_key::SessionKey as RoutingSessionKey;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the session_new tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionNewArgs {
    /// Optional topic summary for the closing session.
    /// The LLM should provide a brief summary of the conversation being closed.
    #[serde(default)]
    pub topic: Option<String>,

    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,
}

/// Output from session_new tool
#[derive(Debug, Clone, Serialize)]
pub struct SessionNewOutput {
    /// The old session key that was closed
    pub old_session_key: String,
    /// The new session key that was created
    pub new_session_key: String,
    /// Topic assigned to the closed session (if any)
    pub topic: Option<String>,
    /// Human-readable status message
    pub message: String,
}

/// Tool that creates a new conversation session.
#[derive(Clone)]
pub struct SessionNewTool {
    session_manager: Arc<SessionManager>,
}

impl SessionNewTool {
    pub fn new(session_manager: Arc<SessionManager>) -> Self {
        Self { session_manager }
    }
}

#[async_trait]
impl AlephTool for SessionNewTool {
    const NAME: &'static str = "session_new";
    const DESCRIPTION: &'static str =
        "Start a new conversation session. Closes the current session \
         (optionally with a topic summary) and begins a fresh one. \
         Use when the user wants to start over, begin a new topic, \
         or explicitly requests a new session.";

    type Args = SessionNewArgs;
    type Output = SessionNewOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "session_new()".to_string(),
            "session_new(topic='讨论了项目架构设计')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session_key_str = &args.__session_key;

        if session_key_str.is_empty() {
            return Err(crate::error::AlephError::tool(
                "session_new: no session context available (session key not injected)",
            ));
        }

        // Parse routing key (supports epoch)
        let routing_key = RoutingSessionKey::parse(session_key_str).ok_or_else(|| {
            crate::error::AlephError::tool(format!(
                "session_new: failed to parse session key '{}'",
                session_key_str
            ))
        })?;

        // Close old session
        let legacy_key = LegacySessionKey::from_key_string(session_key_str);
        if let Some(ref lk) = legacy_key {
            if let Err(e) = self
                .session_manager
                .close_session(lk, args.topic.clone())
                .await
            {
                warn!("session_new: failed to close old session: {}", e);
            }
        }

        // Create new session with next epoch
        let new_routing_key = routing_key.with_next_epoch();
        let new_key_str = new_routing_key.to_key_string();
        let new_legacy = LegacySessionKey::from_new(&new_routing_key);
        if let Err(e) = self.session_manager.get_or_create(&new_legacy).await {
            warn!("session_new: failed to create new session: {}", e);
        }

        info!(
            old = %session_key_str,
            new = %new_key_str,
            "New session created via tool"
        );

        let topic_suffix = args
            .topic
            .as_ref()
            .map(|t| format!(" ({})", t))
            .unwrap_or_default();

        Ok(SessionNewOutput {
            old_session_key: session_key_str.clone(),
            new_session_key: new_key_str,
            topic: args.topic,
            message: format!("新对话已开始{}", topic_suffix),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::SessionManagerConfig;
    use crate::tools::AlephTool;
    use tempfile::tempdir;

    fn test_session_manager() -> Arc<SessionManager> {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.into_path().join("test.db"),
            ..Default::default()
        };
        Arc::new(SessionManager::new(config).unwrap())
    }

    #[test]
    fn test_tool_definition() {
        let sm = test_session_manager();
        let tool = SessionNewTool::new(sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "session_new");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_empty_session_key_errors() {
        let sm = test_session_manager();
        let tool = SessionNewTool::new(sm);

        let result = tool
            .call(SessionNewArgs {
                topic: None,
                __session_key: String::new(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_new_session_basic() {
        let sm = test_session_manager();
        let tool = SessionNewTool::new(sm);

        let result = tool
            .call(SessionNewArgs {
                topic: Some("测试话题".into()),
                __session_key: "agent:main:default".into(),
            })
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.old_session_key, "agent:main:default");
        assert_eq!(output.new_session_key, "agent:main:default:s1");
        assert_eq!(output.topic, Some("测试话题".into()));
    }
}
