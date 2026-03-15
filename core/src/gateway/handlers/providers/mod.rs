//! Providers RPC Handlers
//!
//! Handlers for AI provider management: list, get, create, update, delete, test, setDefault.

mod types;
mod helpers;
mod handlers;

#[cfg(test)]
mod tests;

// Re-export public types
pub use types::{
    ProviderInfo, TestResult,
    GetParams, UpdateParams, CreateParams, DeleteParams,
    TestParams, SetDefaultParams,
    ProviderConfigJson,
};

// Re-export handler functions
pub use handlers::{
    handle_list, handle_get, handle_update, handle_create, handle_delete,
    handle_test, handle_needs_setup,
    handle_set_default_config_only, handle_set_default,
};

// Re-export parse_params from parent for use by handlers submodule
use super::parse_params;
