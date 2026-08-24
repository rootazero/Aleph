//! Process-global [`ExtensionManager`] handle.
//!
//! The manager is constructed once at server boot and registered here via
//! [`init_extension_manager`]. Core-layer subsystems (providers, tools) reach
//! it through [`try_extension_manager`] without a reverse dependency on the
//! gateway layer. The gateway's `get_extension_manager` wraps this accessor
//! with a `JsonRpcResponse`-shaped error for its RPC handlers.

use super::ExtensionManager;
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

/// `ConsumerDecides`, by weight of evidence: about twenty production readers,
/// each writing its own meaning for absence. `plugin_manage` answers a named
/// error; `hooks_admin` and `handlers::services` answer their own refusals;
/// `tools::usage::report` and `hub::reconcile` return early and report nothing
/// missing. The sharpest is `hooks::executor`, where an early return means a
/// registered hook simply never fires — §5.10's "hook 注册了 ≠ hook 会触发",
/// arriving here through a handle rather than through a matcher.
static EXTENSION_MANAGER: CapabilitySlot<Arc<ExtensionManager>> =
    CapabilitySlot::new("extension/manager", MissingSemantics::ConsumerDecides);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn extension_manager_slot() -> &'static dyn SlotStatus {
    &EXTENSION_MANAGER
}

/// Register the process-global extension manager.
///
/// Called once during server startup, before any plugin RPC handler or hook
/// fire-site reads the manager. Returns `Err(manager)` if already initialized.
pub fn init_extension_manager(manager: Arc<ExtensionManager>) -> Result<(), Arc<ExtensionManager>> {
    // The clone keeps this signature: `CapabilitySlot::install` consumes the
    // value and answers a bool, while `Err(_)` here carries the REJECTED
    // manager back. Only `start/helpers.rs` reads it, and only as a
    // discriminant (`Err(_existing)`), but migrating a handle must be
    // invisible from outside — so the echo is reconstructed rather than the
    // caller changed. One `Arc` clone on the boot path.
    let rejected = Arc::clone(&manager);
    if EXTENSION_MANAGER.install(manager) {
        Ok(())
    } else {
        Err(rejected)
    }
}

/// Borrow the process-global extension manager, if it has been registered.
pub fn try_extension_manager() -> Option<&'static Arc<ExtensionManager>> {
    EXTENSION_MANAGER.get()
}

/// Whether the process-global extension manager has been registered.
pub fn is_extension_manager_initialized() -> bool {
    EXTENSION_MANAGER.get().is_some()
}
