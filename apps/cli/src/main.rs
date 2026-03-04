//! Aleph CLI - Reference Implementation of Aleph Protocol Client
//!
//! This CLI demonstrates how to build a client that communicates with
//! Aleph Gateway using only the protocol types from `aleph-protocol`.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                        aleph-cli                             │
//! │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐ │
//! │  │  CLI    │  │ Client  │  │Commands │  │   Terminal UI   │ │
//! │  │ (clap)  │→ │(WS+RPC) │→ │ Handler │→ │   (ratatui)     │ │
//! │  └─────────┘  └─────────┘  └─────────┘  └─────────────────┘ │
//! └───────────────────────────────┬─────────────────────────────┘
//!                                 │ WebSocket (JSON-RPC 2.0)
//!                                 ↓
//!                         Aleph Gateway Server
//! ```

mod client;
mod commands;
mod config;
mod error;
pub(crate) mod output;
mod tui;

use clap::{Parser, Subcommand};
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::config::CliConfig;
use crate::error::CliResult;

/// Aleph CLI - Personal AI Assistant Client
#[derive(Parser)]
#[command(name = "aleph")]
#[command(author, version, about, long_about = None)]
pub(crate) struct Cli {
    /// Gateway server URL
    #[arg(short, long, default_value = "ws://127.0.0.1:18789")]
    server: String,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    /// Output in JSON format (applies to all subcommands)
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start interactive chat session
    Chat {
        /// Session key (creates new if not specified)
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Send a single message and get response
    Ask {
        /// The message to send
        message: String,

        /// Session key
        #[arg(short, long)]
        session: Option<String>,
    },

    /// Check server health
    Health,

    /// List available tools
    Tools {
        /// Filter by category
        #[arg(short, long)]
        category: Option<String>,
    },

    /// Manage sessions
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Manage guest invitations and permissions
    Guests {
        #[command(subcommand)]
        action: commands::guests::GuestsAction,
    },

    /// Show server information
    Info,

    /// Connect and authenticate with the server
    Connect {
        /// Device name for this client
        #[arg(short, long, default_value = "aleph-cli")]
        device: String,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Manage Gateway daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Call any Gateway RPC method directly
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },

    /// AI provider management
    Providers {
        #[command(subcommand)]
        action: ProvidersAction,
    },

    /// Model management
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },

    /// Memory management
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },

    /// Plugin lifecycle management
    Plugins {
        #[command(subcommand)]
        action: PluginsAction,
    },

    /// Skill management (file-based and markdown)
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },

    /// Workspace management
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// Log management
    Logs {
        #[command(subcommand)]
        action: LogsAction,
    },

    /// Generate shell completion script
    Completion {
        /// Shell type (bash, zsh, fish, elvish, powershell)
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print config file path
    File,
    /// Get configuration (optionally by section: gateway, agents, channels, etc.)
    Get {
        /// Config section name (e.g., gateway, agents, channels)
        section: Option<String>,
    },
    /// Set a configuration value
    Set {
        /// Dot-separated config path (e.g., gateway.port)
        path: String,
        /// Value to set (JSON or plain string)
        value: String,
    },
    /// Validate current configuration
    Validate,
}

#[derive(Subcommand)]
enum GatewayAction {
    /// Call an RPC method
    Call {
        /// RPC method name (e.g., "health", "providers.list")
        method: String,
        /// JSON params (optional, e.g., '{"section": "general"}')
        params: Option<String>,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Show Gateway server status
    Status,
    /// Start Gateway server
    Start,
    /// Stop Gateway server
    Stop,
    /// Restart Gateway server
    Restart,
    /// View Gateway logs
    Logs {
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
        /// Filter by log level (e.g., warn, error)
        #[arg(short, long)]
        level: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProvidersAction {
    /// List all AI providers
    List,
    /// Get provider details
    Get { name: String },
    /// Add a new provider
    Add {
        name: String,
        /// Provider type (e.g., openai, anthropic, ollama)
        #[arg(long)]
        r#type: String,
        /// API key
        #[arg(long)]
        api_key: String,
        /// Base URL (optional)
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Test provider connectivity
    Test { name: String },
    /// Set as default provider
    SetDefault { name: String },
    /// Remove a provider
    Remove { name: String },
}

#[derive(Subcommand)]
enum ModelsAction {
    /// List all available models
    List,
    /// Get model details
    Get { model_id: String },
    /// Show model capabilities
    Capabilities { model_id: String },
}

#[derive(Subcommand)]
enum MemoryAction {
    /// Search memory
    Search {
        /// Search query
        query: String,
        /// Maximum results to return
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Show memory statistics
    Stats,
    /// Clear memory
    Clear {
        /// Only clear facts, keep other memories
        #[arg(long)]
        facts_only: bool,
    },
    /// Compress and optimize memory
    Compress,
    /// Delete a specific memory entry
    Delete {
        /// Memory entry ID
        id: String,
    },
}

#[derive(Subcommand)]
enum PluginsAction {
    /// List installed plugins
    List,
    /// Install a plugin from source (URL, path, or zip)
    Install { source: String },
    /// Uninstall a plugin
    Uninstall { name: String },
    /// Enable a disabled plugin
    Enable { name: String },
    /// Disable a plugin
    Disable { name: String },
    /// Call a plugin tool
    Call {
        /// Plugin name
        plugin: String,
        /// Tool name
        tool: String,
        /// JSON params (optional)
        params: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillsAction {
    /// List all skills (file-based and runtime-loaded)
    List,
    /// Install a skill from source
    Install { source: String },
    /// Reload a markdown skill
    Reload { name: String },
    /// Delete/unload a skill
    Delete { name: String },
}

#[derive(Subcommand)]
enum SessionAction {
    /// List all sessions
    List,
    /// Create a new session
    New {
        /// Session name
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Delete a session
    Delete {
        /// Session key to delete
        key: String,
    },
}

#[derive(Subcommand)]
enum WorkspaceAction {
    /// List all workspaces
    List,
    /// Create a new workspace
    Create {
        /// Workspace name
        name: String,
        /// Optional description
        #[arg(long)]
        description: Option<String>,
    },
    /// Switch to a workspace
    Switch {
        /// Workspace name to switch to
        name: String,
    },
    /// Show the currently active workspace
    Active,
    /// Archive a workspace
    Archive {
        /// Workspace name to archive
        name: String,
    },
}

#[derive(Subcommand)]
enum LogsAction {
    /// Get current log level
    Level,
    /// Set log level (trace, debug, info, warn, error)
    SetLevel {
        /// Log level to set
        level: String,
    },
    /// Show log directory path
    Dir,
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let cli = Cli::parse();

    // Initialize logging with unified file + console output
    let default_filter = if cli.verbose { "debug" } else { "info" };
    if let Err(e) = aleph_logging::init_component_logging("cli", 7, default_filter) {
        eprintln!("Failed to init file logging: {e}");
        // Fallback to console-only
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(if cli.verbose {
                EnvFilter::new("debug")
            } else {
                EnvFilter::new("info")
            })
            .init();
    }

    // Load configuration
    let config = CliConfig::load(cli.config.as_deref())?;
    let server_url = cli.server;

    info!("Aleph CLI v{}", env!("CARGO_PKG_VERSION"));

    match cli.command {
        Some(Commands::Health) => {
            commands::health::run(&server_url).await?;
        }
        Some(Commands::Info) => {
            commands::info::run(&server_url, cli.json).await?;
        }
        Some(Commands::Tools { category }) => {
            commands::tools::run(&server_url, category.as_deref()).await?;
        }
        Some(Commands::Connect { device }) => {
            commands::connect::run(&server_url, &device, &config).await?;
        }
        Some(Commands::Ask { message, session }) => {
            commands::ask::run(&server_url, &message, session.as_deref(), &config).await?;
        }
        Some(Commands::Chat { session }) => {
            commands::chat::run(&server_url, session.as_deref(), &config).await?;
        }
        Some(Commands::Session { action }) => match action {
            SessionAction::List => {
                commands::session::list(&server_url, &config).await?;
            }
            SessionAction::New { name } => {
                commands::session::create(&server_url, name.as_deref(), &config).await?;
            }
            SessionAction::Delete { key } => {
                commands::session::delete(&server_url, &key, &config).await?;
            }
        },
        Some(Commands::Guests { action }) => {
            commands::guests::handle_guests(&server_url, action, &config).await?;
        }
        Some(Commands::Config { action }) => match action {
            ConfigAction::File => {
                commands::config_cmd::file();
            }
            ConfigAction::Get { section } => {
                commands::config_cmd::get(&server_url, section.as_deref(), &config).await?;
            }
            ConfigAction::Set { path, value } => {
                commands::config_cmd::set(&server_url, &path, &value, &config).await?;
            }
            ConfigAction::Validate => {
                commands::config_cmd::validate(&server_url, &config).await?;
            }
        },
        Some(Commands::Daemon { action }) => match action {
            DaemonAction::Status => {
                commands::daemon::status(&server_url).await?;
            }
            DaemonAction::Start => {
                commands::daemon::start()?;
            }
            DaemonAction::Stop => {
                commands::daemon::stop(&server_url).await?;
            }
            DaemonAction::Restart => {
                commands::daemon::restart(&server_url).await?;
            }
            DaemonAction::Logs { lines, level } => {
                commands::daemon::logs(&server_url, lines, level.as_deref()).await?;
            }
        },
        Some(Commands::Providers { action }) => match action {
            ProvidersAction::List => {
                commands::providers_cmd::list(&server_url, cli.json).await?;
            }
            ProvidersAction::Get { name } => {
                commands::providers_cmd::get(&server_url, &name, cli.json).await?;
            }
            ProvidersAction::Add {
                name,
                r#type,
                api_key,
                base_url,
            } => {
                commands::providers_cmd::add(
                    &server_url,
                    &name,
                    &r#type,
                    &api_key,
                    base_url.as_deref(),
                    cli.json,
                )
                .await?;
            }
            ProvidersAction::Test { name } => {
                commands::providers_cmd::test(&server_url, &name, cli.json).await?;
            }
            ProvidersAction::SetDefault { name } => {
                commands::providers_cmd::set_default(&server_url, &name, cli.json).await?;
            }
            ProvidersAction::Remove { name } => {
                commands::providers_cmd::remove(&server_url, &name, cli.json).await?;
            }
        },
        Some(Commands::Models { action }) => match action {
            ModelsAction::List => {
                commands::models_cmd::list(&server_url, cli.json).await?;
            }
            ModelsAction::Get { model_id } => {
                commands::models_cmd::get(&server_url, &model_id, cli.json).await?;
            }
            ModelsAction::Capabilities { model_id } => {
                commands::models_cmd::capabilities(&server_url, &model_id, cli.json).await?;
            }
        },
        Some(Commands::Memory { action }) => match action {
            MemoryAction::Search { query, limit } => {
                commands::memory_cmd::search(&server_url, &query, limit, cli.json).await?
            }
            MemoryAction::Stats => {
                commands::memory_cmd::stats(&server_url, cli.json).await?
            }
            MemoryAction::Clear { facts_only } => {
                commands::memory_cmd::clear(&server_url, facts_only, cli.json).await?
            }
            MemoryAction::Compress => {
                commands::memory_cmd::compress(&server_url, cli.json).await?
            }
            MemoryAction::Delete { id } => {
                commands::memory_cmd::delete(&server_url, &id, cli.json).await?
            }
        },
        Some(Commands::Plugins { action }) => match action {
            PluginsAction::List => {
                commands::plugins_cmd::list(&server_url, cli.json).await?;
            }
            PluginsAction::Install { source } => {
                commands::plugins_cmd::install(&server_url, &source, cli.json).await?;
            }
            PluginsAction::Uninstall { name } => {
                commands::plugins_cmd::uninstall(&server_url, &name, cli.json).await?;
            }
            PluginsAction::Enable { name } => {
                commands::plugins_cmd::enable(&server_url, &name, cli.json).await?;
            }
            PluginsAction::Disable { name } => {
                commands::plugins_cmd::disable(&server_url, &name, cli.json).await?;
            }
            PluginsAction::Call {
                plugin,
                tool,
                params,
            } => {
                commands::plugins_cmd::call(
                    &server_url,
                    &plugin,
                    &tool,
                    params.as_deref(),
                    cli.json,
                )
                .await?;
            }
        },
        Some(Commands::Skills { action }) => match action {
            SkillsAction::List => {
                commands::skills_cmd::list(&server_url, cli.json).await?;
            }
            SkillsAction::Install { source } => {
                commands::skills_cmd::install(&server_url, &source, cli.json).await?;
            }
            SkillsAction::Reload { name } => {
                commands::skills_cmd::reload(&server_url, &name, cli.json).await?;
            }
            SkillsAction::Delete { name } => {
                commands::skills_cmd::delete(&server_url, &name, cli.json).await?;
            }
        },
        Some(Commands::Workspace { action }) => match action {
            WorkspaceAction::List => {
                commands::workspace_cmd::list(&server_url, cli.json).await?;
            }
            WorkspaceAction::Create { name, description } => {
                commands::workspace_cmd::create(
                    &server_url,
                    &name,
                    description.as_deref(),
                    cli.json,
                )
                .await?;
            }
            WorkspaceAction::Switch { name } => {
                commands::workspace_cmd::switch(&server_url, &name, cli.json).await?;
            }
            WorkspaceAction::Active => {
                commands::workspace_cmd::active(&server_url, cli.json).await?;
            }
            WorkspaceAction::Archive { name } => {
                commands::workspace_cmd::archive(&server_url, &name, cli.json).await?;
            }
        },
        Some(Commands::Gateway { action }) => match action {
            GatewayAction::Call { method, params } => {
                commands::gateway_cmd::call(&server_url, &method, params.as_deref(), cli.json)
                    .await?
            }
        },
        Some(Commands::Logs { action }) => match action {
            LogsAction::Level => {
                commands::logs_cmd::level(&server_url, cli.json).await?;
            }
            LogsAction::SetLevel { level } => {
                commands::logs_cmd::set_level(&server_url, &level, cli.json).await?;
            }
            LogsAction::Dir => {
                commands::logs_cmd::dir(&server_url, cli.json).await?;
            }
        },
        Some(Commands::Completion { shell }) => {
            commands::completion::run(shell);
        }
        None => {
            // Default: start interactive chat
            commands::chat::run(&server_url, None, &config).await?;
        }
    }

    Ok(())
}
