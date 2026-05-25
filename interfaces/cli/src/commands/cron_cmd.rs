//! Cron job management commands.
//!
//! All actions are thin envelopes around `cron.*` JSON-RPC. The server enforces
//! every business rule (schedule validation, ScheduleKind tagging, persistence);
//! the CLI just transports flags into JSON and renders responses.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

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

/// Inspect a single job (cron.get).
pub async fn get(server_url: &str, job_id: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = json!({ "job_id": job_id });
    let result: Value = client.call("cron.get", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}

/// Show execution history (cron.runs).
pub async fn runs(
    server_url: &str,
    job_id: &str,
    limit: usize,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = json!({ "job_id": job_id, "limit": limit });
    let result: Value = client.call("cron.runs", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}

/// Build the JSON body for `cron.create`.
///
/// Extracted so we can unit-test the schedule-resolution logic without an
/// actual gateway.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_create_body(
    name: &str,
    schedule_expr: Option<&str>,
    schedule_kind_json: Option<&str>,
    agent: &str,
    prompt: &str,
    timezone: Option<&str>,
    tags_csv: Option<&str>,
) -> Result<Value, CliError> {
    if schedule_expr.is_none() && schedule_kind_json.is_none() {
        return Err(CliError::Other(
            "missing --schedule (cron expression) or --schedule-kind (JSON)".into(),
        ));
    }

    let mut body = serde_json::Map::new();
    body.insert("name".into(), Value::String(name.into()));
    body.insert("agent_id".into(), Value::String(agent.into()));
    body.insert("prompt".into(), Value::String(prompt.into()));

    if let Some(sk) = schedule_kind_json {
        let parsed: Value = serde_json::from_str(sk)
            .map_err(|e| CliError::Other(format!("invalid --schedule-kind JSON: {e}")))?;
        body.insert("schedule_kind".into(), parsed);
    } else if let Some(expr) = schedule_expr {
        // Use the server-side fallback path which treats `schedule` as a cron expr.
        body.insert("schedule".into(), Value::String(expr.into()));
    }

    if let Some(tz) = timezone {
        body.insert("timezone".into(), Value::String(tz.into()));
    }
    if let Some(tags) = tags_csv {
        let arr: Vec<Value> = tags
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.to_string()))
            .collect();
        body.insert("tags".into(), Value::Array(arr));
    }
    Ok(Value::Object(body))
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    server_url: &str,
    name: &str,
    schedule_expr: Option<&str>,
    schedule_kind_json: Option<&str>,
    agent: &str,
    prompt: &str,
    timezone: Option<&str>,
    tags_csv: Option<&str>,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let body = build_create_body(
        name,
        schedule_expr,
        schedule_kind_json,
        agent,
        prompt,
        timezone,
        tags_csv,
    )?;

    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;
    let result: Value = client.call("cron.create", Some(body)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("✓ Cron job created");
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    client.close().await?;
    Ok(())
}

/// Build the JSON body for `cron.update`. Only non-None fields are included.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_update_body(
    job_id: &str,
    name: Option<&str>,
    agent: Option<&str>,
    prompt: Option<&str>,
    schedule_expr: Option<&str>,
    schedule_kind_json: Option<&str>,
    timezone: Option<&str>,
    tags_csv: Option<&str>,
    enable: bool,
    disable: bool,
) -> Result<Value, CliError> {
    let mut body = serde_json::Map::new();
    body.insert("job_id".into(), Value::String(job_id.into()));

    if let Some(n) = name {
        body.insert("name".into(), Value::String(n.into()));
    }
    if let Some(a) = agent {
        body.insert("agent_id".into(), Value::String(a.into()));
    }
    if let Some(p) = prompt {
        body.insert("prompt".into(), Value::String(p.into()));
    }
    if let Some(sk) = schedule_kind_json {
        let parsed: Value = serde_json::from_str(sk)
            .map_err(|e| CliError::Other(format!("invalid --schedule-kind JSON: {e}")))?;
        body.insert("schedule_kind".into(), parsed);
    } else if let Some(expr) = schedule_expr {
        // Server-side update accepts the same tagged ScheduleKind shape; emit
        // the explicit Cron variant rather than relying on a legacy field.
        body.insert(
            "schedule_kind".into(),
            json!({"kind": "cron", "expr": expr}),
        );
    }
    if let Some(tz) = timezone {
        body.insert("timezone".into(), Value::String(tz.into()));
    }
    if let Some(tags) = tags_csv {
        let arr: Vec<Value> = tags
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.to_string()))
            .collect();
        body.insert("tags".into(), Value::Array(arr));
    }
    if enable {
        body.insert("enabled".into(), Value::Bool(true));
    } else if disable {
        body.insert("enabled".into(), Value::Bool(false));
    }
    Ok(Value::Object(body))
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    server_url: &str,
    job_id: &str,
    name: Option<&str>,
    agent: Option<&str>,
    prompt: Option<&str>,
    schedule_expr: Option<&str>,
    schedule_kind_json: Option<&str>,
    timezone: Option<&str>,
    tags_csv: Option<&str>,
    enable: bool,
    disable: bool,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let body = build_update_body(
        job_id,
        name,
        agent,
        prompt,
        schedule_expr,
        schedule_kind_json,
        timezone,
        tags_csv,
        enable,
        disable,
    )?;

    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;
    let result: Value = client.call("cron.update", Some(body)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("✓ Cron job updated");
    }

    client.close().await?;
    Ok(())
}

pub async fn delete(
    server_url: &str,
    job_id: &str,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let params = json!({ "job_id": job_id });
    let result: Value = client.call("cron.delete", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("✓ Cron job deleted: {job_id}");
    }

    client.close().await?;
    Ok(())
}

pub async fn toggle(
    server_url: &str,
    job_id: &str,
    enable: bool,
    disable: bool,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    client.authenticate(config).await?;

    let mut body = serde_json::Map::new();
    body.insert("job_id".into(), Value::String(job_id.into()));
    if enable {
        body.insert("enabled".into(), Value::Bool(true));
    } else if disable {
        body.insert("enabled".into(), Value::Bool(false));
    }

    let result: Value = client.call("cron.toggle", Some(Value::Object(body))).await?;

    if json {
        output::print_json(&result);
    } else {
        let new_enabled = result.get("enabled").and_then(|v| v.as_bool());
        match new_enabled {
            Some(v) => println!("{job_id}: enabled={v}"),
            None => println!("{}", serde_json::to_string_pretty(&result)?),
        }
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_body_requires_schedule_or_kind() {
        let err = build_create_body("job", None, None, "main", "", None, None).unwrap_err();
        assert!(err.to_string().contains("missing --schedule"));
    }

    #[test]
    fn create_body_with_cron_expr_emits_legacy_schedule_field() {
        let body =
            build_create_body("job", Some("*/5 * * * *"), None, "main", "ping", None, None)
                .expect("valid");
        assert_eq!(body["name"], "job");
        assert_eq!(body["agent_id"], "main");
        assert_eq!(body["schedule"], "*/5 * * * *");
        // schedule_kind absent when only --schedule supplied
        assert!(body.get("schedule_kind").is_none());
    }

    #[test]
    fn create_body_with_schedule_kind_overrides() {
        let kind = r#"{"kind":"interval","every_ms":60000}"#;
        let body = build_create_body("job", None, Some(kind), "main", "", None, None).expect("ok");
        assert_eq!(body["schedule_kind"]["kind"], "interval");
        assert_eq!(body["schedule_kind"]["every_ms"], 60_000);
    }

    #[test]
    fn create_body_tags_csv_splits_and_trims() {
        let body = build_create_body(
            "job",
            Some("* * * * *"),
            None,
            "main",
            "",
            None,
            Some("alpha, beta ,, gamma"),
        )
        .unwrap();
        let tags = body["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0], "alpha");
        assert_eq!(tags[1], "beta");
        assert_eq!(tags[2], "gamma");
    }

    #[test]
    fn update_body_skips_unset_fields() {
        let body = build_update_body(
            "id-1", None, None, None, None, None, None, None, false, false,
        )
        .unwrap();
        let obj = body.as_object().unwrap();
        assert_eq!(obj.len(), 1, "only job_id should remain");
        assert_eq!(obj["job_id"], "id-1");
    }

    #[test]
    fn update_body_enable_takes_precedence_over_disable_when_both_set() {
        // clap normally prevents this via conflicts_with, but unit-test the
        // builder's defensive behaviour: enable=true wins.
        let body = build_update_body(
            "id-1", None, None, None, None, None, None, None, true, true,
        )
        .unwrap();
        assert_eq!(body["enabled"], true);
    }

    #[test]
    fn update_body_disable_emits_false() {
        let body = build_update_body(
            "id-1", None, None, None, None, None, None, None, false, true,
        )
        .unwrap();
        assert_eq!(body["enabled"], false);
    }

    #[test]
    fn update_body_schedule_expr_wraps_in_cron_kind() {
        let body = build_update_body(
            "id-1",
            None,
            None,
            None,
            Some("0 0 * * *"),
            None,
            None,
            None,
            false,
            false,
        )
        .unwrap();
        assert_eq!(body["schedule_kind"]["kind"], "cron");
        assert_eq!(body["schedule_kind"]["expr"], "0 0 * * *");
    }
}
