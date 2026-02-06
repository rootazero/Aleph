//! Logs RPC Handlers
//!
//! Handlers for log level control and log directory access.

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::logging::{get_log_directory, get_log_level, set_log_level, LogLevel};

/// Get current log level
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"logs.getLevel","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"level":"info"},"id":1}
/// ```
pub async fn handle_get_level(request: JsonRpcRequest) -> JsonRpcResponse {
    let level = get_log_level();
    JsonRpcResponse::success(
        request.id,
        json!({
            "level": level.to_filter_string()
        }),
    )
}

/// Parameters for logs.setLevel
#[derive(Debug, Deserialize)]
struct SetLevelParams {
    level: String,
}

/// Set log level
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"logs.setLevel","params":{"level":"debug"},"id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"ok":true,"level":"debug"},"id":1}
/// ```
pub async fn handle_set_level(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: SetLevelParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: level required".to_string(),
            );
        }
    };

    let level = match LogLevel::parse(&params.level) {
        Some(l) => l,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Invalid log level: {}. Valid values: error, warn, info, debug, trace",
                    params.level
                ),
            );
        }
    };

    set_log_level(level);

    JsonRpcResponse::success(
        request.id,
        json!({
            "ok": true,
            "level": level.to_filter_string()
        }),
    )
}

/// Get log directory path
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"logs.getDirectory","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"directory":"/Users/user/.aleph/logs"},"id":1}
/// ```
pub async fn handle_get_directory(request: JsonRpcRequest) -> JsonRpcResponse {
    match get_log_directory() {
        Ok(dir) => JsonRpcResponse::success(
            request.id,
            json!({
                "directory": dir.to_string_lossy()
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            -32000, // Server error
            format!("Failed to get log directory: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_get_level() {
        let request = JsonRpcRequest::with_id("logs.getLevel", None, json!(1));
        let response = handle_get_level(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result["level"].is_string());
    }

    #[tokio::test]
    async fn test_set_level() {
        let request = JsonRpcRequest::new(
            "logs.setLevel",
            Some(json!({"level": "debug"})),
            Some(json!(1)),
        );
        let response = handle_set_level(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["level"], "debug");

        // Reset to info
        set_log_level(LogLevel::Info);
    }

    #[tokio::test]
    async fn test_set_level_invalid() {
        let request = JsonRpcRequest::new(
            "logs.setLevel",
            Some(json!({"level": "invalid"})),
            Some(json!(1)),
        );
        let response = handle_set_level(request).await;

        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_get_directory() {
        let request = JsonRpcRequest::with_id("logs.getDirectory", None, json!(1));
        let response = handle_get_directory(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result["directory"].is_string());
        assert!(result["directory"].as_str().unwrap().contains("logs"));
    }
}
