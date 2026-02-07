//! Guest management commands
//!
//! Provides CLI interface for creating and managing guest invitations.

use clap::Subcommand;
use serde::{Deserialize, Serialize};

use aleph_protocol::{GuestScope, Invitation};
use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

#[derive(Subcommand)]
pub enum GuestsAction {
    /// Create a new guest invitation
    ///
    /// Examples:
    ///   aleph guests invite --name "Mom" --tools translate,summarize
    ///   aleph guests invite --name "Guest" --tools "*" --session-ttl 7d
    ///   aleph guests invite --name "Collaborator" --tools "memory:*,search:*"
    Invite {
        /// Guest display name
        #[arg(short, long)]
        name: String,

        /// Allowed tools (comma-separated)
        ///
        /// Examples:
        ///   --tools translate,summarize
        ///   --tools "shell:*"  (all shell tools)
        ///   --tools "*"        (all tools)
        #[arg(short, long)]
        tools: String,

        /// Session expiry after activation (e.g., 7d, 30d)
        ///
        /// This sets the expiry for the activated guest session.
        #[arg(long)]
        session_ttl: Option<String>,

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
            session_ttl,
            format,
        } => handle_invite(server_url, &name, &tools, session_ttl.as_deref(), format, config).await,
        GuestsAction::List { format } => handle_list(server_url, format, config).await,
    }
}

/// Handle guest invitation creation
async fn handle_invite(
    server_url: &str,
    name: &str,
    tools: &str,
    session_ttl: Option<&str>,
    format: OutputFormat,
    config: &CliConfig,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Authenticate first
    client.authenticate(config).await?;

    // Parse tools into GuestScope
    let scope = parse_tools(tools)?;

    // Parse session TTL if provided
    let session_ttl_seconds = session_ttl.map(parse_ttl).transpose()?;

    #[derive(Serialize)]
    struct CreateInvitationParams {
        name: String,
        scope: GuestScope,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_ttl_seconds: Option<i64>,
    }

    let params = CreateInvitationParams {
        name: name.to_string(),
        scope,
        session_ttl_seconds,
    };

    #[derive(Deserialize)]
    struct CreateInvitationResponse {
        invitation: Invitation,
    }

    let response: CreateInvitationResponse =
        client.call("guests.createInvitation", Some(params)).await?;

    match format {
        OutputFormat::Text => {
            println!("✓ Guest invitation created");
            println!();
            println!("  Name:       {}", response.invitation.name);
            println!("  Token:      {}", response.invitation.token);
            println!("  Created:    {}", format_timestamp(response.invitation.created_at));
            if let Some(ttl) = session_ttl_seconds {
                println!("  Session TTL: {}", format_duration(ttl));
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

    #[derive(Deserialize)]
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
                    println!("• {} ({})", inv.name, inv.token);
                    println!("  Created: {}", format_timestamp(inv.created_at));
                    if let Some(ttl) = inv.session_ttl_seconds {
                        println!("  Session TTL: {}", format_duration(ttl));
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

/// Parse tools string into GuestScope
fn parse_tools(tools: &str) -> CliResult<GuestScope> {
    if tools == "*" {
        return Ok(GuestScope::AllTools);
    }

    let tool_list: Vec<String> = tools
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if tool_list.is_empty() {
        return Err("No tools specified".into());
    }

    Ok(GuestScope::SpecificTools(tool_list))
}

/// Parse TTL string (e.g., "7d", "30d", "1h") into seconds
fn parse_ttl(ttl: &str) -> CliResult<i64> {
    let ttl = ttl.trim();
    if ttl.is_empty() {
        return Err("Empty TTL string".into());
    }

    let (num_str, unit) = ttl.split_at(ttl.len() - 1);
    let num: i64 = num_str
        .parse()
        .map_err(|_| format!("Invalid TTL number: {}", num_str))?;

    let seconds = match unit {
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        _ => return Err(format!("Invalid TTL unit: {}. Use s, m, h, or d", unit).into()),
    };

    Ok(seconds)
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

/// Format duration in seconds to human-readable string
fn format_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ttl() {
        assert_eq!(parse_ttl("30s").unwrap(), 30);
        assert_eq!(parse_ttl("5m").unwrap(), 300);
        assert_eq!(parse_ttl("2h").unwrap(), 7200);
        assert_eq!(parse_ttl("7d").unwrap(), 604800);
        assert!(parse_ttl("invalid").is_err());
        assert!(parse_ttl("10x").is_err());
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(300), "5m");
        assert_eq!(format_duration(7200), "2h");
        assert_eq!(format_duration(604800), "7d");
    }

    #[test]
    fn test_parse_tools() {
        // All tools
        match parse_tools("*").unwrap() {
            GuestScope::AllTools => {}
            _ => panic!("Expected AllTools"),
        }

        // Specific tools
        match parse_tools("translate,summarize").unwrap() {
            GuestScope::SpecificTools(tools) => {
                assert_eq!(tools, vec!["translate", "summarize"]);
            }
            _ => panic!("Expected SpecificTools"),
        }

        // Empty should error
        assert!(parse_tools("").is_err());
    }
}
