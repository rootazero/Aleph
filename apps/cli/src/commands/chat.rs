//! Interactive chat command — launches the TUI

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;

/// Run interactive chat via TUI
pub async fn run(
    server_url: &str,
    session: Option<&str>,
    config: &CliConfig,
) -> CliResult<()> {
    // Connect to gateway
    let (client, events) = AlephClient::connect(server_url).await?;

    // Authenticate
    client.authenticate(config).await?;

    // Determine session key
    let session_key = session
        .map(|s| s.to_string())
        .or_else(|| config.default_session.clone())
        .unwrap_or_else(|| {
            format!(
                "chat-{}",
                uuid::Uuid::new_v4()
                    .to_string()
                    .split('-')
                    .next()
                    .unwrap_or("0000")
            )
        });

    // Launch TUI
    crate::tui::run(client, events, config, session_key).await
}
