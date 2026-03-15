//! Failure tracking types for auth profiles.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

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

impl AuthProfileFailureReason {
    /// Classify HTTP status code into failure reason
    pub fn from_status(status: u16) -> Self {
        match status {
            400 => Self::Format,
            401 => Self::Auth,
            402 | 403 => Self::Billing,
            429 => Self::RateLimit,
            408 | 504 => Self::Timeout,
            _ => Self::Unknown,
        }
    }
}

/// Per-profile usage statistics for round-robin and cooldown tracking
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProfileUsageStats {
    /// Last successful use timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<u64>,
    /// Cooldown expiry for rate limit errors (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<u64>,
    /// Disabled expiry for billing errors (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_until: Option<u64>,
    /// Reason for being disabled
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<AuthProfileFailureReason>,
    /// Total error count (resets after failure window)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_count: Option<u32>,
    /// Per-reason failure counts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_counts: Option<HashMap<AuthProfileFailureReason, u32>>,
    /// Last failure timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<u64>,
}

impl ProfileUsageStats {
    /// Get the timestamp when this profile becomes usable again
    pub fn unusable_until(&self) -> Option<u64> {
        let values: Vec<u64> = [self.cooldown_until, self.disabled_until]
            .into_iter()
            .flatten()
            .filter(|&v| v > 0)
            .collect();

        if values.is_empty() {
            None
        } else {
            Some(*values.iter().max().unwrap())
        }
    }

    /// Check if profile is currently in cooldown
    pub fn is_in_cooldown(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.unusable_until().is_some_and(|until| now < until)
    }
}
