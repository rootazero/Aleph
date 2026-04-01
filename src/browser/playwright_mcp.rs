//! PlaywrightMcpDriver — manages Playwright MCP sessions.
//!
//! Spawns `@playwright/mcp` as a stdio MCP server per session key.
//! Sessions are lazily created on first tool call and cached by session key.

use std::collections::HashMap;

use tokio::sync::RwLock;

use super::error::BrowserError;
use super::profile::PlaywrightMcpConfig;
use crate::mcp::{ExternalServerConfig, McpClient};

/// A running Playwright MCP session.
struct PlaywrightMcpSession {
    client: McpClient,
}

/// Manages Playwright MCP sessions with lazy creation and key-based caching.
pub struct PlaywrightMcpDriver {
    sessions: RwLock<HashMap<String, PlaywrightMcpSession>>,
    config: PlaywrightMcpConfig,
}

impl PlaywrightMcpDriver {
    pub fn new(config: PlaywrightMcpConfig) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Call a tool on the Playwright MCP server for the given session key.
    /// Creates the session lazily if it doesn't exist.
    pub async fn call_tool(
        &self,
        session_key: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        self.ensure_session(session_key).await?;

        let sessions = self.sessions.read().await;
        let session = sessions.get(session_key).ok_or_else(|| {
            BrowserError::PlaywrightError("Session not found after creation".into())
        })?;

        // MCP tools are namespaced with server prefix: "playwright-{session_key}:{tool}"
        let server_name = format!("playwright-{session_key}");
        let full_tool_name = format!("{server_name}:{tool_name}");
        let result = match session.client.call_tool(&full_tool_name, args).await {
            Ok(r) => r,
            Err(e) => {
                let err_str = e.to_string();
                let is_transport_error = err_str.contains("broken pipe")
                    || err_str.contains("connection reset")
                    || err_str.contains("process exited")
                    || err_str.contains("channel closed");
                if is_transport_error {
                    tracing::warn!(
                        "Playwright MCP transport error for session '{session_key}': {err_str}"
                    );
                    // Drop read lock before destroying session
                    drop(sessions);
                    self.destroy_session(session_key).await;
                }
                return Err(BrowserError::PlaywrightError(err_str));
            }
        };

        if !result.success {
            return Err(BrowserError::PlaywrightError(
                result
                    .error
                    .unwrap_or_else(|| "Unknown Playwright MCP error".into()),
            ));
        }
        Ok(result.content)
    }

    /// Ensure a session exists for the given key, creating one if needed.
    async fn ensure_session(&self, session_key: &str) -> Result<(), BrowserError> {
        // Fast path: check if session already exists
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(session_key) {
                return Ok(());
            }
        }

        // Slow path: create a new session
        let mut sessions = self.sessions.write().await;

        // Double-check after acquiring write lock
        if sessions.contains_key(session_key) {
            return Ok(());
        }

        let session = self.create_session(session_key).await?;
        sessions.insert(session_key.to_string(), session);
        Ok(())
    }

    /// Create a new MCP session by spawning the Playwright MCP server.
    async fn create_session(
        &self,
        session_key: &str,
    ) -> Result<PlaywrightMcpSession, BrowserError> {
        let server_name = format!("playwright-{session_key}");
        let config = ExternalServerConfig {
            name: server_name,
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            env: HashMap::new(),
            cwd: None,
            requires_runtime: Some("node".into()),
            timeout_seconds: Some(60),
        };

        let client = McpClient::new();
        client.start_external_server(config).await.map_err(|e| {
            BrowserError::PlaywrightError(format!("Failed to start Playwright MCP: {e}"))
        })?;

        tracing::info!("Playwright MCP session started for key '{session_key}'");
        Ok(PlaywrightMcpSession { client })
    }

    /// Destroy a session (for cleanup after transport errors).
    pub async fn destroy_session(&self, session_key: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.remove(session_key) {
            let _ = session.client.stop_all().await;
            tracing::info!("Playwright MCP session destroyed for key '{session_key}'");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playwright_mcp_driver_new() {
        let config = PlaywrightMcpConfig::default();
        let driver = PlaywrightMcpDriver::new(config);
        let sessions = driver.sessions.try_read().unwrap();
        assert!(sessions.is_empty());
    }
}
