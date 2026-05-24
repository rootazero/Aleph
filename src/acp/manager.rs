//! AcpAdapterManager — lifecycle management for ACP harness sessions.
//!
//! Supports runtime dynamic harness registration and unregistration.

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::adapters::{CustomAcpAdapter, GenericAcpAdapter};
use crate::acp::protocol::{AcpErrorCode, AcpOperationError};
use crate::acp::session::{AcpSession, CancelHandle};
use crate::acp::AcpChunkCallback;
use crate::config::types::acp::AcpAdapterEntry;
use crate::error::Result;
use crate::sync_primitives::Arc;
use tokio::sync::Mutex as AsyncMutex;

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
            let store_clone = {
                let mut store = sessions_ref.lock().unwrap_or_else(|e| e.into_inner());
                match event {
                    super::AcpSessionEvent::Created {
                        ref harness_id,
                        ref acp_session_id,
                        ref cwd,
                        ref session_name,
                    } => {
                        // Match the full triple so a `backend` name doesn't
                        // displace the unnamed/default entry under the same cwd.
                        store.retain(|s| {
                            !(s.harness_id == *harness_id
                                && s.cwd == *cwd
                                && s.session_name == *session_name)
                        });
                        store.push(crate::acp::session::PersistedAcpSession {
                            harness_id: harness_id.clone(),
                            acp_session_id: acp_session_id.clone(),
                            cwd: cwd.clone(),
                            created_at: chrono::Utc::now(),
                            last_used_at: chrono::Utc::now(),
                            session_name: session_name.clone(),
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
                        ref session_name,
                    } => {
                        store.retain(|s| {
                            !(s.harness_id == *harness_id
                                && s.cwd == *cwd
                                && s.session_name == *session_name)
                        });
                    }
                }
                store.clone()
            };
            save_persisted_sessions(&store_clone);
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
///
/// Includes an optional `name` so callers can run multiple parallel
/// sessions in the same repo (mirrors acpx's `-s backend` / `-s frontend`).
/// The empty string is the canonical "default" name and is the legacy
/// shape — old callers via `SessionKey::new` see no behavioral change.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct SessionKey {
    harness_id: String,
    cwd: PathBuf,
    name: String,
}

impl SessionKey {
    /// Default (unnamed) session for the given harness + cwd.
    pub fn new(harness_id: &str, cwd: &str) -> Self {
        Self::with_name(harness_id, cwd, None)
    }

    /// Named session — `None` is equivalent to `new()` (empty name).
    pub fn with_name(harness_id: &str, cwd: &str, name: Option<&str>) -> Self {
        Self {
            harness_id: harness_id.to_string(),
            cwd: canonicalize_cwd(cwd),
            name: name.unwrap_or("").to_string(),
        }
    }

    pub fn harness_id(&self) -> &str {
        &self.harness_id
    }

    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }

    /// Empty string for the default/unnamed session.
    pub fn name(&self) -> &str {
        &self.name
    }
}

fn canonicalize_cwd(cwd: &str) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| {
        let path = PathBuf::from(cwd);
        let normalized = normalize_path(&path);
        let final_path = if normalized.is_absolute() {
            normalized
        } else {
            std::env::current_dir()
                .map(|cd| cd.join(&normalized))
                .unwrap_or(normalized)
        };
        debug!(
            cwd,
            ?final_path,
            "SessionKey: canonicalize failed, using normalized path"
        );
        final_path
    })
}

fn normalize_path(path: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => result.push(prefix.as_os_str()),
            Component::RootDir => result.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if result.parent().is_some() {
                    result.pop();
                } else {
                    result.push("..");
                }
            }
            Component::Normal(name) => result.push(name),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
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
/// Pooled session slot.
///
/// `session` is an `Arc<AsyncMutex<AcpSession>>` so concurrent prompts to the
/// same (harness, cwd) serialize via the inner mutex instead of racing
/// remove/re-insert on the map. Different keys still progress in parallel.
///
/// `cancel` is a cloned handle that writes `session/cancel` directly to the
/// child's stdin — it does NOT acquire `session`, so it can interrupt an
/// in-flight prompt that currently owns the inner mutex.
#[derive(Clone)]
pub struct SessionEntry {
    pub session: Arc<AsyncMutex<AcpSession>>,
    pub cancel: CancelHandle,
}

impl SessionEntry {
    fn new(session: AcpSession) -> Self {
        let cancel = session.cancel_handle();
        Self {
            session: Arc::new(AsyncMutex::new(session)),
            cancel,
        }
    }
}

/// All harness and config state is behind `RwLock` for runtime dynamic management.
pub struct AcpAdapterManager {
    adapters: RwLock<HashMap<String, Arc<dyn AcpAdapter>>>,
    configs: RwLock<HashMap<String, AcpAdapterEntry>>,
    /// Active sessions for NativeAcp harnesses, keyed by (harness_id, cwd).
    ///
    /// Outer `RwLock` guards the map shape (insert/remove). Each entry's
    /// inner `AsyncMutex` serializes operations on that specific session.
    sessions: RwLock<HashMap<SessionKey, SessionEntry>>,
    /// Optional persistence callback for session state changes.
    persistence_hook: RwLock<Option<super::PersistenceHook>>,
    /// Optional broadcast hook so the gateway can push
    /// `acp.sessions.changed` whenever the pool mutates. Kept separate from
    /// `persistence_hook` so disk-persistence and live-broadcast wire up
    /// independently (one consumer can exist without the other).
    gateway_change_hook: RwLock<Option<super::GatewayChangeHook>>,
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
            gateway_change_hook: RwLock::new(None),
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
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessDenied,
                format!("Cannot register disabled harness '{}'", id),
            )
            .into());
        }

        let harness = Self::build_harness(&id, &entry);

        // For a new harness, we only need harnesses + configs (no sessions to kill).
        // This is safe: register only inserts into harnesses/configs, and no other
        // code path holds adapters.write while waiting on configs.write.
        let mut adapters = self.adapters.write().await;
        let mut configs = self.configs.write().await;

        if adapters.contains_key(&id) {
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessDenied,
                format!(
                    "Harness '{}' is already registered. Use update_harness to modify it.",
                    id
                ),
            )
            .into());
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
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessDenied,
                format!(
                    "Cannot unregister preset harness '{}'. Disable it via update_harness instead.",
                    id
                ),
            )
            .into());
        }

        // Acquire locks in consistent order: sessions → harnesses → configs
        let mut sessions = self.sessions.write().await;
        let mut adapters = self.adapters.write().await;
        let mut configs = self.configs.write().await;

        if adapters.remove(id).is_none() {
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessNotFound,
                format!("Harness '{}' is not registered", id),
            )
            .into());
        }

        configs.remove(id);

        // Also kill any active sessions for this harness (all cwds)
        let keys_to_remove: Vec<SessionKey> = sessions
            .keys()
            .filter(|k| k.harness_id == id)
            .cloned()
            .collect();
        for key in keys_to_remove {
            if let Some(entry) = sessions.remove(&key) {
                let mut session = entry.session.lock().await;
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
        let kill_sessions = |sessions: &mut HashMap<SessionKey, SessionEntry>, harness_id: &str| {
            let keys_to_remove: Vec<SessionKey> = sessions
                .keys()
                .filter(|k| k.harness_id == harness_id)
                .cloned()
                .collect();
            let mut removed = Vec::new();
            for key in keys_to_remove {
                if let Some(entry) = sessions.remove(&key) {
                    removed.push(entry);
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
            for entry in removed {
                let mut session = entry.session.lock().await;
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
        for entry in removed {
            let mut session = entry.session.lock().await;
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

    /// Set the gateway broadcast hook fired on every pool mutation. Used to
    /// publish `acp.sessions.changed` so panels can re-fetch live.
    pub async fn set_gateway_change_hook(&self, hook: super::GatewayChangeHook) {
        let mut h = self.gateway_change_hook.write().await;
        *h = Some(hook);
    }

    /// Emit a persistence event AND fan out to the gateway broadcast hook.
    /// Both consumers are independent — either may be unset.
    async fn emit_persistence_event(&self, event: super::AcpSessionEvent) {
        // Notify the gateway first; it's payload-free so re-fetches don't
        // race the disk write. The gateway hook is cheap (broadcast send).
        let notify = {
            let gh = self.gateway_change_hook.read().await;
            gh.clone()
        };
        if let Some(n) = notify {
            n();
        }
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
            let key =
                SessionKey::with_name(&entry.harness_id, &entry.cwd, entry.session_name.as_deref());

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
            self.sessions
                .write()
                .await
                .insert(key, SessionEntry::new(session));
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
        self.ensure_session_named(harness_id, cwd, None).await
    }

    /// Like `ensure_session`, but with an explicit session name so callers
    /// can keep multiple parallel sessions in the same repo
    /// (mirrors acpx's `-s backend` / `-s frontend`). `None` is the
    /// default/unnamed session.
    pub async fn ensure_session_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let _ = self
            .acquire_live_entry(harness_id, cwd, session_name)
            .await?;
        Ok(())
    }

    /// Look up or spawn a live `SessionEntry` for the given harness + cwd
    /// + optional session name.
    ///
    /// Race-safe: if two tasks call simultaneously, one wins the spawn and
    /// the other's spawned session is killed. A dead/errored entry is
    /// evicted and replaced with a fresh `SessionEntry` (new Arc + new
    /// cancel handle), so callers always get a live session.
    ///
    /// Returned entry's inner session mutex is NOT held — caller may lock it.
    async fn acquire_live_entry(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<SessionEntry> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);

        // Fast path: existing entry whose process is still alive.
        let existing = self.sessions.read().await.get(&key).cloned();
        if let Some(entry) = existing {
            let is_live = {
                let mut s = entry.session.lock().await;
                s.is_alive() && s.state() != crate::acp::protocol::AcpSessionState::Error
            };
            if is_live {
                return Ok(entry);
            }
            // Dead — evict, then fall through to spawn a replacement.
            warn!(
                harness_id,
                "ACP session died or entered error state, respawning"
            );
            self.emit_persistence_event(super::AcpSessionEvent::Removed {
                harness_id: harness_id.to_string(),
                cwd: cwd.to_string(),
                session_name: session_name.map(str::to_string),
            })
            .await;
            self.sessions.write().await.remove(&key);
        }

        // Slow path: spawn outside any lock, then double-check.
        let harness = {
            let adapters = self.adapters.read().await;
            Arc::clone(adapters.get(harness_id).ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::HarnessNotFound,
                    format!("Unknown ACP harness: '{}'", harness_id),
                )
            })?)
        };
        let mut new_session = harness.spawn_session(Some(cwd)).await?;

        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get(&key).cloned() {
            // Race lost — drop our extra session.
            debug!(
                harness_id,
                "ACP session race: another task spawned first, dropping ours"
            );
            new_session.kill().await;
            return Ok(existing);
        }

        info!(harness_id, "ACP session started");
        let entry = SessionEntry::new(new_session);
        sessions.insert(key, entry.clone());
        Ok(entry)
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
        self.prompt_named(
            harness_id,
            prompt_text,
            cwd,
            None,
            mode,
            reuse_session,
            on_chunk,
        )
        .await
    }

    /// Like `prompt`, but with an explicit session name. `None` is the
    /// default/unnamed session and is equivalent to `prompt()`.
    #[allow(clippy::too_many_arguments)]
    pub async fn prompt_named(
        &self,
        harness_id: &str,
        prompt_text: &str,
        cwd: &str,
        session_name: Option<&str>,
        mode: Option<AdapterMode>,
        reuse_session: bool,
        on_chunk: Option<AcpChunkCallback>,
    ) -> Result<String> {
        // Resolve effective mode and validate
        let (effective_mode, timeout) = {
            let adapters = self.adapters.read().await;
            let harness = adapters.get(harness_id).ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::HarnessNotFound,
                    format!("Unknown ACP harness: '{}'", harness_id),
                )
            })?;

            let effective = mode.unwrap_or_else(|| harness.mode());

            // Validate mode is supported
            if !harness.supported_modes().contains(&effective) {
                return Err(AcpOperationError::new(
                    AcpErrorCode::ModeUnsupported,
                    format!(
                        "Harness '{}' does not support {:?} mode",
                        harness_id, effective
                    ),
                )
                .into());
            }

            let timeout = harness.build_config(Some(cwd)).timeout;
            (effective, timeout)
        };
        // harnesses read lock dropped here

        match effective_mode {
            AdapterMode::NativeAcp => {
                let key = SessionKey::with_name(harness_id, cwd, session_name);

                // If not reusing, evict any existing session first so the
                // next acquire_live_entry spawns a fresh one.
                if !reuse_session {
                    if let Some(old) = self.sessions.write().await.remove(&key) {
                        let mut s = old.session.lock().await;
                        s.kill().await;
                    }
                }

                let entry = self
                    .acquire_live_entry(harness_id, cwd, session_name)
                    .await?;

                // Lock the inner mutex — concurrent prompts to the same
                // (harness, cwd) queue here. The cancel handle (held outside
                // this lock by the manager) can still fire `session/cancel`.
                let mut session = entry.session.lock().await;

                let result = session
                    .prompt(prompt_text, cwd, timeout, on_chunk.as_ref())
                    .await;

                match result {
                    Ok((text, _notifications)) => {
                        if session.is_alive() {
                            if let Some(sid) = session.acp_session_id() {
                                // Idempotent Created: ensure_session can't fire
                                // it because session_id is None at spawn time.
                                self.emit_persistence_event(super::AcpSessionEvent::Created {
                                    harness_id: harness_id.to_string(),
                                    acp_session_id: sid,
                                    cwd: cwd.to_string(),
                                    session_name: session_name.map(str::to_string),
                                })
                                .await;
                            }
                        } else {
                            // Process died — evict the entry. Drop session
                            // lock first to avoid holding it across the write.
                            drop(session);
                            self.sessions.write().await.remove(&key);
                            self.emit_persistence_event(super::AcpSessionEvent::Removed {
                                harness_id: harness_id.to_string(),
                                cwd: cwd.to_string(),
                                session_name: session_name.map(str::to_string),
                            })
                            .await;
                            warn!(harness_id, "ACP session died after prompt, evicted");
                        }
                        Ok(text)
                    }
                    Err(e) => {
                        if session.is_alive() {
                            session.kill().await;
                        }
                        drop(session);
                        self.sessions.write().await.remove(&key);
                        self.emit_persistence_event(super::AcpSessionEvent::Removed {
                            harness_id: harness_id.to_string(),
                            cwd: cwd.to_string(),
                            session_name: session_name.map(str::to_string),
                        })
                        .await;
                        Err(e)
                    }
                }
            }
            AdapterMode::Oneshot => {
                // Clone Arc ref under brief read lock; execute_oneshot may take minutes.
                let harness = {
                    let adapters = self.adapters.read().await;
                    Arc::clone(adapters.get(harness_id).ok_or_else(|| {
                        AcpOperationError::new(
                            AcpErrorCode::HarnessNotFound,
                            format!("Unknown ACP harness: '{}'", harness_id),
                        )
                    })?)
                };
                harness.execute_oneshot(prompt_text, cwd).await
            }
        }
    }

    /// Cooperatively cancel the in-flight prompt on the specified harness + cwd.
    ///
    /// Uses the pre-cloned `CancelHandle` so the cancel notification can
    /// race ahead of an in-flight prompt that currently holds the
    /// per-session mutex. The agent will respond with a `stopReason`
    /// notification, the prompt resolves, and the next caller sees the
    /// session in `Idle` state again.
    ///
    /// Returns `SessionDead` if no entry exists for this harness/cwd.
    pub async fn cancel(&self, harness_id: &str, cwd: &str) -> Result<()> {
        self.cancel_named(harness_id, cwd, None).await
    }

    /// Like `cancel`, but targets a specific named session.
    pub async fn cancel_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);
        let entry = self
            .sessions
            .read()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::SessionDead,
                    format!(
                        "No active ACP session for '{}' in '{}'{}",
                        harness_id,
                        cwd,
                        session_name
                            .map(|n| format!(" (session '{n}')"))
                            .unwrap_or_default()
                    ),
                )
            })?;
        entry.cancel.send_cancel().await
    }

    /// Run a session-control RPC against the existing session for
    /// (harness, cwd). Returns `SessionDead` if the session is gone.
    /// Inner errors are normalized to `SessionControlUnsupported` when
    /// the adapter doesn't implement the method.
    pub async fn set_mode(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        mode_id: &str,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.set_mode(mode_id, timeout).await
    }

    pub async fn set_model(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        model_id: &str,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.set_model(model_id, timeout).await
    }

    pub async fn set_config_option(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.set_config_option(key, value, timeout).await
    }

    /// Authenticate the existing session. `credential` is opaque — typically
    /// pulled from env `ACP_AUTH_<METHOD_ID>` by the caller.
    pub async fn authenticate(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
        method_id: &str,
        credential: &str,
    ) -> Result<()> {
        let (entry, timeout) = self
            .entry_and_timeout(harness_id, cwd, session_name)
            .await?;
        let mut s = entry.session.lock().await;
        s.authenticate(method_id, credential, timeout).await
    }

    async fn entry_and_timeout(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<(SessionEntry, std::time::Duration)> {
        let timeout = {
            let adapters = self.adapters.read().await;
            let harness = adapters.get(harness_id).ok_or_else(|| {
                AcpOperationError::new(
                    AcpErrorCode::HarnessNotFound,
                    format!("Unknown ACP harness: '{}'", harness_id),
                )
            })?;
            harness.build_config(Some(cwd)).timeout
        };
        let entry = self.acquire_live_entry(harness_id, cwd, session_name).await?;
        Ok((entry, timeout))
    }

    /// Snapshot of every pooled session for diagnostics + panel display.
    ///
    /// Acquires a brief lock per entry to read state. Does NOT block a
    /// running prompt — uses `try_lock` and falls back to `Busy` when held.
    pub async fn list_sessions(&self) -> Vec<SessionSnapshot> {
        let entries: Vec<(SessionKey, SessionEntry)> = {
            let sessions = self.sessions.read().await;
            sessions
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let mut out = Vec::with_capacity(entries.len());
        for (key, entry) in entries {
            let (alive, state, sid) = match entry.session.try_lock() {
                Ok(mut s) => (s.is_alive(), s.state(), s.acp_session_id()),
                Err(_) => (
                    true,
                    crate::acp::protocol::AcpSessionState::Busy,
                    None,
                ),
            };
            out.push(SessionSnapshot {
                harness_id: key.harness_id.clone(),
                cwd: key.cwd.to_string_lossy().into_owned(),
                session_name: if key.name.is_empty() {
                    None
                } else {
                    Some(key.name.clone())
                },
                acp_session_id: sid,
                alive,
                state,
            });
        }
        out.sort_by(|a, b| {
            a.harness_id
                .cmp(&b.harness_id)
                .then(a.cwd.cmp(&b.cwd))
                .then(a.session_name.cmp(&b.session_name))
        });
        out
    }

    /// Shut down a single named session. Cancels any in-flight prompt,
    /// removes the entry from the pool, kills the subprocess, then emits
    /// `Removed` so panels and persistence stay in sync. Idempotent — no
    /// error when the session is already gone.
    pub async fn shutdown_named(
        &self,
        harness_id: &str,
        cwd: &str,
        session_name: Option<&str>,
    ) -> Result<()> {
        let key = SessionKey::with_name(harness_id, cwd, session_name);

        // Fire cancel first so the agent gets a chance to flush partial
        // output before we evict. Best-effort — a missing session is fine.
        let cancel_handle = self
            .sessions
            .read()
            .await
            .get(&key)
            .map(|entry| entry.cancel.clone());
        if let Some(handle) = cancel_handle {
            let _ = handle.send_cancel().await;
        }

        // Now evict + kill. Drop the write lock before locking the inner
        // session mutex to avoid holding both at once.
        let entry = self.sessions.write().await.remove(&key);
        if let Some(entry) = entry {
            let mut s = entry.session.lock().await;
            s.kill().await;
            drop(s);
            self.emit_persistence_event(super::AcpSessionEvent::Removed {
                harness_id: harness_id.to_string(),
                cwd: cwd.to_string(),
                session_name: session_name.map(str::to_string),
            })
            .await;
        }
        Ok(())
    }

    /// Kill all active sessions.
    pub async fn shutdown_all(&self) {
        let entries: Vec<(SessionKey, SessionEntry)> = {
            let mut sessions = self.sessions.write().await;
            let drained = sessions
                .drain()
                .collect::<Vec<_>>();
            drained
        };
        for (key, entry) in entries {
            info!(harness_id = %key.harness_id, cwd = ?key.cwd, "Shutting down ACP session");
            let mut session = entry.session.lock().await;
            session.kill().await;
        }
    }
}

/// Lightweight view of a pooled ACP session — used by gateway RPC + panel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSnapshot {
    pub harness_id: String,
    pub cwd: String,
    /// `None` for the default unnamed session.
    pub session_name: Option<String>,
    pub acp_session_id: Option<String>,
    pub alive: bool,
    pub state: crate::acp::protocol::AcpSessionState,
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

    #[test]
    fn test_normalize_path_removes_dot() {
        let path = std::path::Path::new("/tmp/./foo");
        let normalized = normalize_path(path);
        assert_eq!(normalized, std::path::PathBuf::from("/tmp/foo"));
    }

    #[test]
    fn test_normalize_path_resolves_dotdot() {
        let path = std::path::Path::new("/tmp/foo/../bar");
        let normalized = normalize_path(path);
        assert_eq!(normalized, std::path::PathBuf::from("/tmp/bar"));
    }

    #[test]
    fn test_normalize_path_preserves_relative() {
        let path = std::path::Path::new("foo/../bar");
        let normalized = normalize_path(path);
        assert_eq!(normalized, std::path::PathBuf::from("bar"));
    }

    #[test]
    fn test_normalize_path_empty_becomes_dot() {
        let path = std::path::Path::new(".");
        let normalized = normalize_path(path);
        assert_eq!(normalized, std::path::PathBuf::from("."));
    }

    // ── Phase 1.5: named sessions ────────────────────────────────────────

    #[test]
    fn test_session_key_default_name_matches_legacy() {
        let legacy = SessionKey::new("claude-code", "/tmp");
        let named = SessionKey::with_name("claude-code", "/tmp", None);
        assert_eq!(legacy, named, "None name must equal legacy new()");
        assert_eq!(legacy.name(), "");
    }

    #[test]
    fn test_session_key_distinguishes_names_same_cwd() {
        let backend = SessionKey::with_name("claude-code", "/tmp", Some("backend"));
        let frontend = SessionKey::with_name("claude-code", "/tmp", Some("frontend"));
        let default = SessionKey::with_name("claude-code", "/tmp", None);
        assert_ne!(backend, frontend, "different names → different keys");
        assert_ne!(backend, default, "named != unnamed");
        assert_eq!(backend.name(), "backend");
        assert_eq!(frontend.name(), "frontend");
    }

    #[test]
    fn test_session_key_accessors() {
        let key = SessionKey::with_name("claude-code", "/tmp", Some("alpha"));
        assert_eq!(key.harness_id(), "claude-code");
        assert_eq!(key.name(), "alpha");
        assert!(key.cwd().to_string_lossy().contains("tmp"));
    }

    // ── Phase 2: SessionSnapshot for panel ───────────────────────────────

    #[tokio::test]
    async fn test_list_sessions_empty_pool() {
        let manager = AcpAdapterManager::new();
        let snaps = manager.list_sessions().await;
        assert!(snaps.is_empty(), "no sessions yet → empty snapshot list");
    }

    // ── Phase 3 follow-ups: persistence + gateway broadcast ──────────────

    /// `PersistedAcpSession` deserializes legacy snapshots that lack the
    /// `session_name` field (treats them as the default unnamed session).
    #[test]
    fn test_persisted_session_legacy_compat() {
        let legacy = r#"{
            "harness_id": "claude-code",
            "acp_session_id": "abc123",
            "cwd": "/tmp/repo",
            "created_at": "2026-05-24T00:00:00Z",
            "last_used_at": "2026-05-24T00:00:00Z"
        }"#;
        let parsed: crate::acp::session::PersistedAcpSession =
            serde_json::from_str(legacy).expect("legacy snapshot must parse");
        assert_eq!(parsed.session_name, None);
    }

    /// `set_gateway_change_hook` fires every time `emit_persistence_event`
    /// fans out — confirms the broadcast wiring used by `acp.sessions.changed`.
    #[tokio::test]
    async fn test_gateway_change_hook_fires_on_emit() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let manager = AcpAdapterManager::new();
        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        manager
            .set_gateway_change_hook(Arc::new(move || {
                c2.fetch_add(1, Ordering::SeqCst);
            }))
            .await;

        // Two direct emits — fan-out should fire the hook each time even
        // without a persistence_hook installed.
        manager
            .emit_persistence_event(crate::acp::AcpSessionEvent::Created {
                harness_id: "claude-code".to_string(),
                acp_session_id: "s1".to_string(),
                cwd: "/tmp/a".to_string(),
                session_name: None,
            })
            .await;
        manager
            .emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                harness_id: "claude-code".to_string(),
                cwd: "/tmp/a".to_string(),
                session_name: Some("backend".to_string()),
            })
            .await;
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
