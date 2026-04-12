pub mod backend;
pub mod bootstrap;
pub mod chrome_mcp;
pub mod chrome_mcp_backend;
mod chrome_mcp_snapshot;
mod discovery;
pub mod error;
pub mod manager;
pub mod network_policy;
pub mod playwright_cli;
pub use playwright_cli::{CliOutput, PageMeta, PlaywrightCliDriver};
pub mod playwright_cli_backend;
pub use playwright_cli_backend::PlaywrightCliBackend;
pub mod playwright_mcp;
pub mod playwright_mcp_backend;

pub mod profile;
pub mod types;

pub use backend::BrowserBackend;
pub use bootstrap::{BootstrapStatus, ComponentStatus};
pub use chrome_mcp::ChromeMcpDriver;
pub use chrome_mcp_backend::ChromeMcpBackend;
pub use discovery::find_chromium;
pub use error::BrowserError;
pub use playwright_mcp::PlaywrightMcpDriver;
pub use playwright_mcp_backend::PlaywrightMcpBackend;
pub use types::{
    ActionTarget, BrowserConfig, LaunchMode, ScreenshotOpts, ScreenshotOutput, ScrollDirection,
    SnapshotOutput, StorageKind, TabId,
};
