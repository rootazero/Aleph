//! Concrete ownership of Memory resources and admitted work.

mod bundle;
mod context;
mod embedding;
mod operation;
mod search;
mod services;
mod session_end;
mod state;
mod tasks;

#[cfg(test)]
mod service_lifecycle_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod watcher_lifecycle_tests;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::capability::descriptor::CapabilityId;
use crate::capability::facade::{Reference, ResolveError};
use crate::capability::ownership::{
    CapturedBinding, LifetimeScope, OwnerRef, OwnershipTree, RegisterError, VisibilityScope,
};
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
};
use crate::capability::provider_registry::ProviderRegistration;
use crate::capability::provider_registry::{ActivationError, ProviderActivator};
use crate::error::AlephError;
use crate::sync_primitives::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub(crate) use context::carry;
pub use embedding::MemoryEmbeddingBatch;
pub use operation::{MemoryHandle, MemoryOperation};
pub(crate) use search::MemorySearchSettings;
pub use services::MemoryServicesConfig;
pub use state::{MemoryState, MemoryStatus};

pub struct MemoryRuntimeConfig {
    pub metadata_path: PathBuf,
    pub note_memory_dir: PathBuf,
}

pub struct MemoryRuntime {
    config: MemoryRuntimeConfig,
    services_config: std::sync::OnceLock<Option<MemoryServicesConfig>>,
    ownership: Arc<OwnershipTree>,
    inner: Mutex<MemoryInner>,
    tasks: tasks::MemoryTasks,
    pub(crate) search_settings: search::MemorySearchSettings,
    changed: Notify,
    provider: ProviderHandle,
    observations: std::sync::OnceLock<Arc<crate::capability::events::SourceEventFactory>>,
    #[cfg(test)]
    initialization_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

struct MemoryInner {
    status: MemoryStatus,
    bundle: Option<Arc<bundle::MemoryBundle>>,
    driver: Option<JoinHandle<()>>,
    admitted: usize,
    rotation_failed: bool,
    source_observation: Option<(
        CapturedBinding,
        Arc<crate::capability::events::SourceObservation>,
    )>,
}

pub(crate) fn resource_id() -> CapabilityId {
    CapabilityId {
        namespace: "aleph/resources".into(),
        name: "memory.runtime".into(),
    }
}

pub(crate) fn required_service() -> aleph_slot_api::capability::Service {
    aleph_slot_api::capability::Service {
        relation: aleph_slot_api::capability::ServiceRelation::Requires,
        target: resource_id(),
    }
}

impl MemoryRuntime {
    pub fn new(config: MemoryRuntimeConfig, ownership: Arc<OwnershipTree>) -> Arc<Self> {
        Self::new_sources(config, ownership, None)
    }

    pub fn new_with_services(
        config: MemoryRuntimeConfig,
        ownership: Arc<OwnershipTree>,
        services: MemoryServicesConfig,
    ) -> Arc<Self> {
        Self::new_sources(config, ownership, Some(services))
    }

    /// Install external inputs once, before the first owned resource is opened.
    pub fn configure_services(&self, services: MemoryServicesConfig) -> Result<(), AlephError> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner.status.state != MemoryState::Off
            || inner.bundle.is_some()
            || inner.driver.is_some()
            || self.services_config.get().is_some()
        {
            return Err(AlephError::other(
                "memory service inputs are already installed or initialization has begun",
            ));
        }
        self.services_config
            .set(Some(services))
            .map_err(|_| AlephError::other("memory service inputs are already installed"))
    }

    pub(crate) fn injection_mode(&self) -> crate::config::types::memory::MemoryInjectionMode {
        self.memory_config(|memory| memory.injection_mode)
            .unwrap_or_default()
    }

    pub(crate) fn memory_config<R>(
        &self,
        read: impl FnOnce(&crate::config::types::memory::MemoryConfig) -> R,
    ) -> Option<R> {
        let bundle = self
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .bundle
            .clone()?;
        Some(read(&bundle.services().ok()?.config))
    }

    fn new_sources(
        config: MemoryRuntimeConfig,
        ownership: Arc<OwnershipTree>,
        services_config: Option<MemoryServicesConfig>,
    ) -> Arc<Self> {
        let provider = ProviderHandle::new(ProviderLifecycle::new(
            ProviderId::new("aleph/providers/memory.runtime").expect("static provider id"),
            ProviderTransport::InProcess,
        ));
        provider
            .transition(ProviderState::Cold)
            .expect("new provider must enter Cold");
        Arc::new(Self {
            config,
            services_config: services_config.map_or_else(std::sync::OnceLock::new, |services| {
                std::sync::OnceLock::from(Some(services))
            }),
            ownership,
            inner: Mutex::new(MemoryInner {
                status: MemoryStatus {
                    state: MemoryState::Off,
                    first_error: None,
                },
                bundle: None,
                driver: None,
                admitted: 0,
                rotation_failed: false,
                source_observation: None,
            }),
            tasks: tasks::MemoryTasks::default(),
            search_settings: search::MemorySearchSettings::default(),
            changed: Notify::new(),
            provider,
            observations: std::sync::OnceLock::new(),
            #[cfg(test)]
            initialization_gate: Mutex::new(None),
        })
    }

    /// A fresh entry captures once; descendants keep their existing admission.
    pub(crate) fn operation(self: &Arc<Self>) -> Result<MemoryOperation, AlephError> {
        if let Some(operation) = MemoryOperation::current() {
            return operation
                .belongs_to(self)
                .then_some(operation)
                .ok_or_else(|| AlephError::other("memory operation belongs to another runtime"));
        }
        self.capture(&Reference {
            id: resource_id(),
            visibility: VisibilityScope::default(),
        })
        .and_then(|handle| handle.admit())
        .map_err(|error| AlephError::other(format!("memory unavailable: {error:?}")))
    }

    pub(crate) fn current_reconciler(
        &self,
    ) -> Option<Arc<crate::memory::events::handler::MemoryCommandHandler>> {
        let bundle = {
            let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            (inner.status.state == MemoryState::Running)
                .then(|| inner.bundle.clone())
                .flatten()
        }?;
        bundle.current_reconciler()
    }

    pub(crate) fn install_source_observer(
        &self,
        factory: Arc<crate::capability::events::SourceEventFactory>,
    ) {
        let _ = self.observations.set(factory);
    }

    pub(crate) fn with_source_observation<R>(
        &self,
        read: impl FnOnce(Option<Arc<crate::capability::events::SourceObservation>>) -> R,
    ) -> R {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read(
            inner
                .source_observation
                .as_ref()
                .map(|(_, observation)| observation.clone()),
        )
    }

    pub(crate) fn source_observation_for_binding(
        &self,
        binding: &CapturedBinding,
    ) -> Option<Arc<crate::capability::events::SourceObservation>> {
        let inner = self.inner.try_lock().ok()?;
        inner
            .source_observation
            .as_ref()
            .filter(|(source, _)| source == binding)
            .map(|(_, observation)| observation.clone())
    }

    pub(crate) fn provider_registration(self: &Arc<Self>) -> ProviderRegistration {
        ProviderRegistration::new(
            self.descriptor(&VisibilityScope::default())
                .expect("memory ownership must be available at registration"),
            self.provider.clone(),
        )
        .with_activator(Arc::new(MemoryActivator::new(self)))
    }

    pub(crate) fn provider(&self) -> ProviderHandle {
        self.provider.clone()
    }

    pub fn status(&self) -> MemoryStatus {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .status
            .clone()
    }

    pub async fn start(self: &Arc<Self>) -> Result<MemoryStatus, AlephError> {
        let mut requested = false;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                match inner.status.state {
                    MemoryState::Off if requested => return Err(Self::failure(&inner)),
                    MemoryState::Off if inner.driver.is_none() => {
                        self.services_config.get_or_init(|| None);
                        inner.status.state = MemoryState::Starting;
                        inner.rotation_failed = false;
                        let _ = self.provider.transition(ProviderState::Starting);
                        self.spawn_driver(&mut inner, true);
                        requested = true;
                    }
                    MemoryState::Running => return Ok(inner.status.clone()),
                    MemoryState::Starting => requested = true,
                    MemoryState::Draining => {
                        return Err(AlephError::other("memory runtime is draining"));
                    }
                    MemoryState::Failed => return Err(Self::failure(&inner)),
                    MemoryState::Off => {}
                }
            }
            tokio::select! {
                _ = &mut changed => {},
                _ = futures::future::poll_fn(|cx| self.poll_driver(cx)) => {},
            }
        }
    }

    pub async fn stop(self: &Arc<Self>) -> Result<MemoryStatus, AlephError> {
        let mut requested = false;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                match inner.status.state {
                    MemoryState::Off => {
                        return match &inner.status.first_error {
                            Some(error) => Err(AlephError::other(error.clone())),
                            None => Ok(inner.status.clone()),
                        };
                    }
                    MemoryState::Starting | MemoryState::Running => {
                        self.begin_drain(&mut inner);
                        requested = true;
                    }
                    MemoryState::Draining => requested = true,
                    MemoryState::Failed if requested => return Err(Self::failure(&inner)),
                    MemoryState::Failed if inner.driver.is_none() => {
                        self.begin_drain(&mut inner);
                        self.spawn_driver(&mut inner, false);
                        requested = true;
                    }
                    MemoryState::Failed => {}
                }
            }
            tokio::select! {
                _ = &mut changed => {},
                _ = futures::future::poll_fn(|cx| self.poll_driver(cx)) => {},
            }
        }
    }

    pub(crate) async fn record_turn(self: &Arc<Self>) {
        let Ok(operation) = self.operation() else {
            return;
        };
        let Ok(services) = operation.services() else {
            return;
        };
        if let Some(compression) = &services.compression {
            operation
                .scope(async { compression.record_turn_and_maybe_compress() })
                .await;
        }
    }

    pub(crate) fn retain_worker(&self, handle: JoinHandle<()>) {
        self.tasks.retain(handle);
        self.changed.notify_waiters();
    }

    pub(crate) fn worker_stop(&self) -> Result<tokio::sync::watch::Receiver<bool>, AlephError> {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .bundle
            .as_ref()
            .map(|bundle| bundle.subscribe_stop())
            .ok_or_else(|| AlephError::other("memory bundle is unavailable"))
    }

    pub fn capture(self: &Arc<Self>, reference: &Reference) -> Result<MemoryHandle, ResolveError> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if reference.id != resource_id() || inner.status.state != MemoryState::Running {
            return Err(ResolveError::Unknown);
        }
        Ok(MemoryHandle {
            runtime: Arc::downgrade(self),
            binding: self.capture_binding(reference)?,
        })
    }

    pub(crate) fn is_backed_by_tree(&self, tree: &Arc<OwnershipTree>) -> bool {
        Arc::ptr_eq(&self.ownership, tree)
    }

    pub(crate) fn descriptor(
        &self,
        visibility: &VisibilityScope,
    ) -> Result<crate::capability::descriptor::CapabilityDescriptor, ResolveError> {
        use crate::capability::descriptor::{
            CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityKind,
            CapabilityRevision, ProjectionMetadata, SchemaRef,
        };
        use sha2::{Digest, Sha256};
        self.capture_binding(&Reference {
            id: resource_id(),
            visibility: visibility.clone(),
        })?;
        let schema = serde_json::to_value(schemars::schema_for!(MemoryStatus))
            .map_err(|_| ResolveError::Unknown)?;
        let mut canonical = Vec::new();
        crate::tools::descriptor::canonicalize_json(&schema, &mut canonical);
        Ok(CapabilityDescriptor {
            id: resource_id(),
            kind: CapabilityKind::Resource,
            schema: SchemaRef {
                version: 1,
                fingerprint: Sha256::digest(canonical).into(),
            },
            revision: CapabilityRevision(0),
            lifetime: LifetimeScope::Runtime,
            visibility: visibility.clone(),
            owner: OwnerRef::Runtime,
            services: Vec::new(),
            metadata: ProjectionMetadata::default(),
            facets: CapabilityFacets::new().with(CapabilityFacet::ProviderLifecycle),
        })
    }

    pub(crate) fn capture_binding(
        &self,
        reference: &Reference,
    ) -> Result<CapturedBinding, ResolveError> {
        if reference.visibility == VisibilityScope::default() {
            self.ownership
                .register_if_absent(
                    reference.id.clone(),
                    OwnerRef::Runtime,
                    LifetimeScope::Runtime,
                    reference.visibility.clone(),
                )
                .map_err(|error| match error {
                    RegisterError::Revoked => ResolveError::Revoked,
                    RegisterError::Disposed | RegisterError::GenerationExhausted => {
                        ResolveError::StaleOwner
                    }
                })?;
        }
        let binding = CapturedBinding {
            capability: reference.id.clone(),
            visibility: reference.visibility.clone(),
            generation: self
                .ownership
                .generation(&reference.id, &reference.visibility)
                .ok_or(ResolveError::NotVisible)?,
            owner: OwnerRef::Runtime,
            lifetime: LifetimeScope::Runtime,
        };
        self.ownership
            .with_current_bindings(std::slice::from_ref(&binding), || ())
            .map_err(|_| ResolveError::StaleOwner)?;
        Ok(binding)
    }

    fn spawn_driver(self: &Arc<Self>, inner: &mut MemoryInner, initialize: bool) {
        let runtime = self.clone();
        inner.driver = Some(tokio::spawn(async move {
            let completion = DriverCompletion(Arc::downgrade(&runtime));
            runtime.drive(initialize).await;
            drop(completion);
        }));
    }

    fn begin_drain(&self, inner: &mut MemoryInner) {
        if inner.status.state == MemoryState::Draining {
            return;
        }
        inner.status.state = MemoryState::Draining;
        let _ = self.provider.transition(ProviderState::Stopping);
        inner.rotation_failed = match self.ownership.rotate_capability(&resource_id()) {
            Ok(()) => false,
            Err(error) => {
                Self::record_error(
                    inner,
                    format!("memory ownership rotation failed: {error:?}"),
                );
                true
            }
        };
        if let Some(bundle) = &inner.bundle {
            bundle.request_stop();
        }
        self.changed.notify_waiters();
    }

    async fn drive(self: &Arc<Self>, initialize: bool) {
        if initialize {
            let opened = bundle::MemoryBundle::open(&self.config, Arc::downgrade(self));
            {
                let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                match opened {
                    Ok(bundle) => inner.bundle = Some(bundle),
                    Err(error) => {
                        Self::record_error(&mut inner, error.to_string());
                        self.begin_drain(&mut inner);
                    }
                }
            }
            self.changed.notify_waiters();
            #[cfg(test)]
            {
                let gate = self
                    .initialization_gate
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take();
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
            }
            let initialization = {
                let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                MemoryOperation::for_initialization(self, &mut inner)
            };
            if let Some((operation, bundle)) = initialization {
                if let Err(error) = operation
                    .scope(bundle.initialize(
                        &self.config,
                        self.services_config.get().and_then(Option::as_ref),
                        self,
                    ))
                    .await
                {
                    let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                    Self::record_error(&mut inner, error.to_string());
                    self.begin_drain(&mut inner);
                }
            }
            let publish = || {
                let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                let maintenance = MemoryOperation::for_initialization(self, &mut inner);
                if let Some((operation, bundle)) = &maintenance {
                    let reference = Reference {
                        id: resource_id(),
                        visibility: VisibilityScope::default(),
                    };
                    let mounted = self
                        .capture_binding(&reference)
                        .map_err(|error| {
                            AlephError::other(format!("memory resource binding failed: {error:?}"))
                        })
                        .and_then(|binding| bundle.mount(self, operation).map(|()| binding));
                    match mounted {
                        Ok(binding) => {
                            inner.status.state = MemoryState::Running;
                            let _ = self.provider.transition(ProviderState::Ready);
                            inner.source_observation =
                                self.observations.get().and_then(|factory| {
                                    self.descriptor(&VisibilityScope::default())
                                        .ok()
                                        .map(|descriptor| (binding, factory.system(descriptor)))
                                });
                            if let Some((_, observation)) = &inner.source_observation {
                                observation.mark_committed();
                            }
                        }
                        Err(error) => {
                            Self::record_error(&mut inner, error.to_string());
                            self.begin_drain(&mut inner);
                        }
                    }
                }
                if inner.status.state == MemoryState::Draining {
                    if let Some(bundle) = &inner.bundle {
                        bundle.request_stop();
                    }
                }
                self.changed.notify_waiters();
                maintenance
            };
            let (maintenance, publication) = if let Some(factory) = self.observations.get() {
                match factory
                    .policy
                    .with_source_publication(&resource_id(), publish)
                {
                    Ok((maintenance, running)) => (maintenance, Some(running)),
                    Err(_) => {
                        let mut inner =
                            self.inner.lock().unwrap_or_else(|error| error.into_inner());
                        Self::record_error(&mut inner, "memory source publication stopped".into());
                        self.begin_drain(&mut inner);
                        (None, None)
                    }
                }
            } else {
                (publish(), None)
            };
            let observation = self
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .source_observation
                .as_ref()
                .map(|(_, observation)| observation.clone());
            if let Some(observation) = observation {
                observation.flush();
            }
            drop(publication);
            // The last permit may lock the source on Drop; release it outside
            // the source gate after every spawned task has a retained handle.
            drop(maintenance);
        }

        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let draining = {
                let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                (inner.status.state == MemoryState::Draining)
                    .then(|| inner.bundle.clone())
                    .flatten()
            };
            if let Some(bundle) = draining {
                if let Err(error) = bundle.drain_watcher().await {
                    let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                    Self::record_error(&mut inner, error.to_string());
                }
            }
            let ready_to_close = {
                let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                inner.status.state == MemoryState::Draining && inner.admitted == 0
            };
            if ready_to_close && self.tasks.is_empty() {
                let bundle = self
                    .inner
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .bundle
                    .clone();
                let result = bundle.as_ref().map_or(Ok(()), |bundle| bundle.close());
                let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                match result {
                    Ok(()) => {
                        inner.bundle = None;
                        inner.status.state = if inner.rotation_failed {
                            MemoryState::Failed
                        } else {
                            MemoryState::Off
                        };
                        let _ = if inner.rotation_failed {
                            self.provider.transition(ProviderState::Failed)
                        } else {
                            self.provider.transition(ProviderState::Stopped)
                        };
                    }
                    Err(error) => {
                        Self::record_error(&mut inner, error.to_string());
                        inner.status.state = MemoryState::Failed;
                        let _ = self.provider.transition(ProviderState::Failed);
                    }
                }
                let observation = inner
                    .source_observation
                    .take()
                    .map(|(_, observation)| observation);
                if let Some(observation) = &observation {
                    observation.mark_withdrawn();
                }
                drop(inner);
                if let Some(observation) = observation {
                    observation.flush();
                }
                self.changed.notify_waiters();
                return;
            }
            tokio::select! {
                observation = futures::future::poll_fn(|cx| self.tasks.poll_completed(cx)) => {
                    if let Err(error) = observation {
                        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
                        Self::record_error(&mut inner, format!("memory task failed: {error}"));
                        self.begin_drain(&mut inner);
                    }
                    self.changed.notify_waiters();
                }
                _ = &mut changed => {},
            }
        }
    }

    fn poll_driver(&self, cx: &mut Context<'_>) -> Poll<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let Some(handle) = inner.driver.as_mut() else {
            return Poll::Pending;
        };
        let Poll::Ready(result) = Pin::new(handle).poll(cx) else {
            return Poll::Pending;
        };
        inner.driver = None;
        if let Err(error) = result {
            Self::record_error(&mut inner, format!("memory lifecycle task failed: {error}"));
            inner.status.state = MemoryState::Failed;
        }
        self.changed.notify_waiters();
        Poll::Ready(())
    }

    fn record_error(inner: &mut MemoryInner, error: String) {
        inner.status.first_error.get_or_insert(error);
    }

    fn failure(inner: &MemoryInner) -> AlephError {
        AlephError::other(
            inner
                .status
                .first_error
                .clone()
                .unwrap_or_else(|| "memory startup did not reach Running".into()),
        )
    }
}

struct DriverCompletion(std::sync::Weak<MemoryRuntime>);

impl Drop for DriverCompletion {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.upgrade() {
            runtime.changed.notify_waiters();
        }
    }
}

pub(crate) struct MemoryActivator {
    runtime: std::sync::Weak<MemoryRuntime>,
}

impl MemoryActivator {
    pub(crate) fn new(runtime: &Arc<MemoryRuntime>) -> Self {
        Self {
            runtime: Arc::downgrade(runtime),
        }
    }
}

impl ProviderActivator for MemoryActivator {
    fn activate(&self) -> Result<(), ActivationError> {
        let runtime = self.runtime.upgrade().ok_or(ActivationError::Failed)?;
        let state = runtime.status().state;
        match state {
            MemoryState::Running => Ok(()),
            _ => Err(ActivationError::Rejected),
        }
    }
}
