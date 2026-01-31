//! Cron CLI command implementations.

use crate::cli::{print_json, print_list_table, print_success, CliError, GatewayClient, OutputFormat};
use serde::Deserialize;
use serde_json::{json, Value};

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

/// Handle cron list command
pub async fn handle_list(client: &GatewayClient, format: OutputFormat) -> Result<(), CliError> {
    let result: Value = client.call_raw("cron.list", None).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let jobs: Vec<CronJob> = serde_json::from_value(
                result.get("jobs").cloned().unwrap_or(result.clone()),
            )
            .unwrap_or_default();

            if jobs.is_empty() {
                println!("No cron jobs configured");
                return Ok(());
            }

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

            print_list_table(headers, &rows);
        }
    }

    Ok(())
}

/// Handle cron status command
pub async fn handle_status(client: &GatewayClient, format: OutputFormat) -> Result<(), CliError> {
    let result: Value = client.call_raw("cron.status", None).await?;

    match format {
        OutputFormat::Json => {
            print_json(&result)?;
        }
        OutputFormat::Table => {
            let json_str = serde_json::to_string_pretty(&result)?;
            println!("{}", json_str);
        }
    }

    Ok(())
}

/// Handle cron run command
pub async fn handle_run(client: &GatewayClient, job_id: String) -> Result<(), CliError> {
    let params = json!({ "job_id": job_id });
    client.call_raw("cron.run", Some(params)).await?;
    print_success(&format!("Triggered job: {}", job_id));
    Ok(())
}
