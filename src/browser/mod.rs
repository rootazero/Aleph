pub mod backend;
pub(crate) mod chrome_mcp;
pub(crate) mod chrome_mcp_backend;
// TODO(plan-1 task 6): remove this allow. Consumers land in Task 5 (playwright_cli.rs) and Task 6 (manager.rs); until then this module has no non-test caller and `-D warnings` (justfile:486, CI:345) would fail.
#[allow(dead_code)]
pub(crate) mod chromium_launch;
// TODO(plan-1 task 5): remove this allow. Task 5 (playwright_cli.rs) is the
// only consumer; until then this module has no non-test caller and
// `-D warnings` (justfile:486, CI:345) would fail.
#[allow(dead_code)]
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
