//! Connect and authenticate command

use crate::output::{self, icon, theme};
use aleph_client::{AlephClient, CliConfig, CliResult};

/// Run connect command
pub async fn run(
    server_url: &str,
    device_name: &str,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = {
        let _spin = (!json).then(|| output::Spinner::start(format!("Connecting to {server_url}")));
        let result = AlephClient::connect(server_url).await;
        if let Some(s) = _spin {
            s.stop().await;
        }
        result?
    };

    // Create a modified config with the device name
    let mut config = config.clone();
    config.device_name = device_name.to_string();

    let token = {
        let _spin =
            (!json).then(|| output::Spinner::start(format!("Authenticating as '{device_name}'")));
        let result = client.authenticate(&config).await;
        if let Some(s) = _spin {
            s.stop().await;
        }
        result?
    };

    if json {
        let result = serde_json::json!({
            "status": "connected",
            "device": device_name,
            "token": token,
        });
        output::print_json(&result);
    } else {
        println!(
            "{} {}",
            theme::paint(theme::Style::Success, icon::ok()),
            theme::paint(theme::Style::Bold, "Connected successfully!")
        );
        println!();
        println!(
            "  {}: {}…",
            theme::paint(theme::Style::Muted, "Auth token"),
            &token[..20.min(token.len())]
        );
        println!();
        println!(
            "{}",
            theme::paint(
                theme::Style::Muted,
                "To save this token for future sessions, add to your config:"
            )
        );
        println!("  auth_token = \"{}\"", token);
    }

    // Save token to config
    let mut config = config.clone();
    config.set_auth_token(token, None)?;
    if !json {
        println!();
        println!(
            "{} Token saved to config file.",
            theme::paint(theme::Style::Success, icon::ok())
        );
    }

    client.close().await?;
    Ok(())
}
