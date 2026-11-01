use std::future::Future;
use std::sync::Weak;

use crate::capability::facade::ResolveError;
use crate::capability::ownership::CapturedBinding;
use crate::error::AlephError;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::sync_primitives::Arc;
use tokio::sync::oneshot;

use super::bundle::MemoryBundle;
use super::{MemoryInner, MemoryRuntime, MemoryState};

tokio::task_local! {
    static CURRENT_OPERATION: MemoryOperation;
    static OWNED_OPERATION: MemoryOperation;
}

#[derive(Clone)]
pub struct MemoryHandle {
    pub(crate) runtime: Weak<MemoryRuntime>,
    pub(crate) binding: CapturedBinding,
}

impl MemoryHandle {
    pub fn admit(&self) -> Result<MemoryOperation, ResolveError> {
        self.with_admission(|admission| {
            let runtime = admission.runtime.clone();
            runtime
                .ownership
                .with_current_bindings(std::slice::from_ref(&self.binding), || admission.commit())
                .map_err(|_| ResolveError::StaleOwner)
        })
    }

    /// The caller adds its canonical Tool gate and captured bindings here.
    /// The source gate stays held until that synchronous commit returns.
    pub(crate) fn with_admission<R>(
        &self,
        commit: impl FnOnce(&mut MemoryAdmission<'_>) -> Result<R, ResolveError>,
    ) -> Result<R, ResolveError> {
        let runtime = self.runtime.upgrade().ok_or(ResolveError::Unknown)?;
        let mut inner = runtime
            .inner
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if inner.status.state != MemoryState::Running {
            return Err(ResolveError::Unknown);
        }
        let bundle = inner.bundle.clone().ok_or(ResolveError::Unknown)?;
        commit(&mut MemoryAdmission {
            runtime: &runtime,
            inner: &mut inner,
            bundle,
        })
    }
}

pub(crate) struct MemoryAdmission<'a> {
    runtime: &'a Arc<MemoryRuntime>,
    inner: &'a mut MemoryInner,
    bundle: Arc<MemoryBundle>,
}

impl MemoryAdmission<'_> {
    pub(crate) fn commit(&mut self) -> MemoryOperation {
        self.inner.admitted += 1;
        MemoryOperation {
            inner: Arc::new(MemoryOperationInner {
                runtime: Arc::downgrade(self.runtime),
                bundle: self.bundle.clone(),
            }),
        }
    }
}

#[derive(Clone)]
pub struct MemoryOperation {
    inner: Arc<MemoryOperationInner>,
}

struct MemoryOperationInner {
    runtime: Weak<MemoryRuntime>,
    bundle: Arc<MemoryBundle>,
}

impl Drop for MemoryOperationInner {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .admitted -= 1;
            runtime.changed.notify_waiters();
        }
    }
}

impl MemoryOperation {
    pub(super) fn for_initialization(
        runtime: &Arc<MemoryRuntime>,
        inner: &mut MemoryInner,
    ) -> Option<(Self, Arc<MemoryBundle>)> {
        if inner.status.state != MemoryState::Starting {
            return None;
        }
        let bundle = inner.bundle.clone()?;
        let operation = MemoryAdmission {
            runtime,
            inner,
            bundle: bundle.clone(),
        }
        .commit();
        Some((operation, bundle))
    }

    pub async fn scope<F: Future>(self, future: F) -> F::Output {
        CURRENT_OPERATION.scope(self, future).await
    }

    pub(crate) fn current() -> Option<Self> {
        CURRENT_OPERATION.try_with(Clone::clone).ok()
    }

    pub(crate) fn matches_backend(
        &self,
        owner: &Weak<MemoryRuntime>,
        backend: &SqliteMemoryBackend,
    ) -> bool {
        Weak::ptr_eq(owner, &self.inner.runtime) && std::ptr::eq(backend, self.backend().as_ref())
    }

    pub(crate) fn belongs_to(&self, runtime: &Arc<MemoryRuntime>) -> bool {
        Weak::ptr_eq(&self.inner.runtime, &Arc::downgrade(runtime))
    }

    pub(crate) fn services(&self) -> Result<Arc<super::services::MemoryServices>, AlephError> {
        self.inner.bundle.services()
    }

    /// Own a root operation before the caller starts waiting for its result.
    pub(crate) async fn run<F, R>(&self, future: F) -> Result<R, AlephError>
    where
        F: Future<Output = Result<R, AlephError>> + Send + 'static,
        R: Send + 'static,
    {
        // A nested call already belongs to this retained task. Keep its original
        // sealed dispatch proof rather than creating a new task without it.
        if OWNED_OPERATION
            .try_with(|owned| Arc::ptr_eq(&owned.inner, &self.inner))
            .unwrap_or(false)
        {
            return self.clone().scope(future).await;
        }
        let (sender, receiver) = oneshot::channel();
        self.spawn(super::carry(async move {
            let _ = sender.send(future.await);
        }))?;
        receiver
            .await
            .map_err(|_| AlephError::other("memory task ended without a result"))?
    }

    pub(crate) fn search_settings(
        &self,
    ) -> Result<super::search::MemorySearchSettings, AlephError> {
        self.inner
            .runtime
            .upgrade()
            .map(|runtime| runtime.search_settings.clone())
            .ok_or_else(|| AlephError::other("memory owner is gone"))
    }

    pub(crate) fn backend(&self) -> &Arc<SqliteMemoryBackend> {
        &self.inner.bundle.backend
    }

    pub fn spawn<F>(&self, future: F) -> Result<(), AlephError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let runtime = self
            .inner
            .runtime
            .upgrade()
            .ok_or_else(|| AlephError::other("memory owner is gone"))?;
        runtime.tasks.retain(tokio::spawn(
            OWNED_OPERATION.scope(self.clone(), self.clone().scope(future)),
        ));
        runtime.changed.notify_waiters();
        Ok(())
    }

    pub async fn run_blocking<F, R>(&self, work: F) -> Result<R, AlephError>
    where
        F: FnOnce() -> Result<R, AlephError> + Send + 'static,
        R: Send + 'static,
    {
        let runtime = self
            .inner
            .runtime
            .upgrade()
            .ok_or_else(|| AlephError::other("memory owner is gone"))?;
        let operation = self.clone();
        let completion = self.clone();
        let (result, receiver) = oneshot::channel();
        runtime.tasks.retain(tokio::task::spawn_blocking(move || {
            // Keep the admission alive through result delivery and closure teardown.
            let _completion = completion;
            let _ = result.send(CURRENT_OPERATION.sync_scope(operation, work));
        }));
        runtime.changed.notify_waiters();
        receiver
            .await
            .map_err(|_| AlephError::other("memory blocking task ended without a result"))?
    }
}
