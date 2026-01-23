//! PendingParam - Pending parameter waiting for user input

use std::time::Instant;

/// Pending parameter waiting for user input
///
/// When an intent is detected but requires additional parameters,
/// a PendingParam is created to track what's needed and when it was requested.
#[derive(Debug, Clone)]
pub struct PendingParam {
    /// Parameter name (e.g., "location", "url")
    pub name: String,

    /// Intent type this parameter is required for
    pub required_for: String,

    /// The prompt text shown to user
    pub prompt: String,

    /// When this pending param was created
    pub created_at: Instant,
}

impl PendingParam {
    /// Create a new pending parameter
    pub fn new(
        name: impl Into<String>,
        required_for: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            required_for: required_for.into(),
            prompt: prompt.into(),
            created_at: Instant::now(),
        }
    }

    /// Check if this pending param has expired (default: 5 minutes)
    pub fn is_expired(&self) -> bool {
        self.is_expired_after_secs(300) // 5 minutes
    }

    /// Check if expired after given seconds
    pub fn is_expired_after_secs(&self, seconds: u64) -> bool {
        self.created_at.elapsed().as_secs() > seconds
    }
}
