//! Cron job management commands.
//!
//! All actions are thin envelopes around `cron.*` JSON-RPC. The server enforces
//! every business rule (schedule validation, `ScheduleKind` tagging, persistence);
//! the CLI just transports flags into JSON and renders responses.

use serde_json::{json, Value};

use aleph_protocol::cron::{CronJobRow, CronListResponse};

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

/// List all cron jobs.
///
/// Parses [`CronListResponse`] — the type the server CONSTRUCTS — instead of a
/// private struct guessing at the shape. The guess this replaced declared a
/// required `schedule: String`, a key `cron.list` has never sent, so
/// `from_value` errored for every non-empty job list, `.unwrap_or_default()`
/// turned that into an empty `Vec`, and this command reported
/// "No cron jobs configured" with exit code 0 on a server running jobs. Its
/// `description` / `last_run` / `next_run` columns were dead for the same
/// reason.
///
/// A decode failure is now propagated rather than folded into emptiness:
/// "I could not read the answer" and "there are no jobs" are two different
/// facts and only one of them is this command's to report.
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("cron.list", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let response: CronListResponse = serde_json::from_value(result.clone())
            .map_err(|e| CliError::Other(format!("cron.list returned an unreadable shape: {e}")))?;

        if response.jobs.is_empty() {
            println!("No cron jobs configured");
        } else {
            let headers = &["ID", "NAME", "SCHEDULE", "STATE", "NEXT RUN", "LAST RUN"];
            let rows: Vec<Vec<String>> = response
                .jobs
                .iter()
                .map(|j| {
                    vec![
                        j.id.clone(),
                        j.name.clone(),
                        j.schedule_summary(),
                        job_state(j),
                        render_epoch_ms(j.next_run_at),
                        render_epoch_ms(j.last_run_at),
                    ]
                })
                .collect();

            output::print_table(headers, &rows, false, &result);
        }
    }

    client.close().await?;
    Ok(())
}

/// The one word that answers "will this job fire again?".
///
/// `enabled` alone cannot: a permanently-failed job is *parked* — switch still
/// on, nothing scheduled — so a column that shows only the switch shows a dead
/// job as healthy. That is why `parked` is computed server-side, and it reached
/// no client until this row type existed.
fn job_state(job: &CronJobRow) -> String {
    if !job.enabled {
        "disabled".to_string()
    } else if job.parked {
        "parked".to_string()
    } else {
        "enabled".to_string()
    }
}

/// Render an epoch-**milliseconds** timestamp, or `-` when there is none.
fn render_epoch_ms(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => chrono::DateTime::from_timestamp_millis(ms)
            .map_or_else(|| ms.to_string(), |dt| dt.to_rfc3339()),
        None => "-".to_string(),
    }
}

/// Show cron scheduler status
pub async fn status(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "job_id": job_id });
    let result: Value = client.call("cron.run", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Triggered job: {job_id}");
    }

    client.close().await?;
    Ok(())
}

/// Inspect a single job (cron.get).
pub async fn get(server_url: &str, job_id: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

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
pub fn build_create_body(
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
            .map(str::trim)
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

    let (client, _events) = AlephClient::connect(server_url, config).await?;
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
pub fn build_update_body(
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
    timeout_secs: Option<u64>,
    clear_timeout: bool,
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
            .map(str::trim)
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
    // timeout override: --clear-timeout sends Null; --timeout-secs N sends N*1000;
    // both absent sends nothing (server treats as no-op).
    if clear_timeout {
        body.insert("timeout_ms".into(), Value::Null);
    } else if let Some(secs) = timeout_secs {
        let ms = i64::try_from(secs)
            .ok()
            .and_then(|s| s.checked_mul(1000))
            .ok_or_else(|| CliError::Other(format!("--timeout-secs {secs} overflows i64 ms")))?;
        if ms <= 0 {
            return Err(CliError::Other(
                "--timeout-secs must be > 0 (use --clear-timeout to remove)".to_string(),
            ));
        }
        body.insert("timeout_ms".into(), Value::Number(ms.into()));
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
    timeout_secs: Option<u64>,
    clear_timeout: bool,
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
        timeout_secs,
        clear_timeout,
    )?;

    let (client, _events) = AlephClient::connect(server_url, config).await?;
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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

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
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let mut body = serde_json::Map::new();
    body.insert("job_id".into(), Value::String(job_id.into()));
    if enable {
        body.insert("enabled".into(), Value::Bool(true));
    } else if disable {
        body.insert("enabled".into(), Value::Bool(false));
    }

    let result: Value = client
        .call("cron.toggle", Some(Value::Object(body)))
        .await?;

    if json {
        output::print_json(&result);
    } else {
        let new_enabled = result.get("enabled").and_then(serde_json::Value::as_bool);
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
        let body = build_create_body("job", Some("*/5 * * * *"), None, "main", "ping", None, None)
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
            "id-1", None, None, None, None, None, None, None, false, false, None, false,
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
            "id-1", None, None, None, None, None, None, None, true, true, None, false,
        )
        .unwrap();
        assert_eq!(body["enabled"], true);
    }

    #[test]
    fn update_body_disable_emits_false() {
        let body = build_update_body(
            "id-1", None, None, None, None, None, None, None, false, true, None, false,
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
            None,
            false,
        )
        .unwrap();
        assert_eq!(body["schedule_kind"]["kind"], "cron");
        assert_eq!(body["schedule_kind"]["expr"], "0 0 * * *");
    }

    /// D1: --timeout-secs N emits timeout_ms as N*1000.
    #[test]
    fn update_body_timeout_secs_emits_ms() {
        let body = build_update_body(
            "id-1",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            Some(600),
            false,
        )
        .unwrap();
        assert_eq!(body["timeout_ms"], 600_000);
    }

    /// D1: --clear-timeout emits an explicit null.
    #[test]
    fn update_body_clear_timeout_emits_null() {
        let body = build_update_body(
            "id-1", None, None, None, None, None, None, None, false, false, None, true,
        )
        .unwrap();
        assert!(body["timeout_ms"].is_null());
        assert!(body.as_object().unwrap().contains_key("timeout_ms"));
    }

    /// D1: neither flag → field absent.
    #[test]
    fn update_body_no_timeout_flag_omits_field() {
        let body = build_update_body(
            "id-1", None, None, None, None, None, None, None, false, false, None, false,
        )
        .unwrap();
        assert!(!body.as_object().unwrap().contains_key("timeout_ms"));
    }

    /// D1: rejects timeout_secs that would overflow i64 ms.
    #[test]
    fn update_body_timeout_secs_overflow_rejected() {
        let err = build_update_body(
            "id-1",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            Some(u64::MAX),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("overflows"));
    }
}
