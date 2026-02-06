//! CoreState - manages the AlephCore instance lifecycle
//!
//! This module provides thread-safe access to the AlephCore instance
//! for use across multiple Tauri commands.

use std::sync::{Arc, RwLock};
use tracing::warn;

use crate::error::{AlephError, Result};

/// Thread-safe state wrapper for AlephCore
pub struct CoreState {
    core: RwLock<Option<Arc<alephcore::AlephCore>>>,
}

impl CoreState {
    /// Create a new empty CoreState
    pub fn new() -> Self {
        Self {
            core: RwLock::new(None),
        }
    }

    /// Initialize the core with an AlephCore instance
    pub fn initialize(&self, core: Arc<alephcore::AlephCore>) {
        let mut guard = self.core.write().unwrap_or_else(|e| {
            warn!("CoreState write lock poisoned, recovering");
            e.into_inner()
        });
        *guard = Some(core);
    }

    /// Get a reference to the core, or error if not initialized
    pub fn get_core(&self) -> Result<Arc<alephcore::AlephCore>> {
        let guard = self.core.read().unwrap_or_else(|e| {
            warn!("CoreState read lock poisoned, recovering");
            e.into_inner()
        });

        guard
            .clone()
            .ok_or_else(|| AlephError::Core("Aleph core not initialized".to_string()))
    }

    /// Check if the core is initialized
    #[allow(dead_code)]
    pub fn is_initialized(&self) -> bool {
        let guard = self.core.read().unwrap_or_else(|e| {
            warn!("CoreState read lock poisoned, recovering");
            e.into_inner()
        });
        guard.is_some()
    }

    /// Shutdown the core (clears the reference)
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        let mut guard = self.core.write().unwrap_or_else(|e| {
            warn!("CoreState write lock poisoned, recovering");
            e.into_inner()
        });
        *guard = None;
    }
}

impl Default for CoreState {
    fn default() -> Self {
        Self::new()
    }
}

// Safety: CoreState uses RwLock which provides thread-safety
unsafe impl Send for CoreState {}
unsafe impl Sync for CoreState {}
