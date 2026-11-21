use aleph_client::DeviceCredentialStore;
use aleph_protocol::native_credentials::{DeviceConnectCredential, PanelCredentialLease};
use aleph_protocol::panel_shell::{CredentialCall, CredentialError, CredentialReply};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio::sync::{oneshot, Semaphore};

trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<DeviceConnectCredential>, CredentialError>;
    fn save(&self, credential: &DeviceConnectCredential) -> Result<(), CredentialError>;
    fn delete(&self) -> Result<bool, CredentialError>;
}

// Storage and transport policy remain owned by the canonical client store.
struct GatewayStore {
    gateway: String,
    namespace: Result<Option<String>, CredentialError>,
}

impl GatewayStore {
    fn open(&self) -> Result<DeviceCredentialStore, CredentialError> {
        let namespace = self.namespace.as_ref().map_err(|error| *error)?;
        match namespace {
            None => DeviceCredentialStore::for_gateway(&self.gateway)
                .map_err(|_| CredentialError::Unavailable),
            #[cfg(all(debug_assertions, feature = "qa-service"))]
            Some(service) => DeviceCredentialStore::for_gateway_in_service(&self.gateway, service)
                .map_err(|_| CredentialError::Unavailable),
            #[cfg(not(all(debug_assertions, feature = "qa-service")))]
            Some(_) => Err(CredentialError::Unavailable),
        }
    }
}

impl CredentialStore for GatewayStore {
    fn load(&self) -> Result<Option<DeviceConnectCredential>, CredentialError> {
        self.open()?
            .load_for_connect()
            .map_err(|_| CredentialError::Unavailable)
    }
    fn save(&self, credential: &DeviceConnectCredential) -> Result<(), CredentialError> {
        self.open()?
            .save_for_connect(credential)
            .map_err(|_| CredentialError::Unavailable)
    }
    fn delete(&self) -> Result<bool, CredentialError> {
        self.open()?
            .delete_for_connect()
            .map_err(|_| CredentialError::Unavailable)
    }
}

fn qa_namespace(nonce: Option<&str>) -> Result<Option<String>, CredentialError> {
    let Some(_nonce) = nonce else {
        return Ok(None);
    };
    #[cfg(all(debug_assertions, feature = "qa-service"))]
    {
        DeviceCredentialStore::qa_service_for_nonce(_nonce)
            .map(Some)
            .map_err(|_| CredentialError::Unavailable)
    }
    #[cfg(not(all(debug_assertions, feature = "qa-service")))]
    Err(CredentialError::Unavailable)
}

#[derive(Default)]
struct LeaseState {
    next: u64,
    active: Option<(u64, u64)>,
}

struct Inner {
    gateway: String,
    store: Arc<dyn CredentialStore>,
    generation: AtomicU64,
    lease: Mutex<LeaseState>,
    worker: Arc<Semaphore>,
}

#[derive(Clone)]
pub(crate) struct CredentialService(Arc<Inner>);

impl CredentialService {
    pub(crate) fn new(gateway: &str, qa_nonce: Option<&str>) -> Self {
        // HostSettings owns admission and WS/WSS derivation for this immutable target.
        let gateway = gateway.to_owned();
        Self::with_backend(
            gateway.clone(),
            Arc::new(GatewayStore {
                gateway,
                namespace: qa_namespace(qa_nonce),
            }),
        )
    }

    fn with_backend(gateway: String, store: Arc<dyn CredentialStore>) -> Self {
        Self(Arc::new(Inner {
            gateway,
            store,
            generation: AtomicU64::new(0),
            lease: Mutex::new(LeaseState::default()),
            worker: Arc::new(Semaphore::new(1)),
        }))
    }

    #[cfg(test)]
    fn with_store(store: Arc<dyn CredentialStore>) -> Self {
        Self::with_backend("ws://127.0.0.1:18790/ws".to_owned(), store)
    }

    pub(crate) fn invalidate(&self) {
        // Never take the OS-I/O gate on the UI thread. In-flight effects cannot be
        // undone, but their reply and every later use of their lease are revoked.
        self.0.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::SeqCst)
    }

    pub(crate) async fn invoke(&self, generation: u64, call: CredentialCall) -> CredentialReply {
        let Ok(permit) = self.0.worker.clone().try_acquire_owned() else {
            return CredentialReply::Failed {
                error: CredentialError::Unavailable,
            };
        };
        let service = self.clone();
        let (send, receive) = oneshot::channel();
        // NSH does not promise an entered Tokio runtime. The permit stays with
        // the real worker even when its UI future or document disappears.
        if std::thread::Builder::new()
            .name("panel-credentials".to_owned())
            .spawn(move || {
                let _permit = permit;
                let _ = send.send(service.invoke_blocking(generation, call));
            })
            .is_err()
        {
            return CredentialReply::Failed {
                error: CredentialError::Unavailable,
            };
        }
        receive.await.unwrap_or(CredentialReply::Failed {
            error: CredentialError::Unavailable,
        })
    }

    fn invoke_blocking(&self, generation: u64, call: CredentialCall) -> CredentialReply {
        let run = || {
            let mut state = self
                .0
                .lease
                .lock()
                .map_err(|_| CredentialError::Unavailable)?;
            if generation != self.generation() {
                return Err(CredentialError::Rejected);
            }
            let reply = match call {
                CredentialCall::Load {} => {
                    state.active = None;
                    let credential = self.0.store.load()?;
                    state.next = state
                        .next
                        .checked_add(1)
                        .ok_or(CredentialError::Unavailable)?;
                    state.active = Some((generation, state.next));
                    CredentialReply::Loaded {
                        lease: PanelCredentialLease {
                            gateway_url: self.0.gateway.clone(),
                            lease_id: state.next,
                            credential,
                        },
                    }
                }
                CredentialCall::Save {
                    lease_id,
                    credential,
                } => {
                    if state.active != Some((generation, lease_id)) {
                        return Err(CredentialError::Rejected);
                    }
                    self.0.store.save(&credential)?;
                    CredentialReply::Saved {}
                }
                CredentialCall::Delete { lease_id } => {
                    if state.active != Some((generation, lease_id)) {
                        return Err(CredentialError::Rejected);
                    }
                    CredentialReply::Deleted {
                        existed: self.0.store.delete()?,
                    }
                }
            };
            if generation != self.generation() {
                state.active = None;
                return Err(CredentialError::Rejected);
            }
            Ok(reply)
        };
        run().unwrap_or_else(|error| CredentialReply::Failed { error })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::native_credentials::DeviceConnectCredential;
    use std::sync::{Condvar, Mutex};

    #[cfg(not(all(debug_assertions, feature = "qa-service")))]
    #[test]
    fn qa_without_isolated_store_feature_never_selects_production_namespace() {
        assert_eq!(
            qa_namespace(Some(&"a".repeat(64))),
            Err(CredentialError::Unavailable)
        );
        assert_eq!(qa_namespace(None), Ok(None));
    }

    #[cfg(all(debug_assertions, feature = "qa-service"))]
    #[test]
    fn isolated_namespace_is_derived_by_the_existing_store_owner() {
        assert!(qa_namespace(Some("invalid")).is_err());
        let namespace = qa_namespace(Some(&"a".repeat(64))).unwrap().unwrap();
        assert_eq!(
            namespace,
            DeviceCredentialStore::qa_service_for_nonce(&"a".repeat(64)).unwrap()
        );
        assert_eq!(qa_namespace(None), Ok(None));
    }

    #[cfg(all(debug_assertions, feature = "qa-service"))]
    #[test]
    #[ignore = "real OS keyring; unique QA namespace only, never production credentials"]
    fn isolated_os_store_roundtrip_and_revoked_lease() {
        let nonce = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let store = Arc::new(GatewayStore {
            gateway: "ws://127.0.0.1:18790/ws".to_owned(),
            namespace: qa_namespace(Some(&nonce)),
        });
        struct Cleanup(Arc<GatewayStore>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete();
            }
        }
        let _cleanup = Cleanup(store.clone());
        assert!(store.load().unwrap().is_none());
        let service = CredentialService::with_backend(store.gateway.clone(), store.clone());
        let id = load(&service);
        assert_eq!(
            block_on(service.invoke(
                service.generation(),
                CredentialCall::Save {
                    lease_id: id,
                    credential: credential(),
                }
            )),
            CredentialReply::Saved {}
        );
        assert_eq!(store.load().unwrap(), Some(credential()));
        service.invalidate();
        assert!(matches!(
            block_on(service.invoke(
                service.generation(),
                CredentialCall::Delete { lease_id: id }
            )),
            CredentialReply::Failed {
                error: CredentialError::Rejected
            }
        ));
        assert_eq!(store.load().unwrap(), Some(credential()));
        let next = load(&service);
        assert_eq!(
            block_on(service.invoke(
                service.generation(),
                CredentialCall::Delete { lease_id: next }
            )),
            CredentialReply::Deleted { existed: true }
        );
        assert!(store.load().unwrap().is_none());
    }

    #[derive(Default)]
    struct MemoryStore {
        value: Mutex<Option<DeviceConnectCredential>>,
        effects: AtomicU64,
        block_load: Option<Arc<(Mutex<bool>, Condvar)>>,
    }

    impl CredentialStore for MemoryStore {
        fn load(&self) -> Result<Option<DeviceConnectCredential>, CredentialError> {
            if let Some(gate) = &self.block_load {
                let mut ready = gate.0.lock().unwrap();
                self.effects.fetch_add(1, Ordering::SeqCst);
                gate.1.notify_all();
                while !*ready {
                    ready = gate.1.wait(ready).unwrap();
                }
            }
            Ok(self.value.lock().unwrap().clone())
        }
        fn save(&self, credential: &DeviceConnectCredential) -> Result<(), CredentialError> {
            self.effects.fetch_add(1, Ordering::SeqCst);
            *self.value.lock().unwrap() = Some(credential.clone());
            Ok(())
        }
        fn delete(&self) -> Result<bool, CredentialError> {
            self.effects.fetch_add(1, Ordering::SeqCst);
            Ok(self.value.lock().unwrap().take().is_some())
        }
    }

    fn service(store: Arc<MemoryStore>) -> CredentialService {
        CredentialService::with_store(store)
    }
    fn load(service: &CredentialService) -> u64 {
        match service.invoke_blocking(service.generation(), CredentialCall::Load {}) {
            CredentialReply::Loaded { lease } => lease.lease_id,
            other => panic!("expected lease, got {other:?}"),
        }
    }
    fn credential() -> DeviceConnectCredential {
        DeviceConnectCredential::new("device", "test-token").unwrap()
    }

    #[test]
    fn load_save_delete_reuses_the_same_lease_and_real_effects() {
        let store = Arc::new(MemoryStore::default());
        let service = service(store.clone());
        let id = load(&service);
        assert_eq!(
            service.invoke_blocking(
                service.generation(),
                CredentialCall::Save {
                    lease_id: id,
                    credential: credential()
                }
            ),
            CredentialReply::Saved {}
        );
        assert_eq!(*store.value.lock().unwrap(), Some(credential()));
        assert_eq!(
            service.invoke_blocking(
                service.generation(),
                CredentialCall::Delete { lease_id: id }
            ),
            CredentialReply::Deleted { existed: true }
        );
        assert_eq!(*store.value.lock().unwrap(), None);
    }

    #[test]
    fn navigation_revokes_old_lease_before_any_store_effect() {
        let store = Arc::new(MemoryStore::default());
        let service = service(store.clone());
        let generation = service.generation();
        let id = load(&service);
        service.invalidate();
        for epoch in [generation, service.generation()] {
            assert!(matches!(
                service.invoke_blocking(
                    epoch,
                    CredentialCall::Save {
                        lease_id: id,
                        credential: credential()
                    }
                ),
                CredentialReply::Failed { .. }
            ));
        }
        assert_eq!(store.effects.load(Ordering::SeqCst), 0);
        let next = load(&service);
        assert_ne!(next, id);
    }

    #[test]
    fn a_new_load_supersedes_old_lease_and_unknown_ids_have_no_effect() {
        let store = Arc::new(MemoryStore::default());
        let service = service(store.clone());
        let old = load(&service);
        let new = load(&service);
        for id in [0, old, new + 1] {
            assert!(matches!(
                service.invoke_blocking(
                    service.generation(),
                    CredentialCall::Delete { lease_id: id }
                ),
                CredentialReply::Failed { .. }
            ));
        }
        assert_eq!(store.effects.load(Ordering::SeqCst), 0);
        assert_eq!(
            service.invoke_blocking(
                service.generation(),
                CredentialCall::Delete { lease_id: new }
            ),
            CredentialReply::Deleted { existed: false }
        );
    }

    #[test]
    fn navigation_while_keyring_is_blocked_withholds_result_without_ui_lock() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let store = Arc::new(MemoryStore {
            block_load: Some(gate.clone()),
            ..Default::default()
        });
        let service = service(store.clone());
        let worker = service.clone();
        let generation = service.generation();
        let thread =
            std::thread::spawn(move || worker.invoke_blocking(generation, CredentialCall::Load {}));
        let mut ready = gate.0.lock().unwrap();
        while store.effects.load(Ordering::SeqCst) == 0 {
            ready = gate.1.wait(ready).unwrap();
        }
        service.invalidate();
        *ready = true;
        gate.1.notify_all();
        drop(ready);
        assert!(matches!(
            thread.join().unwrap(),
            CredentialReply::Failed { .. }
        ));
    }

    #[test]
    fn blocked_worker_rejects_overlap_and_keeps_its_permit_until_real_exit() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let store = Arc::new(MemoryStore {
            block_load: Some(gate.clone()),
            ..Default::default()
        });
        let service = service(store.clone());
        let worker = service.clone();
        let thread = std::thread::spawn(move || {
            block_on(worker.invoke(worker.generation(), CredentialCall::Load {}))
        });
        let mut ready = gate.0.lock().unwrap();
        while store.effects.load(Ordering::SeqCst) == 0 {
            let wait = gate
                .1
                .wait_timeout(ready, std::time::Duration::from_secs(2))
                .unwrap();
            assert!(!wait.1.timed_out(), "credential worker did not enter store");
            ready = wait.0;
        }
        service.invalidate();
        assert_eq!(
            block_on(service.invoke(service.generation(), CredentialCall::Load {})),
            CredentialReply::Failed {
                error: CredentialError::Unavailable
            },
        );
        assert_eq!(store.effects.load(Ordering::SeqCst), 1);
        *ready = true;
        gate.1.notify_all();
        drop(ready);
        assert_eq!(
            thread.join().unwrap(),
            CredentialReply::Failed {
                error: CredentialError::Rejected
            }
        );
        assert!(matches!(
            block_on(service.invoke(service.generation(), CredentialCall::Load {})),
            CredentialReply::Loaded { .. }
        ));
    }

    #[test]
    fn async_io_does_not_require_a_tokio_runtime() {
        let service = service(Arc::new(MemoryStore::default()));
        assert!(matches!(
            block_on(service.invoke(service.generation(), CredentialCall::Load {})),
            CredentialReply::Loaded { .. }
        ));
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        struct Wake(std::thread::Thread);
        impl std::task::Wake for Wake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::task::Waker::from(Arc::new(Wake(std::thread::current())));
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::thread::park(),
            }
        }
    }
}
