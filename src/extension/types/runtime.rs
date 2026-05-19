//! Runtime interaction types for plugin extensions
//!
//! This module contains types for plugin runtime interactions including:
//! - Background services (lifecycle management)
//! - HTTP routes (plugin-provided endpoints)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Service Types (V2 Background Services)
// =============================================================================

/// Service state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ServiceState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

/// Running service information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub id: String,
    pub plugin_id: String,
    pub name: String,
    pub state: ServiceState,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

/// Service lifecycle result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResult {
    pub success: bool,
    pub message: Option<String>,
    pub data: Option<serde_json::Value>,
}

impl ServiceResult {
    pub fn ok() -> Self {
        Self {
            success: true,
            message: None,
            data: None,
        }
    }

    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(msg.into()),
            data: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(msg.into()),
            data: None,
        }
    }
}

// =============================================================================
// HTTP Route Types (V2 Plugin HTTP Endpoints)
// =============================================================================

/// HTTP request from plugin route
///
/// Represents an incoming HTTP request to a plugin-provided endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    /// HTTP method (e.g., "GET", "POST", "PUT", "DELETE")
    pub method: String,
    /// Request path (e.g., "/api/webhook")
    pub path: String,
    /// HTTP headers as key-value pairs
    pub headers: HashMap<String, String>,
    /// Query string parameters
    pub query: HashMap<String, String>,
    /// Request body (for POST/PUT/PATCH requests)
    pub body: Option<serde_json::Value>,
    /// Path parameters extracted from route patterns (e.g., ":id" -> "123")
    pub path_params: HashMap<String, String>,
}

/// HTTP response from plugin handler
///
/// Response to send back to the HTTP client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// HTTP status code (e.g., 200, 404, 500)
    pub status: u16,
    /// HTTP response headers
    pub headers: HashMap<String, String>,
    /// Response body
    pub body: Option<serde_json::Value>,
}

impl HttpResponse {
    /// Create a 200 OK response with no body
    pub fn ok() -> Self {
        Self {
            status: 200,
            headers: HashMap::new(),
            body: None,
        }
    }

    /// Create a 200 OK response with JSON body
    pub fn json(data: serde_json::Value) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        Self {
            status: 200,
            headers,
            body: Some(data),
        }
    }

    /// Create an error response with the given status code and message
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: Some(serde_json::json!({"error": message.into()})),
        }
    }

    /// Create a 404 Not Found response
    pub fn not_found() -> Self {
        Self::error(404, "Not Found")
    }

    /// Create a 400 Bad Request response
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::error(400, message)
    }

    /// Create a 500 Internal Server Error response
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::error(500, message)
    }
}
