/// Provider error classification for advisory/observability consumers.
///
/// This module classifies errors into transient (retriable) vs permanent
/// categories. Its production consumers are the MoA advisor health tracker
/// (`moa/advisor_health.rs`) and gateway event surfaces (`ModelInfo`) — the
/// failover engine itself does *not* read these types; its retry/migrate
/// decisions are classified in `llm_retry` + `failover/decision.rs`.
use serde::{Deserialize, Serialize};

use crate::error::AlephError;

// --- Error Classification ---

/// Transient errors that may resolve on their own.
/// These trigger cooldown/degraded state rather than permanent unavailability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransientError {
    /// Provider returned 429. The vendor's Retry-After hint, when present, is
    /// consumed upstream by `failover`'s model/provider cooldowns; by the time
    /// an error reaches this classifier the hint is already spent, so the
    /// variant deliberately carries no `retry_after` payload.
    RateLimited,
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

// --- AlephError → ProviderError conversion ---

impl From<&AlephError> for Option<ProviderError> {
    /// Convert an `AlephError` to a `ProviderError` for health tracking.
    ///
    /// Returns None for errors that are request-specific (not provider-level).
    fn from(error: &AlephError) -> Self {
        match error {
            AlephError::RateLimitError { .. } => {
                Some(ProviderError::Transient(TransientError::RateLimited))
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

    // --- AlephError conversion tests ---

    #[test]
    fn aleph_rate_limit_converts() {
        let err = AlephError::rate_limit("429 Too Many Requests");
        let provider_err: Option<ProviderError> = (&err).into();
        assert_eq!(
            provider_err,
            Some(ProviderError::Transient(TransientError::RateLimited))
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
}
