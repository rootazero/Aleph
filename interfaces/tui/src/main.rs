//! Aleph TUI - Standalone Binary
//!
//! Thin wrapper that parses CLI arguments and delegates to `aleph_tui::run()`.

use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use aleph_tui::{CliConfig, CliResult};

/// Aleph TUI - Interactive Terminal Chat
#[derive(Parser)]
#[command(name = "aleph-tui")]
#[command(author, version, about = "Interactive terminal interface for Aleph")]
struct Args {
    /// Gateway server URL
    #[arg(short, long, default_value = aleph_client::DEFAULT_GATEWAY_URL)]
    server: String,

    /// Session key (creates new if not specified)
    #[arg(short = 'k', long)]
    session: Option<String>,

    /// Reopen the most recently active session (by `last_active_at`)
    #[arg(short = 'c', long = "continue", conflicts_with = "session")]
    continue_last: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let args = Args::parse();

    // Initialize logging with unified file + console output
    let default_filter = if args.verbose { "debug" } else { "info" };
    if let Err(e) = aleph_logging::init_component_logging("tui", 7, default_filter) {
        eprintln!("Failed to init file logging: {e}");
        // Fallback to console-only
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(if args.verbose {
                EnvFilter::new("debug")
            } else {
                EnvFilter::new("info")
            })
            .init();
    }

    let config = CliConfig::load(args.config.as_deref())?;

    info!("Aleph TUI v{}", env!("CARGO_PKG_VERSION"));

    aleph_tui::run(
        &args.server,
        None,
        args.session.as_deref(),
        args.continue_last,
        &config,
        args.verbose,
    )
    .await
}
