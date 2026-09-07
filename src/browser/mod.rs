pub mod backend;
pub(crate) mod chrome_mcp;
pub(crate) mod chrome_mcp_backend;
pub(crate) mod chromium_launch;
pub(crate) mod chromium_resolve;
mod discovery;
pub mod error;
pub mod manager;
pub mod network_policy;
pub mod playwright_cli;
pub(crate) mod playwright_cli_backend;
pub mod playwright_launch;
pub(crate) mod post_nav;
pub mod profile;
mod secret_guard;
pub mod tab_registry;
#[cfg(test)]
pub(crate) mod testkit;
pub mod types;
pub(crate) mod wait_probe;

pub use discovery::find_chromium;
pub use error::BrowserError;
// Crate-internal: `live_endpoint` is `pub(crate)` too, and its first real
// consumer (the live view, Plan 2) lives in this crate.
pub(crate) use chromium_launch::CdpEndpoint;
