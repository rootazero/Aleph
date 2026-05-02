//! AcpAdapterManager — lifecycle management for ACP harness sessions.
//!
//! Supports runtime dynamic harness registration and unregistration.

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::adapters::{CustomAcpAdapter, GenericAcpAdapter};
use crate::acp::session::AcpSession;
use crate::acp::AcpChunkCallback;
use crate::config::types::acp::AcpAdapterEntry;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;

// =============================================================================
// Session persistence file I/O
// =============================================================================

/// Default persistence file path for ACP sessions.
fn acp_sessions_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".aleph")
        .join("data")
        .join("acp_sessions.json")
}

/// Load persisted ACP sessions from disk (best-effort).
pub fn load_persisted_sessions() -> Vec<crate::acp::session::PersistedAcpSession> {
    let path = acp_sessions_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            warn!("Failed to parse ACP sessions file: {}", e);
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Save persisted ACP sessions to disk (atomic write).
pub fn save_persisted_sessions(sessions: &[crate::acp::session::PersistedAcpSession]) {
    let path = acp_sessions_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(sessions) {
        Ok(json) => {
            if let Err(e) = crate::utils::atomic_io::write_atomic(&path, json.as_bytes()) {
                warn!("Failed to atomic-write ACP sessions: {}", e);
            }
        }
        Err(e) => warn!("Failed to serialize ACP sessions: {}", e),
    }
}

/// Wire up file-based persistence for ACP sessions.
/// Call this after creating the AcpAdapterManager at startup.
pub async fn wire_persistence(manager: &AcpAdapterManager) {
    use crate::sync_primitives::{Arc, Mutex};

    let sessions = Arc::new(Mutex::new(load_persisted_sessions()));

    let sessions_ref = Arc::clone(&sessions);
    manager
        .set_persistence_hook(Arc::new(move |event: super::AcpSessionEvent| {
            let mut store = sessions_ref.lock().unwrap_or_else(|e| e.into_inner());
            match event {
                super::AcpSessionEvent::Created {
                    ref harness_id,
                    ref acp_session_id,
                    ref cwd,
                } => {
                    store.retain(|s| !(s.harness_id == *harness_id && s.cwd == *cwd));
                    store.push(crate::acp::session::PersistedAcpSession {
                        harness_id: harness_id.clone(),
                        acp_session_id: acp_session_id.clone(),
                        cwd: cwd.clone(),
                        created_at: chrono::Utc::now(),
                        last_used_at: chrono::Utc::now(),
                    });
                }
                super::AcpSessionEvent::Updated {
                    ref harness_id,
                    ref acp_session_id,
                } => {
                    if let Some(entry) = store.iter_mut().find(|s| {
                        s.harness_id == *harness_id && s.acp_session_id == *acp_session_id
                    }) {
                        entry.last_used_at = chrono::Utc::now();
                    }
                }
                super::AcpSessionEvent::Removed {
                    ref harness_id,
                    ref cwd,
                } => {
                    store.retain(|s| !(s.harness_id == *harness_id && s.cwd == *cwd));
                }
            }
            save_persisted_sessions(&store);
        }))
        .await;

    // Restore existing sessions
    let persisted = sessions.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !persisted.is_empty() {
        let restored = manager.restore_sessions(persisted).await;
        if !restored.is_empty() {
            info!(count = restored.len(), "Restored ACP sessions from disk");
        }
    }
}

// =============================================================================
// SessionKey
// =============================================================================

/// Canonicalized session pool key — prevents duplicate sessions for equivalent paths.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct SessionKey {
    harness_id: String,
    cwd: PathBuf,
}

impl SessionKey {
    pub fn new(harness_id: &str, cwd: &str) -> Self {
        Self {
            harness_id: harness_id.to_string(),
            cwd: std::fs::canonicalize(cwd).unwrap_or_else(|_| {
                debug!(cwd, "SessionKey: canonicalize failed, using raw path");
                PathBuf::from(cwd)
            }),
        }
    }
}

// =============================================================================
// AcpAdapterManager
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
pub struct AcpAdapterManager {
    adapters: RwLock<HashMap<String, Arc<dyn AcpAdapter>>>,
    configs: RwLock<HashMap<String, AcpAdapterEntry>>,
    /// Active sessions for NativeAcp harnesses, keyed by (harness_id, cwd).
    sessions: RwLock<HashMap<SessionKey, AcpSession>>,
    /// Optional persistence callback for session state changes.
    persistence_hook: RwLock<Option<super::PersistenceHook>>,
}

impl Default for AcpAdapterManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpAdapterManager {
    /// Create a manager with all default harnesses enabled.
    pub fn new() -> Self {
        let entries: HashMap<String, AcpAdapterEntry> =
            AcpAdapterEntry::all_presets().into_iter().collect();
        Self::from_entries(entries)
    }

    /// Create a manager from a map of harness entries.
    ///
    /// For each entry where `enabled == true`:
    /// - If `entry.preset` matches a known preset, uses the dedicated harness impl
    ///   with `entry.executable` as override.
    /// - Otherwise, uses `CustomAcpAdapter`.
    pub fn from_entries(entries: HashMap<String, AcpAdapterEntry>) -> Self {
        let mut adapters: HashMap<String, Arc<dyn AcpAdapter>> = HashMap::new();
        let mut configs: HashMap<String, AcpAdapterEntry> = HashMap::new();

        for (id, entry) in entries {
            if !entry.enabled {
                continue;
            }
            let harness = Self::build_harness(&id, &entry);
            adapters.insert(id.clone(), harness);
            configs.insert(id, entry);
        }

        Self {
            adapters: RwLock::new(adapters),
            configs: RwLock::new(configs),
            sessions: RwLock::new(HashMap::new()),
            persistence_hook: RwLock::new(None),
        }
    }

    /// Factory: build the right harness implementation from an entry.
    fn build_harness(id: &str, entry: &AcpAdapterEntry) -> Arc<dyn AcpAdapter> {
        if entry.preset.is_some() {
            // All preset harnesses use the generic configuration-driven adapter
            Arc::new(GenericAcpAdapter::from_entry(entry))
        } else {
            // Custom or unknown preset — use CustomAcpAdapter
            Arc::new(CustomAcpAdapter::new(id.to_string(), entry.clone()))
        }
    }

    // =========================================================================
    // Query methods (all async due to RwLock)
    // =========================================================================

    /// List registered harness IDs.
    pub async fn harness_ids(&self) -> Vec<String> {
        let adapters = self.adapters.read().await;
        let mut ids: Vec<String> = adapters.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Check whether a harness with the given ID is registered.
    pub async fn has_harness(&self, id: &str) -> bool {
        let adapters = self.adapters.read().await;
        adapters.contains_key(id)
    }

    /// Get the display name for a registered harness.
    pub async fn display_name(&self, id: &str) -> Option<String> {
        let adapters = self.adapters.read().await;
        adapters.get(id).map(|h| h.display_name().to_string())
    }

    /// Get the execution mode for a registered harness.
    pub async fn harness_mode(&self, id: &str) -> Option<AdapterMode> {
        let adapters = self.adapters.read().await;
        adapters.get(id).map(|h| h.mode())
    }

    /// Return IDs of harnesses whose executables are available on this system.
    pub async fn available_harnesses(&self) -> Vec<String> {
        // Clone Arc refs under brief read lock, then check availability without holding the lock.
        // Each is_available() can take up to 5s — holding the lock would block all writers.
        let snapshot: Vec<(String, Arc<dyn AcpAdapter>)> = {
            let adapters = self.adapters.read().await;
            adapters
                .iter()
                .map(|(id, h)| (id.clone(), Arc::clone(h)))
                .collect()
        };

        let mut available = Vec::new();
        for (id, adapter) in snapshot {
            if adapter.is_available().await {
                available.push(id);
            }
        }
        available.sort();
        available
    }

    /// Check availability of a single harness by ID.
    pub async fn is_harness_available(&self, id: &str) -> bool {
        // Clone Arc ref under brief read lock, then check without holding the lock.
        let harness = {
            let adapters = self.adapters.read().await;
            adapters.get(id).map(Arc::clone)
        };
        match harness {
            Some(h) => h.is_available().await,
            None => false,
        }
    }

    // =========================================================================
    // Dynamic management
    // =========================================================================

    /// Register a new harness at runtime.
    ///
    /// Lock ordering: harnesses → configs
    /// (no sessions lock needed — new harness has no sessions yet)
    pub async fn register_harness(&self, id: String, entry: AcpAdapterEntry) -> Result<()> {
        if !entry.enabled {
            return Err(AlephError::tool(format!(
                "Cannot register disabled harness '{}'",
                id
            )));
        }

        let harness = Self::build_harness(&id, &entry);

        // For a new harness, we only need harnesses + configs (no sessions to kill).
        // This is safe: register only inserts into harnesses/configs, and no other
        // code path holds adapters.write while waiting on configs.write.
        let mut adapters = self.adapters.write().await;
        let mut configs = self.configs.write().await;

        if adapters.contains_key(&id) {
            return Err(AlephError::tool(format!(
                "Harness '{}' is already registered. Use update_harness to modify it.",
                id
            )));
        }

        info!(harness_id = %id, "Registering new ACP harness");
        adapters.insert(id.clone(), harness);
        configs.insert(id, entry);
        Ok(())
    }

    /// Unregister a harness at runtime.
    ///
    /// Rejects preset harness IDs — use `update_harness` to disable them instead.
    ///
    /// Lock ordering: sessions → harnesses → configs
    /// (matches `update_harness` and `ensure_session` to prevent deadlocks)
    pub async fn unregister_harness(&self, id: &str) -> Result<()> {
        if AcpAdapterEntry::is_preset_id(id) {
            return Err(AlephError::tool(format!(
                "Cannot unregister preset harness '{}'. Disable it via update_harness instead.",
                id
            )));
        }

        // Acquire locks in consistent order: sessions → harnesses → configs
        let mut sessions = self.sessions.write().await;
        let mut adapters = self.adapters.write().await;
        let mut configs = self.configs.write().await;

        if adapters.remove(id).is_none() {
            return Err(AlephError::tool(format!(
                "Harness '{}' is not registered",
                id
            )));
        }

        configs.remove(id);

        // Also kill any active sessions for this harness (all cwds)
        let keys_to_remove: Vec<SessionKey> = sessions
            .keys()
            .filter(|k| k.harness_id == id)
            .cloned()
            .collect();
        for key in keys_to_remove {
            if let Some(mut session) = sessions.remove(&key) {
                session.kill().await;
            }
        }

        info!(harness_id = %id, "Unregistered ACP harness");
        Ok(())
    }

    /// Update an existing harness configuration.
    ///
    /// Replaces the harness instance and config. Kills any active session.
    ///
    /// Lock ordering: sessions → harnesses → configs
    /// (matches `ensure_session` which acquires sessions first, then harnesses)
    pub async fn update_harness(&self, id: &str, entry: AcpAdapterEntry) -> Result<()> {
        // Acquire locks in consistent order: sessions → harnesses → configs
        let mut sessions = self.sessions.write().await;
        let mut adapters = self.adapters.write().await;
        let mut configs = self.configs.write().await;

        // Helper: kill all sessions for this harness_id (across all cwds)
        let kill_sessions = |sessions: &mut HashMap<SessionKey, AcpSession>, harness_id: &str| {
            let keys_to_remove: Vec<SessionKey> = sessions
                .keys()
                .filter(|k| k.harness_id == harness_id)
                .cloned()
                .collect();
            let mut removed = Vec::new();
            for key in keys_to_remove {
                if let Some(session) = sessions.remove(&key) {
                    removed.push(session);
                }
            }
            removed
        };

        if !entry.enabled {
            // Disable: remove harness but keep config
            adapters.remove(id);
            configs.insert(id.to_string(), entry);

            // Kill active sessions
            let removed = kill_sessions(&mut sessions, id);
            for mut session in removed {
                session.kill().await;
            }

            info!(harness_id = %id, "Disabled ACP harness");
            return Ok(());
        }

        let harness = Self::build_harness(id, &entry);
        adapters.insert(id.to_string(), harness);
        configs.insert(id.to_string(), entry);

        // Kill any active sessions so they will be respawned with the new config
        let removed = kill_sessions(&mut sessions, id);
        for mut session in removed {
            session.kill().await;
        }

        info!(harness_id = %id, "Updated ACP harness");
        Ok(())
    }

    /// Get the config entry for a specific harness.
    pub async fn get_config(&self, id: &str) -> Option<AcpAdapterEntry> {
        let configs = self.configs.read().await;
        configs.get(id).cloned()
    }

    /// List all config entries as (id, entry) pairs.
    pub async fn list_configs(&self) -> Vec<(String, AcpAdapterEntry)> {
        let configs = self.configs.read().await;
        let mut result: Vec<(String, AcpAdapterEntry)> = configs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    /// Set the persistence hook for session state changes.
    pub async fn set_persistence_hook(&self, hook: super::PersistenceHook) {
        let mut h = self.persistence_hook.write().await;
        *h = Some(hook);
    }

    /// Emit a persistence event (no-op if no hook set).
    async fn emit_persistence_event(&self, event: super::AcpSessionEvent) {
        let hook = self.persistence_hook.read().await;
        if let Some(ref h) = *hook {
            h(event);
        }
    }

    /// Restore sessions from persisted state. Returns list of successfully restored harness IDs.
    pub async fn restore_sessions(
        &self,
        persisted: Vec<crate::acp::session::PersistedAcpSession>,
    ) -> Vec<String> {
        let mut restored = Vec::new();
        for entry in persisted {
            let key = SessionKey::new(&entry.harness_id, &entry.cwd);

            // Clone Arc ref under brief read lock, then spawn without holding it.
            // spawn_session may take seconds (process startup + initialize handshake).
            let harness = {
                let adapters = self.adapters.read().await;
                match adapters.get(&entry.harness_id) {
                    Some(h) => Arc::clone(h),
                    None => {
                        warn!(harness_id = %entry.harness_id, "Harness not found, skipping restore");
                        continue;
                    }
                }
            };
            // read lock dropped — writers can proceed while we spawn

            let mut session = match harness.spawn_session(Some(&entry.cwd)).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(harness_id = %entry.harness_id, error = %e, "Failed to spawn for restore");
                    continue;
                }
            };

            let timeout = std::time::Duration::from_secs(30);
            if session
                .load_acp_session(&entry.acp_session_id, &entry.cwd, timeout)
                .await
                .is_err()
            {
                if let Err(e) = session.create_acp_session(&entry.cwd, timeout).await {
                    warn!(harness_id = %entry.harness_id, error = %e, "Failed to create new session on restore");
                    continue;
                }
            }

            info!(harness_id = %entry.harness_id, "Restored ACP session");
            self.sessions.write().await.insert(key, session);
            restored.push(entry.harness_id);
        }
        restored
    }

    // =========================================================================
    // Session management
    // =========================================================================

    /// Ensure a live ACP session exists for the given NativeAcp harness + cwd.
    ///
    /// - If a session exists and is alive, this is a no-op.
    /// - If a session exists but is dead, it is removed and respawned.
    /// - If no session exists, a new one is spawned.
    ///
    /// Only meaningful for NativeAcp harnesses; oneshot harnesses don't need sessions.
    ///
    /// Race safety: after spawning (which happens without holding the session lock),
    /// we re-check whether another task already inserted a live session for the same
    /// key. If so, we drop our freshly spawned session instead of overwriting.
    pub async fn ensure_session(&self, harness_id: &str, cwd: &str) -> Result<()> {
        let key = SessionKey::new(harness_id, cwd);

        // Check if we already have a live session
        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(&key) {
                if session.is_alive() {
                    return Ok(());
                }
                // Dead session — remove it
                warn!(harness_id, "ACP session died, respawning");
                self.emit_persistence_event(super::AcpSessionEvent::Removed {
                    harness_id: harness_id.to_string(),
                    cwd: cwd.to_string(),
                })
                .await;
                sessions.remove(&key);
            }
        }

        // Clone Arc ref under brief read lock, then spawn without holding it.
        // spawn_session may take seconds (process startup + initialize handshake).
        let harness = {
            let adapters = self.adapters.read().await;
            Arc::clone(adapters.get(harness_id).ok_or_else(|| {
                AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
            })?)
        };

        let mut new_session = harness.spawn_session(Some(cwd)).await?;

        // Re-check under write lock: another task may have raced us and already
        // inserted a live session for the same key while we were spawning.
        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get_mut(&key) {
            if existing.is_alive() {
                // Another task won the race — drop our session
                debug!(
                    harness_id,
                    "ACP session race: another task already spawned, dropping ours"
                );
                new_session.kill().await;
                return Ok(());
            }
            // Existing is dead — remove it
            sessions.remove(&key);
        }

        info!(harness_id, "ACP session started");
        // Note: Created persistence event is NOT emitted here because spawn_session
        // only calls initialize(), not create_acp_session(). The session_id is None.
        // The Created event fires in prompt() after create_acp_session() sets the ID.
        sessions.insert(key, new_session);
        Ok(())
    }

    /// Send a prompt to the specified harness, using the appropriate mode.
    ///
    /// - **NativeAcp**: Extracts session from pool (brief lock), uses it, re-inserts if alive.
    /// - **Oneshot**: Spawns a fresh process, waits for output.
    ///
    /// `mode`: Override the harness default mode. `None` uses `harness.mode()`.
    /// `reuse_session`: If `false`, kills any existing session and starts fresh.
    /// `on_chunk`: Streaming callback (wired in Task 7).
    pub async fn prompt(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
        mode: Option<AdapterMode>,
        reuse_session: bool,
        on_chunk: Option<AcpChunkCallback>,
    ) -> Result<String> {
        // Resolve effective mode and validate
        let (effective_mode, timeout) = {
            let adapters = self.adapters.read().await;
            let harness = adapters.get(harness_id).ok_or_else(|| {
                AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
            })?;

            let effective = mode.unwrap_or_else(|| harness.mode());

            // Validate mode is supported
            if !harness.supported_modes().contains(&effective) {
                return Err(AlephError::tool(format!(
                    "Harness '{}' does not support {:?} mode",
                    harness_id, effective
                )));
            }

            let timeout = harness.build_config(Some(cwd)).timeout;
            (effective, timeout)
        };
        // harnesses read lock dropped here

        match effective_mode {
            AdapterMode::NativeAcp => {
                let key = SessionKey::new(harness_id, cwd);

                // If not reusing, kill existing session first
                if !reuse_session {
                    let mut pool = self.sessions.write().await;
                    if let Some(mut session) = pool.remove(&key) {
                        session.kill().await;
                    }
                }

                // Ensure a live session exists
                self.ensure_session(harness_id, cwd).await?;

                // Extract session from pool (brief write lock)
                let mut session = {
                    let mut pool = self.sessions.write().await;
                    pool.remove(&key).ok_or_else(|| {
                        AlephError::tool(format!(
                            "ACP session for '{}' disappeared unexpectedly",
                            harness_id
                        ))
                    })?
                };
                // Write lock released — other harness calls can proceed

                let result = session
                    .prompt(prompt_text, cwd, timeout, on_chunk.as_ref())
                    .await;

                // Re-insert session if still alive
                if session.is_alive() {
                    if let Some(sid) = session.acp_session_id() {
                        // Use Created (idempotent: retain + push in hook) because
                        // ensure_session cannot emit Created (session_id is None at spawn time).
                        self.emit_persistence_event(super::AcpSessionEvent::Created {
                            harness_id: harness_id.to_string(),
                            acp_session_id: sid.to_string(),
                            cwd: cwd.to_string(),
                        })
                        .await;
                    }
                    self.sessions.write().await.insert(key, session);
                } else {
                    self.emit_persistence_event(super::AcpSessionEvent::Removed {
                        harness_id: harness_id.to_string(),
                        cwd: cwd.to_string(),
                    })
                    .await;
                    warn!(
                        harness_id,
                        "ACP session died after prompt, not re-inserting"
                    );
                }

                let (text, _notifications) = result?;
                Ok(text)
            }
            AdapterMode::Oneshot => {
                // Clone Arc ref under brief read lock; execute_oneshot may take minutes.
                let harness = {
                    let adapters = self.adapters.read().await;
                    Arc::clone(adapters.get(harness_id).ok_or_else(|| {
                        AlephError::tool(format!("Unknown ACP harness: '{}'", harness_id))
                    })?)
                };
                harness.execute_oneshot(prompt_text, cwd).await
            }
        }
    }

    /// Cancel the current operation on the specified harness + cwd.
    pub async fn cancel(&self, harness_id: &str, cwd: &str) -> Result<()> {
        let key = SessionKey::new(harness_id, cwd);
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(&key).ok_or_else(|| {
            AlephError::tool(format!(
                "No active ACP session for '{}' in '{}'",
                harness_id, cwd
            ))
        })?;
        session.cancel().await
    }

    /// Kill all active sessions.
    pub async fn shutdown_all(&self) {
        let mut sessions = self.sessions.write().await;
        for (key, session) in sessions.iter_mut() {
            info!(harness_id = %key.harness_id, cwd = ?key.cwd, "Shutting down ACP session");
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
        let manager = AcpAdapterManager::new();
        let ids = manager.harness_ids().await;
        assert!(ids.contains(&"claude-code".to_string()));
        assert!(ids.contains(&"codex".to_string()));
        assert!(ids.contains(&"gemini".to_string()));
    }

    #[tokio::test]
    async fn test_manager_has_harness() {
        let manager = AcpAdapterManager::new();
        assert!(manager.has_harness("claude-code").await);
        assert!(!manager.has_harness("unknown").await);
    }

    #[tokio::test]
    async fn test_manager_disable_harness() {
        let mut entries: HashMap<String, AcpAdapterEntry> =
            AcpAdapterEntry::all_presets().into_iter().collect();
        entries.get_mut("codex").unwrap().enabled = false;
        let manager = AcpAdapterManager::from_entries(entries);
        assert!(!manager.has_harness("codex").await);
        assert!(manager.has_harness("claude-code").await);
        assert!(manager.has_harness("gemini").await);
    }

    #[tokio::test]
    async fn test_manager_display_name() {
        let manager = AcpAdapterManager::new();
        assert_eq!(
            manager.display_name("claude-code").await,
            Some("Claude Code".to_string())
        );
        assert_eq!(
            manager.display_name("codex").await,
            Some("Codex".to_string())
        );
        assert_eq!(
            manager.display_name("gemini").await,
            Some("Gemini".to_string())
        );
        assert_eq!(manager.display_name("unknown").await, None);
    }

    #[tokio::test]
    async fn test_manager_harness_modes() {
        let manager = AcpAdapterManager::new();
        assert_eq!(
            manager.harness_mode("gemini").await,
            Some(AdapterMode::NativeAcp)
        );
        assert_eq!(
            manager.harness_mode("claude-code").await,
            Some(AdapterMode::Oneshot)
        );
        assert_eq!(
            manager.harness_mode("codex").await,
            Some(AdapterMode::Oneshot)
        );
    }

    #[tokio::test]
    async fn test_manager_executable_override() {
        let mut entries: HashMap<String, AcpAdapterEntry> =
            AcpAdapterEntry::all_presets().into_iter().collect();
        entries.get_mut("claude-code").unwrap().executable = Some("/custom/claude".to_string());
        let manager = AcpAdapterManager::from_entries(entries);
        assert!(manager.has_harness("claude-code").await);
        let adapters = manager.adapters.read().await;
        let harness = adapters.get("claude-code").unwrap();
        let cfg = harness.build_config(None);
        assert_eq!(cfg.executable, "/custom/claude");
    }

    #[tokio::test]
    async fn test_from_entries_with_custom_harness() {
        let mut entries = HashMap::new();
        entries.insert(
            "my-tool".to_string(),
            AcpAdapterEntry {
                display_name: "My Tool".to_string(),
                executable: Some("my-tool-bin".to_string()),
                enabled: true,
                ..Default::default()
            },
        );

        let manager = AcpAdapterManager::from_entries(entries);
        assert!(manager.has_harness("my-tool").await);
        assert_eq!(
            manager.display_name("my-tool").await,
            Some("My Tool".to_string())
        );
    }

    #[tokio::test]
    async fn test_from_entries_skips_disabled() {
        let mut entries = HashMap::new();
        entries.insert(
            "disabled-tool".to_string(),
            AcpAdapterEntry {
                display_name: "Disabled".to_string(),
                enabled: false,
                ..Default::default()
            },
        );

        let manager = AcpAdapterManager::from_entries(entries);
        assert!(!manager.has_harness("disabled-tool").await);
    }

    #[tokio::test]
    async fn test_register_harness() {
        let manager = AcpAdapterManager::from_entries(HashMap::new());
        let entry = AcpAdapterEntry {
            display_name: "New Tool".to_string(),
            executable: Some("new-tool".to_string()),
            enabled: true,
            ..Default::default()
        };

        manager
            .register_harness("new-tool".to_string(), entry)
            .await
            .unwrap();
        assert!(manager.has_harness("new-tool").await);
        assert_eq!(
            manager.display_name("new-tool").await,
            Some("New Tool".to_string())
        );
    }

    #[tokio::test]
    async fn test_register_duplicate_fails() {
        let manager = AcpAdapterManager::new();
        let entry = AcpAdapterEntry {
            display_name: "Dup".to_string(),
            enabled: true,
            preset: Some("claude-code".to_string()),
            ..Default::default()
        };

        let result = manager
            .register_harness("claude-code".to_string(), entry)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unregister_custom_harness() {
        let manager = AcpAdapterManager::from_entries(HashMap::new());
        let entry = AcpAdapterEntry {
            display_name: "Temp".to_string(),
            executable: Some("temp".to_string()),
            enabled: true,
            ..Default::default()
        };

        manager
            .register_harness("temp".to_string(), entry)
            .await
            .unwrap();
        assert!(manager.has_harness("temp").await);

        manager.unregister_harness("temp").await.unwrap();
        assert!(!manager.has_harness("temp").await);
    }

    #[tokio::test]
    async fn test_unregister_preset_fails() {
        let manager = AcpAdapterManager::new();
        let result = manager.unregister_harness("claude-code").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_harness() {
        let manager = AcpAdapterManager::new();
        let updated = AcpAdapterEntry {
            display_name: "Claude Code Updated".to_string(),
            executable: Some("/new/path/claude".to_string()),
            enabled: true,
            preset: Some("claude-code".to_string()),
            ..Default::default()
        };

        manager
            .update_harness("claude-code", updated)
            .await
            .unwrap();

        let config = manager.get_config("claude-code").await.unwrap();
        assert_eq!(config.executable, Some("/new/path/claude".to_string()));
    }

    #[tokio::test]
    async fn test_update_harness_disable() {
        let manager = AcpAdapterManager::new();
        assert!(manager.has_harness("codex").await);

        let disabled = AcpAdapterEntry {
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
        let manager = AcpAdapterManager::new();
        let configs = manager.list_configs().await;
        let preset_ids = crate::config::types::acp::AcpAdapterEntry::preset_ids();
        assert_eq!(configs.len(), preset_ids.len());
        // Should be sorted
        for i in 1..configs.len() {
            assert!(
                configs[i - 1].0 <= configs[i].0,
                "configs should be sorted by id"
            );
        }
        for id in preset_ids {
            assert!(
                configs.iter().any(|(k, _)| k == id),
                "missing preset config: {}",
                id
            );
        }
    }

    #[tokio::test]
    async fn test_get_config() {
        let manager = AcpAdapterManager::new();
        let config = manager.get_config("claude-code").await;
        assert!(config.is_some());
        assert_eq!(config.unwrap().display_name, "Claude Code");

        let config = manager.get_config("nonexistent").await;
        assert!(config.is_none());
    }

    #[test]
    fn test_acp_sessions_path() {
        let path = acp_sessions_path();
        assert!(path.to_string_lossy().contains("acp_sessions.json"));
    }

    #[test]
    fn test_session_key_canonicalization() {
        let k1 = SessionKey::new("claude-code", "/tmp");
        let k2 = SessionKey::new("claude-code", "/tmp/");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_session_key_different_cwd() {
        let k1 = SessionKey::new("claude-code", "/tmp");
        let k2 = SessionKey::new("claude-code", "/var");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_session_key_different_harness() {
        let k1 = SessionKey::new("claude-code", "/tmp");
        let k2 = SessionKey::new("codex", "/tmp");
        assert_ne!(k1, k2);
    }
}
