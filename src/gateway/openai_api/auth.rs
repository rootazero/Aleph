//! Authentication helpers for the OpenAI-compatible API.

use axum::http::HeaderMap;
use serde_json::json;

use super::state::OpenAiApiState;

/// Extract a bearer token from an Authorization header value.
///
/// Strips the "Bearer " prefix (case-insensitive first letter) and returns
/// the token. Returns `None` if the prefix is missing or the token is empty.
#[must_use]
pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    // Case-insensitive check for "Bearer " prefix (RFC 6750)
    // Use .get(..7) instead of &s[..7] to avoid panic on non-ASCII input (P7)
    let prefix = header_value.get(..7)?;
    if !prefix.eq_ignore_ascii_case("bearer ") {
        return None;
    }
    let token = &header_value[7..];
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Authorize a `/v1/*` request against the live shared token.
///
/// Fails closed. `None` means no shared token is resident — the token manager
/// was never wired into the server, or the token could not be provisioned at
/// boot — and the request is rejected instead of being accepted with any
/// bearer token: without a resident token there is nothing to compare
/// against, so accepting would hand the LLM surface to anonymous callers.
pub fn authorize(state: &OpenAiApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = extract_bearer_token(auth_header)
        .ok_or_else(|| ApiError::Unauthorized("Missing or invalid Authorization header".into()))?;
    let expected = (state.api_token)()
        .ok_or_else(|| ApiError::Unauthorized("Invalid API key".into()))?;
    if !crate::security::secret_equal(Some(token), Some(expected.as_str())) {
        return Err(ApiError::Unauthorized("Invalid API key".into()));
    }
    Ok(())
}

/// API error types for the OpenAI-compatible endpoint.
#[derive(Debug)]
pub enum ApiError {
    Unauthorized(String),
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    InternalError(String),
    BadGateway(String),
    GatewayTimeout(String),
    ServiceUnavailable(String),
}

impl ApiError {
    /// Returns the HTTP status code for this error.
    #[must_use]
    pub const fn status_code(&self) -> u16 {
        match self {
            Self::Unauthorized(_) => 401,
            Self::BadRequest(_) => 400,
            Self::NotFound(_) => 404,
            Self::Conflict(_) => 409,
            Self::InternalError(_) => 500,
            Self::BadGateway(_) => 502,
            Self::GatewayTimeout(_) => 504,
            Self::ServiceUnavailable(_) => 503,
        }
    }

    /// Returns a machine-readable error code string.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized(_) => "invalid_api_key",
            Self::BadRequest(_) => "invalid_request_error",
            Self::NotFound(_) => "model_not_found",
            Self::Conflict(_) => "agent_busy",
            Self::InternalError(_) => "internal_error",
            Self::BadGateway(_) => "provider_error",
            Self::GatewayTimeout(_) => "timeout",
            Self::ServiceUnavailable(_) => "service_unavailable",
        }
    }

    /// Returns a JSON representation matching the `OpenAI` error format.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let (message, error_type) = match self {
            Self::Unauthorized(msg) => (msg.as_str(), "authentication_error"),
            Self::BadRequest(msg) => (msg.as_str(), "invalid_request_error"),
            Self::NotFound(msg) => (msg.as_str(), "invalid_request_error"),
            Self::Conflict(msg) => (msg.as_str(), "conflict_error"),
            Self::InternalError(msg) => (msg.as_str(), "internal_error"),
            Self::BadGateway(msg) => (msg.as_str(), "upstream_error"),
            Self::GatewayTimeout(msg) => (msg.as_str(), "upstream_error"),
            Self::ServiceUnavailable(msg) => (msg.as_str(), "service_unavailable"),
        };

        json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": self.code()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::http::header::AUTHORIZATION;
    use axum::http::HeaderMap;

    use super::*;
    use crate::sync_primitives::Arc;

    fn state_with_token(token: Option<String>) -> OpenAiApiState {
        OpenAiApiState {
            server_id: "test".to_owned(),
            api_token: Arc::new(move || token.clone()),
            execution_adapter: None,
            provider_map: Arc::new(HashMap::new()),
            agent_registry: None,
            provider_configs: Arc::new(Vec::new()),
            created_at: 0,
            embedding_provider: None,
        }
    }

    fn headers(bearer: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, format!("Bearer {bearer}").parse().unwrap());
        h
    }

    #[test]
    fn a_missing_resident_token_rejects_every_bearer() {
        let state = state_with_token(None);
        assert!(matches!(
            authorize(&state, &headers("anything")),
            Err(ApiError::Unauthorized(_))
        ));
    }

    #[test]
    fn the_resident_token_is_accepted() {
        let state = state_with_token(Some("aleph-token".to_owned()));
        assert!(authorize(&state, &headers("aleph-token")).is_ok());
    }

    #[test]
    fn a_wrong_bearer_is_rejected() {
        let state = state_with_token(Some("aleph-token".to_owned()));
        assert!(matches!(
            authorize(&state, &headers("wrong")),
            Err(ApiError::Unauthorized(_))
        ));
    }

    #[test]
    fn test_extract_bearer_token() {
        // Standard Bearer prefix
        assert_eq!(extract_bearer_token("Bearer sk-abc123"), Some("sk-abc123"));

        // Lowercase bearer prefix
        assert_eq!(extract_bearer_token("bearer sk-abc123"), Some("sk-abc123"));

        // Mixed case (RFC 6750 case-insensitive)
        assert_eq!(extract_bearer_token("BEARER sk-abc123"), Some("sk-abc123"));
        assert_eq!(extract_bearer_token("BeArEr sk-abc123"), Some("sk-abc123"));

        // Wrong prefix (Basic auth)
        assert_eq!(extract_bearer_token("Basic dXNlcjpwYXNz"), None);

        // Empty string
        assert_eq!(extract_bearer_token(""), None);

        // "Bearer " with no token
        assert_eq!(extract_bearer_token("Bearer "), None);
    }

    #[test]
    fn test_api_error_status_codes() {
        assert_eq!(
            ApiError::Unauthorized("unauthorized".into()).status_code(),
            401
        );
        assert_eq!(
            ApiError::BadRequest("bad request".into()).status_code(),
            400
        );
        assert_eq!(ApiError::NotFound("not found".into()).status_code(), 404);
        assert_eq!(ApiError::Conflict("conflict".into()).status_code(), 409);
        assert_eq!(
            ApiError::InternalError("internal error".into()).status_code(),
            500
        );
        assert_eq!(
            ApiError::BadGateway("bad gateway".into()).status_code(),
            502
        );
        assert_eq!(
            ApiError::GatewayTimeout("timeout".into()).status_code(),
            504
        );
        assert_eq!(
            ApiError::ServiceUnavailable("unavailable".into()).status_code(),
            503
        );
    }

    #[test]
    fn test_api_error_codes() {
        assert_eq!(ApiError::Unauthorized("".into()).code(), "invalid_api_key");
        assert_eq!(
            ApiError::BadRequest("".into()).code(),
            "invalid_request_error"
        );
        assert_eq!(ApiError::NotFound("".into()).code(), "model_not_found");
        assert_eq!(ApiError::Conflict("".into()).code(), "agent_busy");
        assert_eq!(ApiError::InternalError("".into()).code(), "internal_error");
        assert_eq!(ApiError::BadGateway("".into()).code(), "provider_error");
        assert_eq!(ApiError::GatewayTimeout("".into()).code(), "timeout");
        assert_eq!(
            ApiError::ServiceUnavailable("".into()).code(),
            "service_unavailable"
        );
    }

    #[test]
    fn test_api_error_to_json() {
        let err = ApiError::Unauthorized("Invalid API key".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Invalid API key");
        assert_eq!(json_val["error"]["type"], "authentication_error");
        assert_eq!(json_val["error"]["code"], "invalid_api_key");

        let err = ApiError::BadRequest("Missing model field".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Missing model field");
        assert_eq!(json_val["error"]["type"], "invalid_request_error");
        assert_eq!(json_val["error"]["code"], "invalid_request_error");

        let err = ApiError::NotFound("Model not found".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Model not found");
        assert_eq!(json_val["error"]["type"], "invalid_request_error");
        assert_eq!(json_val["error"]["code"], "model_not_found");

        let err = ApiError::Conflict("Agent is busy".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Agent is busy");
        assert_eq!(json_val["error"]["type"], "conflict_error");
        assert_eq!(json_val["error"]["code"], "agent_busy");

        let err = ApiError::InternalError("Something went wrong".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Something went wrong");
        assert_eq!(json_val["error"]["type"], "internal_error");
        assert_eq!(json_val["error"]["code"], "internal_error");

        let err = ApiError::BadGateway("Provider failed".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Provider failed");
        assert_eq!(json_val["error"]["type"], "upstream_error");
        assert_eq!(json_val["error"]["code"], "provider_error");

        let err = ApiError::GatewayTimeout("Request timed out".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Request timed out");
        assert_eq!(json_val["error"]["type"], "upstream_error");
        assert_eq!(json_val["error"]["code"], "timeout");

        let err = ApiError::ServiceUnavailable("Service is down".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Service is down");
        assert_eq!(json_val["error"]["type"], "service_unavailable");
        assert_eq!(json_val["error"]["code"], "service_unavailable");
    }
}
