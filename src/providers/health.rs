/// Provider health tracking and error classification.
///
/// This module provides types for tracking provider health state and
/// classifying errors into transient (retriable) vs permanent categories.
/// Used by the failover system to make routing decisions.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::error::AlephError;

// --- Error Classification ---

/// Transient errors that may resolve on their own.
/// These trigger cooldown/degraded state rather than permanent unavailability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransientError {
    /// Provider returned 429 with optional retry-after hint
    RateLimited { retry_after: Option<Duration> },
    /// Provider returned 5xx
    ServerError { status: u16 },
    /// Request timed out
    Timeout,
    /// TCP/TLS connection failed
    ConnectionFailed,
}

/// Permanent errors that require user intervention.
/// These make a provider immediately unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermanentError {
    /// API key invalid or revoked (401/403)
    AuthFailed,
    /// Requested model does not exist on this provider
    ModelNotFound,
}

/// Provider-level error classification.
///
/// Note: 400 `InvalidRequest` is intentionally excluded — it's request-specific,
/// not a provider health issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// Retriable error — provider may recover
    Transient(TransientError),
    /// Non-retriable error — requires user action
    Permanent(PermanentError),
}

// --- Health State ---

/// Base cooldown duration for exponential backoff (30 seconds)
const BASE_COOLDOWN: Duration = Duration::from_secs(30);
/// Maximum cooldown duration cap (5 minutes)
const MAX_COOLDOWN: Duration = Duration::from_secs(300);

/// Tracks the health state of a single provider.
#[derive(Debug, Clone, Default)]
pub enum ProviderHealth {
    /// Provider is operating normally
    #[default]
    Healthy,
    /// Provider has experienced transient failures but may recover
    Degraded {
        since: Instant,
        cooldown_until: Instant,
        consecutive_failures: u32,
    },
    /// Provider is permanently unavailable until user intervention
    Unavailable { since: Instant, reason: String },
}

impl ProviderHealth {
    /// Whether this provider can accept requests right now.
    ///
    /// - Healthy: always usable
    /// - Degraded: usable only after cooldown expires
    /// - Unavailable: never usable
    #[must_use]
    pub fn is_usable(&self) -> bool {
        match self {
            Self::Healthy => true,
            Self::Degraded { cooldown_until, .. } => Instant::now() >= *cooldown_until,
            Self::Unavailable { .. } => false,
        }
    }

    /// Record a successful request, resetting health to Healthy.
    pub fn record_success(&mut self) {
        *self = Self::Healthy;
    }

    /// Record a failed request and update health state accordingly.
    ///
    /// - Transient errors: transition to Degraded with exponential backoff
    /// - Permanent errors: transition to Unavailable immediately
    pub fn record_failure(&mut self, error: &ProviderError) {
        let now = Instant::now();

        match error {
            ProviderError::Transient(transient) => {
                let (consecutive_failures, old_since) = match self {
                    Self::Degraded {
                        consecutive_failures,
                        since,
                        ..
                    } => (*consecutive_failures + 1, Some(*since)),
                    _ => (1, None),
                };

                let cooldown = calculate_cooldown(consecutive_failures, transient);
                *self = Self::Degraded {
                    since: old_since.unwrap_or(now),
                    cooldown_until: now + cooldown,
                    consecutive_failures,
                };
            }
            ProviderError::Permanent(permanent) => {
                let reason = match permanent {
                    PermanentError::AuthFailed => "Authentication failed".to_string(),
                    PermanentError::ModelNotFound => "Model not found".to_string(),
                };
                *self = Self::Unavailable { since: now, reason };
            }
        }
    }

    /// Reset health to Healthy (e.g., after user reconfigures credentials).
    pub fn reset(&mut self) {
        *self = Self::Healthy;
    }
}

/// Calculate cooldown duration with exponential backoff.
///
/// - Base: 30s, doubles each consecutive failure, capped at 5 minutes
/// - `RateLimited` with `retry_after`: uses `max(retry_after`, `base_cooldown`) for first failure
fn calculate_cooldown(consecutive_failures: u32, transient: &TransientError) -> Duration {
    // For first RateLimited with retry_after, use max(retry_after, 30s)
    let base = if consecutive_failures == 1 {
        if let TransientError::RateLimited {
            retry_after: Some(retry_after),
        } = transient
        {
            (*retry_after).max(BASE_COOLDOWN)
        } else {
            BASE_COOLDOWN
        }
    } else {
        BASE_COOLDOWN
    };

    // Exponential backoff: base * 2^(failures-1), capped
    // Use saturating arithmetic to avoid overflow
    let exponent = consecutive_failures.saturating_sub(1);
    let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
    let cooldown = base.saturating_mul(multiplier.try_into().unwrap_or(u32::MAX));

    cooldown.min(MAX_COOLDOWN)
}

// --- AlephError → ProviderError conversion ---

impl From<&AlephError> for Option<ProviderError> {
    /// Convert an `AlephError` to a `ProviderError` for health tracking.
    ///
    /// Returns None for errors that are request-specific (not provider-level).
    fn from(error: &AlephError) -> Self {
        match error {
            AlephError::RateLimitError { .. } => {
                Some(ProviderError::Transient(TransientError::RateLimited {
                    retry_after: None,
                }))
            }
            AlephError::Timeout { .. } | AlephError::ExecutionTimeout { .. } => {
                Some(ProviderError::Transient(TransientError::Timeout))
            }
            AlephError::NetworkError { .. } => {
                Some(ProviderError::Transient(TransientError::ConnectionFailed))
            }
            AlephError::AuthenticationError { .. } => {
                Some(ProviderError::Permanent(PermanentError::AuthFailed))
            }
            AlephError::ProviderError { message, .. } => classify_provider_error_message(message),
            _ => None,
        }
    }
}

/// Classify a `ProviderError` message into a health-relevant error.
///
/// - Messages containing 5xx status codes → Transient(ServerError)
/// - Messages containing 404 + "model" → Permanent(ModelNotFound)
fn classify_provider_error_message(message: &str) -> Option<ProviderError> {
    // Status codes are matched with `has_status_code`, not as substrings: a
    // message quoting `"1500 tokens"` or a request id like `req_5040` carries
    // the digits of a 5xx without being one, and the verdict here decides
    // whether a provider is treated as sick.
    for status in [500, 502, 503, 504, 529] {
        if crate::providers::llm_retry::has_status_code(message, status) {
            return Some(ProviderError::Transient(TransientError::ServerError {
                status,
            }));
        }
    }

    // Check for 404 + model (model not found on provider)
    if crate::providers::llm_retry::has_status_code(message, 404)
        && message.to_lowercase().contains("model")
    {
        return Some(ProviderError::Permanent(PermanentError::ModelNotFound));
    }

    None
}

// --- Routing types ---

/// A resolved model after failover/routing decisions.
///
/// Uses `provider_name: String` rather than `Arc<dyn AiProvider>` so the result
/// can be returned from a trait method without lifetime/ownership complications,
/// and is serialization-friendly. The caller does a second `registry.get()` lookup
/// to obtain the `Arc<dyn AiProvider>` — the negligible race window between
/// resolve and lookup is acceptable for a personal assistant workload.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// Provider that will handle this request
    pub provider_name: String,
    /// Model to use on this provider
    pub model: String,
    /// Whether this is a fallback (not the user's first choice)
    pub is_fallback: bool,
    /// The model the user originally requested
    pub original_model: String,
}

/// Model information for API responses and logging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier
    pub model: String,
    /// Provider name
    pub provider: String,
    /// Whether this was a fallback selection
    pub is_fallback: bool,
    /// Original model requested (if different from model)
    pub original_model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_is_usable() {
        let health = ProviderHealth::Healthy;
        assert!(health.is_usable());
    }

    #[test]
    fn degraded_not_usable_during_cooldown() {
        let health = ProviderHealth::Degraded {
            since: Instant::now(),
            cooldown_until: Instant::now() + Duration::from_secs(60),
            consecutive_failures: 1,
        };
        assert!(!health.is_usable());
    }

    #[test]
    fn degraded_usable_after_cooldown() {
        let health = ProviderHealth::Degraded {
            since: Instant::now() - Duration::from_secs(60),
            // Cooldown already expired
            cooldown_until: Instant::now() - Duration::from_secs(1),
            consecutive_failures: 1,
        };
        assert!(health.is_usable());
    }

    #[test]
    fn unavailable_not_usable() {
        let health = ProviderHealth::Unavailable {
            since: Instant::now(),
            reason: "Auth failed".to_string(),
        };
        assert!(!health.is_usable());
    }

    #[test]
    fn record_success_resets_to_healthy() {
        let mut health = ProviderHealth::Degraded {
            since: Instant::now(),
            cooldown_until: Instant::now() + Duration::from_secs(60),
            consecutive_failures: 3,
        };
        health.record_success();
        assert!(matches!(health, ProviderHealth::Healthy));
        assert!(health.is_usable());
    }

    #[test]
    fn record_transient_failure_degrades() {
        let mut health = ProviderHealth::Healthy;
        let error = ProviderError::Transient(TransientError::Timeout);
        health.record_failure(&error);

        match &health {
            ProviderHealth::Degraded {
                consecutive_failures,
                ..
            } => {
                assert_eq!(*consecutive_failures, 1);
            }
            other => panic!("Expected Degraded, got {:?}", other),
        }
        // Should not be usable during cooldown
        assert!(!health.is_usable());
    }

    #[test]
    fn consecutive_failures_increase_cooldown() {
        let mut health = ProviderHealth::Healthy;
        let error = ProviderError::Transient(TransientError::ServerError { status: 500 });

        // First failure: 30s cooldown
        health.record_failure(&error);
        let cooldown_1 = match &health {
            ProviderHealth::Degraded {
                cooldown_until,
                since,
                consecutive_failures,
                ..
            } => {
                assert_eq!(*consecutive_failures, 1);
                *cooldown_until - *since
            }
            other => panic!("Expected Degraded, got {:?}", other),
        };

        // Second failure: should be ~60s (30s * 2^1)
        health.record_failure(&error);
        let cooldown_2 = match &health {
            ProviderHealth::Degraded {
                cooldown_until,
                since,
                consecutive_failures,
                ..
            } => {
                assert_eq!(*consecutive_failures, 2);
                *cooldown_until - *since // extract actual cooldown from state
            }
            other => panic!("Expected Degraded, got {:?}", other),
        };

        // Third failure: should be ~120s (30s * 2^2)
        health.record_failure(&error);
        let cooldown_3 = match &health {
            ProviderHealth::Degraded {
                cooldown_until,
                since,
                consecutive_failures,
                ..
            } => {
                assert_eq!(*consecutive_failures, 3);
                *cooldown_until - *since
            }
            other => panic!("Expected Degraded, got {:?}", other),
        };

        assert!(
            cooldown_2 > cooldown_1,
            "second cooldown ({:?}) should exceed first ({:?})",
            cooldown_2,
            cooldown_1
        );
        assert!(
            cooldown_3 > cooldown_2,
            "third cooldown ({:?}) should exceed second ({:?})",
            cooldown_3,
            cooldown_2
        );
        // Verify approximate values (allow 1s tolerance for Instant arithmetic)
        assert!(cooldown_1 >= Duration::from_secs(29) && cooldown_1 <= Duration::from_secs(31));
        assert!(cooldown_2 >= Duration::from_secs(59) && cooldown_2 <= Duration::from_secs(61));
        assert!(cooldown_3 >= Duration::from_secs(119) && cooldown_3 <= Duration::from_secs(121));
    }

    #[test]
    fn permanent_failure_makes_unavailable() {
        let mut health = ProviderHealth::Healthy;
        let error = ProviderError::Permanent(PermanentError::AuthFailed);
        health.record_failure(&error);

        match &health {
            ProviderHealth::Unavailable { reason, .. } => {
                assert_eq!(reason, "Authentication failed");
            }
            other => panic!("Expected Unavailable, got {:?}", other),
        }
        assert!(!health.is_usable());
    }

    #[test]
    fn reset_restores_healthy() {
        let mut health = ProviderHealth::Unavailable {
            since: Instant::now(),
            reason: "Auth failed".to_string(),
        };
        health.reset();
        assert!(matches!(health, ProviderHealth::Healthy));
        assert!(health.is_usable());
    }

    #[test]
    fn rate_limit_with_retry_after() {
        let mut health = ProviderHealth::Healthy;
        let error = ProviderError::Transient(TransientError::RateLimited {
            retry_after: Some(Duration::from_secs(120)),
        });
        health.record_failure(&error);

        match &health {
            ProviderHealth::Degraded {
                cooldown_until,
                since,
                consecutive_failures,
                ..
            } => {
                assert_eq!(*consecutive_failures, 1);
                let cooldown = *cooldown_until - *since;
                // retry_after=120s > base=30s, so cooldown should be 120s
                assert!(cooldown >= Duration::from_secs(119)); // allow small timing margin
                assert!(cooldown <= Duration::from_secs(121));
            }
            other => panic!("Expected Degraded, got {:?}", other),
        }
    }

    // --- AlephError conversion tests ---

    #[test]
    fn aleph_rate_limit_converts() {
        let err = AlephError::rate_limit("429 Too Many Requests");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Transient(TransientError::RateLimited {
                retry_after: None
            }))
        );
    }

    #[test]
    fn aleph_timeout_converts() {
        let err = AlephError::Timeout { suggestion: None };
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Transient(TransientError::Timeout))
        );
    }

    #[test]
    fn aleph_execution_timeout_converts() {
        let err = AlephError::ExecutionTimeout { timeout_secs: 30 };
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Transient(TransientError::Timeout))
        );
    }

    #[test]
    fn aleph_network_error_converts() {
        let err = AlephError::network("connection refused");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Transient(TransientError::ConnectionFailed))
        );
    }

    #[test]
    fn aleph_auth_error_converts() {
        let err = AlephError::authentication("openai", "401 Unauthorized");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Permanent(PermanentError::AuthFailed))
        );
    }

    #[test]
    fn aleph_provider_error_5xx_converts() {
        let err = AlephError::provider("HTTP 502 Bad Gateway");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Transient(TransientError::ServerError {
                status: 502
            }))
        );
    }

    #[test]
    fn aleph_provider_error_404_model_converts() {
        let err = AlephError::provider("404: model 'gpt-5' not found");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Permanent(PermanentError::ModelNotFound))
        );
    }

    #[test]
    fn aleph_unrelated_error_returns_none() {
        let err = AlephError::other("something unrelated");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(provider_err, None);
    }

    #[test]
    fn digits_inside_a_longer_number_are_not_a_status_code() {
        // A 400 (request-specific, deliberately NOT a health signal) that
        // happens to quote a token count used to classify as a 5xx server
        // error and degrade a healthy provider.
        let err = AlephError::provider("400 invalid request: 1500 tokens exceeds per-message cap");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(provider_err, None);

        // And a request id carrying "404" is not a missing model.
        let err = AlephError::provider("model call failed, request id req_40450");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(provider_err, None);
    }

    #[test]
    fn cooldown_capped_at_max() {
        // With many failures, cooldown should never exceed MAX_COOLDOWN (5 min)
        let cooldown = calculate_cooldown(100, &TransientError::Timeout);
        assert!(cooldown <= MAX_COOLDOWN);
    }

    #[test]
    fn rate_limit_retry_after_less_than_base_uses_base() {
        let mut health = ProviderHealth::Healthy;
        let error = ProviderError::Transient(TransientError::RateLimited {
            retry_after: Some(Duration::from_secs(5)), // Less than 30s base
        });
        health.record_failure(&error);

        match &health {
            ProviderHealth::Degraded {
                cooldown_until,
                since,
                ..
            } => {
                let cooldown = *cooldown_until - *since;
                // Should use base (30s) since retry_after (5s) < base
                assert!(cooldown >= Duration::from_secs(29));
                assert!(cooldown <= Duration::from_secs(31));
            }
            other => panic!("Expected Degraded, got {:?}", other),
        }
    }
}
