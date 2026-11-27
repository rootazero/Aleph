//! Harness registrations, saved associations and Gateway snapshots.

use std::collections::HashMap;

use tracing::warn;

use super::{
    session_closed, AcpAdapterManager, HarnessRegistration, ManagerState, SessionKey,
    SessionSnapshot,
};
use crate::acp::adapter::{AcpAdapter, AdapterMode};
use crate::acp::adapters::{CustomAcpAdapter, GenericAcpAdapter};
use crate::acp::protocol::{AcpErrorCode, AcpOperationError, AcpSessionState};
use crate::acp::session::PersistedAcpSession;
use crate::config::types::acp::{AcpAdapterEntry, TrustLevel};
use crate::error::{AlephError, Result};
use crate::sync_primitives::{Arc, Mutex};

impl HarnessRegistration {
    fn new(id: &str, config: AcpAdapterEntry) -> Self {
        let adapter = config
            .enabled
            .then(|| AcpAdapterManager::build_harness(id, &config));
        Self { config, adapter }
    }
}

impl AcpAdapterManager {
    #[must_use]
    pub fn new() -> Self {
        Self::from_entries(AcpAdapterEntry::all_presets().into_iter().collect())
    }

    #[must_use]
    pub fn from_entries(entries: HashMap<String, AcpAdapterEntry>) -> Self {
        let registrations = entries
            .into_iter()
            .filter(|(_, entry)| entry.enabled)
            .map(|(id, entry)| {
                let registration = HarnessRegistration::new(&id, entry);
                (id, registration)
            })
            .collect();
        Self {
            state: Arc::new(Mutex::new(ManagerState {
                registrations,
                ..ManagerState::default()
            })),
        }
    }

    pub(super) fn build_harness(id: &str, entry: &AcpAdapterEntry) -> Arc<dyn AcpAdapter> {
        if entry.preset.is_some() {
            Arc::new(GenericAcpAdapter::from_entry(entry))
        } else {
            Arc::new(CustomAcpAdapter::new(id.to_owned(), entry.clone()))
        }
    }

    pub(super) fn adapter(&self, id: &str) -> Option<Arc<dyn AcpAdapter>> {
        self.state().registrations.get(id)?.adapter.clone()
    }

    pub(super) async fn admitted_adapter(&self, id: &str) -> Result<Arc<dyn AcpAdapter>> {
        let state = self.state();
        Self::admitted_registration(&state, id).cloned()
    }

    fn admitted_registration<'a>(
        state: &'a ManagerState,
        id: &str,
    ) -> Result<&'a Arc<dyn AcpAdapter>> {
        if state.stopped {
            return Err(session_closed());
        }
        let registration = state
            .registrations
            .get(id)
            .ok_or_else(|| unknown_harness(id))?;
        let adapter = registration
            .adapter
            .as_ref()
            .ok_or_else(|| unknown_harness(id))?;
        if registration.config.trust_level == TrustLevel::Disabled {
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessDenied,
                format!("Delegation to ACP harness '{id}' is disabled"),
            )
            .into());
        }
        Ok(adapter)
    }

    pub(super) fn check_registration(
        state: &ManagerState,
        id: &str,
        adapter: &Arc<dyn AcpAdapter>,
    ) -> Result<()> {
        if !Arc::ptr_eq(Self::admitted_registration(state, id)?, adapter) {
            return Err(session_closed());
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn harness_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self
            .state()
            .registrations
            .iter()
            .filter(|(_, registration)| registration.adapter.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    pub async fn has_harness(&self, id: &str) -> bool {
        self.adapter(id).is_some()
    }

    pub async fn display_name(&self, id: &str) -> Option<String> {
        self.adapter(id)
            .map(|adapter| adapter.display_name().to_owned())
    }

    pub async fn harness_mode(&self, id: &str) -> Option<AdapterMode> {
        self.adapter(id).map(|adapter| adapter.mode())
    }

    pub async fn available_harnesses(&self) -> Vec<String> {
        let adapters: Vec<_> = self
            .state()
            .registrations
            .iter()
            .filter_map(|(id, registration)| {
                registration
                    .adapter
                    .clone()
                    .map(|adapter| (id.clone(), adapter))
            })
            .collect();
        let mut available: Vec<_> = futures::future::join_all(
            adapters
                .into_iter()
                .map(|(id, adapter)| async move { adapter.is_available().await.then_some(id) }),
        )
        .await
        .into_iter()
        .flatten()
        .collect();
        available.sort();
        available
    }

    pub async fn is_harness_available(&self, id: &str) -> bool {
        match self.adapter(id) {
            Some(adapter) => adapter.is_available().await,
            None => false,
        }
    }

    pub async fn register_harness(&self, id: String, entry: AcpAdapterEntry) -> Result<()> {
        if !entry.enabled {
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessDenied,
                format!("Cannot register disabled harness '{id}'"),
            )
            .into());
        }
        let registration = HarnessRegistration::new(&id, entry);
        let mut state = self.state();
        if state.stopped {
            return Err(session_closed());
        }
        if state.registrations.contains_key(&id) {
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessDenied,
                format!("Harness '{id}' is already registered. Use update_harness to modify it."),
            )
            .into());
        }
        state.registrations.insert(id, registration);
        Ok(())
    }

    pub async fn unregister_harness(&self, id: &str) -> Result<()> {
        if AcpAdapterEntry::is_preset_id(id) {
            return Err(AcpOperationError::new(
                AcpErrorCode::HarnessDenied,
                format!("Cannot unregister preset harness '{id}'. Disable it via update_harness instead."),
            ).into());
        }
        let reservations = {
            let mut state = self.state();
            if state.stopped {
                return Err(session_closed());
            }
            if state.registrations.remove(id).is_none() {
                return Err(unknown_harness(id));
            }
            self.withdraw_slots(&mut state, Some(id), true)
        };
        self.finish_retirements(reservations).await
    }

    pub async fn update_harness(&self, id: &str, entry: AcpAdapterEntry) -> Result<()> {
        let registration = HarnessRegistration::new(id, entry);
        let reservations = {
            let mut state = self.state();
            if state.stopped {
                return Err(session_closed());
            }
            let existing = state
                .registrations
                .get_mut(id)
                .ok_or_else(|| unknown_harness(id))?;
            *existing = registration;
            self.withdraw_slots(&mut state, Some(id), true)
        };
        self.finish_retirements(reservations).await
    }

    pub async fn get_config(&self, id: &str) -> Option<AcpAdapterEntry> {
        self.state()
            .registrations
            .get(id)
            .map(|registration| registration.config.clone())
    }

    pub async fn list_configs(&self) -> Vec<(String, AcpAdapterEntry)> {
        let mut configs: Vec<_> = self
            .state()
            .registrations
            .iter()
            .map(|(id, registration)| (id.clone(), registration.config.clone()))
            .collect();
        configs.sort_by(|a, b| a.0.cmp(&b.0));
        configs
    }

    pub async fn set_persistence_hook(&self, hook: crate::acp::PersistenceHook) {
        self.state().persistence_hook = Some(hook);
    }

    pub async fn set_gateway_change_hook(&self, hook: crate::acp::GatewayChangeHook) {
        self.state().gateway_change_hook = Some(hook);
    }

    /// Seed all associations before using normal native admission to load them.
    /// A failed load leaves its remote ID available for a later retry or close.
    pub async fn restore_sessions(&self, persisted: Vec<PersistedAcpSession>) -> Vec<String> {
        let keys = {
            let mut state = self.state();
            if state.stopped {
                return Vec::new();
            }
            let mut keys = Vec::new();
            for record in persisted {
                let key = SessionKey::with_name(
                    &record.harness_id,
                    &record.cwd,
                    record.session_name.as_deref(),
                );
                let slot = state.slots.entry(key.clone()).or_default();
                if slot.live.is_none() && slot.saved_id.is_none() && !slot.epoch.is_cancelled() {
                    slot.saved_id = Some(record.acp_session_id);
                    keys.push((key, Arc::clone(&slot.epoch)));
                }
            }
            if !keys.is_empty() {
                state.publish(None);
            }
            keys
        };
        let mut restored = Vec::new();
        for (key, epoch) in keys {
            let result = async {
                let adapter = self.admitted_adapter(&key.harness_id).await?;
                super::lifecycle::require_mode(&*adapter, &key.harness_id, AdapterMode::NativeAcp)?;
                self.acquire_owner(key.clone(), adapter, false, false, Some(&epoch))
                    .await
            }
            .await;
            match result {
                Ok(_) => restored.push(key.harness_id),
                Err(error) => {
                    warn!(harness = %key.harness_id, %error, "ACP session restore failed")
                }
            }
        }
        restored
    }

    /// Snapshot without waiting for an active prompt to release the session.
    pub async fn list_sessions(&self) -> Vec<SessionSnapshot> {
        let state = self.state();
        let mut snapshots = Vec::new();
        for (key, slot) in &state.slots {
            let (alive, session_state) = match slot.live.as_ref() {
                Some(entry) if !entry.closed.is_cancelled() => match entry.session.try_lock() {
                    Ok(mut session) => {
                        let alive = session.is_alive();
                        (
                            alive,
                            if alive {
                                session.state()
                            } else {
                                AcpSessionState::Error
                            },
                        )
                    }
                    Err(_) => (true, AcpSessionState::Busy),
                },
                _ if slot.saved_id.is_some() => (false, AcpSessionState::Error),
                _ => continue,
            };
            snapshots.push(SessionSnapshot {
                harness_id: key.harness_id.clone(),
                cwd: key.cwd.to_string_lossy().into_owned(),
                session_name: key.session_name_opt().map(str::to_owned),
                acp_session_id: slot.saved_id.clone(),
                alive,
                state: session_state,
                load_error: slot.load_failure.clone(),
            });
        }
        snapshots.sort_by(|a, b| {
            a.harness_id
                .cmp(&b.harness_id)
                .then(a.cwd.cmp(&b.cwd))
                .then(a.session_name.cmp(&b.session_name))
        });
        snapshots
    }
}

fn unknown_harness(id: &str) -> AlephError {
    AcpOperationError::new(
        AcpErrorCode::HarnessNotFound,
        format!("Unknown ACP harness: '{id}'"),
    )
    .into()
}
