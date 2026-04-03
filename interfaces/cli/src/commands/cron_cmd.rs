//! Cron job management commands

use serde::Deserialize;
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

/// Deserialized from JSON-RPC response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CronJob {
    id: String,
    schedule: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    last_run: Option<String>,
    #[serde(default)]
    next_run: Option<String>,
    #[serde(default)]
    enabled: bool,
}

/// List all cron jobs
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let result: Value = client.call("cron.list", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let jobs: Vec<CronJob> =
            serde_json::from_value(result.get("jobs").cloned().unwrap_or(result.clone()))
                .unwrap_or_default();

        if jobs.is_empty() {
            println!("No cron jobs configured");
        } else {
            let headers = &["ID", "Schedule", "Description", "Last Run", "Next Run"];
            let rows: Vec<Vec<String>> = jobs
                .iter()
                .map(|j| {
                    vec![
                        j.id.clone(),
                        j.schedule.clone(),
                        j.description.clone().unwrap_or_else(|| "-".to_string()),
                        j.last_run.clone().unwrap_or_else(|| "-".to_string()),
                        j.next_run.clone().unwrap_or_else(|| "-".to_string()),
                    ]
                })
                .collect();

            output::print_table(headers, &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

/// Show cron scheduler status
pub async fn status(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let result: Value = client.call("cron.status", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}

/// Trigger a cron job manually
pub async fn run(server_url: &str, job_id: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = serde_json::json!({ "job_id": job_id });
    let result: Value = client.call("cron.run", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Triggered job: {}", job_id);
    }

    client.close().await?;
    Ok(())
}
