//! AI provider management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List all configured AI providers
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("providers.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(providers) = result.as_array() {
        for p in providers {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let ptype = p.get("type").and_then(|v| v.as_str()).unwrap_or("-");
            let default = p
                .get("default")
                .and_then(|v| v.as_bool())
                .map(|b| if b { "yes" } else { "no" })
                .unwrap_or("no");
            rows.push(vec![
                name.to_string(),
                ptype.to_string(),
                default.to_string(),
            ]);
        }
    }

    output::print_table(&["Name", "Type", "Default"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Get details of a specific AI provider
pub async fn get(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("providers.get", Some(params)).await?;

    let pairs = vec![
        (
            "Name",
            result
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Type",
            result
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Default",
            result
                .get("default")
                .and_then(|v| v.as_bool())
                .map(|b| if b { "yes" } else { "no" })
                .unwrap_or("no")
                .to_string(),
        ),
        (
            "Base URL",
            result
                .get("base_url")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Models",
            result
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "-".to_string()),
        ),
    ];

    output::print_detail(&pairs, json, &result);

    client.close().await?;
    Ok(())
}

/// Add a new AI provider
pub async fn add(
    server_url: &str,
    name: &str,
    provider_type: &str,
    api_key: &str,
    base_url: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({
        "name": name,
        "type": provider_type,
        "api_key": api_key,
    });
    if let Some(url) = base_url {
        params["base_url"] = serde_json::Value::String(url.to_string());
    }

    let result: Value = client.call("providers.create", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Provider '{}' added successfully.", name);
    }

    client.close().await?;
    Ok(())
}

/// Test provider connectivity
pub async fn test(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("providers.test", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let success = result.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if success {
            println!("Provider '{}' is reachable.", name);
        } else {
            let error = result
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            println!("Provider '{}' test failed: {}", name, error);
        }
    }

    client.close().await?;
    Ok(())
}

/// Set a provider as the default
pub async fn set_default(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("providers.setDefault", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Provider '{}' set as default.", name);
    }

    client.close().await?;
    Ok(())
}

/// Remove a provider
pub async fn remove(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("providers.delete", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Provider '{}' removed.", name);
    }

    client.close().await?;
    Ok(())
}
