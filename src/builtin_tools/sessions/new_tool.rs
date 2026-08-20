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
use crate::gateway::session_store::SessionStore;
use crate::routing::session_key::SessionKey as RoutingSessionKey;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `session_new` tool
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

/// Output from `session_new` tool
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
    session_store: Arc<dyn SessionStore>,
}

impl SessionNewTool {
    pub fn new(session_store: Arc<dyn SessionStore>) -> Self {
        Self { session_store }
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
                "session_new: failed to parse session key '{session_key_str}'"
            ))
        })?;

        // Compute the next-epoch key first. `with_next_epoch` only advances the
        // epoch for Main and DirectMessage sessions; for Group / Task / Subagent
        // / Ephemeral sessions it returns an identical key. Closing then
        // re-opening the same key would be a destructive no-op reported as a
        // fresh conversation, so detect that and bail out honestly instead.
        let new_routing_key = routing_key.with_next_epoch();
        let new_key_str = new_routing_key.to_key_string();
        if new_key_str == *session_key_str {
            return Err(crate::error::AlephError::tool(
                "session_new: starting a new conversation is not supported for this session \
                 type (only direct/main sessions can be rolled over).",
            ));
        }

        // Retire the closing session's autonomous continuations and its `/btw`
        // side session BEFORE the bump — this tool rolls a conversation to the
        // next epoch exactly as the channel `/new` command and the
        // `sessions.new` RPC do, and after the bump a loop/goal keyed under the
        // old epoch is uncancellable and the old side session is unaddressable.
        // Passing `routing_key`, the same parse the bump above used, so the
        // derived side key is the one the turns actually ran on.
        crate::gateway::continuation_lifecycle::terminate_session_continuations(
            &routing_key,
            "session_new",
            Some(self.session_store.clone()),
        );

        // Close old session
        let legacy_key = LegacySessionKey::from_key_string(session_key_str);
        if let Some(ref lk) = legacy_key {
            // BT-D-R4-19: surfacing close failure is required — a silent
            // warn-and-continue leaves the caller believing the old session
            // was closed while it actually remains open, and any subsequent
            // `get_or_create` race can resurrect messages from the old
            // session into the new one. Propagate as a tool error so the
            // agent (and the user) can react.
            self.session_store
                .close_session(lk, args.topic.as_deref())
                .await
                .map_err(|e| {
                    warn!("session_new: failed to close old session: {}", e);
                    crate::error::AlephError::tool(format!(
                        "session_new: failed to close old session: {e}"
                    ))
                })?;
        }

        // Create the new session.
        // BT-D-R4-19: same — a silent failure here returns Ok to the
        // caller with a new_session_key that the store never accepted.
        // Future writes go to a session that doesn't exist; reads on the
        // new key come back empty. Propagate the error.
        self.session_store
            .get_or_create(&new_routing_key)
            .await
            .map_err(|e| {
                warn!("session_new: failed to create new session: {}", e);
                crate::error::AlephError::tool(format!(
                    "session_new: failed to create new session: {e}"
                ))
            })?;

        info!(
            old = %session_key_str,
            new = %new_key_str,
            "New session created via tool"
        );

        let topic_suffix = args
            .topic
            .as_ref()
            .map(|t| format!(" ({t})"))
            .unwrap_or_default();

        Ok(SessionNewOutput {
            old_session_key: session_key_str.clone(),
            new_session_key: new_key_str,
            topic: args.topic,
            message: format!("新对话已开始{topic_suffix}"),
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
        let tool = SessionNewTool::new(sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "session_new");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_empty_session_key_errors() {
        let (_scratch, sm) = test_session_manager();
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
        let (_scratch, sm) = test_session_manager();
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
