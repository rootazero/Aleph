mod app;
mod event;
mod markdown;
mod render;
mod slash;
mod theme;
mod widgets;

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use aleph_protocol::StreamEvent;
use tokio::sync::mpsc;

/// Entry point: run the TUI application
pub async fn run(
    _client: AlephClient,
    _events: mpsc::Receiver<StreamEvent>,
    _config: &CliConfig,
    _session_key: String,
) -> CliResult<()> {
    todo!("TUI main loop - implemented in Task 6")
}
