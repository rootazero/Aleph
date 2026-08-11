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
/// * `server_url` — WebSocket URL of the Aleph Gateway (e.g. `ws://127.0.0.1:18790/ws`)
/// * `agent` — Optional agent name to bind this session to (reserved for future use)
/// * `session` — Optional session key; a new one is generated if `None`
/// * `continue_last` — Reopen the most recently active conversation
/// * `config` — CLI configuration (auth token, default session, etc.)
/// * `verbose` — Start with the TUI's verbose display mode on (shows reasoning)
///
/// # Errors
///
/// Returns an error if the gateway connection, handshake, or TUI launch fails.
pub async fn run(
    server_url: &str,
    _agent: Option<&str>,
    session: Option<&str>,
    continue_last: bool,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    // Connect to gateway
    let (client, events) = AlephClient::connect(server_url, config).await?;

    // Handshake (LAN-trust: no credentials)

    // Determine the session to open. Precedence mirrors `aleph ask`:
    //   1. explicit --session
    //   2. --continue → newest by `last_active_at` (codex `resume` / pi
    //      `--continue` parity: continuing is opt-in, never the default, so a
    //      bare launch cannot silently append to yesterday's thread)
    //   3. config.default_session
    //   4. let the gateway route one
    //
    // Case 4 sends **no key at all** rather than inventing one. The TUI used to
    // mint `chat-<uuid8>`, which `SessionKey::parse` rejects (it wants
    // `agent:{id}:…`) — and an unparseable key does not fail the call, it makes
    // `AgentRouter::route` fall through and mint a fresh epoch. So the client
    // held a key the server had never heard of: every `/usage`, `/tier`,
    // `/compress`, `/undo` and history fetch answered about nothing, and each
    // launch silently started a new conversation. Asking the gateway to route
    // and then adopting what it reports keeps one authority for the key.
    let session_key = if let Some(s) = session {
        Some(s.to_string())
    } else if continue_last {
        match aleph_client::resolve_last_session(&client).await {
            Ok(key) => Some(key),
            Err(e) => {
                // A brand-new install has no session to continue. That is not
                // an error worth refusing to start over — say so and open a
                // fresh conversation.
                eprintln!("--continue: {e}; starting a new conversation");
                None
            }
        }
    } else {
        config.default_session.clone()
    };

    // Launch TUI
    tui::run(client, events, config, session_key, verbose).await
}
