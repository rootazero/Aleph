//! Channel management command handlers

use crate::cli::ChannelsAction;

/// Handle channels subcommands
#[cfg(feature = "gateway")]
pub async fn handle_channels_command(action: ChannelsAction) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::{channels, GatewayClient, OutputFormat};

    match action {
        ChannelsAction::List { json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            channels::handle_list(&client, format).await?;
        }
        ChannelsAction::Status { name, json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            channels::handle_status(&client, name, format).await?;
        }
    }

    Ok(())
}
