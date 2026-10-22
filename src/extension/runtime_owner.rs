//! Tool-only WASM activation ownership. No callable registry or durable intent.

use super::error::{ExtensionError, ExtensionResult};
use super::{ExtensionManager, PluginStatus};
use crate::sync_primitives::Arc;
use futures::FutureExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::manifest::{adapter::AdapterOutput, PluginManifest};
use super::runtime::wasm::WasmMountIdentity;
use super::types::{PluginKind, PluginRecord};
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
};

/// One declaration incarnation, replaced only under the manager's control lock.
/// State/generation live exclusively in ProviderHandle. The retained task shares
/// only mount completion, never a caller's permission or business result.
pub(super) struct RuntimeOwner {
    pub provider: ProviderHandle,
    pub output: AdapterOutput,
    pub manifest: PluginManifest,
    pub record: PluginRecord,
    pub identity: Mutex<Option<WasmMountIdentity>>,
    pub task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub completion: tokio::sync::watch::Sender<Option<Result<(), String>>>,
    /// Admission permits cover the complete guest call. Stop closes this
    /// semaphore and drains all permits before disposing the exact scope.
    pub admission: Arc<Semaphore>,
    pub admission_open: AtomicBool,
    #[cfg(test)]
    pub barrier: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

impl RuntimeOwner {
    pub(super) const MAX_IN_FLIGHT: u32 = 1024;

    pub fn close_admission(&self) {
        self.admission_open.store(false, Ordering::Release);
    }

    pub async fn acquire_permit(&self) -> Result<OwnedSemaphorePermit, String> {
        if !self.admission_open.load(Ordering::Acquire) {
            return Err("runtime owner admission is closed".to_owned());
        }
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "runtime owner admission is unavailable".to_owned())?;
        if !self.admission_open.load(Ordering::Acquire) {
            drop(permit);
            return Err("runtime owner admission is closed".to_owned());
        }
        Ok(permit)
    }

    pub fn new(output: AdapterOutput, manifest: PluginManifest, record: PluginRecord) -> Self {
        let provider = ProviderHandle::new(ProviderLifecycle::new(
            ProviderId::new(format!("plugin/{}", record.id)).expect("nonempty plugin provider id"),
            ProviderTransport::InProcess,
        ));
        provider
            .transition(ProviderState::Cold)
            .expect("new declaration enters Cold");
        let (completion, _) = tokio::sync::watch::channel(None);
        Self {
            provider,
            output,
            manifest,
            record,
            identity: Mutex::new(None),
            task: Mutex::new(None),
            completion,
            admission: Arc::new(Semaphore::new(Self::MAX_IN_FLIGHT as usize)),
            admission_open: AtomicBool::new(true),
            #[cfg(test)]
            barrier: Mutex::new(None),
        }
    }
}

impl ExtensionManager {
    pub(super) fn runtime_owner(&self, id: &str) -> Option<Arc<RuntimeOwner>> {
        self.runtime_owners.lock().unwrap().get(id).cloned()
    }

    pub(super) fn owner_is_current(&self, owner: &Arc<RuntimeOwner>) -> bool {
        self.runtime_owner(&owner.record.id)
            .is_some_and(|current| Arc::ptr_eq(&current, owner))
    }

    /// Called under load_guard. Discovery publishes declarations, never effects.
    pub(super) async fn declare_cold(
        &self,
        output: AdapterOutput,
        mut record: PluginRecord,
        manifest: PluginManifest,
    ) -> PluginStatus {
        let id = record.id.clone();
        record.status = PluginStatus::Cold;
        let owner = Arc::new(RuntimeOwner::new(output, manifest, record.clone()));
        self.runtime_owners.lock().unwrap().insert(id, owner);
        self.plugin_registry.write().await.register_plugin(record);
        PluginStatus::Cold
    }

    /// The task owns the manager and incarnation; cancelling a waiter does not
    /// cancel mount, lose partial effects, or strand Starting.
    pub(super) async fn activate_owner(
        self: &Arc<Self>,
        owner: Arc<RuntimeOwner>,
    ) -> ExtensionResult<()> {
        let mut completion = owner.completion.subscribe();
        {
            let mut task = owner.task.lock().unwrap();
            match owner.provider.state() {
                ProviderState::Ready => return Ok(()),
                ProviderState::Cold => {
                    owner
                        .provider
                        .transition(ProviderState::Starting)
                        .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
                    let generation = owner.provider.generation();
                    let manager = Arc::clone(self);
                    let operation = Arc::clone(&owner);
                    *task = Some(tokio::spawn(async move {
                        let result = std::panic::AssertUnwindSafe(
                            manager.mount_owner(&operation, generation),
                        )
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|_| {
                            Err("activation panicked; retained owner requires recovery".into())
                        });
                        if result.is_ok() {
                            manager.publish_runtime_transition_views().await;
                        }
                        if result.is_err() && operation.provider.state() == ProviderState::Starting
                        {
                            let _ = operation.provider.transition(ProviderState::Failed);
                        }
                        operation.completion.send_replace(Some(result));
                    }));
                }
                ProviderState::Starting => {}
                state => {
                    return Err(ExtensionError::Runtime(format!(
                        "plugin owner rejects activation: {state:?}"
                    )))
                }
            }
        }
        loop {
            if let Some(result) = completion.borrow().clone() {
                return result.map_err(ExtensionError::Runtime);
            }
            completion
                .changed()
                .await
                .map_err(|_| ExtensionError::Runtime("activation owner lost completion".into()))?;
        }
    }

    async fn mount_owner(&self, owner: &Arc<RuntimeOwner>, generation: u64) -> Result<(), String> {
        #[cfg(test)]
        {
            let barrier = owner.barrier.lock().unwrap().take();
            if let Some((entered, release)) = barrier {
                let _ = entered.send(());
                let _ = release.await;
            }
        }
        let _control = self.load_guard.lock().await;
        let id = &owner.record.id;
        if !self.owner_is_current(owner)
            || owner.provider.snapshot() != (ProviderState::Starting, generation)
        {
            return Err("activation incarnation is stale".into());
        }
        self.admit(id, owner.record.origin)
            .await
            .map_err(|e| e.to_string())?;
        // Never accept the loader's duplicate-load no-op as our own mount.
        if self.plugin_loader.read().await.is_loaded(id) {
            return Err("activation found an unowned runtime; explicit recovery required".into());
        }
        let mut record = owner.record.clone();
        record.status = PluginStatus::Starting;
        self.plugin_registry
            .write()
            .await
            .register_plugin(record.clone());
        self.mount_parsed(owner.output.clone(), record, Some(owner.manifest.clone()))
            .await
            .map_err(|e| e.to_string())?;
        let identity = self
            .plugin_loader
            .read()
            .await
            .wasm_mount_identity(id)
            .ok_or_else(|| "mount completed without a WASM identity".to_owned())?;
        // load_guard excludes replacement/control commits until publication.
        // Still fence explicitly: no completion may publish a different owner.
        if !self.owner_is_current(owner)
            || owner.provider.snapshot() != (ProviderState::Starting, generation)
        {
            return Err("mount completion is stale; retained effects require cleanup".into());
        }
        self.admit(id, owner.record.origin)
            .await
            .map_err(|e| e.to_string())?;
        *owner.identity.lock().unwrap() = Some(identity);
        owner
            .provider
            .transition(ProviderState::Ready)
            .map_err(|e| e.to_string())?;
        if let Some(row) = self.plugin_registry.write().await.get_plugin_mut(id) {
            row.status = PluginStatus::Loaded;
        }
        self.sync_runtime_snapshots().await;
        Ok(())
    }
}

/// Deliberately narrow: both the parsed manifest AND the adapter must prove
/// tool-only. Unknown/new capability variants are rejected by this allowlist.
/// Mixed plugins retain the existing eager path, not an implicit lazy fallback.
pub(super) fn eligible(output: &AdapterOutput, manifest: Option<&PluginManifest>) -> bool {
    let Some(m) = manifest else {
        return false;
    };
    m.kind == PluginKind::Wasm
        && m.id == output.plugin_id
        && !output.capabilities.is_empty()
        && output
            .capabilities
            .iter()
            .all(|cap| matches!(cap, super::capability::CapabilityDeclaration::Tool(_)))
        && m.hooks_v2.as_ref().is_none_or(Vec::is_empty)
        && m.services_v2.as_ref().is_none_or(Vec::is_empty)
        && m.commands_v2.as_ref().is_none_or(Vec::is_empty)
        && m.memory_manifest.is_none()
        && m.prompt_v2.is_none()
        && m.capabilities_v2.as_ref().is_none_or(|caps| {
            !caps.dynamic_tools
                && !caps.dynamic_hooks
                && caps.workspace.is_none()
                && caps.http.is_none()
                && caps.secrets.is_none()
        })
        && m.aleph_extensions.as_ref().is_none_or(|ext| {
            ext.services.is_empty() && ext.contexts.is_empty() && ext.capabilities.is_none()
        })
        && m.wasm_resource_limits
            .as_ref()
            .is_none_or(|limits| limits.timeout_secs > 0)
}
