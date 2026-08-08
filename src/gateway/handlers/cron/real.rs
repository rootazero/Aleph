//! Real cron handlers — delegate to `CronService` via `SharedCronService`.

use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::tasks::cron::clock::Clock;
use crate::tasks::cron::service::ops::{validate_schedule_kind, CronJobUpdates};
use crate::tasks::cron::{
    CronJob, CronJobView, FailureAlertConfig, ScheduleKind, SessionTarget, SharedCronService,
};

// ============================================================================
// Helper functions
// ============================================================================

/// Extract a string parameter from a JSON-RPC request
fn extract_str(request: &JsonRpcRequest, key: &str) -> Option<String> {
    match &request.params {
        Some(Value::Object(map)) => map.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()),
        _ => None,
    }
}

/// Fold a top-level `timezone` parameter into the schedule it belongs to.
///
/// `timezone` is *not* a job field — the scheduler reads exactly one place,
/// `ScheduleKind::Cron { tz }`. Normalizing here (the parse boundary shared by
/// Panel, CLI and any future client) keeps that the single write path rather
/// than making every caller learn the tagged-enum shape. Interval and one-shot
/// schedules are absolute, so a timezone on them is a mistake worth reporting
/// instead of dropping.
fn apply_timezone(kind: &mut ScheduleKind, tz: &str) -> Result<(), String> {
    match kind {
        ScheduleKind::Cron { tz: slot, .. } => {
            let trimmed = tz.trim();
            *slot = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            Ok(())
        }
        ScheduleKind::Every { .. } | ScheduleKind::At { .. } => Err(
            "timezone applies only to cron schedules; interval and one-shot schedules are absolute"
                .to_string(),
        ),
    }
}

/// Parse the `failure_alert` parameter into the tri-state update convention.
///
/// Absent → `None` (leave alone); explicit `null` → `Some(None)` (clear);
/// object → `Some(Some(cfg))`. A malformed object is rejected rather than
/// silently ignored: the field names below (`after` / `cooldown_ms` /
/// `target`) are the contract, and a client spelling them differently used to
/// get a success response with nothing stored.
fn parse_failure_alert(
    params: &serde_json::Map<String, Value>,
) -> Result<Option<Option<FailureAlertConfig>>, String> {
    match params.get("failure_alert") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => serde_json::from_value::<FailureAlertConfig>(v.clone())
            .map(|cfg| Some(Some(cfg)))
            .map_err(|e| format!("Invalid failure_alert: {e}")),
    }
}

/// Serialize a `CronJobView` to JSON (includes all new fields)
fn job_view_to_json(view: &CronJobView) -> Value {
    json!({
        "id": view.id,
        "name": view.name,
        "enabled": view.enabled,
        "schedule_kind": view.schedule_kind,
        "agent_id": view.agent_id,
        "source_channel_id": view.source_channel_id,
        "prompt": view.prompt,
        "timezone": view.timezone,
        "tags": view.tags,
        "session_target": view.session_target,
        "created_at": view.created_at,
        "updated_at": view.updated_at,
        // State fields
        "next_run_at": view.state.next_run_at_ms,
        "running_at_ms": view.state.running_at_ms,
        "last_run_at": view.state.last_run_at_ms,
        "last_run_status": view.state.last_run_status,
        "last_error": view.state.last_error,
        "last_error_reason": view.state.last_error_reason,
        "last_duration_ms": view.state.last_duration_ms,
        "consecutive_errors": view.state.consecutive_errors,
        "last_delivery_status": view.state.last_delivery_status,
        // Config fields
        "failure_alert": view.failure_alert,
        "timeout_ms": view.timeout_ms,
    })
}

// ============================================================================
// Handlers
// ============================================================================

/// Handle cron.list RPC request (real)
pub async fn handle_list(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => {
            let jobs_json: Vec<Value> = jobs.iter().map(job_view_to_json).collect();
            JsonRpcResponse::success(request.id, json!({ "jobs": jobs_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list jobs: {e}"),
        ),
    }
}

/// Handle cron.get RPC request (real)
pub async fn handle_get(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let service = cron.lock().await;
    match service.get_job(&job_id).await {
        Ok(view) => JsonRpcResponse::success(request.id, json!({ "job": job_view_to_json(&view) })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get job: {e}"),
        ),
    }
}

/// Handle cron.create RPC request (real)
pub async fn handle_create(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing name");
        }
    };

    let agent_id = params
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string();

    let prompt = params
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Parse schedule_kind from tagged JSON
    let mut schedule_kind = match params.get("schedule_kind") {
        Some(sk) => match serde_json::from_value::<ScheduleKind>(sk.clone()) {
            Ok(kind) => kind,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid schedule_kind: {e}"),
                );
            }
        },
        None => {
            // Fallback: try legacy "schedule" field as cron expression
            match params.get("schedule").and_then(|v| v.as_str()) {
                Some(expr) => ScheduleKind::Cron {
                    expr: expr.to_string(),
                    tz: None,
                    stagger_ms: None,
                },
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Missing schedule_kind or schedule",
                    );
                }
            }
        }
    };

    // Validate At timestamps: reject if in the past
    if let ScheduleKind::At { at, .. } = &schedule_kind {
        let now_ms = chrono::Utc::now().timestamp_millis();
        if *at <= now_ms {
            let at_human = chrono::DateTime::from_timestamp_millis(*at).map_or_else(
                || format!("{at}ms"),
                |dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            );
            let now_human = chrono::Utc::now()
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string();
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Cannot schedule in the past. at={at} resolves to {at_human}, current time is {now_human} (now_ms={now_ms})"
                ),
            );
        }
    }

    // `timezone` is folded into the schedule *before* validation so an
    // operator-typed zone is parsed here rather than collapsing to a job that
    // is accepted and then never fires.
    if let Some(tz) = params.get("timezone").and_then(|v| v.as_str()) {
        if let Err(e) = apply_timezone(&mut schedule_kind, tz) {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, e);
        }
    }
    if let Err(e) = validate_schedule_kind(&schedule_kind) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Invalid schedule: {e}"),
        );
    }

    let mut job = CronJob::new(name, agent_id, prompt, schedule_kind);

    // Optional fields
    if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        job.enabled = enabled;
    }
    match parse_failure_alert(params) {
        Ok(Some(alert)) => job.failure_alert = alert,
        Ok(None) => {}
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    }
    if let Some(tags) = params.get("tags").and_then(|v| v.as_array()) {
        job.tags = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(st) = params.get("session_target") {
        if let Ok(target) = serde_json::from_value::<SessionTarget>(st.clone()) {
            job.session_target = target;
        }
    }
    if let Some(tv) = params.get("timeout_ms") {
        match tv {
            Value::Null => {} // explicit null at create == default (no override)
            Value::Number(n) => match n.as_i64() {
                Some(v) if v > 0 => {
                    job.timeout_ms = Some(v);
                }
                _ => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "timeout_ms must be a positive integer (ms)",
                    );
                }
            },
            _ => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    "timeout_ms must be a positive integer (ms)",
                );
            }
        }
    }

    let service = cron.lock().await;
    match service.add_job(job).await {
        Ok(job_id) => match service.get_job(&job_id).await {
            Ok(view) => {
                JsonRpcResponse::success(request.id, json!({ "job": job_view_to_json(&view) }))
            }
            Err(_) => JsonRpcResponse::success(request.id, json!({ "job": { "id": job_id } })),
        },
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create job: {e}"),
        ),
    }
}

/// Handle cron.update RPC request (real)
pub async fn handle_update(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    // Build partial updates
    let mut updates = CronJobUpdates::default();

    if let Some(name) = params.get("name").and_then(|v| v.as_str()) {
        updates.name = Some(name.to_string());
    }
    if let Some(agent_id) = params.get("agent_id").and_then(|v| v.as_str()) {
        updates.agent_id = Some(agent_id.to_string());
    }
    if let Some(prompt) = params.get("prompt").and_then(|v| v.as_str()) {
        updates.prompt = Some(prompt.to_string());
    }
    if let Some(enabled) = params.get("enabled").and_then(|v| v.as_bool()) {
        updates.enabled = Some(enabled);
    }
    // Explicit null = no-op (back-compat); a present-but-malformed value is
    // rejected to avoid a silent no-op that looks like a successful update.
    if let Some(sk) = params.get("schedule_kind").filter(|v| !v.is_null()) {
        match serde_json::from_value::<ScheduleKind>(sk.clone()) {
            Ok(kind) => updates.schedule_kind = Some(kind),
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid schedule_kind: {e}"),
                );
            }
        }
    }
    if let Some(tags) = params.get("tags").and_then(|v| v.as_array()) {
        updates.tags = Some(
            tags.iter()
                .filter_map(|t| t.as_str().map(|s| s.to_string()))
                .collect(),
        );
    }
    if let Some(st) = params.get("session_target").filter(|v| !v.is_null()) {
        match serde_json::from_value::<SessionTarget>(st.clone()) {
            Ok(target) => updates.session_target = Some(target),
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid session_target: {e}"),
                );
            }
        }
    }
    match parse_failure_alert(params) {
        Ok(alert) => updates.failure_alert = alert,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    }
    // `timezone` needs a schedule to land in. Clients that edit the schedule
    // send both; clients that only change the zone send just `timezone`, so
    // fall back to the job's current schedule rather than dropping it — that
    // silent drop is what let the Panel editor erase a zone set via the tool.
    if let Some(tz) = params.get("timezone").and_then(|v| v.as_str()) {
        let mut kind = match updates.schedule_kind.clone() {
            Some(k) => k,
            None => match cron.lock().await.get_job(&job_id).await {
                Ok(view) => view.schedule_kind,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to get job: {e}"),
                    );
                }
            },
        };
        if let Err(e) = apply_timezone(&mut kind, tz) {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, e);
        }
        updates.schedule_kind = Some(kind);
    }
    if let Some(ref kind) = updates.schedule_kind {
        if let Err(e) = validate_schedule_kind(kind) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid schedule: {e}"),
            );
        }
    }
    // timeout_ms tri-state: absent = no-op, Null = clear, positive integer = set.
    // Negative / zero / non-integer values are rejected to avoid silent no-ops
    // that look like a successful update.
    if let Some(tv) = params.get("timeout_ms") {
        match tv {
            Value::Null => updates.timeout_ms = Some(None),
            Value::Number(n) => match n.as_i64() {
                Some(v) if v > 0 => {
                    updates.timeout_ms = Some(Some(v));
                }
                _ => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "timeout_ms must be a positive integer (ms) or null to clear",
                    );
                }
            },
            _ => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    "timeout_ms must be a positive integer (ms) or null to clear",
                );
            }
        }
    }

    let service = cron.lock().await;
    match service.update_job(&job_id, updates).await {
        Ok(()) => match service.get_job(&job_id).await {
            Ok(view) => {
                JsonRpcResponse::success(request.id, json!({ "job": job_view_to_json(&view) }))
            }
            Err(_) => JsonRpcResponse::success(
                request.id,
                json!({ "job": { "id": job_id, "updated": true } }),
            ),
        },
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update job: {e}"),
        ),
    }
}

/// Handle cron.delete RPC request (real)
pub async fn handle_delete(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let service = cron.lock().await;
    match service.delete_job(&job_id).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "deleted": job_id })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete job: {e}"),
        ),
    }
}

/// Handle cron.status RPC request (real)
pub async fn handle_status(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => {
            let enabled_count = jobs.iter().filter(|j| j.enabled).count();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "running": true,
                    "job_count": jobs.len(),
                    "enabled_count": enabled_count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get status: {e}"),
        ),
    }
}

/// Handle cron.run RPC request (real)
///
/// Manually triggers a cron job by setting its `next_run_at_ms` to now.
pub async fn handle_run(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let service = cron.lock().await;
    // Delegate to `CronService::run_job` rather than poking the store
    // directly: it validates (enabled + not already running), propagates a
    // failed persist instead of swallowing it (a dropped write here is
    // silently discarded by the timer's next `force_reload`, so the job never
    // actually runs despite a success reply), and emits the `StateChanged`
    // event so the panel refreshes.
    match service.run_job(&job_id).await {
        Ok(()) => {
            let clock_now = service.state().clock.now_ms();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "triggered": job_id,
                    "status": "queued",
                    "next_run_at_ms": clock_now,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to run job: {e}"),
        ),
    }
}

/// Handle cron.runs RPC request (real)
///
/// Returns the execution history from `SQLite`.
pub async fn handle_runs(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let job_id = match extract_str(&request, "job_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let limit = match &request.params {
        Some(Value::Object(map)) => {
            map.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize
        }
        _ => 20,
    };

    let service = cron.lock().await;
    let store = service.state().store.lock().await;
    match store.get_runs(&job_id, limit) {
        Ok(runs) => {
            let runs_json: Vec<Value> = runs
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "job_id": r.job_id,
                        "trigger_source": r.trigger_source,
                        "status": r.status,
                        "started_at": r.started_at,
                        "ended_at": r.ended_at,
                        "duration_ms": r.duration_ms,
                        "error": r.error,
                        "error_reason": r.error_reason,
                        "delivery_status": r.delivery_status,
                        "created_at": r.created_at,
                    })
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "job_id": job_id, "runs": runs_json }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get runs: {e}"),
        ),
    }
}

/// Handle cron.toggle RPC request (real)
pub async fn handle_toggle(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let job_id = match params.get("job_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing job_id");
        }
    };

    let enabled = params.get("enabled").and_then(|v| v.as_bool());

    let service = cron.lock().await;
    let result = match enabled {
        Some(true) => service.enable_job(&job_id).await.map(|()| true),
        Some(false) => service.disable_job(&job_id).await.map(|()| false),
        None => service.toggle_job(&job_id).await,
    };

    match result {
        Ok(new_enabled) => JsonRpcResponse::success(
            request.id,
            json!({
                "job_id": job_id,
                "enabled": new_enabled,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to toggle job: {e}"),
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The Panel's request DTOs, read at compile time. Comparing against the
    /// *source* is deliberate: a runtime check would need a live Panel build,
    /// and the failure mode being guarded is a field that exists on one side
    /// and is never named on the other.
    const PANEL_CRON_API: &str = include_str!("../../../../interfaces/webchat/src/api/cron.rs");
    const THIS_HANDLER: &str = include_str!("real.rs");

    /// Collect the `pub <name>:` field names of a struct from Rust source.
    fn struct_fields(source: &str, struct_name: &str) -> Vec<String> {
        let start = source
            .find(&format!("pub struct {struct_name} {{"))
            .unwrap_or_else(|| panic!("{struct_name} not found in the Panel cron API"));
        let body = &source[start..];
        let end = body.find("\n}").expect("unterminated struct");
        body[..end]
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("pub ")?;
                let name = rest.split(':').next()?.trim();
                (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                    .then(|| name.to_string())
            })
            .collect()
    }

    /// Every field the Panel can send must be named by this handler.
    ///
    /// "The DTO has the field, the handler never reads it" shipped three
    /// separate times in this one file (`failure_alert`, `enabled` on create,
    /// `session_target` on update) and each time the RPC answered `success`
    /// with the setting discarded. Eyeballing the two field lists does not
    /// work; this does.
    #[test]
    fn every_panel_dto_field_is_read_by_a_handler() {
        let mut missing = Vec::new();
        for dto in ["CreateCronJob", "UpdateCronJob"] {
            for field in struct_fields(PANEL_CRON_API, dto) {
                let probe = format!("params.get(\"{field}\")");
                if !THIS_HANDLER.contains(&probe) {
                    missing.push(format!("{dto}.{field}"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "Panel cron DTO fields with no reader in real.rs: {missing:?}. \
             Either read them or delete them from the DTO — a write-only field \
             is a silent no-op at the call site."
        );
    }

    /// Sanity: the extractor actually sees fields, so the guard above cannot
    /// pass by finding nothing to check.
    #[test]
    fn dto_field_extraction_is_not_vacuous() {
        let fields = struct_fields(PANEL_CRON_API, "CreateCronJob");
        assert!(fields.len() >= 8, "extracted too few fields: {fields:?}");
        assert!(fields.iter().any(|f| f == "failure_alert"));
    }

    #[test]
    fn timezone_lands_in_the_cron_schedule() {
        let mut kind = ScheduleKind::Cron {
            expr: "0 0 9 * * *".to_string(),
            tz: None,
            stagger_ms: None,
        };
        apply_timezone(&mut kind, "Asia/Shanghai").unwrap();
        match kind {
            ScheduleKind::Cron { tz, .. } => assert_eq!(tz.as_deref(), Some("Asia/Shanghai")),
            other => panic!("schedule kind changed: {other:?}"),
        }
    }

    /// An absolute schedule has no timezone to honour, so accepting one would
    /// be the silent drop this change exists to remove.
    #[test]
    fn timezone_on_an_absolute_schedule_is_rejected() {
        let mut kind = ScheduleKind::Every {
            every_ms: 60_000,
            anchor_ms: None,
        };
        assert!(apply_timezone(&mut kind, "Asia/Shanghai").is_err());
    }

    /// An unparseable zone used to produce a job with `next_run_at_ms = None`
    /// — accepted, reported created, never fires.
    #[test]
    fn unknown_timezone_is_an_error_not_a_silent_park() {
        let kind = ScheduleKind::Cron {
            expr: "0 0 9 * * *".to_string(),
            tz: Some("Mars/Olympus_Mons".to_string()),
            stagger_ms: None,
        };
        let err = validate_schedule_kind(&kind).unwrap_err();
        assert!(err.contains("Mars/Olympus_Mons"), "unhelpful error: {err}");
    }

    #[test]
    fn failure_alert_tri_state() {
        let object = json!({
            "failure_alert": {
                "after": 3,
                "cooldown_ms": 60_000,
                "target": {"kind": "Webhook", "url": "https://example.com"},
            }
        });
        let parsed = parse_failure_alert(object.as_object().unwrap())
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(parsed.after, 3);
        assert_eq!(parsed.cooldown_ms, 60_000);

        let cleared = json!({ "failure_alert": null });
        assert!(parse_failure_alert(cleared.as_object().unwrap())
            .unwrap()
            .unwrap()
            .is_none());

        let absent = json!({});
        assert!(parse_failure_alert(absent.as_object().unwrap())
            .unwrap()
            .is_none());
    }

    /// The old Panel spelling (`after_n` / `cooldown` / `kind` / `channel`)
    /// overlapped the backend contract in exactly zero field names, so it
    /// round-tripped as "saved" while storing nothing. It must now fail loudly.
    #[test]
    fn legacy_panel_alert_spelling_is_rejected() {
        let legacy = json!({
            "failure_alert": {
                "after_n": 3,
                "cooldown": "1h",
                "kind": "announce",
                "channel": "telegram",
            }
        });
        assert!(parse_failure_alert(legacy.as_object().unwrap()).is_err());
    }
}
