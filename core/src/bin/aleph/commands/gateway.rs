//! Gateway RPC command handlers

use crate::cli::GatewayAction;

/// Handle gateway subcommands
#[cfg(feature = "gateway")]
pub async fn handle_gateway_command(action: GatewayAction) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::{GatewayClient, print_json};

    match action {
        GatewayAction::Call { method, params, url, timeout } => {
            let client = GatewayClient::new()
                .with_url(&url)
                .with_timeout(timeout);

            let params_value: Option<serde_json::Value> = params
                .map(|p| serde_json::from_str(&p))
                .transpose()?;

            let result: serde_json::Value = client.call_raw(&method, params_value).await?;
            print_json(&result)?;
        }
    }

    Ok(())
}
