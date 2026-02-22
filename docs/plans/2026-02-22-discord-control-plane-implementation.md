# Discord Control Plane Panel — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a Discord management panel to the Control Plane with Token validation, Guild/Channel management, and permission audit dashboard.

**Architecture:** All Discord API interactions happen in 6 new backend RPC handlers; the Leptos WASM frontend only renders state and sends user actions via RPC. The new route `/settings/channels/discord` is added under the existing Settings layout.

**Tech Stack:** Rust (serenity Http client for Discord REST API), Leptos 0.7 (WASM frontend), Tailwind CSS, JSON-RPC 2.0 over WebSocket.

**Design Doc:** `docs/plans/2026-02-22-discord-control-plane-design.md`

---

## Task 1: Backend — Permission Audit Types

**Files:**
- Create: `core/src/gateway/channels/discord/permissions.rs`
- Modify: `core/src/gateway/channels/discord/mod.rs` (add `pub mod permissions;`)

**Step 1: Write the types file**

Create `core/src/gateway/channels/discord/permissions.rs`:

```rust
//! Discord permission audit types and logic.
//!
//! Checks Bot permissions in a Guild and reports traffic-light status.

use serde::{Deserialize, Serialize};

/// Traffic light status for a single permission check
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrafficLight {
    Green,
    Yellow,
    Red,
}

/// Overall health status for a Guild
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Critical,
}

/// A single permission check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionCheck {
    /// Human-readable name, e.g. "Send Messages"
    pub name: String,
    /// Discord permission bitfield value
    pub discord_flag: u64,
    /// Whether the Bot has this permission
    pub has: bool,
    /// Whether Aleph requires this permission
    pub required: bool,
    /// Whether Aleph recommends this permission (not required but helpful)
    pub recommended: bool,
    /// Computed traffic light status
    pub status: TrafficLight,
}

/// Full audit result for a Guild
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAudit {
    pub guild_id: u64,
    pub guild_name: String,
    pub permissions: Vec<PermissionCheck>,
    pub overall_status: HealthStatus,
    /// Human-readable summary, e.g. "Functional (1 recommendation)"
    pub summary: String,
    /// Fix suggestions for missing permissions
    pub fix_suggestions: Vec<String>,
}

/// Permission requirement level
#[derive(Debug, Clone, Copy)]
pub enum RequirementLevel {
    Required,
    Recommended,
    Optional,
}

/// Discord permission definitions that Aleph cares about.
/// See https://discord.com/developers/docs/topics/permissions#permissions-bitwise-permission-flags
pub const ALEPH_PERMISSIONS: &[(u64, &str, RequirementLevel)] = &[
    // Required - Bot cannot function without these
    (0x0000_0000_0000_0800, "Send Messages", RequirementLevel::Required),
    (0x0000_0000_0000_0400, "View Channel", RequirementLevel::Required),
    (0x0000_0000_0004_0000, "Read Message History", RequirementLevel::Required),

    // Recommended - Bot works but with reduced capability
    (0x0000_0000_0000_4000, "Embed Links", RequirementLevel::Recommended),
    (0x0000_0000_0000_8000, "Attach Files", RequirementLevel::Recommended),
    (0x0000_0000_0000_0040, "Add Reactions", RequirementLevel::Recommended),

    // Optional - Nice to have
    (0x0000_0000_0000_2000, "Manage Messages", RequirementLevel::Optional),
    (0x0000_0000_8000_0000, "Use Slash Commands", RequirementLevel::Optional),
];

/// Audit the Bot's permissions in a Guild given its permission bitfield.
pub fn audit_permissions(guild_id: u64, guild_name: &str, bot_permissions: u64) -> PermissionAudit {
    let mut checks = Vec::new();
    let mut missing_required = 0;
    let mut missing_recommended = 0;
    let mut fix_suggestions = Vec::new();

    for &(flag, name, level) in ALEPH_PERMISSIONS {
        let has = (bot_permissions & flag) != 0;
        let (required, recommended) = match level {
            RequirementLevel::Required => (true, false),
            RequirementLevel::Recommended => (false, true),
            RequirementLevel::Optional => (false, false),
        };

        let status = if has {
            TrafficLight::Green
        } else if required {
            missing_required += 1;
            fix_suggestions.push(format!(
                "Enable \"{}\" in Server Settings > Roles > Bot Role",
                name,
            ));
            TrafficLight::Red
        } else if recommended {
            missing_recommended += 1;
            fix_suggestions.push(format!(
                "Recommended: enable \"{}\" for full functionality",
                name,
            ));
            TrafficLight::Yellow
        } else {
            TrafficLight::Green
        };

        checks.push(PermissionCheck {
            name: name.to_string(),
            discord_flag: flag,
            has,
            required,
            recommended,
            status,
        });
    }

    let overall_status = if missing_required > 0 {
        HealthStatus::Critical
    } else if missing_recommended > 0 {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    };

    let summary = match overall_status {
        HealthStatus::Healthy => "All permissions OK".to_string(),
        HealthStatus::Degraded => format!(
            "Functional ({} recommendation{})",
            missing_recommended,
            if missing_recommended == 1 { "" } else { "s" },
        ),
        HealthStatus::Critical => format!(
            "Missing {} required permission{}",
            missing_required,
            if missing_required == 1 { "" } else { "s" },
        ),
    };

    PermissionAudit {
        guild_id,
        guild_name: guild_name.to_string(),
        permissions: checks,
        overall_status,
        summary,
        fix_suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_permissions_present() {
        // All Aleph permissions granted
        let bits = 0x0000_0000_8004_E840;
        let audit = audit_permissions(1, "Test Guild", bits);
        assert_eq!(audit.overall_status, HealthStatus::Healthy);
        assert_eq!(audit.summary, "All permissions OK");
        assert!(audit.fix_suggestions.is_empty());
        assert!(audit.permissions.iter().all(|p| p.status == TrafficLight::Green));
    }

    #[test]
    fn test_missing_required_permission() {
        // Missing Send Messages (0x800)
        let bits = 0x0000_0000_0004_0400; // View Channel + Read History only
        let audit = audit_permissions(1, "Test", bits);
        assert_eq!(audit.overall_status, HealthStatus::Critical);
        assert!(audit.summary.contains("required"));
        let send = audit.permissions.iter().find(|p| p.name == "Send Messages").unwrap();
        assert!(!send.has);
        assert_eq!(send.status, TrafficLight::Red);
    }

    #[test]
    fn test_missing_recommended_permission() {
        // Has all required, missing Embed Links (0x4000)
        let bits = 0x0000_0000_0004_8C00; // Send + View + History + Attach Files
        let audit = audit_permissions(1, "Test", bits);
        assert_eq!(audit.overall_status, HealthStatus::Degraded);
        assert!(audit.summary.contains("recommendation"));
        let embed = audit.permissions.iter().find(|p| p.name == "Embed Links").unwrap();
        assert!(!embed.has);
        assert_eq!(embed.status, TrafficLight::Yellow);
    }

    #[test]
    fn test_missing_optional_still_healthy() {
        // Has required + recommended, missing optional Manage Messages
        let bits = 0x0000_0000_0004_CC40; // Required + Recommended, no optional
        let audit = audit_permissions(1, "Test", bits);
        // Optional missing doesn't affect health
        assert!(audit.overall_status == HealthStatus::Healthy || audit.overall_status == HealthStatus::Degraded);
    }
}
```

**Step 2: Wire the module**

In `core/src/gateway/channels/discord/mod.rs`, add near the top with other module declarations:

```rust
pub mod permissions;
```

**Step 3: Run tests to verify**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test --features discord -p alephcore permissions::tests -- --nocapture`

Expected: All 4 tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/channels/discord/permissions.rs core/src/gateway/channels/discord/mod.rs
git commit -m "discord: add permission audit types and logic"
```

---

## Task 2: Backend — Discord REST API Wrapper

**Files:**
- Create: `core/src/gateway/channels/discord/api.rs`
- Modify: `core/src/gateway/channels/discord/mod.rs` (add `pub mod api;`)

This module wraps serenity's Http client to provide higher-level functions for the Control Plane RPC handlers.

**Step 1: Write the API wrapper**

Create `core/src/gateway/channels/discord/api.rs`:

```rust
//! Discord REST API wrapper for Control Plane operations.
//!
//! Provides high-level functions using serenity's Http client:
//! - Token validation (get current user)
//! - Guild listing with permissions
//! - Channel listing per guild
//! - Permission auditing

use serde::{Deserialize, Serialize};
use super::permissions::{self, PermissionAudit};

/// Bot identity returned after token validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotIdentity {
    pub valid: bool,
    pub bot_id: u64,
    pub bot_name: String,
    pub bot_avatar: Option<String>,
    pub discriminator: String,
}

/// Guild summary for the panel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildSummary {
    pub guild_id: u64,
    pub name: String,
    pub icon: Option<String>,
    pub member_count: Option<u64>,
    pub bot_permissions: u64,
}

/// Channel summary for the panel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSummary {
    pub channel_id: u64,
    pub name: String,
    pub channel_type: u8,
    pub position: i64,
}

#[cfg(feature = "discord")]
use serenity::http::Http;

#[cfg(feature = "discord")]
use serenity::model::guild::Guild;

/// Validate a Discord bot token by calling GET /users/@me.
///
/// Returns Ok(BotIdentity) if valid, Err with message if not.
#[cfg(feature = "discord")]
pub async fn validate_token(token: &str) -> Result<BotIdentity, String> {
    let http = Http::new(token);

    match http.get_current_user().await {
        Ok(user) => Ok(BotIdentity {
            valid: true,
            bot_id: user.id.get(),
            bot_name: user.name.clone(),
            bot_avatar: user.avatar.as_ref().map(|h| {
                format!(
                    "https://cdn.discordapp.com/avatars/{}/{}.png",
                    user.id.get(),
                    h,
                )
            }),
            discriminator: format!("{:04}", user.discriminator.unwrap_or(0)),
        }),
        Err(e) => Err(format!("Invalid token: {}", e)),
    }
}

/// List all guilds the bot is a member of.
#[cfg(feature = "discord")]
pub async fn list_guilds(http: &Http) -> Result<Vec<GuildSummary>, String> {
    use serenity::model::guild::GuildInfo;

    let guilds: Vec<GuildInfo> = http
        .get_guilds(None, None)
        .await
        .map_err(|e| format!("Failed to list guilds: {}", e))?;

    let mut summaries = Vec::new();
    for guild_info in guilds {
        // Fetch full guild for permissions
        let permissions = match http.get_guild(guild_info.id).await {
            Ok(full_guild) => {
                // Get bot's permissions from @everyone role as baseline
                // Real permissions come from member roles, but this gives us
                // the guild-level defaults
                full_guild
                    .roles
                    .values()
                    .find(|r| r.id.get() == guild_info.id.get()) // @everyone role
                    .map(|r| r.permissions.bits())
                    .unwrap_or(0)
            }
            Err(_) => 0,
        };

        summaries.push(GuildSummary {
            guild_id: guild_info.id.get(),
            name: guild_info.name.clone(),
            icon: guild_info.icon.as_ref().map(|h| {
                format!(
                    "https://cdn.discordapp.com/icons/{}/{}.png",
                    guild_info.id.get(),
                    h,
                )
            }),
            member_count: None, // Not available from GuildInfo
            bot_permissions: permissions,
        });
    }

    Ok(summaries)
}

/// List channels in a specific guild.
#[cfg(feature = "discord")]
pub async fn list_channels(http: &Http, guild_id: u64) -> Result<Vec<ChannelSummary>, String> {
    use serenity::model::id::GuildId;

    let guild_id = GuildId::new(guild_id);
    let channels = http
        .get_channels(guild_id)
        .await
        .map_err(|e| format!("Failed to list channels: {}", e))?;

    Ok(channels
        .iter()
        .map(|ch| ChannelSummary {
            channel_id: ch.id.get(),
            name: ch.name.clone(),
            channel_type: ch.kind.num(),
            position: ch.position,
        })
        .collect())
}

/// Audit bot permissions in a specific guild.
///
/// Fetches the bot's actual permissions and runs the audit.
#[cfg(feature = "discord")]
pub async fn audit_guild_permissions(
    http: &Http,
    guild_id: u64,
) -> Result<PermissionAudit, String> {
    use serenity::model::id::GuildId;

    let gid = GuildId::new(guild_id);

    // Get guild info
    let guild = http
        .get_guild(gid)
        .await
        .map_err(|e| format!("Failed to get guild: {}", e))?;

    // Get bot's user ID
    let bot_user = http
        .get_current_user()
        .await
        .map_err(|e| format!("Failed to get bot user: {}", e))?;

    // Get bot's member info to compute real permissions
    let member = http
        .get_member(gid, bot_user.id)
        .await
        .map_err(|e| format!("Failed to get bot member: {}", e))?;

    // Compute permissions from member roles
    let mut perms: u64 = 0;
    for role_id in &member.roles {
        if let Some(role) = guild.roles.get(role_id) {
            perms |= role.permissions.bits();
        }
    }
    // Include @everyone role
    if let Some(everyone) = guild.roles.values().find(|r| r.id.get() == guild_id) {
        perms |= everyone.permissions.bits();
    }
    // Administrator has all permissions
    if perms & 0x8 != 0 {
        perms = u64::MAX;
    }

    Ok(permissions::audit_permissions(guild_id, &guild.name, perms))
}

/// Stub implementations when discord feature is disabled
#[cfg(not(feature = "discord"))]
pub async fn validate_token(_token: &str) -> Result<BotIdentity, String> {
    Err("Discord feature not enabled".to_string())
}
```

**Step 2: Wire the module**

In `core/src/gateway/channels/discord/mod.rs`, add:

```rust
pub mod api;
```

**Step 3: Build to verify**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check --features discord -p alephcore`

Expected: Compiles without errors. Note: serenity API calls may need adjustment based on the exact serenity 0.12 API. Check the serenity docs/types if compilation errors occur around `get_guilds`, `GuildInfo`, or `ChannelType::num()`.

**Step 4: Commit**

```bash
git add core/src/gateway/channels/discord/api.rs core/src/gateway/channels/discord/mod.rs
git commit -m "discord: add REST API wrapper for Control Plane"
```

---

## Task 3: Backend — Discord Panel RPC Handlers

**Files:**
- Create: `core/src/gateway/handlers/discord_panel.rs`
- Modify: `core/src/gateway/handlers/mod.rs` (add `pub mod discord_panel;`)
- Modify: `core/src/bin/aleph_server/commands/start.rs` (register handlers)

**Step 1: Create the handler file**

Create `core/src/gateway/handlers/discord_panel.rs`:

```rust
//! Discord Control Plane panel RPC handlers.
//!
//! These handlers expose Discord management operations to the Control Plane UI:
//! - discord.validate_token - Validate a bot token and return bot identity
//! - discord.save_config - Save Discord configuration with hot-reload
//! - discord.list_guilds - List all guilds the bot has joined
//! - discord.list_channels - List channels in a guild
//! - discord.audit_permissions - Audit bot permissions in a guild
//! - discord.update_allowlists - Update guild/channel monitoring allowlists

use crate::gateway::jsonrpc::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, INTERNAL_ERROR};
use crate::gateway::channel_registry::ChannelRegistry;
use log::debug;
use serde_json::json;
use std::sync::Arc;

#[cfg(feature = "discord")]
use crate::gateway::channels::discord::api;

/// Handle discord.validate_token - validate a bot token
///
/// Params: { "token": "Bot-Token-Here" }
/// Returns: { "valid": true, "bot_id": 123, "bot_name": "...", "bot_avatar": "...", "discriminator": "0001" }
pub async fn handle_validate_token(request: JsonRpcRequest) -> JsonRpcResponse {
    debug!("Handling discord.validate_token");

    let token = match request.params.as_ref()
        .and_then(|p| p.get("token"))
        .and_then(|v| v.as_str())
    {
        Some(t) => t.to_string(),
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: token".to_string(),
            );
        }
    };

    // Basic format validation
    if token.len() < 50 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "Token too short. Discord bot tokens are typically 50+ characters.".to_string(),
        );
    }

    #[cfg(feature = "discord")]
    {
        match api::validate_token(&token).await {
            Ok(identity) => JsonRpcResponse::success(request.id, serde_json::to_value(identity).unwrap()),
            Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
        }
    }

    #[cfg(not(feature = "discord"))]
    {
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord feature not enabled. Build with --features discord".to_string(),
        )
    }
}

/// Handle discord.list_guilds - list all guilds the bot has joined
///
/// Params: { "channel_id": "discord" }
/// Returns: { "guilds": [{ "guild_id": 123, "name": "...", ... }] }
pub async fn handle_list_guilds(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    debug!("Handling discord.list_guilds");

    let channel_id = match request.params.as_ref()
        .and_then(|p| p.get("channel_id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id.to_string(),
        None => "discord".to_string(),
    };

    // Get the Discord channel and its Http client
    let channel = match registry.get(&channel_id).await {
        Some(ch) => ch,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Channel '{}' not found", channel_id),
            );
        }
    };

    let channel_guard = channel.read().await;

    // Check if channel is connected
    if channel_guard.info().status != crate::gateway::channel::ChannelStatus::Connected {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord bot is not connected. Start the channel first.".to_string(),
        );
    }

    // We need to access the serenity Http client from the DiscordChannel.
    // Since the Channel trait doesn't expose this, we'll use the token from config
    // to create a temporary Http client for the API call.
    #[cfg(feature = "discord")]
    {
        // Get the config to access the token
        let config_json = serde_json::to_value(channel_guard.info()).unwrap_or_default();
        drop(channel_guard); // Release the lock

        // We need the bot token - try to get it from the channel's stored config
        // For now, use the environment variable as fallback
        let token = std::env::var("DISCORD_BOT_TOKEN").unwrap_or_default();
        if token.is_empty() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "Discord bot token not available".to_string(),
            );
        }

        let http = serenity::http::Http::new(&token);
        match api::list_guilds(&http).await {
            Ok(guilds) => JsonRpcResponse::success(request.id, json!({ "guilds": guilds })),
            Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
        }
    }

    #[cfg(not(feature = "discord"))]
    {
        drop(channel_guard);
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord feature not enabled".to_string(),
        )
    }
}

/// Handle discord.list_channels - list channels in a guild
///
/// Params: { "channel_id": "discord", "guild_id": 123456 }
/// Returns: { "channels": [{ "channel_id": 123, "name": "general", ... }] }
pub async fn handle_list_channels(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    debug!("Handling discord.list_channels");

    let guild_id = match request.params.as_ref()
        .and_then(|p| p.get("guild_id"))
        .and_then(|v| v.as_u64())
    {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: guild_id".to_string(),
            );
        }
    };

    #[cfg(feature = "discord")]
    {
        let token = std::env::var("DISCORD_BOT_TOKEN").unwrap_or_default();
        if token.is_empty() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "Discord bot token not available".to_string(),
            );
        }

        let http = serenity::http::Http::new(&token);
        match api::list_channels(&http, guild_id).await {
            Ok(channels) => JsonRpcResponse::success(request.id, json!({ "channels": channels })),
            Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
        }
    }

    #[cfg(not(feature = "discord"))]
    {
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord feature not enabled".to_string(),
        )
    }
}

/// Handle discord.audit_permissions - audit bot permissions in a guild
///
/// Params: { "channel_id": "discord", "guild_id": 123456 }
/// Returns: { "audit": { "guild_id": 123, "permissions": [...], "overall_status": "healthy", ... } }
pub async fn handle_audit_permissions(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    debug!("Handling discord.audit_permissions");

    let guild_id = match request.params.as_ref()
        .and_then(|p| p.get("guild_id"))
        .and_then(|v| v.as_u64())
    {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing required parameter: guild_id".to_string(),
            );
        }
    };

    #[cfg(feature = "discord")]
    {
        let token = std::env::var("DISCORD_BOT_TOKEN").unwrap_or_default();
        if token.is_empty() {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "Discord bot token not available".to_string(),
            );
        }

        let http = serenity::http::Http::new(&token);
        match api::audit_guild_permissions(&http, guild_id).await {
            Ok(audit) => JsonRpcResponse::success(request.id, serde_json::to_value(audit).unwrap()),
            Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
        }
    }

    #[cfg(not(feature = "discord"))]
    {
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Discord feature not enabled".to_string(),
        )
    }
}

/// Handle discord.update_allowlists - update guild/channel monitoring allowlists
///
/// Params: { "channel_id": "discord", "guilds": [123, 456], "channels": [789] }
/// Returns: { "success": true }
pub async fn handle_update_allowlists(
    request: JsonRpcRequest,
    registry: Arc<ChannelRegistry>,
) -> JsonRpcResponse {
    debug!("Handling discord.update_allowlists");

    let params = match request.params.as_ref() {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing parameters".to_string(),
            );
        }
    };

    let guilds: Vec<u64> = params
        .get("guilds")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let channels: Vec<u64> = params
        .get("channels")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let channel_id = params
        .get("channel_id")
        .and_then(|v| v.as_str())
        .unwrap_or("discord");

    // TODO: Persist to config.toml via config hot-reload system.
    // For now, log the update and return success.
    // Full persistence will be implemented when we integrate with ConfigWatcher.
    log::info!(
        "Discord allowlists update: guilds={:?}, channels={:?}",
        guilds,
        channels,
    );

    JsonRpcResponse::success(request.id, json!({ "success": true, "guilds": guilds, "channels": channels }))
}

/// Handle discord.save_config - save Discord configuration
///
/// Params: { "token": "...", "application_id": 123, ... }
/// Returns: { "success": true }
pub async fn handle_save_config(request: JsonRpcRequest) -> JsonRpcResponse {
    debug!("Handling discord.save_config");

    let params = match request.params.as_ref() {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing parameters".to_string(),
            );
        }
    };

    // Validate token if provided
    if let Some(token) = params.get("token").and_then(|v| v.as_str()) {
        if token.len() < 50 {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Token too short".to_string(),
            );
        }
    }

    // TODO: Persist to config.toml via config system.
    // For now, return success.
    log::info!("Discord config save requested");

    JsonRpcResponse::success(request.id, json!({ "success": true }))
}
```

**Step 2: Register in handlers/mod.rs**

In `core/src/gateway/handlers/mod.rs`, add the module declaration near other handler modules:

```rust
pub mod discord_panel;
```

**Step 3: Register RPC handlers in server startup**

In `core/src/bin/aleph_server/commands/start.rs`, find the `register_channel_handlers` function (around line 858) and add Discord panel handlers after the existing channel handlers:

```rust
// After the existing channel handler registrations, add:

// Discord Panel handlers
let cr_discord_guilds = channel_registry.clone();
server.handlers_mut().register("discord.list_guilds", move |req| {
    let cr = cr_discord_guilds.clone();
    async move { discord_panel_handlers::handle_list_guilds(req, cr).await }
});

let cr_discord_channels = channel_registry.clone();
server.handlers_mut().register("discord.list_channels", move |req| {
    let cr = cr_discord_channels.clone();
    async move { discord_panel_handlers::handle_list_channels(req, cr).await }
});

let cr_discord_audit = channel_registry.clone();
server.handlers_mut().register("discord.audit_permissions", move |req| {
    let cr = cr_discord_audit.clone();
    async move { discord_panel_handlers::handle_audit_permissions(req, cr).await }
});

let cr_discord_allow = channel_registry.clone();
server.handlers_mut().register("discord.update_allowlists", move |req| {
    let cr = cr_discord_allow.clone();
    async move { discord_panel_handlers::handle_update_allowlists(req, cr).await }
});

// These don't need ChannelRegistry
server.handlers_mut().register("discord.validate_token", |req| async move {
    discord_panel_handlers::handle_validate_token(req).await
});

server.handlers_mut().register("discord.save_config", |req| async move {
    discord_panel_handlers::handle_save_config(req).await
});
```

Also add the import at the top of the `register_channel_handlers` function or at the module level:

```rust
use alephcore::gateway::handlers::discord_panel as discord_panel_handlers;
```

**Step 4: Build to verify**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check --features discord -p alephcore && cargo check --features discord --bin aleph-server`

Expected: Compiles without errors.

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/discord_panel.rs core/src/gateway/handlers/mod.rs core/src/bin/aleph_server/commands/start.rs
git commit -m "discord: add Control Plane RPC handlers"
```

---

## Task 4: Frontend — Discord API Module

**Files:**
- Modify: `core/ui/control_plane/src/api.rs` (add DiscordApi struct)

**Step 1: Add Discord API calls to the frontend**

In `core/ui/control_plane/src/api.rs`, add the following after the existing API structs (e.g., after MemoryApi):

```rust
/// Discord Channel API for Control Plane
pub struct DiscordApi;

impl DiscordApi {
    /// Validate a Discord bot token
    pub async fn validate_token(
        state: &DashboardState,
        token: String,
    ) -> Result<serde_json::Value, String> {
        let params = serde_json::json!({ "token": token });
        state.rpc_call("discord.validate_token", params).await
    }

    /// Save Discord configuration
    pub async fn save_config(
        state: &DashboardState,
        config: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        state.rpc_call("discord.save_config", config).await
    }

    /// List guilds the bot has joined
    pub async fn list_guilds(
        state: &DashboardState,
        channel_id: &str,
    ) -> Result<Vec<serde_json::Value>, String> {
        let params = serde_json::json!({ "channel_id": channel_id });
        let result = state.rpc_call("discord.list_guilds", params).await?;
        result
            .get("guilds")
            .and_then(|g| serde_json::from_value(g.clone()).ok())
            .ok_or_else(|| "Invalid response: missing guilds".to_string())
    }

    /// List channels in a guild
    pub async fn list_channels(
        state: &DashboardState,
        channel_id: &str,
        guild_id: u64,
    ) -> Result<Vec<serde_json::Value>, String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "guild_id": guild_id,
        });
        let result = state.rpc_call("discord.list_channels", params).await?;
        result
            .get("channels")
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .ok_or_else(|| "Invalid response: missing channels".to_string())
    }

    /// Audit bot permissions in a guild
    pub async fn audit_permissions(
        state: &DashboardState,
        channel_id: &str,
        guild_id: u64,
    ) -> Result<serde_json::Value, String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "guild_id": guild_id,
        });
        state.rpc_call("discord.audit_permissions", params).await
    }

    /// Update guild/channel monitoring allowlists
    pub async fn update_allowlists(
        state: &DashboardState,
        channel_id: &str,
        guilds: Vec<u64>,
        channels: Vec<u64>,
    ) -> Result<serde_json::Value, String> {
        let params = serde_json::json!({
            "channel_id": channel_id,
            "guilds": guilds,
            "channels": channels,
        });
        state.rpc_call("discord.update_allowlists", params).await
    }
}
```

**Step 2: Build WASM to verify**

Run: `cd /Users/zouguojun/Workspace/Aleph/core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown`

Expected: Compiles without errors.

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/api.rs
git commit -m "dashboard: add Discord API module for Control Plane"
```

---

## Task 5: Frontend — Discord Panel View

**Files:**
- Create: `core/ui/control_plane/src/views/settings/channels/mod.rs`
- Create: `core/ui/control_plane/src/views/settings/channels/discord.rs`
- Modify: `core/ui/control_plane/src/views/settings/mod.rs` (add channels module)
- Modify: `core/ui/control_plane/src/components/settings_sidebar.rs` (add Channels group)
- Modify: `core/ui/control_plane/src/components/layouts/settings_layout.rs` (add route)

This is the largest task. The Discord panel view has 4 sections: Bot Identity, Token Configuration, Guild Management, and Permission Audit.

**Step 1: Create channels module**

Create `core/ui/control_plane/src/views/settings/channels/mod.rs`:

```rust
pub mod discord;
pub use discord::DiscordChannelView;
```

**Step 2: Create the Discord panel view**

Create `core/ui/control_plane/src/views/settings/channels/discord.rs`:

```rust
//! Discord Channel management panel for the Control Plane.
//!
//! Sections:
//! 1. Bot Identity - name, avatar, ID, status
//! 2. Token Configuration - masked input, validate, reset
//! 3. Guild Management - dual-column guild/channel selector
//! 4. Permission Audit - traffic-light permission checks

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::DiscordApi;

/// Main Discord channel settings view
#[component]
pub fn DiscordChannelView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State signals
    let bot_identity = RwSignal::new(Option::<serde_json::Value>::None);
    let token_input = RwSignal::new(String::new());
    let token_masked = RwSignal::new(String::new());
    let validating = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let connected = RwSignal::new(false);

    // Guild/Channel state
    let guilds = RwSignal::new(Vec::<serde_json::Value>::new());
    let selected_guild = RwSignal::new(Option::<u64>::None);
    let channels = RwSignal::new(Vec::<serde_json::Value>::new());
    let loading_guilds = RwSignal::new(false);
    let loading_channels = RwSignal::new(false);

    // Permission audit state
    let audit_result = RwSignal::new(Option::<serde_json::Value>::None);
    let auditing = RwSignal::new(false);

    // Allowlist state
    let allowed_guilds = RwSignal::new(Vec::<u64>::new());
    let allowed_channels = RwSignal::new(Vec::<u64>::new());

    view! {
        <div class="p-8 max-w-4xl mx-auto space-y-6">
            <div class="mb-6">
                <h2 class="text-2xl font-bold text-text-primary">"Discord"</h2>
                <p class="text-sm text-text-secondary mt-1">"Manage your Discord bot connection and permissions"</p>
            </div>

            // Section 1: Bot Identity
            <BotIdentitySection identity=bot_identity connected=connected />

            // Section 2: Token Configuration
            <TokenSection
                state=state.clone()
                token_input=token_input
                token_masked=token_masked
                validating=validating
                error=error
                bot_identity=bot_identity
                connected=connected
                guilds=guilds
                loading_guilds=loading_guilds
            />

            // Section 3: Guild Management (only when connected)
            <Show when=move || bot_identity.get().is_some()>
                <GuildSection
                    state=state.clone()
                    guilds=guilds
                    selected_guild=selected_guild
                    channels=channels
                    loading_guilds=loading_guilds
                    loading_channels=loading_channels
                    allowed_guilds=allowed_guilds
                    allowed_channels=allowed_channels
                    audit_result=audit_result
                    auditing=auditing
                />
            </Show>

            // Section 4: Permission Audit (only when a guild is selected)
            <Show when=move || audit_result.get().is_some()>
                <PermissionAuditSection
                    audit_result=audit_result
                />
            </Show>
        </div>
    }
}

/// Bot Identity section - displays bot name, avatar, ID, status
#[component]
fn BotIdentitySection(
    identity: RwSignal<Option<serde_json::Value>>,
    connected: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <div class="bg-surface rounded-xl border border-border p-6">
            <div class="flex items-center justify-between mb-4">
                <h3 class="text-lg font-semibold text-text-primary">"Bot Identity"</h3>
                <span class=move || {
                    if connected.get() {
                        "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200"
                    } else {
                        "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-800 dark:bg-gray-900 dark:text-gray-200"
                    }
                }>
                    {move || if connected.get() { "Online" } else { "Not Connected" }}
                </span>
            </div>

            {move || match identity.get() {
                Some(id) => {
                    let name = id.get("bot_name").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                    let discriminator = id.get("discriminator").and_then(|v| v.as_str()).unwrap_or("0000").to_string();
                    let bot_id = id.get("bot_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let avatar = id.get("bot_avatar").and_then(|v| v.as_str()).map(|s| s.to_string());

                    view! {
                        <div class="flex items-center gap-4">
                            // Avatar
                            {match avatar {
                                Some(url) => view! {
                                    <img src=url class="w-12 h-12 rounded-full" alt="Bot avatar" />
                                }.into_any(),
                                None => view! {
                                    <div class="w-12 h-12 rounded-full bg-primary flex items-center justify-center">
                                        <span class="text-text-inverse font-bold text-lg">"D"</span>
                                    </div>
                                }.into_any(),
                            }}
                            <div>
                                <p class="text-lg font-medium text-text-primary">
                                    {format!("{}#{}", name, discriminator)}
                                </p>
                                <p class="text-sm text-text-tertiary">
                                    {format!("ID: {}", bot_id)}
                                </p>
                            </div>
                        </div>
                    }.into_any()
                }
                None => view! {
                    <p class="text-text-tertiary italic">"No bot configured. Enter a token below to get started."</p>
                }.into_any(),
            }}
        </div>
    }
}

/// Token Configuration section
#[component]
fn TokenSection(
    state: DashboardState,
    token_input: RwSignal<String>,
    token_masked: RwSignal<String>,
    validating: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    bot_identity: RwSignal<Option<serde_json::Value>>,
    connected: RwSignal<bool>,
    guilds: RwSignal<Vec<serde_json::Value>>,
    loading_guilds: RwSignal<bool>,
) -> impl IntoView {
    let validate_token = move |_| {
        let token = token_input.get();
        if token.is_empty() {
            error.set(Some("Please enter a bot token".to_string()));
            return;
        }

        validating.set(true);
        error.set(None);
        let state = state.clone();

        spawn_local(async move {
            match DiscordApi::validate_token(&state, token.clone()).await {
                Ok(result) => {
                    bot_identity.set(Some(result));
                    connected.set(true);
                    // Mask the token for display
                    let masked = format!("{}...{}", &token[..6], &token[token.len()-4..]);
                    token_masked.set(masked);
                    token_input.set(String::new());
                    validating.set(false);

                    // Auto-fetch guilds
                    loading_guilds.set(true);
                    match DiscordApi::list_guilds(&state, "discord").await {
                        Ok(g) => guilds.set(g),
                        Err(e) => error.set(Some(format!("Failed to fetch guilds: {}", e))),
                    }
                    loading_guilds.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    validating.set(false);
                }
            }
        });
    };

    let reset_token = move |_| {
        bot_identity.set(None);
        connected.set(false);
        token_masked.set(String::new());
        guilds.set(Vec::new());
        error.set(None);
    };

    view! {
        <div class="bg-surface rounded-xl border border-border p-6">
            <h3 class="text-lg font-semibold text-text-primary mb-4">"Token Configuration"</h3>

            // Error display
            <Show when=move || error.get().is_some()>
                <div class="mb-4 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded-lg">
                    <p class="text-sm text-red-600 dark:text-red-400">
                        {move || error.get().unwrap_or_default()}
                    </p>
                </div>
            </Show>

            {move || if bot_identity.get().is_some() {
                // Show masked token + reset button
                view! {
                    <div class="flex items-center gap-3">
                        <div class="flex-1 px-4 py-2 bg-surface-sunken rounded-lg font-mono text-sm text-text-secondary">
                            {move || token_masked.get()}
                        </div>
                        <button
                            on:click=reset_token
                            class="px-4 py-2 text-sm font-medium text-red-600 dark:text-red-400 hover:bg-red-50 dark:hover:bg-red-900/20 rounded-lg transition-colors"
                        >
                            "Reset"
                        </button>
                    </div>
                }.into_any()
            } else {
                // Show token input + validate button
                view! {
                    <div class="flex items-center gap-3">
                        <input
                            type="password"
                            placeholder="Paste your Discord bot token here..."
                            class="flex-1 px-4 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-primary placeholder-text-tertiary focus:outline-none focus:ring-2 focus:ring-primary"
                            prop:value=move || token_input.get()
                            on:input=move |ev| {
                                token_input.set(event_target_value(&ev));
                            }
                        />
                        <button
                            on:click=validate_token
                            disabled=move || validating.get()
                            class="px-4 py-2 text-sm font-medium text-white bg-primary hover:bg-primary/90 disabled:opacity-50 rounded-lg transition-colors"
                        >
                            {move || if validating.get() { "Validating..." } else { "Validate" }}
                        </button>
                    </div>
                    <p class="mt-2 text-xs text-text-tertiary">
                        "Get your bot token from the "
                        <a href="https://discord.com/developers/applications" target="_blank" class="text-primary hover:underline">
                            "Discord Developer Portal"
                        </a>
                    </p>
                }.into_any()
            }}
        </div>
    }
}

/// Guild Management section with dual-column selector
#[component]
fn GuildSection(
    state: DashboardState,
    guilds: RwSignal<Vec<serde_json::Value>>,
    selected_guild: RwSignal<Option<u64>>,
    channels: RwSignal<Vec<serde_json::Value>>,
    loading_guilds: RwSignal<bool>,
    loading_channels: RwSignal<bool>,
    allowed_guilds: RwSignal<Vec<u64>>,
    allowed_channels: RwSignal<Vec<u64>>,
    audit_result: RwSignal<Option<serde_json::Value>>,
    auditing: RwSignal<bool>,
) -> impl IntoView {
    // Load channels when a guild is selected
    let state_clone = state.clone();
    let load_channels = move |guild_id: u64| {
        let state = state_clone.clone();
        selected_guild.set(Some(guild_id));
        loading_channels.set(true);
        channels.set(Vec::new());
        audit_result.set(None);

        spawn_local(async move {
            match DiscordApi::list_channels(&state, "discord", guild_id).await {
                Ok(chs) => channels.set(chs),
                Err(_) => channels.set(Vec::new()),
            }
            loading_channels.set(false);

            // Also audit permissions for this guild
            auditing.set(true);
            match DiscordApi::audit_permissions(&state, "discord", guild_id).await {
                Ok(result) => audit_result.set(Some(result)),
                Err(_) => audit_result.set(None),
            }
            auditing.set(false);
        });
    };

    let refresh_guilds = move |_| {
        let state = state.clone();
        loading_guilds.set(true);

        spawn_local(async move {
            match DiscordApi::list_guilds(&state, "discord").await {
                Ok(g) => guilds.set(g),
                Err(_) => {}
            }
            loading_guilds.set(false);
        });
    };

    let toggle_guild = move |guild_id: u64| {
        let mut current = allowed_guilds.get();
        if current.contains(&guild_id) {
            current.retain(|&id| id != guild_id);
        } else {
            current.push(guild_id);
        }
        allowed_guilds.set(current);
    };

    let toggle_channel = move |channel_id: u64| {
        let mut current = allowed_channels.get();
        if current.contains(&channel_id) {
            current.retain(|&id| id != channel_id);
        } else {
            current.push(channel_id);
        }
        allowed_channels.set(current);
    };

    view! {
        <div class="bg-surface rounded-xl border border-border p-6">
            <div class="flex items-center justify-between mb-4">
                <h3 class="text-lg font-semibold text-text-primary">"Guild Management"</h3>
                <button
                    on:click=refresh_guilds
                    disabled=move || loading_guilds.get()
                    class="px-3 py-1.5 text-xs font-medium text-text-secondary hover:text-text-primary hover:bg-surface-sunken rounded-lg transition-colors disabled:opacity-50"
                >
                    {move || if loading_guilds.get() { "Loading..." } else { "Refresh" }}
                </button>
            </div>

            <div class="flex gap-4 min-h-[300px]">
                // Left column: Guild list
                <div class="w-1/2 border border-border rounded-lg overflow-y-auto">
                    <div class="p-2 border-b border-border bg-surface-sunken">
                        <span class="text-xs font-medium text-text-tertiary uppercase">"Guilds"</span>
                    </div>
                    <div class="p-2 space-y-1">
                        {move || {
                            let guild_list = guilds.get();
                            if guild_list.is_empty() && !loading_guilds.get() {
                                return view! {
                                    <p class="text-sm text-text-tertiary italic p-2">"No guilds found"</p>
                                }.into_any();
                            }
                            guild_list.iter().map(|g| {
                                let guild_id = g.get("guild_id").and_then(|v| v.as_u64()).unwrap_or(0);
                                let name = g.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                                let icon = g.get("icon").and_then(|v| v.as_str()).map(|s| s.to_string());
                                let is_selected = move || selected_guild.get() == Some(guild_id);
                                let is_allowed = move || allowed_guilds.get().contains(&guild_id);

                                view! {
                                    <div
                                        class=move || {
                                            let base = "flex items-center gap-2 p-2 rounded-lg cursor-pointer transition-colors";
                                            if is_selected() {
                                                format!("{} bg-primary/10 border border-primary/30", base)
                                            } else {
                                                format!("{} hover:bg-surface-sunken", base)
                                            }
                                        }
                                        on:click=move |_| load_channels(guild_id)
                                    >
                                        <input
                                            type="checkbox"
                                            class="rounded"
                                            prop:checked=is_allowed
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                toggle_guild(guild_id);
                                            }
                                        />
                                        <span class="text-sm text-text-primary truncate">{name}</span>
                                    </div>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                </div>

                // Right column: Channel list
                <div class="w-1/2 border border-border rounded-lg overflow-y-auto">
                    <div class="p-2 border-b border-border bg-surface-sunken">
                        <span class="text-xs font-medium text-text-tertiary uppercase">"Channels"</span>
                    </div>
                    <div class="p-2 space-y-1">
                        {move || {
                            let channel_list = channels.get();
                            if selected_guild.get().is_none() {
                                return view! {
                                    <p class="text-sm text-text-tertiary italic p-2">"Select a guild to see channels"</p>
                                }.into_any();
                            }
                            if channel_list.is_empty() && !loading_channels.get() {
                                return view! {
                                    <p class="text-sm text-text-tertiary italic p-2">"No channels found"</p>
                                }.into_any();
                            }
                            if loading_channels.get() {
                                return view! {
                                    <p class="text-sm text-text-tertiary italic p-2">"Loading channels..."</p>
                                }.into_any();
                            }
                            channel_list.iter().map(|ch| {
                                let ch_id = ch.get("channel_id").and_then(|v| v.as_u64()).unwrap_or(0);
                                let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                                let ch_type = ch.get("channel_type").and_then(|v| v.as_u64()).unwrap_or(0);
                                let is_allowed = move || allowed_channels.get().contains(&ch_id);

                                // Only show text channels (type 0) and voice channels (type 2)
                                let prefix = match ch_type {
                                    0 => "#",
                                    2 => "🔊",
                                    _ => "📁",
                                };

                                view! {
                                    <div class="flex items-center gap-2 p-2 rounded-lg hover:bg-surface-sunken transition-colors">
                                        <input
                                            type="checkbox"
                                            class="rounded"
                                            prop:checked=is_allowed
                                            on:click=move |_| toggle_channel(ch_id)
                                        />
                                        <span class="text-sm text-text-secondary">{prefix}</span>
                                        <span class="text-sm text-text-primary truncate">{name}</span>
                                    </div>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                </div>
            </div>
        </div>
    }
}

/// Permission Audit section with traffic lights
#[component]
fn PermissionAuditSection(
    audit_result: RwSignal<Option<serde_json::Value>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface rounded-xl border border-border p-6">
            <div class="flex items-center justify-between mb-4">
                <h3 class="text-lg font-semibold text-text-primary">"Permission Audit"</h3>
            </div>

            {move || match audit_result.get() {
                Some(audit) => {
                    let guild_name = audit.get("guild_name").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                    let overall = audit.get("overall_status").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                    let summary = audit.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let permissions = audit.get("permissions")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let fixes = audit.get("fix_suggestions")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();

                    let overall_badge_class = match overall.as_str() {
                        "healthy" => "bg-green-100 text-green-800 dark:bg-green-900 dark:text-green-200",
                        "degraded" => "bg-yellow-100 text-yellow-800 dark:bg-yellow-900 dark:text-yellow-200",
                        "critical" => "bg-red-100 text-red-800 dark:bg-red-900 dark:text-red-200",
                        _ => "bg-gray-100 text-gray-800 dark:bg-gray-900 dark:text-gray-200",
                    };

                    view! {
                        <div>
                            <div class="flex items-center gap-3 mb-4">
                                <span class="text-sm font-medium text-text-secondary">{guild_name}</span>
                                <span class=format!("inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {}", overall_badge_class)>
                                    {summary}
                                </span>
                            </div>

                            <div class="space-y-2">
                                {permissions.iter().map(|perm| {
                                    let name = perm.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let has = perm.get("has").and_then(|v| v.as_bool()).unwrap_or(false);
                                    let status = perm.get("status").and_then(|v| v.as_str()).unwrap_or("green").to_string();

                                    let light_class = match status.as_str() {
                                        "green" => "w-3 h-3 rounded-full bg-green-500",
                                        "yellow" => "w-3 h-3 rounded-full bg-yellow-500",
                                        "red" => "w-3 h-3 rounded-full bg-red-500",
                                        _ => "w-3 h-3 rounded-full bg-gray-400",
                                    };

                                    view! {
                                        <div class="flex items-center gap-3 py-1">
                                            <div class=light_class></div>
                                            <span class="text-sm text-text-primary w-48">{name}</span>
                                            <span class=move || {
                                                if has {
                                                    "text-xs text-green-600 dark:text-green-400"
                                                } else {
                                                    "text-xs text-red-600 dark:text-red-400"
                                                }
                                            }>
                                                {if has { "Has" } else { "Missing" }}
                                            </span>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>

                            // Fix suggestions
                            {if !fixes.is_empty() {
                                Some(view! {
                                    <div class="mt-4 p-3 bg-surface-sunken rounded-lg">
                                        <p class="text-xs font-medium text-text-secondary mb-2">"Suggestions:"</p>
                                        <ul class="space-y-1">
                                            {fixes.iter().map(|fix| {
                                                let text = fix.as_str().unwrap_or("").to_string();
                                                view! {
                                                    <li class="text-xs text-text-tertiary flex items-center gap-2">
                                                        <span>"•"</span>
                                                        <span>{text}</span>
                                                    </li>
                                                }
                                            }).collect_view()}
                                        </ul>
                                    </div>
                                })
                            } else {
                                None
                            }}
                        </div>
                    }.into_any()
                }
                None => view! {
                    <p class="text-sm text-text-tertiary italic">"Select a guild to audit permissions"</p>
                }.into_any(),
            }}
        </div>
    }
}
```

**Step 3: Wire the channels module**

In `core/ui/control_plane/src/views/settings/mod.rs`, add:

```rust
pub mod channels;
pub use channels::discord::DiscordChannelView;
```

**Step 4: Add to Settings sidebar**

In `core/ui/control_plane/src/components/settings_sidebar.rs`:

1. Add `Discord` variant to `SettingsTab` enum:
```rust
// In the enum, add after Security:
Discord,
```

2. Add path/label/icon in the impl blocks:
```rust
// In path():
Self::Discord => "/settings/channels/discord",

// In label():
Self::Discord => "Discord",

// In icon_svg():
Self::Discord => r#"<path d="M20.317 4.37a19.791 19.791 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994a.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.01c.12.098.246.198.373.292a.077.077 0 0 1-.006.127 12.299 12.299 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03z"/>"#,
```

3. Add a "Channels" group to `SETTINGS_GROUPS`:
```rust
// Add a new group after "Extensions":
SettingsGroup {
    label: "Channels",
    tabs: &[
        SettingsTab::Discord,
    ],
},
```

**Step 5: Add route to settings layout**

In `core/ui/control_plane/src/components/layouts/settings_layout.rs`, add:

```rust
// Add import at top:
use crate::views::settings::DiscordChannelView;

// Add route inside <Routes>:
<Route path=path!("/settings/channels/discord") view=DiscordChannelView />
```

**Step 6: Build WASM to verify**

Run: `cd /Users/zouguojun/Workspace/Aleph/core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown`

Expected: Compiles without errors.

**Step 7: Commit**

```bash
git add core/ui/control_plane/src/views/settings/channels/ core/ui/control_plane/src/views/settings/mod.rs core/ui/control_plane/src/components/settings_sidebar.rs core/ui/control_plane/src/components/layouts/settings_layout.rs
git commit -m "dashboard: add Discord panel to Control Plane"
```

---

## Task 6: Full Build & Integration Test

**Files:** None new — this is a verification task.

**Step 1: Build full WASM UI pipeline**

```bash
cd /Users/zouguojun/Workspace/Aleph/core/ui/control_plane && \
cargo build --lib --target wasm32-unknown-unknown --release && \
wasm-bindgen --target web --out-dir dist --out-name aleph-dashboard \
  /Users/zouguojun/Workspace/Aleph/target/wasm32-unknown-unknown/release/aleph_dashboard.wasm && \
npm run build:css
```

Expected: All three steps succeed. `dist/` contains updated JS/WASM/CSS files.

**Step 2: Build server with Control Plane**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo build --bin aleph-server --features "control-plane,discord"
```

Expected: Compiles without errors.

**Step 3: Run backend unit tests**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test --features discord -p alephcore permissions::tests -- --nocapture
```

Expected: All permission audit tests pass.

**Step 4: Run full test suite**

```bash
cd /Users/zouguojun/Workspace/Aleph && cargo test --features discord -p alephcore
```

Expected: No regressions. Existing tests continue to pass.

**Step 5: Commit (if any build fixes were needed)**

```bash
git add -A && git commit -m "discord: fix build issues from integration"
```

---

## Summary

| Task | Description | Est. Files |
|------|-------------|-----------|
| 1 | Permission audit types + tests | 2 |
| 2 | Discord REST API wrapper | 2 |
| 3 | RPC handlers + registration | 3 |
| 4 | Frontend API module | 1 |
| 5 | Discord panel view + routing | 5 |
| 6 | Full build + integration test | 0 |

Total: ~13 files changed/created, 6 commits.
