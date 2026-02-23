//! CLI argument definitions for Aleph Gateway
//!
//! This module contains all Clap-based command line argument parsing structures.

use std::path::PathBuf;
use clap::{Parser, Subcommand};

/// Default PID file location
pub const DEFAULT_PID_FILE: &str = "~/.aleph/gateway.pid";
/// Default log file location for daemon mode
pub const DEFAULT_LOG_FILE: &str = "~/.aleph/gateway.log";

/// Aleph Gateway - WebSocket control plane for AI agents
#[derive(Parser, Debug)]
#[command(name = "aleph-gateway")]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Subcommand (start, stop, status)
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Configuration file path (TOML)
    #[arg(long, short = 'c')]
    pub config: Option<PathBuf>,

    /// Run as daemon (background process)
    #[arg(long, short = 'd')]
    pub daemon: bool,

    /// PID file path (for daemon mode)
    #[arg(long, default_value = DEFAULT_PID_FILE)]
    pub pid_file: String,

    /// Log file path (for daemon mode)
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Bind address
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,

    /// Port number
    #[arg(long, default_value = "18789")]
    pub port: u16,

    /// Force start even if port appears to be in use
    #[arg(long)]
    pub force: bool,

    /// Log level (error, warn, info, debug, trace)
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Maximum number of concurrent connections
    #[arg(long, default_value = "1000")]
    pub max_connections: usize,

    /// WebChat UI directory (serves static files)
    #[arg(long)]
    pub webchat_dir: Option<PathBuf>,

    /// WebChat HTTP port (default: same as WebSocket port)
    #[arg(long)]
    pub webchat_port: Option<u16>,
}

/// Gateway subcommands
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the gateway (default)
    Start,
    /// Stop a running daemon
    Stop,
    /// Check gateway status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage device pairing
    Pairing {
        #[command(subcommand)]
        action: PairingAction,
    },
    /// Manage approved devices
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
    },
    /// Manage plugins
    Plugins {
        #[command(subcommand)]
        action: PluginsAction,
    },
    /// Gateway RPC tools
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage channels
    Channels {
        #[command(subcommand)]
        action: ChannelsAction,
    },
    /// Manage cron jobs
    Cron {
        #[command(subcommand)]
        action: CronAction,
    },
    /// Audit tool risk and execution history
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },
    /// Manage encrypted secrets
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
}

/// Pairing subcommands
#[derive(Subcommand, Debug)]
pub enum PairingAction {
    /// List pending pairing requests
    List,
    /// Approve a pairing request
    Approve {
        /// The 6-digit pairing code
        code: String,
    },
    /// Reject a pairing request
    Reject {
        /// The 6-digit pairing code
        code: String,
    },
}

/// Devices subcommands
#[derive(Subcommand, Debug)]
pub enum DevicesAction {
    /// List approved devices
    List,
    /// Revoke an approved device
    Revoke {
        /// The device ID to revoke
        device_id: String,
    },
}

/// Plugins subcommands
#[derive(Subcommand, Debug)]
pub enum PluginsAction {
    /// List installed plugins
    List,
    /// Install a plugin from Git URL
    Install {
        /// Git URL of the plugin repository
        url: String,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: String,
    },
    /// Enable a plugin
    Enable {
        /// Plugin name
        name: String,
    },
    /// Disable a plugin
    Disable {
        /// Plugin name
        name: String,
    },
}

/// Gateway subcommands
#[derive(Subcommand, Debug)]
pub enum GatewayAction {
    /// Call an RPC method on the Gateway
    Call {
        /// RPC method name (e.g., "health", "config.get")
        method: String,

        /// JSON parameters
        #[arg(long, short = 'p')]
        params: Option<String>,

        /// Gateway WebSocket URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,

        /// Timeout in milliseconds
        #[arg(long, default_value = "30000")]
        timeout: u64,
    },
}

/// Config subcommands
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Get configuration (all or specific path)
    Get {
        /// Config path (e.g., "general.language")
        path: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Set a configuration value
    Set {
        /// Config path (e.g., "general.language")
        path: String,

        /// Value to set (JSON or string)
        value: String,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Edit configuration in editor
    Edit,
    /// Validate configuration
    Validate {
        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Reload configuration
    Reload {
        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Output JSON Schema
    Schema {
        /// Output file path
        #[arg(long, short = 'o')]
        output: Option<String>,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
}

/// Channels subcommands
#[derive(Subcommand, Debug)]
pub enum ChannelsAction {
    /// List all channels
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Get channel status
    Status {
        /// Channel name (optional, all if not specified)
        name: Option<String>,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
}

/// Cron subcommands
#[derive(Subcommand, Debug)]
pub enum CronAction {
    /// List cron jobs
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Get cron service status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
    /// Trigger a cron job manually
    Run {
        /// Job ID to run
        job_id: String,

        /// Gateway URL
        #[arg(long, default_value = "ws://127.0.0.1:18789")]
        url: String,
    },
}

/// Audit subcommands
#[derive(Subcommand, Debug)]
pub enum AuditAction {
    /// List all tools with risk scores
    Tools,
    /// Show detailed tool info and execution history
    Tool {
        /// Tool name to query
        name: String,

        /// Maximum number of execution records to show
        #[arg(long, short = 'l', default_value = "10")]
        limit: usize,
    },
    /// Show all escalation events
    Escalations {
        /// Maximum number of escalation records to show
        #[arg(long, short = 'l', default_value = "20")]
        limit: usize,
    },
}

/// Secret subcommands
#[derive(Subcommand, Debug)]
pub enum SecretAction {
    /// Initialize secret vault with current ALEPH_MASTER_KEY
    Init,
    /// Set a secret value (prompts when --value is omitted)
    Set {
        /// Secret name
        name: String,

        /// Secret value (avoid shell history by omitting and using prompt)
        #[arg(long)]
        value: Option<String>,
    },
    /// List stored secret names (never values)
    List,
    /// Delete a secret
    Delete {
        /// Secret name
        name: String,
    },
    /// Verify a secret exists
    Verify {
        /// Secret name
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_parses_plugins_list() {
        let args = Args::try_parse_from(["aleph-gateway", "plugins", "list"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                assert!(matches!(action, PluginsAction::List));
            }
            _ => panic!("Expected Plugins command with List action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_install() {
        let args = Args::try_parse_from([
            "aleph-gateway",
            "plugins",
            "install",
            "https://github.com/example/plugin.git",
        ]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Install { url } = action {
                    assert_eq!(url, "https://github.com/example/plugin.git");
                } else {
                    panic!("Expected PluginsAction::Install");
                }
            }
            _ => panic!("Expected Plugins command with Install action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_uninstall() {
        let args = Args::try_parse_from(["aleph-gateway", "plugins", "uninstall", "my-plugin"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Uninstall { name } = action {
                    assert_eq!(name, "my-plugin");
                } else {
                    panic!("Expected PluginsAction::Uninstall");
                }
            }
            _ => panic!("Expected Plugins command with Uninstall action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_enable() {
        let args = Args::try_parse_from(["aleph-gateway", "plugins", "enable", "my-plugin"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Enable { name } = action {
                    assert_eq!(name, "my-plugin");
                } else {
                    panic!("Expected PluginsAction::Enable");
                }
            }
            _ => panic!("Expected Plugins command with Enable action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_disable() {
        let args = Args::try_parse_from(["aleph-gateway", "plugins", "disable", "my-plugin"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Disable { name } = action {
                    assert_eq!(name, "my-plugin");
                } else {
                    panic!("Expected PluginsAction::Disable");
                }
            }
            _ => panic!("Expected Plugins command with Disable action"),
        }
    }

    #[test]
    fn test_cli_parses_config_get() {
        let args = Args::try_parse_from(["aleph-gateway", "config", "get"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Config { action }) => {
                if let ConfigAction::Get { path, json, .. } = action {
                    assert!(path.is_none());
                    assert!(!json);
                } else {
                    panic!("Expected ConfigAction::Get");
                }
            }
            _ => panic!("Expected Config command with Get action"),
        }
    }

    #[test]
    fn test_cli_parses_config_get_with_path() {
        let args = Args::try_parse_from(["aleph-gateway", "config", "get", "general.language", "--json"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Config { action }) => {
                if let ConfigAction::Get { path, json, .. } = action {
                    assert_eq!(path, Some("general.language".to_string()));
                    assert!(json);
                } else {
                    panic!("Expected ConfigAction::Get");
                }
            }
            _ => panic!("Expected Config command with Get action"),
        }
    }

    #[test]
    fn test_cli_parses_config_set() {
        let args = Args::try_parse_from(["aleph-gateway", "config", "set", "general.language", "zh-Hans"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Config { action }) => {
                if let ConfigAction::Set { path, value, .. } = action {
                    assert_eq!(path, "general.language");
                    assert_eq!(value, "zh-Hans");
                } else {
                    panic!("Expected ConfigAction::Set");
                }
            }
            _ => panic!("Expected Config command with Set action"),
        }
    }

    #[test]
    fn test_cli_parses_config_edit() {
        let args = Args::try_parse_from(["aleph-gateway", "config", "edit"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Config { action }) => {
                assert!(matches!(action, ConfigAction::Edit));
            }
            _ => panic!("Expected Config command with Edit action"),
        }
    }

    #[test]
    fn test_cli_parses_config_validate() {
        let args = Args::try_parse_from(["aleph-gateway", "config", "validate"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Config { action }) => {
                assert!(matches!(action, ConfigAction::Validate { .. }));
            }
            _ => panic!("Expected Config command with Validate action"),
        }
    }

    #[test]
    fn test_cli_parses_config_reload() {
        let args = Args::try_parse_from(["aleph-gateway", "config", "reload"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Config { action }) => {
                assert!(matches!(action, ConfigAction::Reload { .. }));
            }
            _ => panic!("Expected Config command with Reload action"),
        }
    }

    #[test]
    fn test_cli_parses_config_schema() {
        let args = Args::try_parse_from(["aleph-gateway", "config", "schema", "-o", "schema.json"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Config { action }) => {
                if let ConfigAction::Schema { output, .. } = action {
                    assert_eq!(output, Some("schema.json".to_string()));
                } else {
                    panic!("Expected ConfigAction::Schema");
                }
            }
            _ => panic!("Expected Config command with Schema action"),
        }
    }

    #[test]
    fn test_cli_parses_audit_tools() {
        let args = Args::try_parse_from(["aleph-gateway", "audit", "tools"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Audit { action }) => {
                assert!(matches!(action, AuditAction::Tools));
            }
            _ => panic!("Expected Audit command with Tools action"),
        }
    }

    #[test]
    fn test_cli_parses_audit_tool() {
        let args = Args::try_parse_from(["aleph-gateway", "audit", "tool", "my_tool"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Audit { action }) => {
                if let AuditAction::Tool { name, limit } = action {
                    assert_eq!(name, "my_tool");
                    assert_eq!(limit, 10); // default value
                } else {
                    panic!("Expected AuditAction::Tool");
                }
            }
            _ => panic!("Expected Audit command with Tool action"),
        }
    }

    #[test]
    fn test_cli_parses_audit_tool_with_limit() {
        let args = Args::try_parse_from(["aleph-gateway", "audit", "tool", "my_tool", "--limit", "50"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Audit { action }) => {
                if let AuditAction::Tool { name, limit } = action {
                    assert_eq!(name, "my_tool");
                    assert_eq!(limit, 50);
                } else {
                    panic!("Expected AuditAction::Tool");
                }
            }
            _ => panic!("Expected Audit command with Tool action"),
        }
    }

    #[test]
    fn test_cli_parses_audit_escalations() {
        let args = Args::try_parse_from(["aleph-gateway", "audit", "escalations"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Audit { action }) => {
                if let AuditAction::Escalations { limit } = action {
                    assert_eq!(limit, 20); // default value
                } else {
                    panic!("Expected AuditAction::Escalations");
                }
            }
            _ => panic!("Expected Audit command with Escalations action"),
        }
    }

    #[test]
    fn test_cli_parses_audit_escalations_with_limit() {
        let args = Args::try_parse_from(["aleph-gateway", "audit", "escalations", "-l", "100"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Audit { action }) => {
                if let AuditAction::Escalations { limit } = action {
                    assert_eq!(limit, 100);
                } else {
                    panic!("Expected AuditAction::Escalations");
                }
            }
            _ => panic!("Expected Audit command with Escalations action"),
        }
    }

    #[test]
    fn test_cli_parses_secret_set() {
        let args = Args::try_parse_from([
            "aleph-gateway",
            "secret",
            "set",
            "openai.main",
            "--value",
            "sk-ant-test",
        ]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Secret { action }) => {
                if let SecretAction::Set { name, value } = action {
                    assert_eq!(name, "openai.main");
                    assert_eq!(value.as_deref(), Some("sk-ant-test"));
                } else {
                    panic!("Expected SecretAction::Set");
                }
            }
            _ => panic!("Expected Secret command with Set action"),
        }
    }

    #[test]
    fn test_cli_parses_secret_verify() {
        let args = Args::try_parse_from(["aleph-gateway", "secret", "verify", "wallet.main"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Secret { action }) => {
                if let SecretAction::Verify { name } = action {
                    assert_eq!(name, "wallet.main");
                } else {
                    panic!("Expected SecretAction::Verify");
                }
            }
            _ => panic!("Expected Secret command with Verify action"),
        }
    }
}
