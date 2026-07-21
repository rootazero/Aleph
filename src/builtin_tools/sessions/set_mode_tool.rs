//! Session set-mode tool — switch the current session's usage mode.
//!
//! Lets the LLM switch chat / work / code conversationally ("进入编码模式"),
//! complementing the Panel composer's mode pill (R8: Everything is a Tool).
//! Writes the same `identity_meta.custom["session_mode"]` carrier the pill
//! writes through `sessions.patch`, so the next turn's `resolve_turn_mode`
//! picks it up and rebuilds the tool surface under the new partition.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::types::policies::{SessionMode, MODE_SESSION_KEY};
use crate::error::Result;
use crate::gateway::router::SessionKey as LegacySessionKey;
use crate::gateway::session_store::types::SessionPatch;
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `session_set_mode` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionSetModeArgs {
    /// Target usage mode: "chat" (lightweight conversation), "work"
    /// (multi-step productivity, the default), or "code" (software
    /// development).
    pub mode: String,

    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,
}

/// Output from `session_set_mode` tool
#[derive(Debug, Clone, Serialize)]
pub struct SessionSetModeOutput {
    pub session_key: String,
    pub mode: String,
    pub message: String,
}

/// Tool that switches the current session's usage mode.
#[derive(Clone)]
pub struct SessionSetModeTool {
    session_store: Arc<dyn SessionStore>,
}

impl SessionSetModeTool {
    pub fn new(session_store: Arc<dyn SessionStore>) -> Self {
        Self { session_store }
    }
}

#[async_trait]
impl AlephTool for SessionSetModeTool {
    const NAME: &'static str = "session_set_mode";
    const DESCRIPTION: &'static str =
        "Switch this session's usage mode: 'chat' (quick Q&A and conversation, \
         no artifacts), 'work' (multi-step productivity and deliverables — the \
         default), or 'code' (repo-focused software engineering). Each mode \
         presents a different tool surface; the switch takes effect on the \
         NEXT turn, so finish or end the current turn after switching. Call it \
         only when the user asks to switch; you may *suggest* a switch when \
         the task clearly outgrows the current mode.";

    type Args = SessionSetModeArgs;
    type Output = SessionSetModeOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "session_set_mode(mode='code')".to_string(),
            "session_set_mode(mode='chat')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session_key_str = &args.__session_key;

        if session_key_str.is_empty() {
            return Err(crate::error::AlephError::tool(
                "session_set_mode: no session context available (session key not injected)",
            ));
        }

        // Same fail-loud contract as chat.send / sessions.patch: an unknown
        // id must not silently persist as a junk override.
        let mode = SessionMode::from_id(args.mode.trim()).ok_or_else(|| {
            crate::error::AlephError::tool(format!(
                "session_set_mode: unknown mode '{}' (expected chat / work / code)",
                args.mode
            ))
        })?;

        let legacy_key = LegacySessionKey::from_key_string(session_key_str).ok_or_else(|| {
            crate::error::AlephError::tool(format!(
                "session_set_mode: failed to parse session key '{session_key_str}'"
            ))
        })?;

        let patch = SessionPatch {
            metadata: Some(serde_json::json!({ MODE_SESSION_KEY: mode.id() })),
            ..Default::default()
        };
        self.session_store
            .patch_session(&legacy_key, &patch)
            .await
            .map_err(|e| {
                crate::error::AlephError::tool(format!(
                    "session_set_mode: failed to persist mode: {e}"
                ))
            })?;

        info!(
            session = %session_key_str,
            mode = mode.id(),
            "Session mode updated via tool"
        );

        Ok(SessionSetModeOutput {
            session_key: session_key_str.clone(),
            mode: mode.id().to_string(),
            message: format!(
                "会话模式已切换为 {} — 新的工具面将在下一轮生效。",
                mode.id()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::tools::AlephTool;
    use tempfile::tempdir;

    fn test_session_manager() -> Arc<SessionManager> {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.keep().join("test.db"),
            ..Default::default()
        };
        Arc::new(SessionManager::new(config).unwrap())
    }

    #[test]
    fn test_tool_definition() {
        let sm = test_session_manager();
        let tool = SessionSetModeTool::new(sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "session_set_mode");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_empty_session_key_errors() {
        let sm = test_session_manager();
        let tool = SessionSetModeTool::new(sm);

        let result = tool
            .call(SessionSetModeArgs {
                mode: "code".into(),
                __session_key: String::new(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unknown_mode_errors() {
        let sm = test_session_manager();
        let tool = SessionSetModeTool::new(sm);

        let result = tool
            .call(SessionSetModeArgs {
                mode: "game".into(),
                __session_key: "agent:main:default".into(),
            })
            .await;

        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("game"), "error should name the bad mode: {err}");
    }

    #[tokio::test]
    async fn test_set_mode_persists_the_session_key() {
        let sm = test_session_manager();

        let key = LegacySessionKey::Main {
            agent_id: "main".into(),
            main_key: "default".into(),
            epoch: 0,
        };
        let _ = SessionStore::get_or_create(&*sm, &key).await.unwrap();

        let tool = SessionSetModeTool::new(Arc::clone(&sm) as Arc<dyn SessionStore>);
        let result = tool
            .call(SessionSetModeArgs {
                mode: "code".into(),
                __session_key: "agent:main:default".into(),
            })
            .await
            .unwrap();
        assert_eq!(result.mode, "code");

        // The value must land in identity_meta.custom under MODE_SESSION_KEY —
        // the exact carrier resolve_turn_mode reads next turn.
        let meta = SessionStore::get_metadata(&*sm, &key)
            .await
            .unwrap()
            .expect("session metadata");
        let stored = meta
            .identity_meta
            .and_then(|im| im.custom.get(MODE_SESSION_KEY).cloned());
        assert_eq!(stored, Some(serde_json::Value::String("code".into())));
    }
}
