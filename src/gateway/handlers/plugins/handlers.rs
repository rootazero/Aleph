//! Plugins RPC Handlers
//!
//! Handlers for plugin management: list, install, uninstall, enable, disable.

use crate::sync_primitives::Arc;
use once_cell::sync::OnceCell;

use crate::extension::ExtensionManager;
use crate::gateway::protocol::{JsonRpcResponse, INTERNAL_ERROR};

use super::types::*;

mod marketplace;
mod install;
mod manage;
mod runtime;

pub use marketplace::*;
pub use install::*;
pub use manage::*;
pub use runtime::*;

// ============================================================================
// Global Extension Manager (for plugin tool calls)
// ============================================================================

/// Global extension manager for plugin handlers.
///
/// This is initialized once at gateway startup via `init_extension_manager()`.
/// The OnceCell ensures thread-safe lazy initialization.
static EXTENSION_MANAGER: OnceCell<Arc<ExtensionManager>> = OnceCell::new();

/// Initialize the extension manager for plugin handlers.
///
/// This should be called once during gateway startup, before any
/// `plugins.callTool` requests are processed.
///
/// # Arguments
///
/// * `manager` - The ExtensionManager instance to use for plugin operations
///
/// # Returns
///
/// * `Ok(())` if initialization succeeded
/// * `Err(manager)` if already initialized (returns the passed manager)
pub fn init_extension_manager(manager: Arc<ExtensionManager>) -> Result<(), Arc<ExtensionManager>> {
    EXTENSION_MANAGER.set(manager)
}

/// Get the extension manager.
///
/// Returns an error response if the manager hasn't been initialized.
// JsonRpcResponse is 152+ bytes but boxing it would complicate all handler call sites
#[allow(clippy::result_large_err)]
pub fn get_extension_manager() -> Result<&'static Arc<ExtensionManager>, JsonRpcResponse> {
    EXTENSION_MANAGER.get().ok_or_else(|| {
        JsonRpcResponse::error(
            None,
            INTERNAL_ERROR,
            "Extension manager not initialized. Gateway startup may have failed.".to_string(),
        )
    })
}

/// Check if the extension manager has been initialized.
pub fn is_extension_manager_initialized() -> bool {
    EXTENSION_MANAGER.get().is_some()
}

// ============================================================================
// Internal helper — build MarketplaceManager from config
// ============================================================================

pub(crate) fn build_marketplace_manager() -> Result<crate::extension::marketplace::MarketplaceManager, String>
{
    use crate::extension::marketplace::types::{MarketplaceConfig, MarketplaceSourceType};
    use std::collections::HashMap;

    let config = crate::config::Config::load().map_err(|e| format!("Config error: {e}"))?;

    let marketplace_configs: HashMap<String, MarketplaceConfig> = config
        .plugin_marketplaces
        .iter()
        .map(|(name, entry)| {
            let source_type = match entry.source_type.as_str() {
                "local" => MarketplaceSourceType::Local,
                _ => MarketplaceSourceType::Github,
            };
            (
                name.clone(),
                MarketplaceConfig {
                    source: entry.source.clone(),
                    source_type,
                },
            )
        })
        .collect();

    Ok(crate::extension::marketplace::MarketplaceManager::new(
        marketplace_configs,
        None,
    ))
}
