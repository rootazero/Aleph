//! Heartbeat task management commands.
//!
//! Pure I/O envelope (R4): every action is a thin map from CLI flags to
//! `heartbeat.*` JSON-RPC and back. No business logic lives here.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TaskState {
    #[serde(default)]
    next_due_ms: Option<i64>,
    #[serde(default)]
    last_probe_at_ms: Option<i64>,
    #[serde(default)]
    last_probe_result: Option<String>,
    #[serde(default)]
    consecutive_errors: u32,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TaskView {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    interval_ms: Option<u64>,
    #[serde(default)]
    state: Option<TaskState>,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    tasks: Vec<TaskView>,
}

pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("heartbeat.list", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let response: ListResponse =
            serde_json::from_value(result.clone()).map_err(|e| CliError::Other(e.to_string()))?;
        if response.tasks.is_empty() {
            println!("No heartbeat tasks configured");
        } else {
            let headers = &["ID", "Name", "Agent", "Enabled", "Interval"];
            let rows: Vec<Vec<String>> = response
                .tasks
                .iter()
                .map(|t| {
                    vec![
                        t.id.clone(),
                        t.name.clone().unwrap_or_default(),
                        t.agent_id.clone().unwrap_or_default(),
                        t.enabled.to_string(),
                        t.interval_ms.map_or_else(|| "-".into(), format_interval),
                    ]
                })
                .collect();
            output::print_table(headers, &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

pub async fn get(server_url: &str, config: &CliConfig, task_id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let params = json!({ "task_id": task_id });
    let result: Value = client.call("heartbeat.get", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    client.close().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    interval: &str,
    tool: &str,
    agent: &str,
    params: Option<&str>,
    trigger: Option<&str>,
    disabled: bool,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let probe_params = match params {
        Some(s) => Some(
            serde_json::from_str::<Value>(s)
                .map_err(|e| CliError::Other(format!("invalid --params JSON: {e}")))?,
        ),
        None => None,
    };
    let trigger_value = match trigger {
        Some(s) => Some(
            serde_json::from_str::<Value>(s)
                .map_err(|e| CliError::Other(format!("invalid --trigger JSON: {e}")))?,
        ),
        None => None,
    };

    let mut probe = serde_json::Map::new();
    probe.insert("tool_name".into(), Value::String(tool.to_string()));
    if let Some(p) = probe_params {
        probe.insert("tool_params".into(), p);
    }
    if let Some(tc) = trigger_value {
        probe.insert("trigger_condition".into(), tc);
    }

    let body = json!({
        "name": name,
        "agent_id": agent,
        "interval": interval,
        "probe": probe,
        "enabled": !disabled,
    });

    let result: Value = client.call("heartbeat.create", Some(body)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("✓ Heartbeat created");
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    client.close().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    server_url: &str,
    config: &CliConfig,
    task_id: &str,
    name: Option<&str>,
    agent: Option<&str>,
    interval: Option<&str>,
    enable: bool,
    disable: bool,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut body = serde_json::Map::new();
    body.insert("task_id".into(), Value::String(task_id.to_string()));
    if let Some(n) = name {
        body.insert("name".into(), Value::String(n.to_string()));
    }
    if let Some(a) = agent {
        body.insert("agent_id".into(), Value::String(a.to_string()));
    }
    if let Some(i) = interval {
        body.insert("interval".into(), Value::String(i.to_string()));
    }
    if enable {
        body.insert("enabled".into(), Value::Bool(true));
    } else if disable {
        body.insert("enabled".into(), Value::Bool(false));
    }

    let result: Value = client
        .call("heartbeat.update", Some(Value::Object(body)))
        .await?;
    if json {
        output::print_json(&result);
    } else {
        println!("✓ Heartbeat updated");
    }
    client.close().await?;
    Ok(())
}

pub async fn delete(
    server_url: &str,
    config: &CliConfig,
    task_id: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let params = json!({ "task_id": task_id });
    let result: Value = client.call("heartbeat.delete", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("✓ Heartbeat deleted: {task_id}");
    }
    client.close().await?;
    Ok(())
}

pub async fn toggle(
    server_url: &str,
    config: &CliConfig,
    task_id: &str,
    enable: bool,
    disable: bool,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let mut body = serde_json::Map::new();
    body.insert("task_id".into(), Value::String(task_id.to_string()));
    if enable {
        body.insert("enabled".into(), Value::Bool(true));
    } else if disable {
        body.insert("enabled".into(), Value::Bool(false));
    }
    let result: Value = client
        .call("heartbeat.toggle", Some(Value::Object(body)))
        .await?;
    if json {
        output::print_json(&result);
    } else {
        let new_enabled = result.get("enabled").and_then(serde_json::Value::as_bool);
        match new_enabled {
            Some(v) => println!("{task_id}: enabled={v}"),
            None => println!("{}", serde_json::to_string_pretty(&result)?),
        }
    }
    client.close().await?;
    Ok(())
}

pub async fn wake(
    server_url: &str,
    config: &CliConfig,
    task_id: &str,
    reason: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let mut body = serde_json::Map::new();
    body.insert("task_id".into(), Value::String(task_id.to_string()));
    if let Some(r) = reason {
        body.insert("reason".into(), Value::String(r.to_string()));
    }
    let result: Value = client
        .call("heartbeat.wake", Some(Value::Object(body)))
        .await?;
    if json {
        output::print_json(&result);
    } else {
        println!("✓ Wake enqueued: {task_id}");
    }
    client.close().await?;
    Ok(())
}

pub async fn runs(
    server_url: &str,
    config: &CliConfig,
    task_id: &str,
    limit: usize,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let params = json!({ "task_id": task_id, "limit": limit });
    let result: Value = client.call("heartbeat.runs", Some(params)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    client.close().await?;
    Ok(())
}

/// Format milliseconds as a human-friendly duration ("5m", "1h30m", "30s").
fn format_interval(ms: u64) -> String {
    if ms == 0 {
        return "0".into();
    }
    let total_secs = ms / 1_000;
    if total_secs < 60 {
        return format!("{total_secs}s");
    }
    let mins = total_secs / 60;
    if mins < 60 {
        let rem_secs = total_secs % 60;
        return if rem_secs == 0 {
            format!("{mins}m")
        } else {
            format!("{mins}m{rem_secs}s")
        };
    }
    let hours = mins / 60;
    let rem_mins = mins % 60;
    if rem_mins == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h{rem_mins}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_interval_handles_common_ranges() {
        assert_eq!(format_interval(0), "0");
        assert_eq!(format_interval(45_000), "45s");
        assert_eq!(format_interval(300_000), "5m");
        assert_eq!(format_interval(330_000), "5m30s");
        assert_eq!(format_interval(3_600_000), "1h");
        assert_eq!(format_interval(5_400_000), "1h30m");
    }
}
