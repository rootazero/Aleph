//! Connect command — LAN-trust handshake (no credentials).

use crate::output::{self, icon, theme};
use aleph_client::{AlephClient, CliConfig, CliResult};

/// Run connect command: open a WS connection and perform the credential-free
/// `connect` handshake, reporting the server-assigned role.
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

    // Create a modified config with the device name for the handshake.
    let mut config = config.clone();
    config.device_name = device_name.to_string();

    let role = {
        let _spin =
            (!json).then(|| output::Spinner::start(format!("Connecting as '{device_name}'")));
        let result = client.handshake(&config).await;
        if let Some(s) = _spin {
            s.stop().await;
        }
        result?
    };

    if json {
        let result = serde_json::json!({
            "status": "connected",
            "device": device_name,
            "role": role,
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
            "  {}: {}",
            theme::paint(theme::Style::Muted, "Role"),
            theme::paint(theme::Style::Bold, &role)
        );
    }

    client.close().await?;
    Ok(())
}
