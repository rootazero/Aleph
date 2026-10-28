//! Feature-gated Panel projection and browser-only lifecycle boundary.

pub mod loader;
pub mod projection;
pub mod registry;
pub mod sdk;
mod shell;

pub use shell::PanelShell;

use aleph_protocol::panel::PanelTabKey;
use leptos::prelude::{RwSignal, Update};
use registry::TabRegistry;
use std::sync::{Arc, Mutex};

/// Shell-owned descriptor projection, not an authoritative plugin registry.
/// Provide this context before constructing SDKs; only the Shell mutates it.
#[derive(Clone)]
pub struct PanelHostState {
    registry: Arc<Mutex<TabRegistry>>,
    pub active: RwSignal<Option<PanelTabKey>>,
    revision: RwSignal<u64>,
}

impl PanelHostState {
    pub fn new(registry: TabRegistry) -> Self {
        Self {
            registry: Arc::new(Mutex::new(registry)),
            active: RwSignal::new(None),
            revision: RwSignal::new(0),
        }
    }

    pub fn update_registry(&self, update: impl FnOnce(&mut TabRegistry)) {
        let mut registry = self.registry.lock().unwrap();
        update(&mut registry);
        self.revision.update(|revision| *revision += 1);
    }
}

/// A module's browser cleanup only. Never owns a Core provider or session.
/// Not Clone: replacing or dropping the handle releases its UI exactly once.
pub struct PanelModuleHandle {
    dispose: Option<Box<dyn FnOnce()>>,
}

impl PanelModuleHandle {
    pub fn new(dispose: impl FnOnce() + 'static) -> Self {
        Self {
            dispose: Some(Box::new(dispose)),
        }
    }

    pub fn dispose(&mut self) {
        if let Some(dispose) = self.dispose.take() {
            dispose();
        }
    }
}

impl Drop for PanelModuleHandle {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[derive(Default)]
pub enum PanelMountState {
    #[default]
    Idle,
    Loading {
        key: PanelTabKey,
    },
    Mounted {
        key: PanelTabKey,
        handle: PanelModuleHandle,
    },
    Failed {
        key: PanelTabKey,
        error: String,
    },
    Disabled,
}

impl PanelMountState {
    pub fn begin_load(&mut self, key: PanelTabKey) -> bool {
        if matches!(self, Self::Disabled) {
            return false;
        }
        *self = Self::Loading { key };
        true
    }

    /// A cancelled or superseded load must release its late browser handle.
    /// The loader additionally owns attempt identity for same-key retries.
    pub fn mount(&mut self, key: PanelTabKey, handle: PanelModuleHandle) -> bool {
        if !matches!(self, Self::Loading { key: current } if current == &key) {
            return false;
        }
        *self = Self::Mounted { key, handle };
        true
    }

    pub fn fail(&mut self, key: &PanelTabKey, error: String) -> bool {
        if !matches!(self, Self::Loading { key: current } if current == key) {
            return false;
        }
        *self = Self::Failed {
            key: key.clone(),
            error,
        };
        true
    }

    pub fn unmount(&mut self) {
        if !matches!(self, Self::Disabled) {
            *self = Self::Idle;
        }
    }

    pub fn disable(&mut self) {
        *self = Self::Disabled;
    }

    pub fn enable(&mut self) {
        if matches!(self, Self::Disabled) {
            *self = Self::Idle;
        }
    }
}
