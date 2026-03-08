use std::time::Duration;

use super::task::TaskState;

/// A2A error type — aligned with JSON-RPC error codes
#[derive(Debug, thiserror::Error)]
pub enum A2AError {
    // JSON-RPC standard errors
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    #[error("Method not found: {0}")]
    MethodNotFound(String),
    #[error("Invalid params: {0}")]
    InvalidParams(String),
    #[error("Internal error: {0}")]
    InternalError(String),

    // A2A business errors
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Task not cancelable in state: {0:?}")]
    TaskNotCancelable(TaskState),
    #[error("Push notification not supported")]
    PushNotSupported,
    #[error("Unsupported content type")]
    UnsupportedContentType,

    // Security errors
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Forbidden: insufficient trust level")]
    Forbidden,

    // Client-side errors
    #[error("Agent unreachable: {0}")]
    AgentUnreachable(String),
    #[error("No matching agent for intent")]
    NoMatchingAgent,
    #[error("Timeout after {0:?}")]
    Timeout(Duration),
}

impl A2AError {
    /// Returns the JSON-RPC error code for this error
    pub fn error_code(&self) -> i64 {
        match self {
            Self::ParseError(_) => -32700,
            Self::InvalidRequest(_) => -32600,
            Self::MethodNotFound(_) => -32601,
            Self::InvalidParams(_) => -32602,
            Self::InternalError(_) => -32603,
            Self::TaskNotFound(_) => -32001,
            Self::TaskNotCancelable(_) => -32002,
            Self::PushNotSupported => -32003,
            Self::UnsupportedContentType => -32004,
            Self::Unauthorized => -32000,
            Self::Forbidden => -32005,
            Self::AgentUnreachable(_) => -32603,
            Self::NoMatchingAgent => -32603,
            Self::Timeout(_) => -32603,
        }
    }

    /// Convert to a JSON-RPC error object
    pub fn to_jsonrpc_error(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.error_code(),
            "message": self.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_jsonrpc_standard() {
        assert_eq!(A2AError::ParseError("bad".into()).error_code(), -32700);
        assert_eq!(A2AError::InvalidRequest("bad".into()).error_code(), -32600);
        assert_eq!(
            A2AError::MethodNotFound("unknown".into()).error_code(),
            -32601
        );
        assert_eq!(A2AError::InvalidParams("bad".into()).error_code(), -32602);
        assert_eq!(A2AError::InternalError("oops".into()).error_code(), -32603);
    }

    #[test]
    fn error_code_a2a_business() {
        assert_eq!(A2AError::TaskNotFound("t1".into()).error_code(), -32001);
        assert_eq!(
            A2AError::TaskNotCancelable(TaskState::Completed).error_code(),
            -32002
        );
        assert_eq!(A2AError::PushNotSupported.error_code(), -32003);
        assert_eq!(A2AError::UnsupportedContentType.error_code(), -32004);
    }

    #[test]
    fn error_code_security() {
        assert_eq!(A2AError::Unauthorized.error_code(), -32000);
        assert_eq!(A2AError::Forbidden.error_code(), -32005);
    }

    #[test]
    fn error_code_client() {
        assert_eq!(
            A2AError::AgentUnreachable("host".into()).error_code(),
            -32603
        );
        assert_eq!(A2AError::NoMatchingAgent.error_code(), -32603);
        assert_eq!(
            A2AError::Timeout(Duration::from_secs(30)).error_code(),
            -32603
        );
    }

    #[test]
    fn to_jsonrpc_error_format() {
        let err = A2AError::TaskNotFound("task-123".to_string());
        let json = err.to_jsonrpc_error();
        assert_eq!(json["code"], -32001);
        assert_eq!(json["message"], "Task not found: task-123");
    }

    #[test]
    fn error_display_messages() {
        assert_eq!(
            A2AError::ParseError("bad json".into()).to_string(),
            "Parse error: bad json"
        );
        assert_eq!(A2AError::Unauthorized.to_string(), "Unauthorized");
        assert_eq!(
            A2AError::Forbidden.to_string(),
            "Forbidden: insufficient trust level"
        );
        assert_eq!(
            A2AError::PushNotSupported.to_string(),
            "Push notification not supported"
        );
        assert_eq!(
            A2AError::TaskNotCancelable(TaskState::Failed).to_string(),
            "Task not cancelable in state: Failed"
        );
        assert!(A2AError::Timeout(Duration::from_secs(30))
            .to_string()
            .contains("30"));
    }

    #[test]
    fn all_variants_have_error_codes() {
        // Ensure every variant maps to a known code range
        let errors: Vec<A2AError> = vec![
            A2AError::ParseError("x".into()),
            A2AError::InvalidRequest("x".into()),
            A2AError::MethodNotFound("x".into()),
            A2AError::InvalidParams("x".into()),
            A2AError::InternalError("x".into()),
            A2AError::TaskNotFound("x".into()),
            A2AError::TaskNotCancelable(TaskState::Completed),
            A2AError::PushNotSupported,
            A2AError::UnsupportedContentType,
            A2AError::Unauthorized,
            A2AError::Forbidden,
            A2AError::AgentUnreachable("x".into()),
            A2AError::NoMatchingAgent,
            A2AError::Timeout(Duration::from_secs(1)),
        ];
        for err in &errors {
            let code = err.error_code();
            assert!(code < 0, "Error code should be negative: {}", code);
        }
    }
}
