use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("Failed to launch browser: {0}")]
    LaunchFailed(String),

    #[error("Tab not found: {0}")]
    TabNotFound(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Browser action failed: {0}")]
    ActionFailed(String),

    #[error("Browser operation timed out after {0}ms")]
    Timeout(u64),

    #[error("Chromium binary not found. Install Chrome/Chromium or specify a binary path.")]
    ChromiumNotFound,

    #[error("Screenshot failed: {0}")]
    ScreenshotFailed(String),

    #[error("Failed to attach to browser: {0}")]
    AttachFailed(String),

    #[error("Chrome DevTools MCP error: {0}")]
    ChromeMcpError(String),

    #[error("Playwright CLI error: {0}")]
    PlaywrightCliError(String),

    #[error("Playwright CLI not installed. Open Settings → Browser → Install All.")]
    PlaywrightCliNotInstalled,

    #[error("No active browser session for '{0}'. Call open/goto first.")]
    NoSession(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Browser profile not found: {0}")]
    ProfileNotFound(String),
}
