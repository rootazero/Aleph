//! ChromeMcpDriver — manages Chrome DevTools MCP sessions.
//!
//! Spawns `chrome-devtools-mcp` as a stdio MCP server per profile.
//! Sessions are lazily created on first tool call and cached by profile name.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::RwLock;

use super::discovery::find_chromium;
use super::error::BrowserError;
use super::profile::ChromeMcpConfig;
use crate::mcp::{ExternalServerConfig, McpClient};

/// A running Chrome DevTools MCP session.
struct ChromeMcpSession {
    client: McpClient,
}

/// Manages Chrome DevTools MCP sessions with lazy creation and profile-keyed caching.
pub struct ChromeMcpDriver {
    sessions: RwLock<HashMap<String, ChromeMcpSession>>,
    config: ChromeMcpConfig,
}

impl ChromeMcpDriver {
    pub fn new(config: ChromeMcpConfig) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Call a tool on the Chrome DevTools MCP server for the given profile.
    /// Creates the session lazily if it doesn't exist.
    pub async fn call_tool(
        &self,
        profile_name: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        self.ensure_session(profile_name).await?;

        let sessions = self.sessions.read().await;
        let session = sessions.get(profile_name).ok_or_else(|| {
            BrowserError::ChromeMcpError("Session not found after creation".into())
        })?;

        // MCP tools are namespaced with server prefix: "chrome-mcp-{profile}:{tool}"
        let server_name = format!("chrome-mcp-{profile_name}");
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
                        "Chrome MCP transport error for profile '{profile_name}': {err_str}"
                    );
                    // Drop read lock before destroying session
                    drop(sessions);
                    self.destroy_session(profile_name).await;
                }
                return Err(BrowserError::ChromeMcpError(err_str));
            }
        };

        if !result.success {
            return Err(BrowserError::ChromeMcpError(
                result
                    .error
                    .unwrap_or_else(|| "Unknown Chrome MCP error".into()),
            ));
        }
        Ok(result.content)
    }

    /// Ensure a session exists for the given profile, creating one if needed.
    async fn ensure_session(&self, profile_name: &str) -> Result<(), BrowserError> {
        // Fast path: check if session already exists
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(profile_name) {
                return Ok(());
            }
        }

        // Slow path: create a new session
        let mut sessions = self.sessions.write().await;

        // Double-check after acquiring write lock
        if sessions.contains_key(profile_name) {
            return Ok(());
        }

        let session = self.create_session(profile_name).await?;
        sessions.insert(profile_name.to_string(), session);
        Ok(())
    }

    /// Create a new MCP session by spawning chrome-devtools-mcp.
    async fn create_session(&self, profile_name: &str) -> Result<ChromeMcpSession, BrowserError> {
        let server_name = format!("chrome-mcp-{profile_name}");
        let config = ExternalServerConfig {
            name: server_name.clone(),
            command: self.config.command.clone(),
            args: self.config.args.clone(),
            env: HashMap::new(),
            cwd: None,
            requires_runtime: Some("node".into()),
            timeout_seconds: Some(60),
        };

        let client = McpClient::new();
        match client.start_external_server(config).await {
            Ok(()) => {
                tracing::info!("Chrome DevTools MCP session started for profile '{profile_name}'");
                tracing::warn!(
                    "Existing-session mode connects to your Chrome with remote debugging enabled. \
                     Any local process can access browser data (cookies, passwords) via the debug port. \
                     This is Chrome's standard debugging interface (same as DevTools)."
                );
                Ok(ChromeMcpSession { client })
            }
            Err(e) => {
                tracing::info!(
                    "Chrome DevTools MCP connection failed, attempting to launch Chrome: {e}"
                );
                self.ensure_chrome_running().await?;

                // Retry after Chrome launch
                let retry_config = ExternalServerConfig {
                    name: server_name,
                    command: self.config.command.clone(),
                    args: self.config.args.clone(),
                    env: HashMap::new(),
                    cwd: None,
                    requires_runtime: Some("node".into()),
                    timeout_seconds: Some(60),
                };

                let retry_client = McpClient::new();
                retry_client
                    .start_external_server(retry_config)
                    .await
                    .map_err(|e: crate::error::AlephError| {
                        BrowserError::AttachFailed(format!(
                            "Failed to connect Chrome DevTools MCP after launching Chrome: {e}"
                        ))
                    })?;

                Ok(ChromeMcpSession {
                    client: retry_client,
                })
            }
        }
    }

    /// Ensure Chrome is running with remote debugging enabled.
    async fn ensure_chrome_running(&self) -> Result<(), BrowserError> {
        if Self::is_chrome_running() {
            return Err(BrowserError::AttachFailed(
                "Chrome is running but remote debugging is not enabled. \
                 Please restart Chrome or enable debugging at chrome://inspect/#remote-debugging"
                    .into(),
            ));
        }

        let chrome_path = find_chromium()?;
        tracing::info!(
            "Launching Chrome with remote debugging: {}",
            chrome_path.display()
        );

        Command::new(&chrome_path)
            .arg("--remote-debugging-port=0")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to launch Chrome: {e}")))?;

        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    }

    /// Check if a Chrome browser process is running on the system.
    fn is_chrome_running() -> bool {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("pgrep")
                .arg("-x")
                .arg("Google Chrome")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("pgrep")
                .arg("-x")
                .arg("chrome")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("tasklist")
            .arg("/FI")
            .arg("IMAGENAME eq chrome.exe")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        false
    }
    }

    /// Destroy a session (for cleanup after transport errors).
    pub async fn destroy_session(&self, profile_name: &str) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.remove(profile_name) {
            let _ = session.client.stop_all().await;
            tracing::info!("Chrome MCP session destroyed for profile '{profile_name}'");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chrome_mcp_driver_new() {
        let config = ChromeMcpConfig::default();
        let driver = ChromeMcpDriver::new(config);
        let sessions = driver.sessions.try_read().unwrap();
        assert!(sessions.is_empty());
    }
}

#[cfg(test)]
mod integration_tests {
    use std::sync::Arc;

    use super::*;
    use crate::browser::backend::BrowserBackend;
    use crate::browser::chrome_mcp_backend::ChromeMcpBackend;

    #[tokio::test]
    #[ignore] // Requires Chrome + npx chrome-devtools-mcp installed
    async fn test_chrome_mcp_list_tools() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config));
        // Ensure session is created
        driver
            .ensure_session("user")
            .await
            .expect("session should start");
        let sessions = driver.sessions.read().await;
        let session = sessions.get("user").expect("session exists");
        let tools = session.client.list_tools().await;
        println!("=== Available MCP tools ({}) ===", tools.len());
        for tool in &tools {
            println!("  {} — {}", tool.name, tool.description);
        }
        assert!(!tools.is_empty(), "Should have tools available");
    }

    #[tokio::test]
    #[ignore]
    async fn test_chrome_mcp_list_tabs_raw() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config));
        driver.ensure_session("user").await.expect("session");
        let sessions = driver.sessions.read().await;
        let session = sessions.get("user").expect("session");

        // Call directly via client with full prefixed name
        println!("=== Calling chrome-mcp-user:list_pages via client...");
        let r1 = session
            .client
            .call_tool("chrome-mcp-user:list_pages", serde_json::json!({}))
            .await;
        println!("=== client result: {r1:?}");

        // Also try raw without prefix via the connection directly
        // Let's just see what tool names the server actually has
        let tools = session.client.list_tools().await;
        let page_tools: Vec<_> = tools
            .iter()
            .filter(|t| t.name.contains("page"))
            .map(|t| &t.name)
            .collect();
        println!("=== page-related tools: {page_tools:?}");
    }

    #[tokio::test]
    #[ignore]
    async fn test_chrome_mcp_list_tabs() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config));
        let backend = ChromeMcpBackend::new(driver, "user".to_string());

        println!("Calling list_tabs...");
        match backend.list_tabs().await {
            Ok(tabs_text) => {
                println!("Open tabs:\n{tabs_text}");
                assert!(!tabs_text.is_empty(), "Should have at least one tab open");
            }
            Err(e) => {
                panic!("list_tabs failed: {e}");
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_chrome_mcp_snapshot() {
        let config = ChromeMcpConfig::default();
        let driver = Arc::new(ChromeMcpDriver::new(config));
        let backend = ChromeMcpBackend::new(driver, "user".to_string());

        let tabs_text = backend.list_tabs().await.expect("list_tabs");
        println!("Tabs for snapshot:\n{tabs_text}");
        // Parse first numeric tab id from text
        let tab_id = tabs_text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let colon_pos = line.find(": ")?;
                let id_str = line.get(..colon_pos)?.trim();
                if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
                    Some(id_str.to_string())
                } else {
                    None
                }
            })
            .nth(1)
            .or_else(|| {
                tabs_text.lines().find_map(|line| {
                    let line = line.trim();
                    let colon_pos = line.find(": ")?;
                    let id_str = line.get(..colon_pos)?.trim();
                    if id_str.chars().all(|c| c.is_ascii_digit()) && !id_str.is_empty() {
                        Some(id_str.to_string())
                    } else {
                        None
                    }
                })
            })
            .expect("need at least one tab");
        let tab_id = &tab_id;

        let snapshot = backend
            .snapshot(tab_id)
            .await
            .expect("snapshot should succeed");
        assert!(
            !snapshot.snapshot_text.is_empty(),
            "Snapshot should have content"
        );
        println!("Snapshot text length: {}", snapshot.snapshot_text.len());
    }
}
