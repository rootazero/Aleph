//! Plugins RPC Handlers
//!
//! Handlers for plugin management: list, install, uninstall, enable, disable.

use crate::sync_primitives::Arc;

use crate::extension::ExtensionManager;
use crate::gateway::protocol::{JsonRpcResponse, INTERNAL_ERROR};

mod install;
mod manage;
mod marketplace;
mod runtime;

// `tests.rs` sat beside these four with no `mod` statement anywhere in the
// crate, so 325 lines of plugin-RPC parameter tests had never been compiled,
// let alone run — and an uncompiled test file is indistinguishable from a
// passing one in every report. Declared here so it is either green or loud.
#[cfg(test)]
mod tests;

pub use install::*;
pub use manage::*;
pub use marketplace::*;
pub use runtime::*;

// ============================================================================
// Global Extension Manager (for plugin tool calls)
// ============================================================================
//
// The process-global handle itself lives in `crate::extension` so Core-layer
// subsystems (providers, tools) can reach it without a reverse dependency on
// the gateway. The gateway re-exports the registration helpers and adds the
// RPC-shaped accessor below.

pub use crate::extension::{
    decline_extension_manager, init_extension_manager, is_extension_manager_initialized,
};

/// Get the extension manager.
///
/// Returns a JSON-RPC error response if the manager hasn't been initialized.
// JsonRpcResponse is 152+ bytes but boxing it would complicate all handler call sites
#[allow(clippy::result_large_err)]
pub fn get_extension_manager() -> Result<&'static Arc<ExtensionManager>, JsonRpcResponse> {
    crate::extension::try_extension_manager().ok_or_else(|| {
        JsonRpcResponse::error(
            None,
            INTERNAL_ERROR,
            "Extension manager not initialized. Gateway startup may have failed.".to_string(),
        )
    })
}

// ============================================================================
// Internal helper — build MarketplaceManager from config
// ============================================================================

pub(crate) fn build_marketplace_manager(
) -> Result<crate::extension::marketplace::MarketplaceManager, String> {
    crate::extension::marketplace::MarketplaceManager::from_config()
}
