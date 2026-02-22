//! Discord REST API Wrapper
//!
//! Higher-level functions wrapping serenity's `Http` client for use by
//! the Control Plane RPC handlers. Each function maps roughly to one
//! panel action in the Discord management UI.

use serde::{Deserialize, Serialize};

use super::permissions::{self, PermissionAudit};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Identity information for the connected Discord bot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotIdentity {
    /// Whether the token validated successfully.
    pub valid: bool,
    /// The bot's user ID.
    pub bot_id: u64,
    /// The bot's username.
    pub bot_name: String,
    /// CDN URL for the bot's avatar (if set).
    pub bot_avatar: Option<String>,
    /// The bot's discriminator (e.g. "0" for new-style usernames).
    pub discriminator: String,
}

/// Summary of a guild (server) the bot is a member of.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildSummary {
    /// Discord guild ID.
    pub guild_id: u64,
    /// Guild name.
    pub name: String,
    /// CDN URL for the guild's icon (if set).
    pub icon: Option<String>,
    /// Approximate member count (if available from the API).
    pub member_count: Option<u64>,
    /// Bot's permission bitfield in this guild.
    pub bot_permissions: u64,
}

/// Summary of a channel within a guild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSummary {
    /// Discord channel ID.
    pub channel_id: u64,
    /// Channel name.
    pub name: String,
    /// Discord channel type as a numeric value (0 = Text, 2 = Voice, 4 = Category, etc.).
    pub channel_type: u8,
    /// Position of the channel in the channel list.
    pub position: i64,
}

// ---------------------------------------------------------------------------
// API functions (discord feature enabled)
// ---------------------------------------------------------------------------

#[cfg(feature = "discord")]
use serenity::{all::GuildId, http::Http};

/// Validate a Discord bot token by calling the Discord API.
///
/// Creates a temporary `Http` client, fetches the current user, and returns
/// a [`BotIdentity`] on success.
#[cfg(feature = "discord")]
pub async fn validate_token(token: &str) -> Result<BotIdentity, String> {
    let http = Http::new(token);

    let user = http
        .get_current_user()
        .await
        .map_err(|e| format!("Failed to validate token: {}", e))?;

    // Build avatar CDN URL if the user has one
    let bot_avatar = user
        .avatar
        .as_ref()
        .map(|hash| {
            format!(
                "https://cdn.discordapp.com/avatars/{}/{}.webp",
                user.id, hash
            )
        });

    let discriminator = user
        .discriminator
        .map(|d| format!("{:04}", d))
        .unwrap_or_else(|| "0".to_string());

    Ok(BotIdentity {
        valid: true,
        bot_id: user.id.get(),
        bot_name: user.name.clone(),
        bot_avatar,
        discriminator,
    })
}

/// List all guilds the bot is a member of.
///
/// Fetches guilds via pagination (up to 200 at a time) and enriches each
/// entry with the bot's permission bitfield from the `GuildInfo` response.
#[cfg(feature = "discord")]
pub async fn list_guilds(http: &Http) -> Result<Vec<GuildSummary>, String> {
    // Fetch guilds using pagination. GuildInfo already includes permissions.
    let guild_infos = http
        .get_guilds(None, Some(200))
        .await
        .map_err(|e| format!("Failed to fetch guilds: {}", e))?;

    let mut summaries = Vec::with_capacity(guild_infos.len());

    for info in &guild_infos {
        // GuildInfo has: id, name, icon (Option<ImageHash>), owner, permissions
        let icon_url = info
            .icon
            .as_ref()
            .map(|hash| {
                format!(
                    "https://cdn.discordapp.com/icons/{}/{}.webp",
                    info.id, hash
                )
            });

        // Try to get approximate member count from the full guild object.
        // This is a best-effort call -- if it fails we just leave it as None.
        let member_count = match http.get_guild(info.id).await {
            Ok(guild) => guild.approximate_member_count,
            Err(_) => None,
        };

        summaries.push(GuildSummary {
            guild_id: info.id.get(),
            name: info.name.clone(),
            icon: icon_url,
            member_count,
            bot_permissions: info.permissions.bits(),
        });
    }

    Ok(summaries)
}

/// List all channels in a guild.
#[cfg(feature = "discord")]
pub async fn list_channels(http: &Http, guild_id: u64) -> Result<Vec<ChannelSummary>, String> {
    let gid = GuildId::new(guild_id);

    let channels = http
        .get_channels(gid)
        .await
        .map_err(|e| format!("Failed to fetch channels for guild {}: {}", guild_id, e))?;

    let summaries = channels
        .iter()
        .map(|ch| ChannelSummary {
            channel_id: ch.id.get(),
            name: ch.name.clone(),
            channel_type: u8::from(ch.kind),
            position: ch.position as i64,
        })
        .collect();

    Ok(summaries)
}

/// Audit the bot's real permissions in a guild.
///
/// Fetches the bot's member record, collects all assigned role permissions,
/// and delegates to [`permissions::audit_permissions`] for the traffic-light
/// analysis.
#[cfg(feature = "discord")]
pub async fn audit_guild_permissions(
    http: &Http,
    guild_id: u64,
) -> Result<PermissionAudit, String> {
    let gid = GuildId::new(guild_id);

    // Fetch guild info for name and roles
    let guild = http
        .get_guild(gid)
        .await
        .map_err(|e| format!("Failed to fetch guild {}: {}", guild_id, e))?;

    // Fetch the bot's member info using the dedicated endpoint
    let member = http
        .get_current_user_guild_member(gid)
        .await
        .map_err(|e| format!("Failed to fetch bot member in guild {}: {}", guild_id, e))?;

    // Compute effective permissions by OR-ing all role permission bitfields.
    // Start with @everyone role permissions (role ID == guild ID).
    let everyone_role_id = serenity::all::RoleId::new(guild_id);
    let mut combined_perms: u64 = guild
        .roles
        .get(&everyone_role_id)
        .map(|r| r.permissions.bits())
        .unwrap_or(0);

    // OR in permissions from each role the bot has
    for role_id in &member.roles {
        if let Some(role) = guild.roles.get(role_id) {
            combined_perms |= role.permissions.bits();
        }
    }

    // Administrator (0x8) grants all permissions
    const ADMINISTRATOR: u64 = 0x8;
    if combined_perms & ADMINISTRATOR != 0 {
        combined_perms = u64::MAX;
    }

    Ok(permissions::audit_permissions(
        guild_id,
        &guild.name,
        combined_perms,
    ))
}

// ---------------------------------------------------------------------------
// Stubs (discord feature disabled)
// ---------------------------------------------------------------------------

/// Validate a Discord bot token (stub when discord feature is disabled).
#[cfg(not(feature = "discord"))]
pub async fn validate_token(_token: &str) -> Result<BotIdentity, String> {
    Err("Discord feature not enabled".to_string())
}

/// List guilds (stub when discord feature is disabled).
#[cfg(not(feature = "discord"))]
pub async fn list_guilds(_http: &()) -> Result<Vec<GuildSummary>, String> {
    Err("Discord feature not enabled".to_string())
}

/// List channels (stub when discord feature is disabled).
#[cfg(not(feature = "discord"))]
pub async fn list_channels(_http: &(), _guild_id: u64) -> Result<Vec<ChannelSummary>, String> {
    Err("Discord feature not enabled".to_string())
}

/// Audit guild permissions (stub when discord feature is disabled).
#[cfg(not(feature = "discord"))]
pub async fn audit_guild_permissions(
    _http: &(),
    _guild_id: u64,
) -> Result<PermissionAudit, String> {
    Err("Discord feature not enabled".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_identity_serde_roundtrip() {
        let identity = BotIdentity {
            valid: true,
            bot_id: 123456789,
            bot_name: "TestBot".to_string(),
            bot_avatar: Some("https://cdn.discordapp.com/avatars/123/abc.webp".to_string()),
            discriminator: "0001".to_string(),
        };

        let json = serde_json::to_string(&identity).expect("serialize");
        let back: BotIdentity = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.bot_id, 123456789);
        assert_eq!(back.bot_name, "TestBot");
        assert!(back.valid);
        assert!(back.bot_avatar.is_some());
    }

    #[test]
    fn test_guild_summary_serde_roundtrip() {
        let summary = GuildSummary {
            guild_id: 9876543210,
            name: "My Server".to_string(),
            icon: None,
            member_count: Some(42),
            bot_permissions: 0x400 | 0x800, // View Channel + Send Messages
        };

        let json = serde_json::to_string(&summary).expect("serialize");
        let back: GuildSummary = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.guild_id, 9876543210);
        assert_eq!(back.name, "My Server");
        assert_eq!(back.member_count, Some(42));
        assert_eq!(back.bot_permissions, 0xC00);
    }

    #[test]
    fn test_channel_summary_serde_roundtrip() {
        let summary = ChannelSummary {
            channel_id: 111222333,
            name: "general".to_string(),
            channel_type: 0, // Text
            position: 0,
        };

        let json = serde_json::to_string(&summary).expect("serialize");
        let back: ChannelSummary = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.channel_id, 111222333);
        assert_eq!(back.name, "general");
        assert_eq!(back.channel_type, 0);
        assert_eq!(back.position, 0);
    }
}
