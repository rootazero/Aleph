//! Browser Types
//!
//! Shared types for browser automation operations.

/// Result type for browser operations
pub type BrowserResult<T> = Result<T, BrowserError>;

/// Errors that can occur in browser operations
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("Browser not started")]
    NotStarted,

    #[error("Browser already running")]
    AlreadyRunning,

    #[error("Chrome executable not found")]
    ExecutableNotFound,

    #[error("Failed to launch browser: {0}")]
    LaunchFailed(String),

    #[error("CDP connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Navigation failed: {0}")]
    NavigationFailed(String),

    #[error("Element not found: {0}")]
    ElementNotFound(String),

    #[error("Action failed: {0}")]
    ActionFailed(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Element reference cache for stable targeting
#[derive(Debug, Clone)]
pub struct ElementRef {
    pub ref_id: String,
    pub selector: String,
    pub role: String,
    pub name: String,
}

/// Allocation policy for browser instances
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationPolicy {
    /// All contexts share one browser process
    SingleInstance,
    /// Each context gets a dedicated browser process
    MultiInstance,
    /// Automatically decide based on system resources
    Adaptive,
}
