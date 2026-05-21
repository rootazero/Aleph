//! Process-global [`ExtensionManager`] handle.
//!
//! The manager is constructed once at server boot and registered here via
//! [`init_extension_manager`]. Core-layer subsystems (providers, tools) reach
//! it through [`try_extension_manager`] without a reverse dependency on the
//! gateway layer. The gateway's `get_extension_manager` wraps this accessor
//! with a `JsonRpcResponse`-shaped error for its RPC handlers.

use once_cell::sync::OnceCell;

use super::ExtensionManager;
use crate::sync_primitives::Arc;

static EXTENSION_MANAGER: OnceCell<Arc<ExtensionManager>> = OnceCell::new();

/// Register the process-global extension manager.
///
/// Called once during server startup, before any plugin RPC handler or hook
/// fire-site reads the manager. Returns `Err(manager)` if already initialized.
pub fn init_extension_manager(manager: Arc<ExtensionManager>) -> Result<(), Arc<ExtensionManager>> {
    EXTENSION_MANAGER.set(manager)
}

/// Borrow the process-global extension manager, if it has been registered.
pub fn try_extension_manager() -> Option<&'static Arc<ExtensionManager>> {
    EXTENSION_MANAGER.get()
}

/// Whether the process-global extension manager has been registered.
pub fn is_extension_manager_initialized() -> bool {
    EXTENSION_MANAGER.get().is_some()
}
