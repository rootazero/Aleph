//! Failure classification for auth profiles.

use serde::{Deserialize, Serialize};

/// Reason for auth profile failure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProfileFailureReason {
    /// Authentication error (401)
    Auth,
    /// Format/validation error (400)
    Format,
    /// Rate limit exceeded (429)
    RateLimit,
    /// Billing/quota error (402/403)
    Billing,
    /// Request timeout
    Timeout,
    /// Unknown/other error
    Unknown,
}
