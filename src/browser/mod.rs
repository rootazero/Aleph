pub mod backend;
pub(crate) mod chrome_mcp;
pub(crate) mod chrome_mcp_backend;
// TODO(plan-1 task 6): remove this allow. Task 5 wired the launch half
// (`ChromiumChild` / `ChromiumLaunchSpec` / `CdpEndpoint` /
// `DEVTOOLS_PORT_DEADLINE`); what is still without a non-test caller is the
// orphan-reaper half Task 6 owns — `ArgvProbe`, `argv_names_dir`,
// `argv_probe`, `ProcessFacts`, `reap_orphans`, `reap_orphans_now`. Those six
// are exactly what must make the allow unnecessary; `-D warnings`
// (justfile:486, CI:345) fails without it until then.
#[allow(dead_code)]
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
