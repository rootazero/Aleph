//! Browser-local loading. The shared host registry is a descriptor projection only.

use super::{registry::TabRegistry, sdk::PanelSdk, PanelHostState};
use aleph_protocol::panel::{PanelTabDescriptor, PanelTabKey, PanelTabStatus, PANEL_UI_API_MAJOR};
use futures::{future::LocalBoxFuture, FutureExt};
use leptos::prelude::{GetUntracked, Set};
use std::{cell::RefCell, fmt, rc::Rc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelLoadError {
    ImportFailed,
    InvalidDescriptor,
    UnsupportedApiMajor,
    MountFailed,
    UnmountFailed,
    Cancelled,
}

impl fmt::Display for PanelLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PanelLoadError {}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum PanelLoadStatus {
    #[default]
    Idle,
    Loading {
        key: PanelTabKey,
    },
    Mounted {
        key: PanelTabKey,
    },
    Failed {
        key: PanelTabKey,
        error: PanelLoadError,
    },
}

struct LoadedModule {
    descriptor: serde_json::Value,
    mount: Box<dyn FnOnce(PanelSdk) -> Result<MountedTab, PanelLoadError>>,
}

struct MountedTab {
    unmount: Option<Box<dyn FnOnce() -> Result<(), PanelLoadError>>>,
}

impl MountedTab {
    #[cfg(any(test, target_arch = "wasm32"))]
    fn new(unmount: impl FnOnce() -> Result<(), PanelLoadError> + 'static) -> Self {
        Self {
            unmount: Some(Box::new(unmount)),
        }
    }

    fn dispose(&mut self) -> Result<(), PanelLoadError> {
        self.unmount.take().map_or(Ok(()), |unmount| unmount())
    }
}

impl Drop for MountedTab {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

struct Resources {
    sdk: PanelSdk,
    mounted: Option<MountedTab>,
    cleanup: Option<Box<dyn FnOnce()>>,
}

impl Resources {
    fn dispose(&mut self) -> Result<(), PanelLoadError> {
        let result = self
            .mounted
            .take()
            .map_or(Ok(()), |mut handle| handle.dispose());
        self.sdk.dispose();
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
        result
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

#[derive(Default)]
struct LoadState {
    generation: u64,
    descriptor: Option<PanelTabDescriptor>,
    status: PanelLoadStatus,
    resources: Option<Resources>,
}

pub struct PanelLoader {
    host: PanelHostState,
    state: Rc<RefCell<LoadState>>,
}

impl PanelLoader {
    pub fn new(registry: TabRegistry) -> Self {
        Self {
            host: PanelHostState::new(registry),
            state: Rc::new(RefCell::new(LoadState::default())),
        }
    }

    pub fn host(&self) -> PanelHostState {
        self.host.clone()
    }

    pub fn status(&self) -> PanelLoadStatus {
        self.state.borrow().status.clone()
    }

    pub fn deactivate(&mut self) -> Result<(), PanelLoadError> {
        let (key, resources) = {
            let mut state = self.state.borrow_mut();
            state.generation = state
                .generation
                .checked_add(1)
                .expect("activation generation exhausted");
            let key = state.descriptor.take().map(|descriptor| descriptor.key);
            (key, state.resources.take())
        };
        let result = resources.map_or(Ok(()), |mut resources| resources.dispose());
        self.state.borrow_mut().status = match (key, result) {
            (Some(key), Err(error)) => PanelLoadStatus::Failed { key, error },
            _ => PanelLoadStatus::Idle,
        };
        result
    }

    /// Reconcile after changing the host projection. Revocation never invokes
    /// provider/session lifecycle operations; it only releases this UI.
    pub fn reconcile(&mut self) -> Result<(), PanelLoadError> {
        let valid = self
            .state
            .borrow()
            .descriptor
            .as_ref()
            .is_none_or(|descriptor| {
                self.host.registry.lock().unwrap().get(&descriptor.key) == Some(descriptor)
                    && descriptor.status == PanelTabStatus::Enabled
            });
        if !valid {
            return self.deactivate();
        }
        Ok(())
    }

    pub fn activate(
        &mut self,
        key: PanelTabKey,
        sidebar_root: web_sys::Element,
        content_root: web_sys::Element,
        sdk: PanelSdk,
    ) -> LocalBoxFuture<'static, Result<(), PanelLoadError>> {
        #[cfg(target_arch = "wasm32")]
        {
            let sidebar = sidebar_root.clone();
            let content = content_root.clone();
            self.activate_inner(
                key,
                sdk,
                move |descriptor| browser::load(descriptor, sidebar_root, content_root),
                Some(Box::new(move || {
                    sidebar.set_text_content(None);
                    content.set_text_content(None);
                })),
            )
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (sidebar_root, content_root);
            self.activate_inner(
                key,
                sdk,
                |_| async { Err(PanelLoadError::ImportFailed) }.boxed_local(),
                None,
            )
        }
    }

    #[cfg(test)]
    fn activate_with(
        &mut self,
        key: PanelTabKey,
        sdk: PanelSdk,
        import: impl FnOnce(
                PanelTabDescriptor,
            ) -> LocalBoxFuture<'static, Result<LoadedModule, PanelLoadError>>
            + 'static,
    ) -> LocalBoxFuture<'static, Result<(), PanelLoadError>> {
        self.activate_inner(key, sdk, import, None)
    }

    fn activate_inner(
        &mut self,
        key: PanelTabKey,
        sdk: PanelSdk,
        import: impl FnOnce(
                PanelTabDescriptor,
            ) -> LocalBoxFuture<'static, Result<LoadedModule, PanelLoadError>>
            + 'static,
        cleanup: Option<Box<dyn FnOnce()>>,
    ) -> LocalBoxFuture<'static, Result<(), PanelLoadError>> {
        // Invalidate synchronously, before the future can be polled. Same-key
        // retries are different attempts even though their registry keys match.
        let previous = self.deactivate();
        let mut resources = Resources {
            sdk,
            mounted: None,
            cleanup,
        };
        let descriptor = self.host.registry.lock().unwrap().get(&key).cloned();
        let error = previous.err().or_else(|| match descriptor.as_ref() {
            None => Some(PanelLoadError::InvalidDescriptor),
            Some(descriptor) if descriptor.status != PanelTabStatus::Enabled => {
                Some(PanelLoadError::Cancelled)
            }
            Some(descriptor) if descriptor.api_major != PANEL_UI_API_MAJOR => {
                Some(PanelLoadError::UnsupportedApiMajor)
            }
            _ => None,
        });
        if let Some(error) = error {
            let _ = resources.dispose();
            self.state.borrow_mut().status = PanelLoadStatus::Failed { key, error };
            return async move { Err(error) }.boxed_local();
        }
        let descriptor = descriptor.expect("validated descriptor");
        if self.host.active.get_untracked().as_ref() != Some(&key) {
            self.host.active.set(Some(key.clone()));
        }
        let generation = {
            let mut state = self.state.borrow_mut();
            state.descriptor = Some(descriptor.clone());
            state.status = PanelLoadStatus::Loading { key: key.clone() };
            state.resources = Some(resources);
            state.generation
        };
        let state = self.state.clone();
        let host = self.host.clone();
        async move {
            if !current(&state, &host, generation, &descriptor) {
                cancel(&state, generation);
                return Err(PanelLoadError::Cancelled);
            }
            let result = import(descriptor.clone()).await;
            if !current(&state, &host, generation, &descriptor) {
                cancel(&state, generation);
                return Err(PanelLoadError::Cancelled);
            }
            let result = result.and_then(|module| {
                let actual: PanelTabDescriptor = serde_json::from_value(module.descriptor)
                    .map_err(|_| PanelLoadError::InvalidDescriptor)?;
                if actual.api_major != PANEL_UI_API_MAJOR {
                    return Err(PanelLoadError::UnsupportedApiMajor);
                }
                // Entry roots and permissions belong to the host projection,
                // not the module. describe supplies ABI identity, never grants.
                if actual.key != descriptor.key
                    || actual.label.trim().is_empty()
                    || actual.entry.trim().is_empty()
                {
                    return Err(PanelLoadError::InvalidDescriptor);
                }
                let sdk = state
                    .borrow()
                    .resources
                    .as_ref()
                    .expect("current resources")
                    .sdk
                    .clone();
                (module.mount)(sdk)
            });
            // A synchronous mount may navigate via the SDK. Dispose that late
            // handle instead of publishing it as the newly selected tab.
            if !current(&state, &host, generation, &descriptor) {
                drop(result);
                cancel(&state, generation);
                return Err(PanelLoadError::Cancelled);
            }
            match result {
                Ok(handle) => {
                    let mut state = state.borrow_mut();
                    state.resources.as_mut().expect("current resources").mounted = Some(handle);
                    state.status = PanelLoadStatus::Mounted { key };
                    Ok(())
                }
                Err(error) => {
                    let resources = state.borrow_mut().resources.take();
                    drop(resources);
                    state.borrow_mut().status = PanelLoadStatus::Failed { key, error };
                    Err(error)
                }
            }
        }
        .boxed_local()
    }
}

impl Drop for PanelLoader {
    fn drop(&mut self) {
        let _ = self.deactivate();
    }
}

fn current(
    state: &RefCell<LoadState>,
    host: &PanelHostState,
    generation: u64,
    descriptor: &PanelTabDescriptor,
) -> bool {
    state.borrow().generation == generation
        && host.active.try_get_untracked().flatten().as_ref() == Some(&descriptor.key)
        && host.registry.lock().unwrap().get(&descriptor.key) == Some(descriptor)
}

fn cancel(state: &RefCell<LoadState>, generation: u64) {
    let resources = {
        let mut state = state.borrow_mut();
        if state.generation != generation {
            return;
        }
        state.descriptor = None;
        state.resources.take()
    };
    drop(resources);
    state.borrow_mut().status = PanelLoadStatus::Idle;
}

#[cfg(target_arch = "wasm32")]
mod browser;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::DashboardState;
    use aleph_protocol::panel::PanelUiTrust;
    use futures::{channel::oneshot, executor::block_on, FutureExt};
    use leptos::prelude::{provide_context, Owner};
    use serde_json::json;
    use std::cell::Cell;

    fn descriptor(id: &str) -> PanelTabDescriptor {
        PanelTabDescriptor {
            key: PanelTabKey {
                plugin_id: "test".into(),
                tab_id: id.into(),
            },
            label: id.into(),
            icon: "terminal".into(),
            order: 0,
            entry: format!("tabs/{id}.js"),
            api_major: PANEL_UI_API_MAJOR,
            trust: PanelUiTrust::Verified,
            permissions: vec![],
            status: PanelTabStatus::Enabled,
            status_detail: None,
        }
    }

    fn fixture() -> PanelLoader {
        let mut registry = TabRegistry::default();
        for id in ["one", "two"] {
            registry.insert_or_replace(descriptor(id)).unwrap();
        }
        let loader = PanelLoader::new(registry);
        provide_context(loader.host());
        loader
    }

    fn sdk(id: &str) -> PanelSdk {
        PanelSdk::new(DashboardState::new(), descriptor(id).key)
    }

    fn module(
        raw: serde_json::Value,
        events: Rc<RefCell<Vec<&'static str>>>,
        fail_unmount: bool,
    ) -> LoadedModule {
        LoadedModule {
            descriptor: raw,
            mount: Box::new(move |_| {
                events.borrow_mut().push("mount");
                Ok(MountedTab::new(move || {
                    events.borrow_mut().push("unmount");
                    if fail_unmount {
                        return Err(PanelLoadError::UnmountFailed);
                    }
                    Ok(())
                }))
            }),
        }
    }

    #[test]
    fn unsupported_major_and_malformed_descriptor_never_mount() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let events = Rc::new(RefCell::new(vec![]));
            let mut incompatible = descriptor("one");
            incompatible.api_major = 2;
            for (raw, error) in [
                (
                    serde_json::to_value(incompatible).unwrap(),
                    PanelLoadError::UnsupportedApiMajor,
                ),
                (json!({"key": null}), PanelLoadError::InvalidDescriptor),
                (
                    serde_json::to_value(descriptor("two")).unwrap(),
                    PanelLoadError::InvalidDescriptor,
                ),
            ] {
                let module = module(raw, events.clone(), false);
                assert_eq!(
                    block_on(
                        loader.activate_with(descriptor("one").key, sdk("one"), move |_| async {
                            Ok(module)
                        }
                        .boxed_local())
                    ),
                    Err(error)
                );
            }
            assert!(events.borrow().is_empty());
        });
    }

    #[test]
    fn import_failure_is_retryable_and_does_not_mutate_projection() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let before = loader.host().registry.lock().unwrap().ordered();
            assert_eq!(
                block_on(
                    loader.activate_with(descriptor("one").key, sdk("one"), |_| async {
                        Err(PanelLoadError::ImportFailed)
                    }
                    .boxed_local())
                ),
                Err(PanelLoadError::ImportFailed)
            );
            assert!(matches!(
                loader.status(),
                PanelLoadStatus::Failed {
                    error: PanelLoadError::ImportFailed,
                    ..
                }
            ));
            assert_eq!(loader.host().registry.lock().unwrap().ordered(), before);
            let events = Rc::new(RefCell::new(vec![]));
            let module = module(
                serde_json::to_value(descriptor("one")).unwrap(),
                events.clone(),
                false,
            );
            assert_eq!(
                block_on(
                    loader.activate_with(descriptor("one").key, sdk("one"), move |_| async {
                        Ok(module)
                    }
                    .boxed_local())
                ),
                Ok(())
            );
            assert_eq!(*events.borrow(), ["mount"]);
        });
    }

    #[test]
    fn navigation_unmounts_before_import_and_disposes_sdk() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let events = Rc::new(RefCell::new(vec![]));
            let old_sdk = sdk("one");
            let module = module(
                serde_json::to_value(descriptor("one")).unwrap(),
                events.clone(),
                false,
            );
            block_on(
                loader.activate_with(descriptor("one").key, old_sdk.clone(), move |_| {
                    async { Ok(module) }.boxed_local()
                }),
            )
            .unwrap();
            let next_events = events.clone();
            let next = loader.activate_with(descriptor("two").key, sdk("two"), move |_| {
                next_events.borrow_mut().push("import");
                async { Err(PanelLoadError::ImportFailed) }.boxed_local()
            });
            assert_eq!(*events.borrow(), ["mount", "unmount"]);
            assert_eq!(
                old_sdk.navigate(descriptor("two").key),
                Err(crate::panel::sdk::PanelSdkError::Disposed)
            );
            assert_eq!(block_on(next), Err(PanelLoadError::ImportFailed));
            assert_eq!(*events.borrow(), ["mount", "unmount", "import"]);
        });
    }

    #[test]
    fn deactivation_cancels_pending_completion_without_mounting() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let events = Rc::new(RefCell::new(vec![]));
            let (tx, rx) = oneshot::channel();
            let pending_sdk = sdk("one");
            let mut future =
                loader.activate_with(descriptor("one").key, pending_sdk.clone(), move |_| {
                    async { rx.await.unwrap() }.boxed_local()
                });
            assert!(future.as_mut().now_or_never().is_none());
            loader.deactivate().unwrap();
            tx.send(Ok(module(
                serde_json::to_value(descriptor("one")).unwrap(),
                events.clone(),
                false,
            )))
            .ok()
            .unwrap();
            assert_eq!(block_on(future), Err(PanelLoadError::Cancelled));
            assert!(events.borrow().is_empty());
            assert_eq!(loader.status(), PanelLoadStatus::Idle);
            assert_eq!(
                pending_sdk.navigate(descriptor("one").key),
                Err(crate::panel::sdk::PanelSdkError::Disposed)
            );
        });
    }

    #[test]
    fn same_key_retry_fences_old_success_and_old_failure() {
        Owner::new().with(|| {
            for fail in [false, true] {
                let mut loader = fixture();
                let events = Rc::new(RefCell::new(vec![]));
                let (tx, rx) = oneshot::channel();
                let mut old = loader.activate_with(descriptor("one").key, sdk("one"), move |_| {
                    async { rx.await.unwrap() }.boxed_local()
                });
                assert!(old.as_mut().now_or_never().is_none());
                let new_module = module(
                    serde_json::to_value(descriptor("one")).unwrap(),
                    events.clone(),
                    false,
                );
                block_on(
                    loader.activate_with(descriptor("one").key, sdk("one"), move |_| {
                        async { Ok(new_module) }.boxed_local()
                    }),
                )
                .unwrap();
                let result = if fail {
                    Err(PanelLoadError::ImportFailed)
                } else {
                    Ok(module(
                        serde_json::to_value(descriptor("one")).unwrap(),
                        events.clone(),
                        false,
                    ))
                };
                tx.send(result).ok().unwrap();
                assert_eq!(block_on(old), Err(PanelLoadError::Cancelled));
                assert!(matches!(loader.status(), PanelLoadStatus::Mounted { .. }));
                assert_eq!(*events.borrow(), ["mount"]);
                loader.deactivate().unwrap();
                assert_eq!(*events.borrow(), ["mount", "unmount"]);
            }
        });
    }

    #[test]
    fn disabled_then_removed_unmounts_exactly_once_even_on_error() {
        Owner::new().with(|| {
            for fail in [false, true] {
                let mut loader = fixture();
                let events = Rc::new(RefCell::new(vec![]));
                let mounted = module(
                    serde_json::to_value(descriptor("one")).unwrap(),
                    events.clone(),
                    fail,
                );
                block_on(
                    loader.activate_with(descriptor("one").key, sdk("one"), move |_| {
                        async { Ok(mounted) }.boxed_local()
                    }),
                )
                .unwrap();
                let mut disabled = descriptor("one");
                disabled.status = PanelTabStatus::Disabled;
                loader
                    .host()
                    .registry
                    .lock()
                    .unwrap()
                    .insert_or_replace(disabled)
                    .unwrap();
                assert_eq!(
                    loader.reconcile(),
                    if fail {
                        Err(PanelLoadError::UnmountFailed)
                    } else {
                        Ok(())
                    }
                );
                loader
                    .host()
                    .registry
                    .lock()
                    .unwrap()
                    .remove(&descriptor("one").key);
                loader.reconcile().unwrap();
                loader.deactivate().unwrap();
                drop(loader);
                assert_eq!(*events.borrow(), ["mount", "unmount"]);
            }
        });
    }

    #[test]
    fn disabled_or_replaced_pending_descriptor_is_cancelled() {
        Owner::new().with(|| {
            for remove in [false, true] {
                let mut loader = fixture();
                let events = Rc::new(RefCell::new(vec![]));
                let (tx, rx) = oneshot::channel();
                let mut future =
                    loader.activate_with(descriptor("one").key, sdk("one"), move |_| {
                        async { rx.await.unwrap() }.boxed_local()
                    });
                assert!(future.as_mut().now_or_never().is_none());
                if remove {
                    loader
                        .host()
                        .registry
                        .lock()
                        .unwrap()
                        .remove(&descriptor("one").key);
                } else {
                    let mut replacement = descriptor("one");
                    replacement.entry = "new.js".into();
                    loader
                        .host()
                        .registry
                        .lock()
                        .unwrap()
                        .insert_or_replace(replacement)
                        .unwrap();
                }
                tx.send(Ok(module(
                    serde_json::to_value(descriptor("one")).unwrap(),
                    events.clone(),
                    false,
                )))
                .ok()
                .unwrap();
                assert_eq!(block_on(future), Err(PanelLoadError::Cancelled));
                assert!(events.borrow().is_empty());
                assert_eq!(loader.status(), PanelLoadStatus::Idle);
            }
        });
    }

    #[test]
    fn unmount_failure_blocks_next_import_but_still_cleans_roots_and_sdk() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let events = Rc::new(RefCell::new(vec![]));
            let cleaned = Rc::new(Cell::new(0));
            let cleanup = cleaned.clone();
            let mounted_sdk = sdk("one");
            let mounted = module(
                serde_json::to_value(descriptor("one")).unwrap(),
                events.clone(),
                true,
            );
            block_on(loader.activate_inner(
                descriptor("one").key,
                mounted_sdk.clone(),
                move |_| async { Ok(mounted) }.boxed_local(),
                Some(Box::new(move || cleanup.set(cleanup.get() + 1))),
            ))
            .unwrap();
            let imported = Rc::new(Cell::new(false));
            let attempted = imported.clone();
            assert_eq!(
                block_on(
                    loader.activate_with(descriptor("two").key, sdk("two"), move |_| {
                        attempted.set(true);
                        async { Err(PanelLoadError::ImportFailed) }.boxed_local()
                    })
                ),
                Err(PanelLoadError::UnmountFailed)
            );
            assert!(!imported.get());
            assert_eq!(cleaned.get(), 1);
            assert_eq!(
                mounted_sdk.navigate(descriptor("two").key),
                Err(crate::panel::sdk::PanelSdkError::Disposed)
            );
            loader.deactivate().unwrap();
            assert_eq!(*events.borrow(), ["mount", "unmount"]);
        });
    }

    #[test]
    fn navigation_during_mount_disposes_late_handle_and_sdk() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let events = Rc::new(RefCell::new(vec![]));
            let recorded = events.clone();
            let current_sdk = sdk("one");
            let module = LoadedModule {
                descriptor: serde_json::to_value(descriptor("one")).unwrap(),
                mount: Box::new(move |sdk| {
                    sdk.navigate(descriptor("two").key).unwrap();
                    Ok(MountedTab::new(move || {
                        recorded.borrow_mut().push("unmount");
                        Ok(())
                    }))
                }),
            };
            assert_eq!(
                block_on(loader.activate_with(
                    descriptor("one").key,
                    current_sdk.clone(),
                    move |_| async { Ok(module) }.boxed_local()
                )),
                Err(PanelLoadError::Cancelled)
            );
            assert_eq!(loader.status(), PanelLoadStatus::Idle);
            assert_eq!(*events.borrow(), ["unmount"]);
            assert_eq!(
                current_sdk.navigate(descriptor("two").key),
                Err(crate::panel::sdk::PanelSdkError::Disposed)
            );
        });
    }

    #[test]
    fn dropping_loader_fences_pending_load_and_disposes_sdk() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let current_sdk = sdk("one");
            let imported = Rc::new(Cell::new(false));
            let mark = imported.clone();
            let pending =
                loader.activate_with(descriptor("one").key, current_sdk.clone(), move |_| {
                    mark.set(true);
                    async { Err(PanelLoadError::ImportFailed) }.boxed_local()
                });
            drop(loader);
            assert_eq!(block_on(pending), Err(PanelLoadError::Cancelled));
            assert!(!imported.get());
            assert_eq!(
                current_sdk.navigate(descriptor("one").key),
                Err(crate::panel::sdk::PanelSdkError::Disposed)
            );
        });
    }

    #[test]
    fn mount_error_and_unmount_error_leave_retry_usable() {
        Owner::new().with(|| {
            let mut loader = fixture();
            let attempted = Rc::new(Cell::new(false));
            let mark = attempted.clone();
            let failing = LoadedModule {
                descriptor: serde_json::to_value(descriptor("one")).unwrap(),
                mount: Box::new(move |_| {
                    mark.set(true);
                    Err(PanelLoadError::MountFailed)
                }),
            };
            assert_eq!(
                block_on(
                    loader.activate_with(descriptor("one").key, sdk("one"), move |_| async {
                        Ok(failing)
                    }
                    .boxed_local())
                ),
                Err(PanelLoadError::MountFailed)
            );
            assert!(attempted.get());
            let events = Rc::new(RefCell::new(vec![]));
            let module = module(
                serde_json::to_value(descriptor("one")).unwrap(),
                events.clone(),
                true,
            );
            block_on(
                loader.activate_with(descriptor("one").key, sdk("one"), move |_| {
                    async { Ok(module) }.boxed_local()
                }),
            )
            .unwrap();
            assert_eq!(loader.deactivate(), Err(PanelLoadError::UnmountFailed));
            assert_eq!(loader.deactivate(), Ok(()));
            assert_eq!(*events.borrow(), ["mount", "unmount"]);
        });
    }
}
