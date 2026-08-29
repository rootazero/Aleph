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
    ///
    /// Falls back to `server` in the config file, then to
    /// `ws://127.0.0.1:18790/ws` — see the same field on `aleph` for why this
    /// is not a clap `default_value`. Both binaries load the same `CliConfig`,
    /// so both had the same dead key.
    #[arg(short, long)]
    server: Option<String>,

    /// Session key (creates new if not specified)
    #[arg(short = 'k', long)]
    session: Option<String>,

    /// Reopen the most recently active session (by `last_active_at`)
    ///
    /// **No short flag**, and the reason is worth a sentence: `-c` was already
    /// `--config` on this binary before this option existed, and claiming it
    /// twice is not a warning — clap's debug asserts turn it into a panic
    /// **before `main` runs**, so a debug `aleph-tui` refused to start at all,
    /// with any arguments. A release build does not assert; it silently
    /// resolves the letter to one of the two.
    ///
    /// The letter goes to the older, released meaning. `--session` set the
    /// precedent in this same struct: `-s` was taken by `--server`, so it took
    /// `-k` rather than a collision. `aleph chat -c` keeps the short form for
    /// anyone who wants the keystroke — the two commands have to resolve to the
    /// same THREAD (they share `resolve_last_session`), not to the same letter.
    #[arg(long = "continue", conflicts_with = "session")]
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

    let server_url = args
        .server
        .clone()
        .unwrap_or_else(|| config.server.clone());

    aleph_tui::run(
        &server_url,
        None,
        args.session.as_deref(),
        args.continue_last,
        &config,
        args.verbose,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::Args;
    use clap::CommandFactory;

    /// clap's own validator, run as a test.
    ///
    /// It caught a duplicate `-c` (`--config` and a newly added `--continue`)
    /// that made every debug launch of this binary panic in `Args::parse()`.
    /// Nothing else could have: the collision is not a compile error, `cargo
    /// check` does not build `#[cfg(test)]`, and the repo's minimal
    /// verification set does not reach this crate at all — so the only witness
    /// was starting the program, which is precisely what "compile + unit tests"
    /// verification does not do.
    #[test]
    fn the_cli_definition_is_valid() {
        Args::command().debug_assert();
    }
}
