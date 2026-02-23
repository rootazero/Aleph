//! Discord Control Plane Panel Handlers
//!
//! RPC handlers that bridge the Control Plane frontend (Leptos WASM) with
//! the Discord REST API wrapper.  Each handler corresponds to one panel
//! action in the Discord management UI.
//!
//! Methods:
//!
//! | RPC Method                  | Description                          |
//! |-----------------------------|--------------------------------------|
//! | `discord.validate_token`    | Validate a bot token against the API |
//! | `discord.save_config`       | Persist Discord config (TODO)        |
//! | `discord.list_guilds`       | List guilds the bot is in            |
//! | `discord.list_channels`     | List channels in a guild             |
//! | `discord.audit_permissions` | Audit bot permissions in a guild     |
//! | `discord.update_allowlists` | Update guild/channel allowlists      |

use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{debug, info};

use crate::gateway::channel::{ChannelId, ChannelStatus};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::interfaces::discord::api;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default channel ID for Discord when none is specified.
const DEFAULT_CHANNEL_ID: &str = "discord";

/// Minimum acceptable length for a Discord bot token.
const MIN_TOKEN_LENGTH: usize = 50;

/// Check that a Discord channel exists in the registry and is connected.
async fn require_connected_channel(
    registry: &ChannelRegistry,
    channel_id_str: &str,
) -> Result<(), (i32, String)> {
    let channel_id = ChannelId::new(channel_id_str);
    match registry.get(&channel_id).await {
        Some(channel_arc) => {
            let channel = channel_arc.read().await;
            if channel.info().status != ChannelStatus::Connected {
                Err((
                    INTERNAL_ERROR,
                    format!(
                        "Discord channel '{}' is not connected (status: {:?})",
                        channel_id_str,
                        channel.info().status,
                    ),
                ))
            } else {
                Ok(())
            }
        }
        None => Err((
            INVALID_PARAMS,
            format!("Channel '{}' not found in registry", channel_id_str),
        )),
    }
}

/// Extract a string field from params, returning an error response on failure.
fn param_str<'a>(params: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

/// Extract a u64 field from params.
fn param_u64(params: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    params.get(key).and_then(|v| v.as_u64())
}

// ---------------------------------------------------------------------------
// 1. discord.validate_token
// ---------------------------------------------------------------------------

/// Validate a Discord bot token by calling the Discord API.
///
/// **Params:** `{ "token": "Bot-Token-Here" }`
///
/// Returns bot identity JSON on success.
pub async fn handle_validate_token(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params object",
            );
        }
    };

    let token = match param_str(params, "token") {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required field: token",
            );
        }
    };

    // Basic format validation
    if token.len() < MIN_TOKEN_LENGTH {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Token too short (got {} chars, minimum {})",
                token.len(),
                MIN_TOKEN_LENGTH,
            ),
        );
    }

    debug!("Validating Discord bot token (length={})", token.len());

    match api::validate_token(token).await {
        Ok(identity) => {
            let value = serde_json::to_value(&identity).unwrap_or(json!({}));
            JsonRpcResponse::success(request.id, value)
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Token validation failed: {}", e),
        ),
    }
}

// ---------------------------------------------------------------------------
// 2. discord.save_config
// ---------------------------------------------------------------------------

/// Save Discord configuration.
///
/// **Params:** `{ "token": "...", "application_id": 123, ... }`
///
/// TODO: Actual config persistence -- for now just logs and returns success.
pub async fn handle_save_config(request: JsonRpcRequest) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params object",
            );
        }
    };

    // Validate token format if one is provided
    if let Some(token) = param_str(params, "token") {
        if token.len() < MIN_TOKEN_LENGTH {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Token too short (got {} chars, minimum {})",
                    token.len(),
                    MIN_TOKEN_LENGTH,
                ),
            );
        }
    }

    // TODO: persist config to disk / config store
    info!("discord.save_config called (persistence not yet implemented)");
    debug!("Config keys received: {:?}", params.keys().collect::<Vec<_>>());

    JsonRpcResponse::success(request.id, json!({ "success": true }))
}

// ---------------------------------------------------------------------------
// 3. discord.list_guilds
// ---------------------------------------------------------------------------

/// List all guilds the Discord bot is a member of.
///
/// **Params:** `{ "channel_id": "discord" }` (optional, defaults to "discord")
pub async fn handle_list_guilds(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let params = request
        .params
        .as_ref()
        .and_then(|v| v.as_object());

    let channel_id = params
        .and_then(|p| param_str(p, "channel_id"))
        .unwrap_or(DEFAULT_CHANNEL_ID);

    // Check channel exists and is connected
    if let Err((code, msg)) = require_connected_channel(&registry, channel_id).await {
        return JsonRpcResponse::error(request.id, code, msg);
    }

    debug!("Listing guilds for Discord channel '{}'", channel_id);

    // Get bot token from environment
    let token = match std::env::var("DISCORD_BOT_TOKEN") {
        Ok(t) => t,
        Err(_) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "DISCORD_BOT_TOKEN environment variable not set",
            );
        }
    };

    #[cfg(feature = "discord")]
    {
        let http = serenity::http::Http::new(&token);
        match api::list_guilds(&http).await {
            Ok(guilds) => {
                let value = serde_json::to_value(&guilds).unwrap_or(json!([]));
                JsonRpcResponse::success(request.id, json!({ "guilds": value }))
            }
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list guilds: {}", e),
            ),
        }
    }

    #[cfg(not(feature = "discord"))]
    {
        let _ = token;
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord feature not enabled",
        )
    }
}

// ---------------------------------------------------------------------------
// 4. discord.list_channels
// ---------------------------------------------------------------------------

/// List all channels in a Discord guild.
///
/// **Params:** `{ "channel_id": "discord", "guild_id": 123456 }`
pub async fn handle_list_channels(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params object",
            );
        }
    };

    let channel_id = param_str(params, "channel_id").unwrap_or(DEFAULT_CHANNEL_ID);

    let guild_id = match param_u64(params, "guild_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required field: guild_id",
            );
        }
    };

    // Check channel exists and is connected
    if let Err((code, msg)) = require_connected_channel(&registry, channel_id).await {
        return JsonRpcResponse::error(request.id, code, msg);
    }

    debug!(
        "Listing channels for guild {} via Discord channel '{}'",
        guild_id, channel_id,
    );

    // Get bot token from environment
    let token = match std::env::var("DISCORD_BOT_TOKEN") {
        Ok(t) => t,
        Err(_) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "DISCORD_BOT_TOKEN environment variable not set",
            );
        }
    };

    #[cfg(feature = "discord")]
    {
        let http = serenity::http::Http::new(&token);
        match api::list_channels(&http, guild_id).await {
            Ok(channels) => {
                let value = serde_json::to_value(&channels).unwrap_or(json!([]));
                JsonRpcResponse::success(request.id, json!({ "channels": value }))
            }
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to list channels: {}", e),
            ),
        }
    }

    #[cfg(not(feature = "discord"))]
    {
        let _ = (token, guild_id);
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord feature not enabled",
        )
    }
}

// ---------------------------------------------------------------------------
// 5. discord.audit_permissions
// ---------------------------------------------------------------------------

/// Audit the bot's permissions in a Discord guild.
///
/// **Params:** `{ "channel_id": "discord", "guild_id": 123456 }`
pub async fn handle_audit_permissions(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params object",
            );
        }
    };

    let channel_id = param_str(params, "channel_id").unwrap_or(DEFAULT_CHANNEL_ID);

    let guild_id = match param_u64(params, "guild_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required field: guild_id",
            );
        }
    };

    // Check channel exists and is connected
    if let Err((code, msg)) = require_connected_channel(&registry, channel_id).await {
        return JsonRpcResponse::error(request.id, code, msg);
    }

    debug!(
        "Auditing permissions for guild {} via Discord channel '{}'",
        guild_id, channel_id,
    );

    // Get bot token from environment
    let token = match std::env::var("DISCORD_BOT_TOKEN") {
        Ok(t) => t,
        Err(_) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "DISCORD_BOT_TOKEN environment variable not set",
            );
        }
    };

    #[cfg(feature = "discord")]
    {
        let http = serenity::http::Http::new(&token);
        match api::audit_guild_permissions(&http, guild_id).await {
            Ok(audit) => {
                let value = serde_json::to_value(&audit).unwrap_or(json!({}));
                JsonRpcResponse::success(request.id, value)
            }
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to audit permissions: {}", e),
            ),
        }
    }

    #[cfg(not(feature = "discord"))]
    {
        let _ = (token, guild_id);
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord feature not enabled",
        )
    }
}

// ---------------------------------------------------------------------------
// 6. discord.update_allowlists
// ---------------------------------------------------------------------------

/// Update guild and channel allowlists for the Discord channel.
///
/// **Params:** `{ "channel_id": "discord", "guilds": [123, 456], "channels": [789] }`
///
/// TODO: Actual config persistence -- for now just logs and returns success.
pub async fn handle_update_allowlists(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params object",
            );
        }
    };

    let channel_id = param_str(params, "channel_id").unwrap_or(DEFAULT_CHANNEL_ID);

    // Parse guild and channel allowlists (default to empty arrays)
    let guilds: Vec<u64> = params
        .get("guilds")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
        .unwrap_or_default();

    let channels: Vec<u64> = params
        .get("channels")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
        .unwrap_or_default();

    // Verify channel exists (not strictly required to be connected for config updates)
    let cid = ChannelId::new(channel_id);
    if registry.get(&cid).await.is_none() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Channel '{}' not found in registry", channel_id),
        );
    }

    // TODO: persist allowlists to config
    info!(
        "discord.update_allowlists called for channel '{}': guilds={:?}, channels={:?} (persistence not yet implemented)",
        channel_id, guilds, channels,
    );

    JsonRpcResponse::success(
        request.id,
        json!({
            "success": true,
            "guilds": guilds,
            "channels": channels,
        }),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_request(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    // -- validate_token -------------------------------------------------------

    #[tokio::test]
    async fn test_validate_token_missing_params() {
        let req = JsonRpcRequest::with_id("discord.validate_token", None, json!(1));
        let res = handle_validate_token(req).await;
        assert!(res.is_error());
        let err = res.error.unwrap();
        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_validate_token_missing_token_field() {
        let req = make_request("discord.validate_token", json!({}));
        let res = handle_validate_token(req).await;
        assert!(res.is_error());
        assert!(res.error.unwrap().message.contains("token"));
    }

    #[tokio::test]
    async fn test_validate_token_too_short() {
        let req = make_request("discord.validate_token", json!({ "token": "short" }));
        let res = handle_validate_token(req).await;
        assert!(res.is_error());
        assert!(res.error.unwrap().message.contains("too short"));
    }

    // -- save_config ----------------------------------------------------------

    #[tokio::test]
    async fn test_save_config_missing_params() {
        let req = JsonRpcRequest::with_id("discord.save_config", None, json!(1));
        let res = handle_save_config(req).await;
        assert!(res.is_error());
    }

    #[tokio::test]
    async fn test_save_config_success_without_token() {
        let req = make_request("discord.save_config", json!({ "application_id": 123 }));
        let res = handle_save_config(req).await;
        assert!(res.is_success());
        let result = res.result.unwrap();
        assert_eq!(result["success"], true);
    }

    #[tokio::test]
    async fn test_save_config_rejects_short_token() {
        let req = make_request(
            "discord.save_config",
            json!({ "token": "too-short", "application_id": 123 }),
        );
        let res = handle_save_config(req).await;
        assert!(res.is_error());
        assert!(res.error.unwrap().message.contains("too short"));
    }

    // -- list_guilds (without registry) ---------------------------------------

    #[tokio::test]
    async fn test_list_guilds_channel_not_found() {
        let registry = Arc::new(ChannelRegistry::new());
        let req = make_request("discord.list_guilds", json!({ "channel_id": "nonexistent" }));
        let res = handle_list_guilds(req, registry).await;
        assert!(res.is_error());
        assert!(res.error.unwrap().message.contains("not found"));
    }

    // -- list_channels --------------------------------------------------------

    #[tokio::test]
    async fn test_list_channels_missing_guild_id() {
        let registry = Arc::new(ChannelRegistry::new());
        let req = make_request("discord.list_channels", json!({ "channel_id": "discord" }));
        let res = handle_list_channels(req, registry).await;
        assert!(res.is_error());
        assert!(res.error.unwrap().message.contains("guild_id"));
    }

    // -- audit_permissions ----------------------------------------------------

    #[tokio::test]
    async fn test_audit_permissions_missing_guild_id() {
        let registry = Arc::new(ChannelRegistry::new());
        let req = make_request("discord.audit_permissions", json!({ "channel_id": "discord" }));
        let res = handle_audit_permissions(req, registry).await;
        assert!(res.is_error());
        assert!(res.error.unwrap().message.contains("guild_id"));
    }

    // -- update_allowlists ----------------------------------------------------

    #[tokio::test]
    async fn test_update_allowlists_channel_not_found() {
        let registry = Arc::new(ChannelRegistry::new());
        let req = make_request(
            "discord.update_allowlists",
            json!({
                "channel_id": "nonexistent",
                "guilds": [123],
                "channels": [456],
            }),
        );
        let res = handle_update_allowlists(req, registry).await;
        assert!(res.is_error());
        assert!(res.error.unwrap().message.contains("not found"));
    }
}
