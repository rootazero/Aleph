//! Aleph TUI - Standalone Binary
//!
//! Thin wrapper that parses CLI arguments and delegates to `aleph_tui::run()`.

use clap::Parser;
use tracing::info;
use tracing_subscriber::{prelude::*, EnvFilter};

mod pairing;

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
    /// `-k` rather than a collision. `al chat -c` keeps the short form for
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

    /// Authenticate with the device credential previously saved in the OS
    /// keyring for this server
    ///
    /// A pure switch: it takes no value, so no secret can appear in argv or
    /// shell history. Fails (does not fall back to an unauthenticated session)
    /// if the keyring cannot be read or holds no credential for the server.
    #[arg(long)]
    paired: bool,

    /// Pair this device for the first time: prompt (hidden, TTY only) for the
    /// one-time pairing ticket from `aleph-server pair`, exchange it for a
    /// long-lived device token, save that token to the OS keyring, then start
    ///
    /// A pure switch: no value form, so the ticket never appears in argv, an
    /// environment variable, the config file or shell history. Fails (no
    /// fallback to an unauthenticated or `--paired` session) if stdin is not a
    /// terminal, the ticket is blank, the exchange fails, or the token cannot
    /// be saved. Mutually exclusive with `--paired`.
    #[arg(long, conflicts_with = "paired")]
    pair: bool,
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let args = Args::parse();

    // This interactive interface requires semantic colors even when its launcher
    // inherits NO_COLOR from a non-interactive parent process.
    crossterm::style::force_color_output(true);

    // Ratatui owns the terminal, including stderr, while the UI is active.
    let default_filter = if args.verbose { "debug" } else { "info" };
    if let Err(e) = aleph_logging::init_component_file_logging("tui", 7, default_filter) {
        // Report before terminal setup; never install a console writer as fallback.
        eprintln!("Failed to init file logging: {e}");
        let _ = tracing_subscriber::registry()
            .with(EnvFilter::new(default_filter))
            .try_init();
    }

    let config = CliConfig::load(args.config.as_deref())?;

    info!("Aleph TUI v{}", env!("CARGO_PKG_VERSION"));

    let server_url = args.server.clone().unwrap_or_else(|| config.server.clone());

    if args.pair {
        // Prompt BEFORE connecting: a non-TTY / blank / cancelled prompt never
        // touches the network, and never falls back to `run` / `--paired`.
        let credential = pairing::prompt_bootstrap_credential().await?;
        aleph_tui::run_with_device_credential(
            &server_url,
            credential,
            args.session.as_deref(),
            args.continue_last,
            &config,
            args.verbose,
        )
        .await
    } else if args.paired {
        aleph_tui::run_with_stored_device_credential(
            &server_url,
            args.session.as_deref(),
            args.continue_last,
            &config,
            args.verbose,
        )
        .await
    } else {
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

    #[test]
    fn paired_is_a_value_less_flag_and_defaults_off() {
        use clap::Parser;
        assert!(!Args::try_parse_from(["aleph-tui"]).unwrap().paired);
        assert!(
            Args::try_parse_from(["aleph-tui", "--paired"])
                .unwrap()
                .paired
        );
        // No value form: a secret cannot be passed through this flag.
        assert!(Args::try_parse_from(["aleph-tui", "--paired=secret"]).is_err());
        // And no flag takes a token / ticket / password / credential.
        let cmd = Args::command();
        for arg in cmd.get_arguments() {
            let id = arg.get_id().as_str();
            assert!(
                !["token", "ticket", "password", "credential"]
                    .iter()
                    .any(|w| id.contains(w)),
                "{id}"
            );
        }
    }

    #[test]
    fn pair_is_a_value_less_flag_defaults_off_and_excludes_paired() {
        use clap::Parser;
        assert!(!Args::try_parse_from(["aleph-tui"]).unwrap().pair);
        assert!(Args::try_parse_from(["aleph-tui", "--pair"]).unwrap().pair);
        // No value form: the ticket can never be passed on the command line.
        assert!(Args::try_parse_from(["aleph-tui", "--pair=secret"]).is_err());
        assert!(Args::try_parse_from(["aleph-tui", "--pair", "secret"]).is_err());
        // Mutually exclusive with --paired.
        assert!(Args::try_parse_from(["aleph-tui", "--pair", "--paired"]).is_err());
        assert!(Args::try_parse_from(["aleph-tui", "--paired", "--pair"]).is_err());
        // Combines with ordinary options.
        assert!(Args::try_parse_from(["aleph-tui", "--pair", "-s", "ws://h/ws"]).is_ok());
    }
}
