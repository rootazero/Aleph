//! Configuration command handlers

use crate::cli::ConfigAction;

/// Handle config subcommands
pub async fn handle_config_command(action: ConfigAction) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::{GatewayClient, OutputFormat, config};

    match action {
        ConfigAction::Get { path, json, url } => {
            let client = GatewayClient::new().with_url(&url);
            let format = OutputFormat::from_json_flag(json);
            config::handle_get(&client, path, format).await?;
        }
        ConfigAction::Set { path, value, url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_set(&client, path, value).await?;
        }
        ConfigAction::Edit => {
            config::handle_edit().await?;
        }
        ConfigAction::Validate { url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_validate(&client).await?;
        }
        ConfigAction::Reload { url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_reload(&client).await?;
        }
        ConfigAction::Schema { output, url } => {
            let client = GatewayClient::new().with_url(&url);
            config::handle_schema(&client, output).await?;
        }
    }

    Ok(())
}
