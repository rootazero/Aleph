//! Session compact tool — trim the current conversation's stored history.
//!
//! Deletes the older messages of the current session, keeping the most recent
//! [`SESSION_COMPACT_KEEP_LAST_N`], so the next turn's prompt is built from a
//! smaller transcript. This is the tool form of the `/compact` (alias
//! `/compress`) slash command and the `session.compact` RPC — the same
//! deterministic `SessionStore::compact(KeepLastN)` operation the TUI and CLI
//! already drive — letting the LLM (R8) or any surface reach it uniformly.
//!
//! Not a context *loss*: the memory layer's hierarchical summaries
//! (`SessionCompactor::post_turn_compress`) already distilled the dropped turns
//! into recallable notes, and recall re-injects their essence on later turns.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::gateway::router::SessionKey as LegacySessionKey;
use crate::gateway::session_store::types::{CompactStrategy, SESSION_COMPACT_KEEP_LAST_N};
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Arguments for the `session_compact` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionCompactArgs {
    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,
}

/// Output from `session_compact` tool
#[derive(Debug, Clone, Serialize)]
pub struct SessionCompactOutput {
    /// Number of older messages removed from the transcript
    pub removed: usize,
    /// Messages remaining after compaction
    pub kept: usize,
    /// Human-readable status message
    pub message: String,
}

/// Tool that compacts the current conversation's stored history.
#[derive(Clone)]
pub struct SessionCompactTool {
    session_store: Arc<dyn SessionStore>,
}

impl SessionCompactTool {
    pub fn new(session_store: Arc<dyn SessionStore>) -> Self {
        Self { session_store }
    }
}

#[async_trait]
impl AlephTool for SessionCompactTool {
    const NAME: &'static str = "session_compact";
    const DESCRIPTION: &'static str =
        "Compact the current conversation: drop the oldest messages while keeping \
         the most recent ones, shrinking the context window. The distilled memory \
         of the dropped turns is preserved and recalled automatically. Use when the \
         user asks to compact, compress, or shorten the conversation.";

    type Args = SessionCompactArgs;
    type Output = SessionCompactOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec!["session_compact()".to_string()])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session_key_str = &args.__session_key;

        if session_key_str.is_empty() {
            return Err(crate::error::AlephError::tool(
                "session_compact: no session context available (session key not injected)",
            ));
        }

        let key = LegacySessionKey::from_key_string(session_key_str).ok_or_else(|| {
            crate::error::AlephError::tool(format!(
                "session_compact: failed to parse session key '{session_key_str}'"
            ))
        })?;

        let before = self
            .session_store
            .get_history(&key, None)
            .await
            .map(|m| m.len())
            .unwrap_or(0);

        let result = self
            .session_store
            .compact(
                &key,
                CompactStrategy::KeepLastN {
                    n: SESSION_COMPACT_KEEP_LAST_N,
                },
            )
            .await
            .map_err(|e| {
                crate::error::AlephError::tool(format!("session_compact: compaction failed: {e}"))
            })?;

        let kept = before.saturating_sub(result.deleted);
        let message = if result.deleted > 0 {
            info!(
                session = %session_key_str,
                removed = result.deleted,
                kept,
                "Session compacted via tool"
            );
            format!(
                "🗜 Compacted {} older message(s); kept the {} most recent.",
                result.deleted, kept
            )
        } else {
            "Session history is already compact.".to_string()
        };

        Ok(SessionCompactOutput {
            removed: result.deleted,
            kept,
            message,
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
        let tool = SessionCompactTool::new(sm);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "session_compact");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_empty_session_key_errors() {
        let sm = test_session_manager();
        let tool = SessionCompactTool::new(sm);

        let result = tool
            .call(SessionCompactArgs {
                __session_key: String::new(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_compact_empty_session_is_noop() {
        let sm = test_session_manager();
        let tool = SessionCompactTool::new(sm);

        // A fresh session with no messages: nothing to drop, honest no-op.
        let output = tool
            .call(SessionCompactArgs {
                __session_key: "agent:main:default".into(),
            })
            .await
            .unwrap();

        assert_eq!(output.removed, 0);
        assert!(output.message.contains("already compact"));
    }
}
