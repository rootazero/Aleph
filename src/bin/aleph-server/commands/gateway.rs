//! Gateway RPC command handlers
//!
//! Spec C policy: **`NoLock`**. `gateway call` is a pure JSON-RPC client
//! over WebSocket; it does not touch `~/.aleph/data/` and therefore
//! never contends for the singleton lock. Marker call to `run_no_lock`
//! at handler entry preserves the policy classification for the
//! reverse-regression check (Task 25).

use crate::cli::GatewayAction;

/// Handle gateway subcommands
pub async fn handle_gateway_command(
    action: GatewayAction,
) -> Result<(), Box<dyn std::error::Error>> {
    use aleph_client::GatewayClient;

    // Spec C Task 19: NoLock policy marker.
    alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(()))?;

    match action {
        GatewayAction::Call {
            method,
            params,
            url,
            timeout,
        } => {
            let client = GatewayClient::new().with_url(&url).with_timeout(timeout);

            let params_value: Option<serde_json::Value> =
                params.map(|p| serde_json::from_str(&p)).transpose()?;

            let result: serde_json::Value = client.call_raw(&method, params_value).await?;
            // Inlined after shared/client dropped its output module (5142efe3b):
            // this bin arm was the module's sole production consumer.
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    Ok(())
}
