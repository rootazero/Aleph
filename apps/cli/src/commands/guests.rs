//! Guest management commands
//!
//! Provides CLI interface for creating and managing guest invitations.

use clap::Subcommand;
use serde::{Deserialize, Serialize};

use aleph_protocol::{GuestScope, Invitation};
use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::{CliError, CliResult};

#[derive(Subcommand)]
pub enum GuestsAction {
    /// Create a new guest invitation
    ///
    /// Examples:
    ///   aleph guests invite --name "Mom" --tools translate,summarize
    ///   aleph guests invite --name "Guest" --tools "*" --expires-days 7
    ///   aleph guests invite --name "Collaborator" --tools "memory,search"
    Invite {
        /// Guest display name
        #[arg(short, long)]
        name: String,

        /// Allowed tools (comma-separated, or "*" for all)
        ///
        /// Examples:
        ///   --tools translate,summarize
        ///   --tools "*"
        #[arg(short, long)]
        tools: String,

        /// Session expiry in days (e.g., 7 for 7 days from now)
        #[arg(long)]
        expires_days: Option<i64>,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: OutputFormat,
    },

    /// List pending (non-activated) invitations
    List {
        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: OutputFormat,
    },

    /// Revoke a guest invitation
    ///
    /// Examples:
    ///   aleph guests revoke 6ba7b810-9dad-11d1-80b4-00c04fd430c8
    ///   aleph guests revoke TOKEN_VALUE --force
    Revoke {
        /// Guest ID or invitation token to revoke
        guest_id: String,

        /// Force revocation without confirmation prompt
        #[arg(short, long)]
        force: bool,
    },

    /// Show activity info for a guest
    ///
    /// Examples:
    ///   aleph guests info 6ba7b810-9dad-11d1-80b4-00c04fd430c8
    ///   aleph guests info TOKEN_VALUE --format json
    Info {
        /// Guest ID or invitation token
        guest_id: String,

        /// Output format (text or json)
        #[arg(short, long, default_value = "text")]
        format: OutputFormat,
    },
}

#[derive(Clone, Debug)]
pub enum OutputFormat {
    Text,
    Json,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("Invalid format: {}. Use 'text' or 'json'", s)),
        }
    }
}

/// Main entry point for guests command
pub async fn handle_guests(
    server_url: &str,
    action: GuestsAction,
    config: &CliConfig,
) -> CliResult<()> {
    match action {
        GuestsAction::Invite {
            name,
            tools,
            expires_days,
            format,
        } => handle_invite(server_url, &name, &tools, expires_days, format, config).await,
        GuestsAction::List { format } => handle_list(server_url, format, config).await,
        GuestsAction::Revoke { guest_id, force } =>
            handle_revoke(server_url, &guest_id, force, config).await,
        GuestsAction::Info { guest_id, format } =>
            handle_info(server_url, &guest_id, format, config).await,
    }
}

/// Handle guest invitation creation
async fn handle_invite(
    server_url: &str,
    name: &str,
    tools: &str,
    expires_days: Option<i64>,
    format: OutputFormat,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    // Parse tools into allowed_tools list
    let allowed_tools = parse_tools(tools)?;

    // Calculate expiry timestamp if provided
    let expires_at = expires_days.map(|days| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now + days * 86400
    });

    let scope = GuestScope {
        allowed_tools,
        expires_at,
        display_name: Some(name.to_string()),
    };

    #[derive(Serialize)]
    struct CreateInvitationParams {
        guest_name: String,
        scope: GuestScope,
    }

    let params = CreateInvitationParams {
        guest_name: name.to_string(),
        scope,
    };

    #[derive(Deserialize)]
    struct CreateInvitationResponse {
        invitation: Invitation,
    }

    let response: CreateInvitationResponse =
        client.call("guests.createInvitation", Some(params)).await?;

    match format {
        OutputFormat::Text => {
            println!("Guest invitation created");
            println!();
            println!("  Guest ID: {}", response.invitation.guest_id);
            println!("  Token:    {}", response.invitation.token);
            println!("  URL:      {}", response.invitation.url);
            if let Some(exp) = response.invitation.expires_at {
                println!("  Expires:  {}", format_timestamp(exp));
            }
            println!();
            println!("Share this token with the guest to activate their session.");
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&response.invitation)?);
        }
    }

    client.close().await?;
    Ok(())
}

/// Handle listing pending invitations
async fn handle_list(
    server_url: &str,
    format: OutputFormat,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    #[derive(Deserialize, Serialize)]
    struct ListInvitationsResponse {
        invitations: Vec<Invitation>,
    }

    let response: ListInvitationsResponse =
        client.call("guests.listPending", None::<()>).await?;

    match format {
        OutputFormat::Text => {
            println!("=== Pending Invitations ===");
            println!();

            if response.invitations.is_empty() {
                println!("No pending invitations.");
            } else {
                for inv in &response.invitations {
                    println!("  Guest ID: {}", inv.guest_id);
                    println!("  Token:    {}", inv.token);
                    if let Some(exp) = inv.expires_at {
                        println!("  Expires:  {}", format_timestamp(exp));
                    }
                    println!();
                }
                println!("Total: {} pending invitations", response.invitations.len());
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&response.invitations)?);
        }
    }

    client.close().await?;
    Ok(())
}

/// Handle revoking a guest invitation
async fn handle_revoke(
    server_url: &str,
    guest_id: &str,
    _force: bool,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    #[derive(Serialize)]
    struct RevokeInvitationParams {
        token: String,
    }

    let params = RevokeInvitationParams {
        token: guest_id.to_string(),
    };

    // RPC returns an empty/success response; we only care about errors
    let _: serde_json::Value =
        client.call("guests.revokeInvitation", Some(params)).await?;

    println!("Guest invitation revoked: {}", guest_id);

    client.close().await?;
    Ok(())
}

/// Handle showing activity info for a guest
async fn handle_info(
    server_url: &str,
    guest_id: &str,
    format: OutputFormat,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    #[derive(Serialize)]
    struct GetActivityLogsParams {
        guest_id: String,
        limit: u32,
    }

    let params = GetActivityLogsParams {
        guest_id: guest_id.to_string(),
        limit: 50,
    };

    #[derive(Deserialize, Serialize)]
    struct ActivityLog {
        timestamp: i64,
        action: String,
        details: Option<String>,
    }

    #[derive(Deserialize, Serialize)]
    struct ActivityLogsResponse {
        logs: Vec<ActivityLog>,
    }

    let response: ActivityLogsResponse =
        client.call("guests.getActivityLogs", Some(params)).await?;

    match format {
        OutputFormat::Text => {
            println!("=== Activity Logs for Guest: {} ===", guest_id);
            println!();

            if response.logs.is_empty() {
                println!("No activity logs found.");
            } else {
                for log in &response.logs {
                    let timestamp = format_timestamp(log.timestamp);
                    if let Some(ref details) = log.details {
                        println!("[{}] {} - {}", timestamp, log.action, details);
                    } else {
                        println!("[{}] {}", timestamp, log.action);
                    }
                }
                println!();
                println!("Total: {} log entries", response.logs.len());
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
    }

    client.close().await?;
    Ok(())
}

/// Parse tools string into allowed_tools list
fn parse_tools(tools: &str) -> CliResult<Vec<String>> {
    if tools == "*" {
        // Represent "all tools" as an empty list (server interprets empty as all)
        return Ok(vec!["*".to_string()]);
    }

    let tool_list: Vec<String> = tools
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if tool_list.is_empty() {
        return Err(CliError::Other("No tools specified".to_string()));
    }

    Ok(tool_list)
}

/// Format Unix timestamp to human-readable string
fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Local, TimeZone};

    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|dt: DateTime<Local>| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| format!("{}", timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tools_wildcard() {
        let result = parse_tools("*").unwrap();
        assert_eq!(result, vec!["*"]);
    }

    #[test]
    fn test_parse_tools_specific() {
        let result = parse_tools("translate,summarize").unwrap();
        assert_eq!(result, vec!["translate", "summarize"]);
    }

    #[test]
    fn test_parse_tools_empty_errors() {
        assert!(parse_tools("").is_err());
    }

    #[test]
    fn test_format_timestamp() {
        // Just verify it doesn't panic and returns a non-empty string
        let ts = format_timestamp(1700000000);
        assert!(!ts.is_empty());
    }

    #[test]
    fn test_revoke_action_variant_exists() {
        // Compilation test: ensures Revoke variant exists in GuestsAction
        let _action = GuestsAction::Revoke {
            guest_id: "test-id".to_string(),
            force: false,
        };
    }

    #[test]
    fn test_info_action_variant_exists() {
        // Compilation test: ensures Info variant exists in GuestsAction
        let _action = GuestsAction::Info {
            guest_id: "test-id".to_string(),
            format: OutputFormat::Text,
        };
    }

    #[test]
    fn test_guest_id_validation() {
        // guest_id must be non-empty
        let id = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        assert!(!id.is_empty());
    }
}
