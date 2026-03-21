//! Aleph TUI Library
//!
//! Interactive terminal interface for Aleph Gateway.
//! Can be used as a library (from CLI's `chat` command) or standalone binary.

pub mod tui;

pub use aleph_client::{AlephClient, CliConfig, CliResult};

/// Launch the TUI, connecting to the given Gateway URL.
///
/// This is the main entry point when using the TUI as a library (e.g. from `aleph chat`).
///
/// # Arguments
/// * `server_url` — WebSocket URL of the Aleph Gateway (e.g. `ws://127.0.0.1:18789`)
/// * `agent` — Optional agent name to bind this session to (reserved for future use)
/// * `session` — Optional session key; a new one is generated if `None`
/// * `config` — CLI configuration (auth token, default session, etc.)
/// * `verbose` — Enable verbose logging (reserved for future use)
pub async fn run(
    server_url: &str,
    _agent: Option<&str>,
    session: Option<&str>,
    config: &CliConfig,
    _verbose: bool,
) -> CliResult<()> {
    // Connect to gateway
    let (client, events) = AlephClient::connect(server_url).await?;

    // Authenticate
    client.authenticate(config).await?;

    // Determine session key
    let session_key = session
        .map(|s| s.to_owned())
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
    tui::run(client, events, config, session_key).await
}
