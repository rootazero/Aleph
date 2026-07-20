//! Harness registration / CRUD / config + session restore + snapshots.

use std::collections::HashMap;

use tokio::sync::RwLock;
use tracing::{info, warn};

use super::{AcpAdapterManager, SessionEntry, SessionKey, SessionSnapshot};
use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::adapters::{CustomAcpAdapter, GenericAcpAdapter};
use crate::acp::protocol::{AcpErrorCode, AcpOperationError};
use crate::config::types::acp::AcpAdapterEntry;
use crate::error::Result;
use crate::sync_primitives::Arc;

impl AcpAdapterManager {
    /// Create a manager with all default harnesses enabled.
    #[must_use]
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
    #[must_use]
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
    pub(super) fn build_harness(id: &str, entry: &AcpAdapterEntry) -> Arc<dyn AcpAdapter> {
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
                format!("Cannot register disabled harness '{id}'"),
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
                format!("Harness '{id}' is already registered. Use update_harness to modify it."),
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
                    "Cannot unregister preset harness '{id}'. Disable it via update_harness instead."
                ),
            )
            .into());
        }

        // Acquire locks in consistent order: sessions → harnesses → configs
        let mut sessions = self.sessions.write().await;
        let mut adapters = self.adapters.write().await;
        let mut configs = self.configs.write().await;

        if adapters.remove(id).is_none() {
            // A previously disabled harness has no adapter entry but still owns
            // a config record. Removing the orphan config is what the user
            // asked `unregister_harness` to do — fail only when neither side
            // knows about the id.
            if configs.remove(id).is_none() {
                return Err(AcpOperationError::new(
                    AcpErrorCode::HarnessNotFound,
                    format!("Harness '{id}' is not registered"),
                )
                .into());
            }
        } else {
            configs.remove(id);
        }

        // Detach all active sessions for this harness (all cwds) from the map,
        // then release every lock BEFORE killing. `kill()` awaits the OS; doing
        // it under the sessions/adapters/configs write locks (and across the
        // per-session mutex, possibly held by a long prompt) would freeze the
        // whole ACP subsystem for the prompt's lifetime.
        let removed: Vec<(SessionKey, SessionEntry)> = sessions
            .iter()
            .filter(|(k, _)| k.harness_id == id)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, _) in &removed {
            sessions.remove(key);
        }
        drop(configs);
        drop(adapters);
        drop(sessions);
        for (_, entry) in &removed {
            let mut session = entry.session.lock().await;
            session.kill().await;
        }
        for (key, _) in removed {
            self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                harness_id: key.harness_id,
                cwd: key.cwd.to_string_lossy().into_owned(),
                session_name: if key.name.is_empty() {
                    None
                } else {
                    Some(key.name)
                },
            })
            .await;
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

        // Refuse to create a brand-new harness via the update path — that is
        // the job of `register_harness`, which validates duplicates and the
        // enabled-flag invariant. Without this check, a typo in the harness id
        // (or a stale UI sending the wrong id) would silently insert a new
        // config and a new adapter instance.
        if !adapters.contains_key(id) && !configs.contains_key(id) {
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessNotFound,
                format!("Harness '{id}' is not registered; use register_harness instead"),
            )
            .into());
        }

        // Helper: remove all sessions for this harness_id (across all cwds)
        let remove_sessions = |sessions: &mut HashMap<SessionKey, SessionEntry>,
                               harness_id: &str| {
            let keys_to_remove: Vec<SessionKey> = sessions
                .keys()
                .filter(|k| k.harness_id == harness_id)
                .cloned()
                .collect();
            let mut removed = Vec::new();
            for key in keys_to_remove {
                if let Some(entry) = sessions.remove(&key) {
                    removed.push((key, entry));
                }
            }
            removed
        };

        if !entry.enabled {
            // Disable: remove harness but keep config
            adapters.remove(id);
            configs.insert(id.to_string(), entry);

            // Detach sessions, then release all locks before killing (kill()
            // awaits the OS and may block on a per-session mutex held by a
            // long-running prompt — don't do it under the write locks).
            let removed = remove_sessions(&mut sessions, id);
            drop(configs);
            drop(adapters);
            drop(sessions);
            for (_, entry) in &removed {
                let mut session = entry.session.lock().await;
                session.kill().await;
            }
            for (key, _) in removed {
                self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                    harness_id: key.harness_id,
                    cwd: key.cwd.to_string_lossy().into_owned(),
                    session_name: if key.name.is_empty() {
                        None
                    } else {
                        Some(key.name)
                    },
                })
                .await;
            }

            info!(harness_id = %id, "Disabled ACP harness");
            return Ok(());
        }

        let harness = Self::build_harness(id, &entry);
        adapters.insert(id.to_string(), harness);
        configs.insert(id.to_string(), entry);

        // Detach any active sessions so they will be respawned with the new
        // config, then release all locks before killing (see note above).
        let removed = remove_sessions(&mut sessions, id);
        drop(configs);
        drop(adapters);
        drop(sessions);
        for (_, entry) in &removed {
            let mut session = entry.session.lock().await;
            session.kill().await;
        }
        for (key, _) in removed {
            self.emit_persistence_event(crate::acp::AcpSessionEvent::Removed {
                harness_id: key.harness_id,
                cwd: key.cwd.to_string_lossy().into_owned(),
                session_name: if key.name.is_empty() {
                    None
                } else {
                    Some(key.name)
                },
            })
            .await;
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
    pub async fn set_persistence_hook(&self, hook: crate::acp::PersistenceHook) {
        let mut h = self.persistence_hook.write().await;
        *h = Some(hook);
    }

    /// Set the gateway broadcast hook fired on every pool mutation. Used to
    /// publish `acp.sessions.changed` so panels can re-fetch live.
    pub async fn set_gateway_change_hook(&self, hook: crate::acp::GatewayChangeHook) {
        let mut h = self.gateway_change_hook.write().await;
        *h = Some(hook);
    }

    /// Emit a persistence event AND fan out to the gateway broadcast hook.
    /// Both consumers are independent — either may be unset.
    pub(super) async fn emit_persistence_event(&self, event: crate::acp::AcpSessionEvent) {
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

            let timeout = harness.build_config(Some(&entry.cwd)).timeout;
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
                Err(_) => {
                    // Lock held by another task (likely an in-flight prompt).
                    // The session is actively in use, so report alive=true.
                    // State is Busy because we can't read the actual state
                    // without blocking the caller.
                    (true, crate::acp::protocol::AcpSessionState::Busy, None)
                }
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
}
