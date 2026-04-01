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
    CreateParams, DeleteParams, GetParams, ProviderConfigJson, ProviderInfo, SetDefaultParams,
    TestParams, TestResult, UpdateParams,
};

// Re-export handler functions
pub use handlers::{
    handle_create, handle_delete, handle_get, handle_list, handle_needs_setup, handle_set_default,
    handle_set_default_config_only, handle_test, handle_test_no_registry, handle_update,
};

// Re-export parse_params from parent for use by handlers submodule
use super::parse_params;
