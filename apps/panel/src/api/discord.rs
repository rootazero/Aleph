use crate::context::DashboardState;

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
