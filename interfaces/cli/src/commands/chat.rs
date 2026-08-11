//! Interactive chat command — launches the TUI

use aleph_client::{CliConfig, CliResult};

/// Run interactive chat via TUI.
///
/// `continue_last` reopens the most recently active conversation — the
/// interactive twin of `aleph ask --last`, resolved by the same shared
/// function so the two commands can never pick different threads.
///
/// # Errors
///
/// Propagates any gateway-connection, handshake or TUI failure.
pub async fn run(
    server_url: &str,
    agent: Option<&str>,
    session: Option<&str>,
    continue_last: bool,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    aleph_tui::run(server_url, agent, session, continue_last, config, verbose).await
}
