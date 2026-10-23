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
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::registry::{
    PluginBatch, PluginBatchEntry, PluginBatchRegistration, PluginPublicationError,
};

// Incarnation IDs, not OwnershipTree generations or canonical revisions. Only
// declaration creation allocates one; describing a Cold owner allocates nothing.
static NEXT_OWNER_GENERATION: Mutex<u64> = Mutex::new(1);

#[derive(Debug)]
pub(super) enum OwnerFailure {
    Publication(PluginPublicationError),
    Revoke(PluginPublicationError),
    Drain(String),
    Join(String),
    Stop(String),
    MissingReceipt,
    MissingScope,
    Dispose(super::effects::DisposeReport),
}

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
    pub owner_generation: u64,
    pub receipt: Mutex<Option<PluginBatchRegistration>>,
    /// Consumed FnOnce disposers cannot be retried: retain exact reports and
    /// mount/receipt evidence on this owner, never silently remove a residual.
    pub failures: Mutex<Vec<OwnerFailure>>,
    stop_requested: tokio::sync::Notify,
    pub task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// One retained cleanup operation, not a second scope/disposer authority.
    pub stop: Mutex<Option<StopOperation>>,
    pub completion: tokio::sync::watch::Sender<Option<Result<(), String>>>,
    /// Admission permits cover the complete guest call. Stop closes this
    /// semaphore and drains all permits before disposing the exact scope.
    pub admission: Arc<Semaphore>,
    pub admission_open: AtomicBool,
    #[cfg(test)]
    pub stop_join_entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    #[cfg(test)]
    pub publication_barrier: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    #[cfg(test)]
    pub barrier: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

pub(super) struct StopOperation {
    pub completion: tokio::sync::watch::Sender<
        Option<Result<super::effects::DisposeReport, super::lifecycle::UnmountError>>,
    >,
    // Dropping the waiter or even this handle never aborts the operation.
    pub _task: tokio::task::JoinHandle<()>,
}

impl RuntimeOwner {
    pub(super) const MAX_IN_FLIGHT: u32 = 1024;

    pub fn failure_summary(&self) -> String {
        self.failures
            .lock()
            .unwrap()
            .iter()
            .map(|failure| match failure {
                OwnerFailure::Publication(error) => format!("publication: {error}"),
                OwnerFailure::Revoke(error) => format!("revoke: {error}"),
                OwnerFailure::Drain(error) => format!("drain: {error}"),
                OwnerFailure::Join(error) => format!("join: {error}"),
                OwnerFailure::Stop(error) => format!("stop: {error}"),
                OwnerFailure::MissingReceipt => "ready owner has no batch receipt".into(),
                OwnerFailure::MissingScope => "mounted owner has no exact effect scope".into(),
                OwnerFailure::Dispose(report) => {
                    format!("dispose: {:?}", report.failures().collect::<Vec<_>>())
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn close_admission(&self) {
        self.admission_open.store(false, Ordering::Release);
        self.stop_requested.notify_waiters();
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
            owner_generation: {
                let mut next = NEXT_OWNER_GENERATION.lock().unwrap();
                let generation = *next;
                *next = next
                    .checked_add(1)
                    .expect("plugin owner incarnation generation exhausted");
                generation
            },
            receipt: Mutex::new(None),
            failures: Mutex::new(Vec::new()),
            stop_requested: tokio::sync::Notify::new(),
            task: Mutex::new(None),
            stop: Mutex::new(None),
            completion,
            admission: Arc::new(Semaphore::new(Self::MAX_IN_FLIGHT as usize)),
            admission_open: AtomicBool::new(true),
            #[cfg(test)]
            stop_join_entered: Mutex::new(None),
            #[cfg(test)]
            barrier: Mutex::new(None),
            #[cfg(test)]
            publication_barrier: Mutex::new(None),
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

    /// The retained task owns only the exact incarnation and a weak manager
    /// reference; cancelling a waiter does not cancel mount, lose partial
    /// effects, or strand Starting.
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
                    let manager = std::sync::Arc::downgrade(self);
                    let operation = Arc::clone(&owner);
                    *task = Some(tokio::spawn(async move {
                        let result = match manager.upgrade() {
                            Some(manager) => std::panic::AssertUnwindSafe(
                                manager.mount_owner(&operation, generation),
                            )
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| {
                                Err("activation panicked; retained owner requires recovery".into())
                            }),
                            None => Err(
                                "activation manager was dropped; retained owner requires recovery"
                                    .into(),
                            ),
                        };
                        if result.is_err() && operation.provider.state() == ProviderState::Starting
                        {
                            operation.close_admission();
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
        // Phase one: a retained task waiting for control has no effects yet.
        // Stop fences it without joining a task blocked on the same load_guard.
        let stopped = owner.stop_requested.notified();
        tokio::pin!(stopped);
        stopped.as_mut().enable();
        if !owner.admission_open.load(Ordering::Acquire) {
            return Err("activation admission is closed".into());
        }
        #[cfg(test)]
        {
            let barrier = owner.barrier.lock().unwrap().take();
            if let Some((entered, release)) = barrier {
                let _ = entered.send(());
                tokio::select! {
                    _ = &mut stopped => return Err("activation fenced before mount".into()),
                    _ = release => {},
                }
            }
        }
        let _control = tokio::select! {
            _ = &mut stopped => return Err("activation fenced before control".into()),
            control = self.load_guard.lock() => control,
        };
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
        #[cfg(test)]
        {
            let barrier = owner.publication_barrier.lock().unwrap().take();
            if let Some((entered, release)) = barrier {
                let _ = entered.send(());
                let _ = release.await;
            }
        }
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
        *owner.identity.lock().unwrap() = Some(identity.clone());
        let publication = self.prepare_owner_batch(owner, generation, identity).await;
        let publication = match publication {
            Ok(batch) => {
                let canonical = self.tool_handler_registry.read().unwrap().clone();
                match canonical {
                    Some(canonical) => canonical.publish_plugin_batch_with_ready_commit(batch),
                    None => Err(PluginPublicationError::InvalidIdentity(
                        "plugin producer reconciliation is not mounted",
                    )),
                }
            }
            Err(error) => Err(error),
        };
        match publication {
            Ok(receipt) => *owner.receipt.lock().unwrap() = Some(receipt),
            Err(error) => {
                let reason = error.to_string();
                owner.close_admission();
                owner
                    .failures
                    .lock()
                    .unwrap()
                    .push(OwnerFailure::Publication(error));
                // No batch was committed. Dispose only this fenced mount. A
                // failed disposer report remains on the owner, without retry.
                let scope = self.scopes.lock().unwrap().remove(id);
                if let Some(scope) = scope {
                    let report = scope.dispose().await;
                    if !report.all_ok() {
                        owner
                            .failures
                            .lock()
                            .unwrap()
                            .push(OwnerFailure::Dispose(report));
                        let _ = owner.provider.transition(ProviderState::Failed);
                    } else {
                        // The scope removed the VM; do not retain its Arc only
                        // for a failed, never-published activation.
                        owner.identity.lock().unwrap().take();
                    }
                }
                // Activation has ended, whether or not disposal left effects.
                // A clean failure is still Failed, never a stranded Starting.
                let _ = owner.provider.transition(ProviderState::Failed);
                let mut row = owner.record.clone();
                row.status = PluginStatus::Error(format!("canonical publication: {reason}"));
                row.error = Some(reason.clone());
                self.plugin_registry.write().await.register_plugin(row);
                self.publish_runtime_transition_views().await;
                return Err(reason);
            }
        }
        if let Some(row) = self.plugin_registry.write().await.get_plugin_mut(id) {
            row.status = PluginStatus::Loaded;
        }
        // This is still under load_guard; stop cannot overtake the unique
        // lifecycle projection boundary or join a task waiting to acquire it.
        self.publish_runtime_transition_views().await;
        Ok(())
    }

    async fn prepare_owner_batch(
        &self,
        owner: &Arc<RuntimeOwner>,
        generation: u64,
        mount: WasmMountIdentity,
    ) -> Result<PluginBatch, PluginPublicationError> {
        let registry = self.plugin_registry.read().await;
        let loader = self.plugin_loader.read().await;
        let entries = registry
            .list_tools_for_plugin(&owner.record.id)
            .into_iter()
            .map(|registration| {
                let binding = loader
                    .bind_wasm(&owner.record.id, &registration.handler)
                    .map_err(|error| PluginPublicationError::InvalidEntry {
                        name: registration.name.clone(),
                        reason: error.to_string(),
                    })?;
                let handler = super::canonical_handler::build_canonical_handler(
                    self.plugin_loader.clone(),
                    owner,
                    generation,
                    binding.clone(),
                    registration.clone(),
                );
                let descriptor =
                    ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
                Ok(PluginBatchEntry {
                    descriptor,
                    handler,
                    binding,
                    max_result_tokens: None,
                })
            })
            .collect::<Result<Vec<_>, PluginPublicationError>>()?;
        Ok(PluginBatch {
            plugin_id: owner.record.id.clone(),
            provider: owner.provider.clone(),
            provider_generation: generation,
            owner_generation: owner.owner_generation,
            mount,
            entries,
        })
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
