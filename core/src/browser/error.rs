use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("Browser is not running. Launch a browser instance first.")]
    NotRunning,

    #[error("Failed to launch browser: {0}")]
    LaunchFailed(String),

    #[error("Browser connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Browser protocol error: {0}")]
    Protocol(String),

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

    #[error("JavaScript evaluation error: {0}")]
    EvalError(String),
}
