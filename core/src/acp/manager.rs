//! AcpHarnessManager — lifecycle management for ACP harness sessions.

use std::collections::HashMap;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::harnesses::{ClaudeCodeHarness, CodexHarness, GeminiHarness};
use crate::acp::session::AcpSession;
use crate::error::{AlephError, Result};

// =============================================================================
// AcpManagerConfig
// =============================================================================

/// Configuration for the ACP harness manager.
#[derive(Debug, Clone, Default)]
pub struct AcpManagerConfig {
    /// Per-harness executable path overrides (key = harness ID).
    pub executables: HashMap<String, String>,
    /// Per-harness enabled flags (key = harness ID). Defaults to true if absent.
    pub enabled: HashMap<String, bool>,
}

// =============================================================================
// AcpHarnessManager
// =============================================================================

/// Manages ACP harness registrations and active sessions.
///
/// Supports two execution modes:
/// - **NativeAcp**: Persistent subprocess with ACP protocol (Gemini).
///   Uses lazy-start sessions that are automatically respawned if dead.
/// - **Oneshot**: Fresh process per prompt (Claude Code, Codex).
///   No persistent session needed.
pub struct AcpHarnessManager {
    harnesses: HashMap<String, Box<dyn AcpHarness>>,
    /// Active sessions for NativeAcp harnesses only.
    sessions: RwLock<HashMap<String, AcpSession>>,
}

impl AcpHarnessManager {
    /// Create a manager with all default harnesses enabled.
    pub fn new() -> Self {
        Self::with_config(AcpManagerConfig::default())
    }

    /// Create a manager using the given configuration.
    ///
    /// Only registers harnesses whose `enabled` flag is not explicitly `false`.
    /// Applies executable overrides from `config.executables`.
    pub fn with_config(config: AcpManagerConfig) -> Self {
        let mut harnesses: HashMap<String, Box<dyn AcpHarness>> = HashMap::new();

        let candidates: Vec<(&str, Box<dyn FnOnce(Option<String>) -> Box<dyn AcpHarness>>)> = vec![
            ("claude-code", Box::new(|exe| Box::new(ClaudeCodeHarness::new(exe)))),
            ("codex", Box::new(|exe| Box::new(CodexHarness::new(exe)))),
            ("gemini", Box::new(|exe| Box::new(GeminiHarness::new(exe)))),
        ];

        for (id, factory) in candidates {
            // Skip if explicitly disabled
            if config.enabled.get(id) == Some(&false) {
                continue;
            }
            let exe_override = config.executables.get(id).cloned();
            let harness = factory(exe_override);
            harnesses.insert(id.to_string(), harness);
        }

        Self {
            harnesses,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// List registered harness IDs.
    pub fn harness_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.harnesses.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Check whether a harness with the given ID is registered.
    pub fn has_harness(&self, id: &str) -> bool {
        self.harnesses.contains_key(id)
    }

    /// Get the display name for a registered harness.
    pub fn display_name(&self, id: &str) -> Option<&str> {
        self.harnesses.get(id).map(|h| h.display_name())
    }

    /// Get the execution mode for a registered harness.
    pub fn harness_mode(&self, id: &str) -> Option<HarnessMode> {
        self.harnesses.get(id).map(|h| h.mode())
    }

    /// Return IDs of harnesses whose executables are available on this system.
    pub async fn available_harnesses(&self) -> Vec<String> {
        let mut available = Vec::new();
        for (id, harness) in &self.harnesses {
            if harness.is_available().await {
                available.push(id.clone());
            }
        }
        available.sort();
        available
    }

    /// Ensure a live ACP session exists for the given NativeAcp harness.
    ///
    /// - If a session exists and is alive, this is a no-op.
    /// - If a session exists but is dead, it is removed and respawned.
    /// - If no session exists, a new one is spawned.
    ///
    /// Only meaningful for NativeAcp harnesses; oneshot harnesses don't need sessions.
    pub async fn ensure_session(&self, harness_id: &str, cwd: &str) -> Result<()> {
        let harness = self.harnesses.get(harness_id).ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
        })?;

        // Check if we already have a live session
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(harness_id) {
                if session.is_alive() {
                    return Ok(());
                }
                // Dead session — remove it
                warn!(harness_id, "ACP session died, respawning");
                sessions.remove(harness_id);
            }
        }

        // Spawn a new session outside the lock
        let session = harness.spawn_session(Some(cwd)).await?;
        info!(harness_id, "ACP session started");

        self.sessions.write().await.insert(harness_id.to_string(), session);
        Ok(())
    }

    /// Send a prompt to the specified harness, using the appropriate mode.
    ///
    /// - **NativeAcp**: Ensures session, sends `session/prompt`, collects streaming response.
    /// - **Oneshot**: Spawns a fresh process, waits for output.
    pub async fn prompt(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
    ) -> Result<String> {
        let harness = self.harnesses.get(harness_id).ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
        })?;

        match harness.mode() {
            HarnessMode::NativeAcp => {
                self.ensure_session(harness_id, cwd).await?;

                let timeout = harness.build_config(Some(cwd)).timeout;

                let mut sessions = self.sessions.write().await;
                let session = sessions.get_mut(harness_id).ok_or_else(|| {
                    AlephError::tool(format!(
                        "ACP session for '{}' disappeared unexpectedly",
                        harness_id
                    ))
                })?;

                let (text, _notifications) = session.prompt(prompt_text, cwd, timeout).await?;
                Ok(text)
            }
            HarnessMode::Oneshot => {
                harness.execute_oneshot(prompt_text, cwd).await
            }
        }
    }

    /// Cancel the current operation on the specified harness.
    pub async fn cancel(&self, harness_id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(harness_id).ok_or_else(|| {
            AlephError::tool(format!(
                "No active ACP session for '{}'",
                harness_id
            ))
        })?;
        session.cancel().await
    }

    /// Kill all active sessions.
    pub async fn shutdown_all(&self) {
        let mut sessions = self.sessions.write().await;
        for (id, session) in sessions.iter_mut() {
            info!(harness_id = %id, "Shutting down ACP session");
            session.kill().await;
        }
        sessions.clear();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_registers_harnesses() {
        let manager = AcpHarnessManager::new();
        let ids = manager.harness_ids();
        assert!(ids.contains(&"claude-code".to_string()));
        assert!(ids.contains(&"codex".to_string()));
        assert!(ids.contains(&"gemini".to_string()));
    }

    #[test]
    fn test_manager_has_harness() {
        let manager = AcpHarnessManager::new();
        assert!(manager.has_harness("claude-code"));
        assert!(!manager.has_harness("unknown"));
    }

    #[test]
    fn test_manager_disable_harness() {
        let mut config = AcpManagerConfig::default();
        config.enabled.insert("codex".to_string(), false);
        let manager = AcpHarnessManager::with_config(config);
        assert!(!manager.has_harness("codex"));
        assert!(manager.has_harness("claude-code"));
        assert!(manager.has_harness("gemini"));
    }

    #[test]
    fn test_manager_display_name() {
        let manager = AcpHarnessManager::new();
        assert_eq!(manager.display_name("claude-code"), Some("Claude Code"));
        assert_eq!(manager.display_name("codex"), Some("Codex"));
        assert_eq!(manager.display_name("gemini"), Some("Gemini"));
        assert_eq!(manager.display_name("unknown"), None);
    }

    #[test]
    fn test_manager_harness_modes() {
        let manager = AcpHarnessManager::new();
        assert_eq!(manager.harness_mode("gemini"), Some(HarnessMode::NativeAcp));
        assert_eq!(manager.harness_mode("claude-code"), Some(HarnessMode::Oneshot));
        assert_eq!(manager.harness_mode("codex"), Some(HarnessMode::Oneshot));
    }

    #[test]
    fn test_manager_executable_override() {
        let mut config = AcpManagerConfig::default();
        config.executables.insert("claude-code".to_string(), "/custom/claude".to_string());
        let manager = AcpHarnessManager::with_config(config);
        assert!(manager.has_harness("claude-code"));
        // Verify override is applied via build_config
        let harness = manager.harnesses.get("claude-code").unwrap();
        let cfg = harness.build_config(None);
        assert_eq!(cfg.executable, "/custom/claude");
    }
}
