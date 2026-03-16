//! AcpHarnessManager — lifecycle management for ACP harness sessions.
//!
//! Supports runtime dynamic harness registration and unregistration.

use std::collections::HashMap;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::acp::harness::{AcpHarness, HarnessMode};
use crate::acp::harnesses::{ClaudeCodeHarness, CodexHarness, CustomHarness, GeminiHarness};
use crate::acp::session::AcpSession;
use crate::config::types::acp::AcpHarnessEntry;
use crate::error::{AlephError, Result};

// =============================================================================
// AcpManagerConfig (backward compat)
// =============================================================================

/// Legacy configuration for the ACP harness manager.
///
/// Prefer [`AcpHarnessManager::from_entries`] for new code.
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
///
/// All harness and config state is behind `RwLock` for runtime dynamic management.
pub struct AcpHarnessManager {
    harnesses: RwLock<HashMap<String, Box<dyn AcpHarness>>>,
    configs: RwLock<HashMap<String, AcpHarnessEntry>>,
    /// Active sessions for NativeAcp harnesses only.
    sessions: RwLock<HashMap<String, AcpSession>>,
}

impl Default for AcpHarnessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpHarnessManager {
    /// Create a manager with all default harnesses enabled.
    pub fn new() -> Self {
        let entries: HashMap<String, AcpHarnessEntry> = AcpHarnessEntry::all_presets()
            .into_iter()
            .collect();
        Self::from_entries(entries)
    }

    /// Create a manager from a map of harness entries.
    ///
    /// For each entry where `enabled == true`:
    /// - If `entry.preset` matches a known preset, uses the dedicated harness impl
    ///   with `entry.executable` as override.
    /// - Otherwise, uses `CustomHarness`.
    pub fn from_entries(entries: HashMap<String, AcpHarnessEntry>) -> Self {
        let mut harnesses: HashMap<String, Box<dyn AcpHarness>> = HashMap::new();
        let mut configs: HashMap<String, AcpHarnessEntry> = HashMap::new();

        for (id, entry) in entries {
            if !entry.enabled {
                continue;
            }
            let harness = Self::build_harness(&id, &entry);
            harnesses.insert(id.clone(), harness);
            configs.insert(id, entry);
        }

        Self {
            harnesses: RwLock::new(harnesses),
            configs: RwLock::new(configs),
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Create a manager using the legacy configuration.
    ///
    /// Converts `AcpManagerConfig` to entries and delegates to `from_entries`.
    pub fn with_config(config: AcpManagerConfig) -> Self {
        let mut entries: HashMap<String, AcpHarnessEntry> = AcpHarnessEntry::all_presets()
            .into_iter()
            .collect();

        // Apply enabled/disabled overrides
        for (id, enabled) in &config.enabled {
            if let Some(entry) = entries.get_mut(id) {
                entry.enabled = *enabled;
            }
        }

        // Apply executable overrides
        for (id, exe) in &config.executables {
            if let Some(entry) = entries.get_mut(id) {
                entry.executable = Some(exe.clone());
            }
        }

        Self::from_entries(entries)
    }

    /// Factory: build the right harness implementation from an entry.
    fn build_harness(id: &str, entry: &AcpHarnessEntry) -> Box<dyn AcpHarness> {
        let preset = entry.preset.as_deref().unwrap_or("");
        match preset {
            "claude_code" => {
                Box::new(ClaudeCodeHarness::new(entry.executable.clone()))
            }
            "codex" => {
                Box::new(CodexHarness::new(entry.executable.clone()))
            }
            "gemini" => {
                Box::new(GeminiHarness::new(entry.executable.clone()))
            }
            _ => {
                // Custom or unknown preset — use CustomHarness
                Box::new(CustomHarness::new(id.to_string(), entry.clone()))
            }
        }
    }

    // =========================================================================
    // Query methods (all async due to RwLock)
    // =========================================================================

    /// List registered harness IDs.
    pub async fn harness_ids(&self) -> Vec<String> {
        let harnesses = self.harnesses.read().await;
        let mut ids: Vec<String> = harnesses.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Check whether a harness with the given ID is registered.
    pub async fn has_harness(&self, id: &str) -> bool {
        let harnesses = self.harnesses.read().await;
        harnesses.contains_key(id)
    }

    /// Get the display name for a registered harness.
    pub async fn display_name(&self, id: &str) -> Option<String> {
        let harnesses = self.harnesses.read().await;
        harnesses.get(id).map(|h| h.display_name().to_string())
    }

    /// Get the execution mode for a registered harness.
    pub async fn harness_mode(&self, id: &str) -> Option<HarnessMode> {
        let harnesses = self.harnesses.read().await;
        harnesses.get(id).map(|h| h.mode())
    }

    /// Return IDs of harnesses whose executables are available on this system.
    pub async fn available_harnesses(&self) -> Vec<String> {
        let harnesses = self.harnesses.read().await;
        let mut available = Vec::new();
        for (id, harness) in harnesses.iter() {
            if harness.is_available().await {
                available.push(id.clone());
            }
        }
        available.sort();
        available
    }

    // =========================================================================
    // Dynamic management
    // =========================================================================

    /// Register a new harness at runtime.
    pub async fn register_harness(&self, id: String, entry: AcpHarnessEntry) -> Result<()> {
        if !entry.enabled {
            return Err(AlephError::tool(format!(
                "Cannot register disabled harness '{}'",
                id
            )));
        }

        let harness = Self::build_harness(&id, &entry);

        let mut harnesses = self.harnesses.write().await;
        let mut configs = self.configs.write().await;

        if harnesses.contains_key(&id) {
            return Err(AlephError::tool(format!(
                "Harness '{}' is already registered. Use update_harness to modify it.",
                id
            )));
        }

        info!(harness_id = %id, "Registering new ACP harness");
        harnesses.insert(id.clone(), harness);
        configs.insert(id, entry);
        Ok(())
    }

    /// Unregister a harness at runtime.
    ///
    /// Rejects preset harness IDs — use `update_harness` to disable them instead.
    pub async fn unregister_harness(&self, id: &str) -> Result<()> {
        if AcpHarnessEntry::is_preset_id(id) {
            return Err(AlephError::tool(format!(
                "Cannot unregister preset harness '{}'. Disable it via update_harness instead.",
                id
            )));
        }

        let mut harnesses = self.harnesses.write().await;
        let mut configs = self.configs.write().await;

        if harnesses.remove(id).is_none() {
            return Err(AlephError::tool(format!(
                "Harness '{}' is not registered",
                id
            )));
        }

        configs.remove(id);

        // Also kill any active session for this harness
        let mut sessions = self.sessions.write().await;
        if let Some(mut session) = sessions.remove(id) {
            session.kill().await;
        }

        info!(harness_id = %id, "Unregistered ACP harness");
        Ok(())
    }

    /// Update an existing harness configuration.
    ///
    /// Replaces the harness instance and config. Kills any active session.
    pub async fn update_harness(&self, id: &str, entry: AcpHarnessEntry) -> Result<()> {
        let mut harnesses = self.harnesses.write().await;
        let mut configs = self.configs.write().await;

        if !entry.enabled {
            // Disable: remove harness but keep config
            harnesses.remove(id);
            configs.insert(id.to_string(), entry);

            // Kill active session
            let mut sessions = self.sessions.write().await;
            if let Some(mut session) = sessions.remove(id) {
                session.kill().await;
            }

            info!(harness_id = %id, "Disabled ACP harness");
            return Ok(());
        }

        let harness = Self::build_harness(id, &entry);
        harnesses.insert(id.to_string(), harness);
        configs.insert(id.to_string(), entry);

        // Kill any active session so it will be respawned with the new config
        let mut sessions = self.sessions.write().await;
        if let Some(mut session) = sessions.remove(id) {
            session.kill().await;
        }

        info!(harness_id = %id, "Updated ACP harness");
        Ok(())
    }

    /// Get the config entry for a specific harness.
    pub async fn get_config(&self, id: &str) -> Option<AcpHarnessEntry> {
        let configs = self.configs.read().await;
        configs.get(id).cloned()
    }

    /// List all config entries as (id, entry) pairs.
    pub async fn list_configs(&self) -> Vec<(String, AcpHarnessEntry)> {
        let configs = self.configs.read().await;
        let mut result: Vec<(String, AcpHarnessEntry)> = configs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    // =========================================================================
    // Session management
    // =========================================================================

    /// Ensure a live ACP session exists for the given NativeAcp harness.
    ///
    /// - If a session exists and is alive, this is a no-op.
    /// - If a session exists but is dead, it is removed and respawned.
    /// - If no session exists, a new one is spawned.
    ///
    /// Only meaningful for NativeAcp harnesses; oneshot harnesses don't need sessions.
    pub async fn ensure_session(&self, harness_id: &str, cwd: &str) -> Result<()> {
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

        // Get harness reference and spawn outside the session lock
        let harnesses = self.harnesses.read().await;
        let harness = harnesses.get(harness_id).ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
        })?;

        let session = harness.spawn_session(Some(cwd)).await?;
        info!(harness_id, "ACP session started");
        drop(harnesses);

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
        let harnesses = self.harnesses.read().await;
        let harness = harnesses.get(harness_id).ok_or_else(|| {
            AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
        })?;

        match harness.mode() {
            HarnessMode::NativeAcp => {
                let timeout = harness.build_config(Some(cwd)).timeout;
                // Drop read lock before calling ensure_session (which needs write)
                drop(harnesses);

                self.ensure_session(harness_id, cwd).await?;

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

    #[tokio::test]
    async fn test_manager_registers_harnesses() {
        let manager = AcpHarnessManager::new();
        let ids = manager.harness_ids().await;
        assert!(ids.contains(&"claude_code".to_string()));
        assert!(ids.contains(&"codex".to_string()));
        assert!(ids.contains(&"gemini".to_string()));
    }

    #[tokio::test]
    async fn test_manager_has_harness() {
        let manager = AcpHarnessManager::new();
        assert!(manager.has_harness("claude_code").await);
        assert!(!manager.has_harness("unknown").await);
    }

    #[tokio::test]
    async fn test_manager_disable_harness() {
        let mut config = AcpManagerConfig::default();
        config.enabled.insert("codex".to_string(), false);
        let manager = AcpHarnessManager::with_config(config);
        assert!(!manager.has_harness("codex").await);
        assert!(manager.has_harness("claude_code").await);
        assert!(manager.has_harness("gemini").await);
    }

    #[tokio::test]
    async fn test_manager_display_name() {
        let manager = AcpHarnessManager::new();
        assert_eq!(manager.display_name("claude_code").await, Some("Claude Code".to_string()));
        assert_eq!(manager.display_name("codex").await, Some("Codex".to_string()));
        assert_eq!(manager.display_name("gemini").await, Some("Gemini".to_string()));
        assert_eq!(manager.display_name("unknown").await, None);
    }

    #[tokio::test]
    async fn test_manager_harness_modes() {
        let manager = AcpHarnessManager::new();
        assert_eq!(manager.harness_mode("gemini").await, Some(HarnessMode::NativeAcp));
        assert_eq!(manager.harness_mode("claude_code").await, Some(HarnessMode::Oneshot));
        assert_eq!(manager.harness_mode("codex").await, Some(HarnessMode::Oneshot));
    }

    #[tokio::test]
    async fn test_manager_executable_override() {
        let mut config = AcpManagerConfig::default();
        config.executables.insert("claude_code".to_string(), "/custom/claude".to_string());
        let manager = AcpHarnessManager::with_config(config);
        assert!(manager.has_harness("claude_code").await);
        // Verify override is applied via build_config
        let harnesses = manager.harnesses.read().await;
        let harness = harnesses.get("claude_code").unwrap();
        let cfg = harness.build_config(None);
        assert_eq!(cfg.executable, "/custom/claude");
    }

    #[tokio::test]
    async fn test_from_entries_with_custom_harness() {
        let mut entries = HashMap::new();
        entries.insert("my-tool".to_string(), AcpHarnessEntry {
            display_name: "My Tool".to_string(),
            executable: Some("my-tool-bin".to_string()),
            enabled: true,
            ..Default::default()
        });

        let manager = AcpHarnessManager::from_entries(entries);
        assert!(manager.has_harness("my-tool").await);
        assert_eq!(manager.display_name("my-tool").await, Some("My Tool".to_string()));
    }

    #[tokio::test]
    async fn test_from_entries_skips_disabled() {
        let mut entries = HashMap::new();
        entries.insert("disabled-tool".to_string(), AcpHarnessEntry {
            display_name: "Disabled".to_string(),
            enabled: false,
            ..Default::default()
        });

        let manager = AcpHarnessManager::from_entries(entries);
        assert!(!manager.has_harness("disabled-tool").await);
    }

    #[tokio::test]
    async fn test_register_harness() {
        let manager = AcpHarnessManager::from_entries(HashMap::new());
        let entry = AcpHarnessEntry {
            display_name: "New Tool".to_string(),
            executable: Some("new-tool".to_string()),
            enabled: true,
            ..Default::default()
        };

        manager.register_harness("new-tool".to_string(), entry).await.unwrap();
        assert!(manager.has_harness("new-tool").await);
        assert_eq!(manager.display_name("new-tool").await, Some("New Tool".to_string()));
    }

    #[tokio::test]
    async fn test_register_duplicate_fails() {
        let manager = AcpHarnessManager::new();
        let entry = AcpHarnessEntry {
            display_name: "Dup".to_string(),
            enabled: true,
            preset: Some("claude_code".to_string()),
            ..Default::default()
        };

        let result = manager.register_harness("claude_code".to_string(), entry).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unregister_custom_harness() {
        let manager = AcpHarnessManager::from_entries(HashMap::new());
        let entry = AcpHarnessEntry {
            display_name: "Temp".to_string(),
            executable: Some("temp".to_string()),
            enabled: true,
            ..Default::default()
        };

        manager.register_harness("temp".to_string(), entry).await.unwrap();
        assert!(manager.has_harness("temp").await);

        manager.unregister_harness("temp").await.unwrap();
        assert!(!manager.has_harness("temp").await);
    }

    #[tokio::test]
    async fn test_unregister_preset_fails() {
        let manager = AcpHarnessManager::new();
        let result = manager.unregister_harness("claude_code").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_harness() {
        let manager = AcpHarnessManager::new();
        let updated = AcpHarnessEntry {
            display_name: "Claude Code Updated".to_string(),
            executable: Some("/new/path/claude".to_string()),
            enabled: true,
            preset: Some("claude_code".to_string()),
            ..Default::default()
        };

        manager.update_harness("claude_code", updated).await.unwrap();

        let config = manager.get_config("claude_code").await.unwrap();
        assert_eq!(config.executable, Some("/new/path/claude".to_string()));
    }

    #[tokio::test]
    async fn test_update_harness_disable() {
        let manager = AcpHarnessManager::new();
        assert!(manager.has_harness("codex").await);

        let disabled = AcpHarnessEntry {
            display_name: "Codex".to_string(),
            enabled: false,
            preset: Some("codex".to_string()),
            ..Default::default()
        };

        manager.update_harness("codex", disabled).await.unwrap();
        assert!(!manager.has_harness("codex").await);
    }

    #[tokio::test]
    async fn test_list_configs() {
        let manager = AcpHarnessManager::new();
        let configs = manager.list_configs().await;
        assert_eq!(configs.len(), 3);
        // Should be sorted
        assert_eq!(configs[0].0, "claude_code");
        assert_eq!(configs[1].0, "codex");
        assert_eq!(configs[2].0, "gemini");
    }

    #[tokio::test]
    async fn test_get_config() {
        let manager = AcpHarnessManager::new();
        let config = manager.get_config("claude_code").await;
        assert!(config.is_some());
        assert_eq!(config.unwrap().display_name, "Claude Code");

        let config = manager.get_config("nonexistent").await;
        assert!(config.is_none());
    }
}
