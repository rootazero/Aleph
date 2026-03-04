//! Model management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List all available models
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("models.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(models) = result.as_array() {
        for m in models {
            let id = m
                .get("id")
                .or(m.get("model_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
            let context_window = m
                .get("context_window")
                .and_then(|v| v.as_u64())
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string());
            rows.push(vec![
                id.to_string(),
                provider.to_string(),
                context_window,
            ]);
        }
    }

    output::print_table(&["Model ID", "Provider", "Context Window"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Get details of a specific model
pub async fn get(server_url: &str, model_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "provider": model_id });
    let result: Value = client.call("models.get", Some(params)).await?;

    let pairs = vec![
        (
            "Model ID",
            result
                .get("id")
                .or(result.get("model_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Provider",
            result
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Context Window",
            result
                .get("context_window")
                .and_then(|v| v.as_u64())
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
    ];

    output::print_detail(&pairs, json, &result);

    client.close().await?;
    Ok(())
}

/// Show model capabilities
pub async fn capabilities(server_url: &str, model_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "provider": model_id });
    let result: Value = client.call("models.capabilities", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Capabilities for {}", model_id);
        println!("{}", "\u{2500}".repeat(22));
        output::print_json(&result);
    }

    client.close().await?;
    Ok(())
}
