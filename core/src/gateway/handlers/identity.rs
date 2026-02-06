//! Identity Management Handlers
//!
//! Provides RPC methods for managing AI identity/soul during sessions.
//!
//! ## Methods
//!
//! | Method | Description |
//! |--------|-------------|
//! | identity.get | Returns the effective SoulManifest |
//! | identity.set | Sets session-level identity override |
//! | identity.clear | Clears session identity override |
//! | identity.list | Lists available identity sources |
//!
//! These handlers require an IdentityResolver to be wired at Gateway initialization.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::thinker::identity::{IdentityResolver, IdentitySource};
use crate::thinker::soul::SoulManifest;

/// Shared identity resolver for handlers
pub type SharedIdentityResolver = Arc<RwLock<IdentityResolver>>;

// ============================================================================
// Request/Response Types
// ============================================================================

/// Request for identity.set
#[derive(Debug, Clone, Deserialize)]
pub struct IdentitySetRequest {
    /// The soul manifest to set as session override
    pub soul: SoulManifest,
}

/// Response for identity.get
#[derive(Debug, Clone, Serialize)]
pub struct IdentityGetResponse {
    /// The effective soul manifest
    pub soul: SoulManifest,
    /// Whether a session override is active
    pub has_session_override: bool,
}

/// Response for identity.list
#[derive(Debug, Clone, Serialize)]
pub struct IdentityListResponse {
    /// Available identity sources
    pub sources: Vec<IdentitySource>,
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Handle identity.get - returns the effective SoulManifest
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"identity.get","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "soul": {
///             "identity": "I am Aleph, your AI assistant.",
///             "directives": ["Be helpful", "Be concise"]
///         },
///         "has_session_override": false
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_get(request: JsonRpcRequest, resolver: SharedIdentityResolver) -> JsonRpcResponse {
    let resolver = resolver.read().await;

    let response = IdentityGetResponse {
        soul: resolver.resolve(),
        has_session_override: resolver.has_session_override(),
    };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize response: {}", e),
        ),
    }
}

/// Handle identity.set - sets session-level identity override
///
/// # Example Request
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "method": "identity.set",
///     "params": {
///         "soul": {
///             "identity": "I am a specialized coding assistant.",
///             "directives": ["Focus on code quality", "Explain concisely"]
///         }
///     },
///     "id": 1
/// }
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"success":true},"id":1}
/// ```
pub async fn handle_set(request: JsonRpcRequest, resolver: SharedIdentityResolver) -> JsonRpcResponse {
    let params = match &request.params {
        Some(params) => params,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: soul".to_string(),
            );
        }
    };

    let set_request: IdentitySetRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid parameters: {}", e),
            );
        }
    };

    let mut resolver = resolver.write().await;
    resolver.set_session_override(set_request.soul);

    JsonRpcResponse::success(request.id, json!({"success": true}))
}

/// Handle identity.clear - clears session identity override
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"identity.clear","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"success":true,"had_override":true},"id":1}
/// ```
pub async fn handle_clear(request: JsonRpcRequest, resolver: SharedIdentityResolver) -> JsonRpcResponse {
    let mut resolver = resolver.write().await;
    let had_override = resolver.has_session_override();
    resolver.clear_session_override();

    JsonRpcResponse::success(
        request.id,
        json!({
            "success": true,
            "had_override": had_override
        }),
    )
}

/// Handle identity.list - lists available identity sources
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"identity.list","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "result": {
///         "sources": [
///             {
///                 "source_type": "global",
///                 "path": "/Users/user/.aleph/soul.md",
///                 "loaded": true
///             },
///             {
///                 "source_type": "session",
///                 "path": "<session>",
///                 "loaded": true
///             }
///         ]
///     },
///     "id": 1
/// }
/// ```
pub async fn handle_list(request: JsonRpcRequest, resolver: SharedIdentityResolver) -> JsonRpcResponse {
    let resolver = resolver.read().await;

    let response = IdentityListResponse {
        sources: resolver.list_sources(),
    };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize response: {}", e),
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    async fn create_test_resolver() -> SharedIdentityResolver {
        Arc::new(RwLock::new(IdentityResolver::new(PathBuf::from(
            "/nonexistent",
        ))))
    }

    #[tokio::test]
    async fn test_handle_get_default() {
        let resolver = create_test_resolver().await;
        let request = JsonRpcRequest::with_id("identity.get", None, json!(1));

        let response = handle_get(request, resolver).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(!result["has_session_override"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_handle_set() {
        let resolver = create_test_resolver().await;

        let soul = SoulManifest {
            identity: "Test identity".to_string(),
            ..Default::default()
        };

        let request = JsonRpcRequest::new(
            "identity.set",
            Some(json!({ "soul": soul })),
            Some(json!(1)),
        );

        let response = handle_set(request, resolver.clone()).await;
        assert!(response.is_success());

        // Verify the override was set
        let resolver = resolver.read().await;
        assert!(resolver.has_session_override());
        let resolved = resolver.resolve();
        assert_eq!(resolved.identity, "Test identity");
    }

    #[tokio::test]
    async fn test_handle_set_missing_params() {
        let resolver = create_test_resolver().await;
        let request = JsonRpcRequest::with_id("identity.set", None, json!(1));

        let response = handle_set(request, resolver).await;

        assert!(response.is_error());
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_clear() {
        let resolver = create_test_resolver().await;

        // First set an override
        {
            let mut r = resolver.write().await;
            r.set_session_override(SoulManifest {
                identity: "Test".to_string(),
                ..Default::default()
            });
        }

        let request = JsonRpcRequest::with_id("identity.clear", None, json!(1));
        let response = handle_clear(request, resolver.clone()).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result["had_override"].as_bool().unwrap());

        // Verify cleared
        let resolver = resolver.read().await;
        assert!(!resolver.has_session_override());
    }

    #[tokio::test]
    async fn test_handle_clear_no_override() {
        let resolver = create_test_resolver().await;
        let request = JsonRpcRequest::with_id("identity.clear", None, json!(1));

        let response = handle_clear(request, resolver).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(!result["had_override"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_handle_list_empty() {
        let resolver = create_test_resolver().await;
        let request = JsonRpcRequest::with_id("identity.list", None, json!(1));

        let response = handle_list(request, resolver).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let sources = result["sources"].as_array().unwrap();
        assert!(sources.is_empty());
    }

    #[tokio::test]
    async fn test_handle_list_with_session() {
        let resolver = create_test_resolver().await;

        // Set an override
        {
            let mut r = resolver.write().await;
            r.set_session_override(SoulManifest {
                identity: "Test".to_string(),
                ..Default::default()
            });
        }

        let request = JsonRpcRequest::with_id("identity.list", None, json!(1));
        let response = handle_list(request, resolver).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let sources = result["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0]["source_type"], "session");
    }
}
