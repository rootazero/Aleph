//! Profiles RPC Handlers
//!
//! Handlers for auth profile management: list and status queries.
//! Profiles are managed through the AuthProfileManager from the providers module.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, RESOURCE_NOT_FOUND};
use crate::providers::profile_manager::{AuthProfileManager, ProfileInfo};

// ============================================================================
// Request/Response Types
// ============================================================================

/// Parameters for profiles.list
#[derive(Debug, Deserialize, Default)]
pub struct ProfilesListRequest {
    /// Optional filter by provider
    #[serde(default)]
    pub provider: Option<String>,
}

/// Response for profiles.list
#[derive(Debug, Serialize)]
pub struct ProfilesListResponse {
    /// List of profile info
    pub profiles: Vec<ProfileInfo>,
}

/// Parameters for profiles.status
#[derive(Debug, Deserialize)]
pub struct ProfilesStatusRequest {
    /// Profile ID to query
    pub profile_id: String,
}

/// Response for profiles.status
#[derive(Debug, Serialize)]
pub struct ProfilesStatusResponse {
    /// Profile ID
    pub id: String,
    /// Provider ID
    pub provider: String,
    /// Whether the profile is available (not in cooldown, key resolvable)
    pub is_available: bool,
    /// Remaining cooldown in milliseconds (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_remaining_ms: Option<u64>,
    /// Current failure count
    pub failure_count: u32,
    /// Error message (if not available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// Handlers
// ============================================================================

/// Handle profiles.list requests
///
/// Lists all configured auth profiles with their current status.
/// Optionally filters by provider.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"profiles.list","id":1}
/// ```
///
/// Or with filter:
///
/// ```json
/// {"jsonrpc":"2.0","method":"profiles.list","params":{"provider":"anthropic"},"id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {
///   "jsonrpc":"2.0",
///   "result":{
///     "profiles":[
///       {
///         "id":"anthropic_primary",
///         "provider":"anthropic",
///         "tier":"primary",
///         "in_cooldown":false,
///         "disabled":false,
///         "failure_count":0,
///         "uses_env_var":true,
///         "key_resolvable":true
///       }
///     ]
///   },
///   "id":1
/// }
/// ```
pub async fn handle_list(
    request: JsonRpcRequest,
    profile_manager: Arc<AuthProfileManager>,
) -> JsonRpcResponse {
    // Parse params (can be empty/missing)
    let params: ProfilesListRequest = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => ProfilesListRequest::default(),
    };

    // Get profiles
    let profiles = match &params.provider {
        Some(provider) => profile_manager.profiles_for_provider(provider),
        None => profile_manager.list_profiles(),
    };

    JsonRpcResponse::success(
        request.id,
        json!(ProfilesListResponse { profiles }),
    )
}

/// Handle profiles.status requests
///
/// Gets detailed status for a specific profile.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"profiles.status","params":{"profile_id":"anthropic_primary"},"id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {
///   "jsonrpc":"2.0",
///   "result":{
///     "id":"anthropic_primary",
///     "provider":"anthropic",
///     "is_available":true,
///     "failure_count":0
///   },
///   "id":1
/// }
/// ```
pub async fn handle_status(
    request: JsonRpcRequest,
    profile_manager: Arc<AuthProfileManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: ProfilesStatusRequest = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
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
                "Missing params: profile_id required".to_string(),
            );
        }
    };

    // Find the profile
    let profiles = profile_manager.list_profiles();
    let profile = profiles.iter().find(|p| p.id == params.profile_id);

    match profile {
        Some(p) => {
            // Determine availability
            let is_available = !p.in_cooldown && !p.disabled && p.key_resolvable;

            // Build error message if not available
            let error = if !is_available {
                let mut reasons = Vec::new();
                if p.in_cooldown {
                    reasons.push("in cooldown");
                }
                if p.disabled {
                    reasons.push("disabled");
                }
                if !p.key_resolvable {
                    reasons.push("API key not resolvable");
                }
                Some(reasons.join(", "))
            } else {
                None
            };

            let response = ProfilesStatusResponse {
                id: p.id.clone(),
                provider: p.provider.clone(),
                is_available,
                cooldown_remaining_ms: p.cooldown_remaining_ms,
                failure_count: p.failure_count,
                error,
            };

            JsonRpcResponse::success(request.id, json!(response))
        }
        None => JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            format!("Profile not found: {}", params.profile_id),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn create_test_manager() -> (TempDir, Arc<AuthProfileManager>) {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("profiles.toml");
        let content = r#"
            [profiles.anthropic_primary]
            provider = "anthropic"
            api_key = "sk-ant-primary"
            tier = "primary"

            [profiles.openai_main]
            provider = "openai"
            api_key = "sk-openai-main"
            tier = "primary"
        "#;
        std::fs::write(&config_path, content).unwrap();

        let agents_dir = temp_dir.path().join("agents");
        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();
        (temp_dir, Arc::new(manager))
    }

    #[tokio::test]
    async fn test_list_all_profiles() {
        let (_temp, manager) = create_test_manager();

        let request = JsonRpcRequest::new("profiles.list", None, Some(json!(1)));
        let response = handle_list(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
    }

    #[tokio::test]
    async fn test_list_with_provider_filter() {
        let (_temp, manager) = create_test_manager();

        let request = JsonRpcRequest::new(
            "profiles.list",
            Some(json!({"provider": "anthropic"})),
            Some(json!(1)),
        );
        let response = handle_list(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let profiles = result["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0]["provider"], "anthropic");
    }

    #[tokio::test]
    async fn test_status_existing_profile() {
        let (_temp, manager) = create_test_manager();

        let request = JsonRpcRequest::new(
            "profiles.status",
            Some(json!({"profile_id": "anthropic_primary"})),
            Some(json!(1)),
        );
        let response = handle_status(request, manager).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["id"], "anthropic_primary");
        assert_eq!(result["provider"], "anthropic");
        assert_eq!(result["is_available"], true);
        assert_eq!(result["failure_count"], 0);
    }

    #[tokio::test]
    async fn test_status_not_found() {
        let (_temp, manager) = create_test_manager();

        let request = JsonRpcRequest::new(
            "profiles.status",
            Some(json!({"profile_id": "nonexistent"})),
            Some(json!(1)),
        );
        let response = handle_status(request, manager).await;

        assert!(response.is_error());
        let error = response.error.unwrap();
        assert_eq!(error.code, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_status_missing_params() {
        let (_temp, manager) = create_test_manager();

        let request = JsonRpcRequest::new("profiles.status", None, Some(json!(1)));
        let response = handle_status(request, manager).await;

        assert!(response.is_error());
        let error = response.error.unwrap();
        assert_eq!(error.code, INVALID_PARAMS);
    }
}
