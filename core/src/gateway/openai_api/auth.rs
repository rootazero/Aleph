//! Authentication helpers for the OpenAI-compatible API.

use serde_json::json;

/// Extract a bearer token from an Authorization header value.
///
/// Strips the "Bearer " prefix (case-insensitive first letter) and returns
/// the token. Returns `None` if the prefix is missing or the token is empty.
pub fn extract_bearer_token(header_value: &str) -> Option<&str> {
    let stripped = if let Some(rest) = header_value.strip_prefix("Bearer ") {
        rest
    } else if let Some(rest) = header_value.strip_prefix("bearer ") {
        rest
    } else {
        return None;
    };

    if stripped.is_empty() {
        None
    } else {
        Some(stripped)
    }
}

/// API error types for the OpenAI-compatible endpoint.
#[derive(Debug)]
pub enum ApiError {
    Unauthorized(String),
    BadRequest(String),
    InternalError(String),
    ServiceUnavailable(String),
}

impl ApiError {
    /// Returns the HTTP status code for this error.
    pub fn status_code(&self) -> u16 {
        match self {
            ApiError::Unauthorized(_) => 401,
            ApiError::BadRequest(_) => 400,
            ApiError::InternalError(_) => 500,
            ApiError::ServiceUnavailable(_) => 503,
        }
    }

    /// Returns a JSON representation matching the OpenAI error format.
    pub fn to_json(&self) -> serde_json::Value {
        let (message, error_type) = match self {
            ApiError::Unauthorized(msg) => (msg.as_str(), "authentication_error"),
            ApiError::BadRequest(msg) => (msg.as_str(), "invalid_request_error"),
            ApiError::InternalError(msg) => (msg.as_str(), "internal_error"),
            ApiError::ServiceUnavailable(msg) => (msg.as_str(), "service_unavailable"),
        };

        json!({
            "error": {
                "message": message,
                "type": error_type
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
            ApiError::InternalError("internal error".into()).status_code(),
            500
        );
        assert_eq!(
            ApiError::ServiceUnavailable("unavailable".into()).status_code(),
            503
        );
    }

    #[test]
    fn test_api_error_to_json() {
        let err = ApiError::Unauthorized("Invalid API key".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Invalid API key");
        assert_eq!(json_val["error"]["type"], "authentication_error");

        let err = ApiError::BadRequest("Missing model field".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Missing model field");
        assert_eq!(json_val["error"]["type"], "invalid_request_error");

        let err = ApiError::InternalError("Something went wrong".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Something went wrong");
        assert_eq!(json_val["error"]["type"], "internal_error");

        let err = ApiError::ServiceUnavailable("Service is down".to_string());
        let json_val = err.to_json();
        assert_eq!(json_val["error"]["message"], "Service is down");
        assert_eq!(json_val["error"]["type"], "service_unavailable");
    }
}
