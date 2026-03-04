//! Connect and authenticate command

use crate::client::AlephClient;
use crate::config::CliConfig;
use crate::error::CliResult;
use crate::output;

/// Run connect command
pub async fn run(server_url: &str, device_name: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Create a modified config with the device name
    let mut config = config.clone();
    config.device_name = device_name.to_string();

    if !json {
        println!("Authenticating as '{}'...", device_name);
    }

    let token = client.authenticate(&config).await?;

    if json {
        let result = serde_json::json!({
            "status": "connected",
            "device": device_name,
            "token": token,
        });
        output::print_json(&result);
    } else {
        println!("✓ Connected successfully!");
        println!();
        println!("Auth token: {}...", &token[..20.min(token.len())]);
        println!();
        println!("To save this token for future sessions, add to your config:");
        println!("  auth_token = \"{}\"", token);
    }

    // Save token to config
    let mut config = config.clone();
    config.set_auth_token(token, None)?;
    if !json {
        println!();
        println!("Token saved to config file.");
    }

    client.close().await?;
    Ok(())
}
