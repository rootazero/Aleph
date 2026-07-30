pub mod backend;
pub mod chrome_mcp;
pub mod chrome_mcp_backend;
mod discovery;
pub mod error;
pub mod manager;
pub mod network_policy;
pub mod playwright_cli;
mod secret_guard;
pub use playwright_cli::{CliOutput, PageMeta, PlaywrightCliDriver};
pub mod playwright_cli_backend;
pub use playwright_cli_backend::PlaywrightCliBackend;
pub(crate) mod post_nav;
pub mod profile;
pub mod tab_registry;
pub mod types;
pub(crate) mod wait_probe;

pub use backend::BrowserBackend;
pub use chrome_mcp::ChromeMcpDriver;
pub use chrome_mcp_backend::ChromeMcpBackend;
pub use discovery::find_chromium;
pub use error::BrowserError;
pub use types::{
    ActionTarget, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId,
};
