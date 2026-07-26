//! `ChromeMcpDriver` — manages Chrome `DevTools` MCP sessions.
//!
//! Spawns `chrome-devtools-mcp` as a stdio MCP server per profile.
//! Sessions are lazily created on first tool call and cached by profile name.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;

use super::discovery::find_chromium;
use super::error::BrowserError;
use super::profile::ChromeMcpConfig;
use crate::mcp::{ExternalServerConfig, McpClient};
use crate::sync_primitives::Mutex;
use crate::utils::no_window::NoWindow;

/// A running Chrome `DevTools` MCP session.
struct ChromeMcpSession {
    client: McpClient,
}

/// Build the argument vector for `chrome --remote-debugging-port=0`,
/// optionally appending a `--host-resolver-rules` MAP argument to pin the
/// browser's hostname resolution to a pre-validated set of IPs (DNS
/// rebinding defense). Baseline flags stay identical whether or not a
/// pin is supplied.
fn chrome_launch_args(pin: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "--remote-debugging-port=0".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
    ];
    if let Some(p) = pin {
        args.push(p.to_string());
    }
    args
}

/// Manages Chrome `DevTools` MCP sessions with lazy creation and profile-keyed caching.
pub struct ChromeMcpDriver {
    sessions: RwLock<HashMap<String, Arc<ChromeMcpSession>>>,
    config: ChromeMcpConfig,
    /// Prevents concurrent Chrome launches from racing.
    chrome_launch_lock: tokio::sync::Mutex<()>,
    /// Per-profile serialization locks. Page selection in chrome-devtools-mcp
    /// is server-side state, so a backend's `select_page` → action pair is two
    /// round-trips that must not interleave with a concurrent same-profile
    /// operation. The backend holds the matching lock across the whole pair.
    /// Mirrors `PlaywrightCliDriver`'s per-session lock.
    profile_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    /// Per-profile pending `--host-resolver-rules` argument to inject into the
    /// next Chrome launch. The backend sets this before each navigation; the
    /// launcher consumes it inside `ensure_chrome_running`. Consumed once
    /// because Chrome's host-resolver-rules are process-wide — once Chrome is
    /// up, the pin is fixed for its lifetime.
    pending_launch_pins: Mutex<HashMap<String, String>>,
    /// Handle to the launched Chrome process, kept alive to prevent
    /// the process from becoming an orphan.
    chrome_child: Mutex<Option<tokio::process::Child>>,
}

impl ChromeMcpDriver {
    #[must_use]
    pub fn new(config: ChromeMcpConfig) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
            chrome_launch_lock: tokio::sync::Mutex::new(()),
            profile_locks: Mutex::new(HashMap::new()),
            pending_launch_pins: Mutex::new(HashMap::new()),
            chrome_child: Mutex::new(None),
        }
    }

    /// Stage a `--host-resolver-rules` argument for the next Chrome launch on
    /// `profile_name`. Passing `None` clears any previously staged value. The
    /// backend calls this immediately before invoking a navigation tool so the
    /// pin is ready when `ensure_chrome_running` needs it.
    pub fn set_pending_launch_pin(&self, profile_name: &str, pin: Option<String>) {
        let mut map = self
            .pending_launch_pins
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match pin {
            Some(p) => {
                map.insert(profile_name.to_string(), p);
            }
            None => {
                map.remove(profile_name);
            }
        }
    }

    /// Consume and return any pending pin arg for `profile_name`. Returns
    /// `None` if nothing was staged or the value was already taken.
    fn take_pending_launch_pin(&self, profile_name: &str) -> Option<String> {
        let mut map = self
            .pending_launch_pins
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(profile_name)
    }

    /// Get (or lazily create) the per-profile serialization lock. The returned
    /// `Arc<Mutex>` is held by the backend across a `select_page` → action
    /// sequence so concurrent operations on the same profile cannot interleave.
    pub fn profile_lock(&self, profile_name: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self.profile_locks.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(profile_name.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Call a tool on the Chrome `DevTools` MCP server for the given profile.
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
        let session = Arc::clone(session);
        drop(sessions);

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
                    // Only destroy if the same session is still stored
                    // (avoid racing a concurrent recreate that replaced
                    // the errored session).
                    self.destroy_session_if_same(profile_name, &session).await;
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
        sessions.insert(profile_name.to_string(), Arc::new(session));
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
                if let Err(e) = client.stop_all().await {
                    tracing::warn!("Failed to stop Chrome DevTools MCP client: {e}");
                }
                self.ensure_chrome_running(profile_name).await?;

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
    async fn ensure_chrome_running(&self, profile_name: &str) -> Result<(), BrowserError> {
        let _guard = self.chrome_launch_lock.lock().await;

        if Self::is_chrome_running().await {
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

        let pin = self.take_pending_launch_pin(profile_name);
        let args = chrome_launch_args(pin.as_deref());

        let mut cmd = Command::new(&chrome_path);
        for a in &args {
            cmd.arg(a);
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .no_window()
            .spawn()
            .map_err(|e| BrowserError::LaunchFailed(format!("Failed to launch Chrome: {e}")))?;

        // Verify the process did not immediately exit instead of blind-sleeping.
        tokio::time::sleep(Duration::from_millis(100)).await;
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(BrowserError::LaunchFailed(format!(
                    "Chrome exited immediately with status {status}"
                )));
            }
            Ok(None) => {}
            Err(e) => {
                return Err(BrowserError::LaunchFailed(format!(
                    "Failed to check Chrome process status: {e}"
                )));
            }
        }

        *self.chrome_child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        Ok(())
    }

    /// Check if a Chrome browser process is running on the system.
    async fn is_chrome_running() -> bool {
        tokio::task::spawn_blocking(|| {
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("pgrep")
                    .arg("-x")
                    .arg("Google Chrome")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
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
                // `tasklist` exits 0 even when no process matches (it prints an
                // "INFO: No tasks…" line), so the command's success status says
                // nothing about whether Chrome is running. Capture stdout and
                // look for the image name instead.
                std::process::Command::new("tasklist")
                    .arg("/NH")
                    .arg("/FI")
                    .arg("IMAGENAME eq chrome.exe")
                    .stderr(Stdio::null())
                    .no_window()
                    .output()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .to_ascii_lowercase()
                            .contains("chrome.exe")
                    })
                    .unwrap_or(false)
            }
            #[cfg(not(any(unix, target_os = "windows")))]
            {
                false
            }
        })
        .await
        .unwrap_or(false)
    }

    /// Destroy a session only if the stored session is still the expected one.
    /// Prevents a transport-error destroy from wiping out a concurrently
    /// recreated session.
    async fn destroy_session_if_same(&self, profile_name: &str, expected: &Arc<ChromeMcpSession>) {
        let session = {
            let mut sessions = self.sessions.write().await;
            match sessions.get(profile_name) {
                Some(current) if Arc::ptr_eq(current, expected) => sessions.remove(profile_name),
                _ => None,
            }
        };
        if let Some(session) = session {
            let _ = session.client.stop_all().await;
            tracing::info!(
                "Chrome MCP session destroyed for profile '{}'",
                profile_name
            );
        }
    }

    /// Destroy a session (for cleanup after transport errors).
    pub async fn destroy_session(&self, profile_name: &str) {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(profile_name)
        };
        if let Some(session) = session {
            let _ = session.client.stop_all().await;
            tracing::info!(
                "Chrome MCP session destroyed for profile '{}'",
                profile_name
            );
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

    #[test]
    fn chrome_launch_args_includes_pin_arg_when_provided() {
        // Baseline flags are present and the pin arg is appended verbatim.
        let pin = "--host-resolver-rules=\"MAP foo 1.2.3.4\"";
        let args = chrome_launch_args(Some(pin));
        assert!(
            args.iter().any(|a| a == pin),
            "pin arg must be passed through verbatim — args = {args:?}"
        );
        assert!(args.contains(&"--remote-debugging-port=0".to_string()));
    }

    #[test]
    fn chrome_launch_args_omits_pin_arg_when_none() {
        // Without a pin, no --host-resolver-rules flag reaches Chrome.
        let args = chrome_launch_args(None);
        assert!(
            !args.iter().any(|a| a.contains("--host-resolver-rules")),
            "no pin → no --host-resolver-rules — args = {args:?}"
        );
        assert!(args.contains(&"--remote-debugging-port=0".to_string()));
    }

    #[test]
    fn chrome_mcp_driver_set_pending_launch_pin_then_take_round_trip() {
        let driver = ChromeMcpDriver::new(ChromeMcpConfig::default());
        driver.set_pending_launch_pin(
            "user",
            Some("--host-resolver-rules=\"MAP x 1.1.1.1\"".to_string()),
        );
        let taken = driver.take_pending_launch_pin("user");
        assert_eq!(
            taken,
            Some("--host-resolver-rules=\"MAP x 1.1.1.1\"".to_string())
        );
        // Consume semantics: second take returns None.
        assert_eq!(driver.take_pending_launch_pin("user"), None);
    }

    #[test]
    fn chrome_mcp_driver_set_pending_launch_pin_none_clears_value() {
        let driver = ChromeMcpDriver::new(ChromeMcpConfig::default());
        driver.set_pending_launch_pin(
            "user",
            Some("--host-resolver-rules=\"MAP x 1.1.1.1\"".to_string()),
        );
        driver.set_pending_launch_pin("user", None);
        assert_eq!(driver.take_pending_launch_pin("user"), None);
    }

    #[test]
    fn chrome_mcp_driver_set_pending_launch_pin_scoped_per_profile() {
        let driver = ChromeMcpDriver::new(ChromeMcpConfig::default());
        driver.set_pending_launch_pin("user-a", Some("PIN-A".to_string()));
        driver.set_pending_launch_pin("user-b", Some("PIN-B".to_string()));
        assert_eq!(
            driver.take_pending_launch_pin("user-a"),
            Some("PIN-A".to_string())
        );
        assert_eq!(
            driver.take_pending_launch_pin("user-b"),
            Some("PIN-B".to_string())
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use std::sync::Arc;

    use super::*;
    use crate::browser::backend::BrowserBackend;
    use crate::browser::chrome_mcp_backend::ChromeMcpBackend;
    use crate::browser::network_policy::BrowserSsrfGuard;

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
        let backend = ChromeMcpBackend::new(
            driver,
            "user".to_string(),
            Arc::new(BrowserSsrfGuard::default()),
        );

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
        let backend = ChromeMcpBackend::new(
            driver,
            "user".to_string(),
            Arc::new(BrowserSsrfGuard::default()),
        );

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
