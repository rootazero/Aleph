//! Interactive chat command — launches the TUI

use aleph_client::{CliConfig, CliResult};

/// Run interactive chat via TUI
pub async fn run(
    server_url: &str,
    agent: Option<&str>,
    session: Option<&str>,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    aleph_tui::run(server_url, agent, session, config, verbose).await
}
