//! Connect command — open a socket and report the role the server grants.
//!
//! The handshake carries no credential. That is correct for the loopback case
//! (`resolve_connection_identity` makes a loopback connection operator before
//! it looks at anything else) and is a real limit everywhere else: a remote
//! gateway walls the connection until it is shown a device token, a bootstrap
//! ticket, or the shared gateway token, and none of the three has a CLI
//! surface. Against a remote server this command reports the wall, not a role.

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
    // The handshake now happens inside `connect`, so the device-name override
    // has to be applied to the config *before* connecting rather than to a
    // second, separate step.
    let mut config = config.clone();
    config.device_name = device_name.to_string();

    let (client, _events) = {
        let _spin = (!json).then(|| {
            output::Spinner::start(format!("Connecting to {server_url} as '{device_name}'"))
        });
        let result = AlephClient::connect(server_url, &config).await;
        if let Some(s) = _spin {
            s.stop().await;
        }
        result?
    };
    let role = client.role().to_string();

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
