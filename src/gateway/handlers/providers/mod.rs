//! Providers RPC Handlers
//!
//! Handlers for AI provider management: list, get, create, update, delete, test, setDefault.

mod handlers;
mod helpers;
mod types;

#[cfg(test)]
mod tests;

// Re-export public types
pub use types::{
    CatalogEntryView, CatalogParams, CreateParams, DeleteParams, GetParams, ProviderConfigJson,
    ProviderHealthRow, ProviderInfo, SetDefaultParams, TestParams, TestResult, UpdateParams,
};

// Re-export handler functions
pub use handlers::{
    handle_catalog, handle_create, handle_create_hot, handle_delete, handle_delete_hot, handle_get,
    handle_healthcheck, handle_list, handle_models_refresh, handle_needs_setup, handle_set_default,
    handle_set_default_config_only, handle_test, handle_update, handle_update_hot,
};

// Re-export parse_params from parent for use by handlers submodule
use super::parse_params;
