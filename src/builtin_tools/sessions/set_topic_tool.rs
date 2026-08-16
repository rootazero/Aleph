//! Session set-topic tool — rename the current session's topic.
//!
//! Allows the LLM to rename a session topic via natural language,
//! complementing the panel UI's inline edit (R9: Everything is a Tool).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::router::SessionKey as LegacySessionKey;
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `session_set_topic` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionSetTopicArgs {
    /// The new topic/title for the session.
    pub topic: String,

    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,
}

/// Output from `session_set_topic` tool
#[derive(Debug, Clone, Serialize)]
pub struct SessionSetTopicOutput {
    pub session_key: String,
    pub topic: String,
    pub message: String,
}

/// Tool that renames the current session's topic.
#[derive(Clone)]
pub struct SessionSetTopicTool {
    session_store: Arc<dyn SessionStore>,
}

impl SessionSetTopicTool {
    pub fn new(session_store: Arc<dyn SessionStore>) -> Self {
        Self { session_store }
    }
}

#[async_trait]
impl AlephTool for SessionSetTopicTool {
    const NAME: &'static str = "session_rename";
    const DESCRIPTION: &'static str =
        "Rename the current session's topic/title. Use when the user \
         asks to change, rename, or set the conversation title or topic.";

    type Args = SessionSetTopicArgs;
    type Output = SessionSetTopicOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session_key_str = &args.__session_key;

        if session_key_str.is_empty() {
            return Err(crate::error::AlephError::tool(
                "session_set_topic: no session context available (session key not injected)",
            ));
        }

        let topic = args.topic.trim();
        if topic.is_empty() {
            return Err(crate::error::AlephError::tool(
                "session_set_topic: topic cannot be empty",
            ));
        }

        // Truncate to 100 chars (P7: boundary validation)
        let topic = if topic.len() > 100 {
            &topic[..topic
                .char_indices()
                .nth(100)
                .map_or(topic.len(), |(i, _)| i)]
        } else {
            topic
        };

        let legacy_key = LegacySessionKey::from_key_string(session_key_str).ok_or_else(|| {
            crate::error::AlephError::tool(format!(
                "session_set_topic: failed to parse session key '{session_key_str}'"
            ))
        })?;

        self.session_store
            .set_topic(&legacy_key, topic)
            .await
            .map_err(|e| {
                crate::error::AlephError::tool(format!(
                    "session_set_topic: failed to set topic: {e}"
                ))
            })?;

        info!(
            session = %session_key_str,
            topic = %topic,
            "Session topic updated via tool"
        );

        Ok(SessionSetTopicOutput {
            session_key: session_key_str.clone(),
            topic: topic.to_string(),
            message: format!("会话主题已更新为: {topic}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::tools::AlephTool;
    use tempfile::tempdir;

    fn test_session_manager() -> (tempfile::TempDir, Arc<SessionManager>) {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("test.db"),
            ..Default::default()
        };
        (temp, Arc::new(SessionManager::new(config).unwrap()))
    }

    #[test]
    fn test_tool_definition() {
        let (_scratch, sm) = test_session_manager();
        let tool = SessionSetTopicTool::new(sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "session_rename");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_empty_session_key_errors() {
        let (_scratch, sm) = test_session_manager();
        let tool = SessionSetTopicTool::new(sm);

        let result = tool
            .call(SessionSetTopicArgs {
                topic: "test".into(),
                __session_key: String::new(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_empty_topic_errors() {
        let (_scratch, sm) = test_session_manager();
        let tool = SessionSetTopicTool::new(sm);

        let result = tool
            .call(SessionSetTopicArgs {
                topic: "   ".into(),
                __session_key: "agent:main:default".into(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_set_topic_basic() {
        let (_scratch, sm) = test_session_manager();

        // Create session first
        let key = LegacySessionKey::Main {
            agent_id: "main".into(),
            main_key: "default".into(),
            epoch: 0,
        };
        let _ = SessionStore::get_or_create(&*sm, &key).await.unwrap();

        let tool = SessionSetTopicTool::new(sm);
        let result = tool
            .call(SessionSetTopicArgs {
                topic: "测试话题".into(),
                __session_key: "agent:main:default".into(),
            })
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.topic, "测试话题");
        assert!(output.message.contains("测试话题"));
    }
}
