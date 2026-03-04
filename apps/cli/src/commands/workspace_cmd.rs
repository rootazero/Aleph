//! Workspace management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List all workspaces
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("workspace.list", None::<()>).await?;

    let mut rows = Vec::new();
    if let Some(workspaces) = result.as_array() {
        for w in workspaces {
            let name = w.get("name").and_then(|v| v.as_str()).unwrap_or("-");
            let status = w.get("status").and_then(|v| v.as_str()).unwrap_or("-");
            let created = w.get("created").and_then(|v| v.as_str()).unwrap_or("-");
            rows.push(vec![
                name.to_string(),
                status.to_string(),
                created.to_string(),
            ]);
        }
    }

    output::print_table(&["Name", "Status", "Created"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Create a new workspace
pub async fn create(
    server_url: &str,
    name: &str,
    description: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let mut params = serde_json::json!({ "name": name });
    if let Some(desc) = description {
        params["description"] = Value::String(desc.to_string());
    }

    let result: Value = client.call("workspace.create", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Workspace '{}' created.", name);
    }

    client.close().await?;
    Ok(())
}

/// Switch to a workspace
pub async fn switch(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("workspace.switch", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Switched to workspace '{}'.", name);
    }

    client.close().await?;
    Ok(())
}

/// Show the currently active workspace
pub async fn active(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("workspace.getActive", None::<()>).await?;

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
            "Status",
            result
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Description",
            result
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Created",
            result
                .get("created")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
    ];

    output::print_detail(&pairs, json, &result);

    client.close().await?;
    Ok(())
}

/// Archive a workspace
pub async fn archive(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("workspace.archive", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Workspace '{}' archived.", name);
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn workspace_params_with_description() {
        let params = serde_json::json!({
            "name": "test-ws",
            "description": "A test workspace",
        });
        assert_eq!(params["name"], "test-ws");
        assert_eq!(params["description"], "A test workspace");
    }

    #[test]
    fn workspace_params_without_description() {
        let mut params = serde_json::json!({ "name": "test-ws" });
        let description: Option<&str> = None;
        if let Some(desc) = description {
            params["description"] = serde_json::Value::String(desc.to_string());
        }
        assert_eq!(params["name"], "test-ws");
        assert!(params.get("description").is_none());
    }
}
