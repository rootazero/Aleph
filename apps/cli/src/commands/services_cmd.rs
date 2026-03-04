//! Background service management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List background services
pub async fn list(
    server_url: &str,
    plugin: Option<&str>,
    state: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({});
    if let Some(p) = plugin {
        params["plugin_id"] = Value::String(p.to_string());
    }
    if let Some(s) = state {
        params["state"] = Value::String(s.to_string());
    }

    let result: Value = client.call("services.list", Some(params)).await?;

    let mut rows = Vec::new();
    if let Some(services) = result.get("services").and_then(|v| v.as_array()) {
        for s in services {
            rows.push(vec![
                s.get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                s.get("plugin_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                s.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                s.get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ]);
        }
    }

    output::print_table(&["ID", "Plugin", "Name", "State"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Get service status
pub async fn status(
    server_url: &str,
    plugin_id: &str,
    service_id: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "plugin_id": plugin_id, "service_id": service_id });
    let result: Value = client.call("services.status", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else if let Some(svc) = result.get("service") {
        let pairs = vec![
            (
                "ID",
                svc.get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Plugin",
                svc.get("plugin_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Name",
                svc.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "State",
                svc.get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Started",
                svc.get("started_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
            (
                "Error",
                svc.get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
            ),
        ];
        output::print_detail(&pairs, false, &result);
    } else {
        println!("Service not found.");
    }

    client.close().await?;
    Ok(())
}

/// Start a service
pub async fn start(
    server_url: &str,
    plugin_id: &str,
    service_id: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "plugin_id": plugin_id, "service_id": service_id });
    let result: Value = client.call("services.start", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Service '{}' started.", service_id);
    }

    client.close().await?;
    Ok(())
}

/// Stop a service
pub async fn stop(
    server_url: &str,
    plugin_id: &str,
    service_id: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "plugin_id": plugin_id, "service_id": service_id });
    let result: Value = client.call("services.stop", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Service '{}' stopped.", service_id);
    }

    client.close().await?;
    Ok(())
}
