//! Authentication helpers for the OpenAI-compatible API.

use serde_json::json;

/// Extract a bearer token from an Authorization header value.
///
/// Strips the "Bearer " prefix (case-insensitive first letter) and returns
/// the token. Returns `None` if the prefix is missing or the token is empty.
pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    // Case-insensitive check for "Bearer " prefix (RFC 6750)
    // Use .get(..7) instead of &s[..7] to avoid panic on non-ASCII input (P7)
    let prefix = header_value.get(..7)?;
    if !prefix.eq_ignore_ascii_case("bearer ") {
        return None;
    }
    let token = &header_value[7..];
    if token.is_empty() { None } else { Some(token) }
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
    pub fn status_code(&self) -> u16 {
        match self {
            ApiError::Unauthorized(_) => 401,
            ApiError::BadRequest(_) => 400,
            ApiError::NotFound(_) => 404,
            ApiError::Conflict(_) => 409,
            ApiError::InternalError(_) => 500,
            ApiError::BadGateway(_) => 502,
            ApiError::GatewayTimeout(_) => 504,
            ApiError::ServiceUnavailable(_) => 503,
        }
    }

    /// Returns a machine-readable error code string.
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthorized(_) => "invalid_api_key",
            ApiError::BadRequest(_) => "invalid_request_error",
            ApiError::NotFound(_) => "model_not_found",
            ApiError::Conflict(_) => "agent_busy",
            ApiError::InternalError(_) => "internal_error",
            ApiError::BadGateway(_) => "provider_error",
            ApiError::GatewayTimeout(_) => "timeout",
            ApiError::ServiceUnavailable(_) => "service_unavailable",
        }
    }

    /// Returns a JSON representation matching the OpenAI error format.
    pub fn to_json(&self) -> serde_json::Value {
        let (message, error_type) = match self {
            ApiError::Unauthorized(msg) => (msg.as_str(), "authentication_error"),
            ApiError::BadRequest(msg) => (msg.as_str(), "invalid_request_error"),
            ApiError::NotFound(msg) => (msg.as_str(), "invalid_request_error"),
            ApiError::Conflict(msg) => (msg.as_str(), "conflict_error"),
            ApiError::InternalError(msg) => (msg.as_str(), "internal_error"),
            ApiError::BadGateway(msg) => (msg.as_str(), "upstream_error"),
            ApiError::GatewayTimeout(msg) => (msg.as_str(), "upstream_error"),
            ApiError::ServiceUnavailable(msg) => (msg.as_str(), "service_unavailable"),
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
    use super::*;

    #[test]
    fn test_extract_bearer_token() {
        // Standard Bearer prefix
        assert_eq!(
            extract_bearer_token("Bearer sk-abc123"),
            Some("sk-abc123")
        );

        // Lowercase bearer prefix
        assert_eq!(
            extract_bearer_token("bearer sk-abc123"),
            Some("sk-abc123")
        );

        // Mixed case (RFC 6750 case-insensitive)
        assert_eq!(
            extract_bearer_token("BEARER sk-abc123"),
            Some("sk-abc123")
        );
        assert_eq!(
            extract_bearer_token("BeArEr sk-abc123"),
            Some("sk-abc123")
        );

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
        assert_eq!(
            ApiError::NotFound("not found".into()).status_code(),
            404
        );
        assert_eq!(
            ApiError::Conflict("conflict".into()).status_code(),
            409
        );
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
        assert_eq!(ApiError::BadRequest("".into()).code(), "invalid_request_error");
        assert_eq!(ApiError::NotFound("".into()).code(), "model_not_found");
        assert_eq!(ApiError::Conflict("".into()).code(), "agent_busy");
        assert_eq!(ApiError::InternalError("".into()).code(), "internal_error");
        assert_eq!(ApiError::BadGateway("".into()).code(), "provider_error");
        assert_eq!(ApiError::GatewayTimeout("".into()).code(), "timeout");
        assert_eq!(ApiError::ServiceUnavailable("".into()).code(), "service_unavailable");
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
