//! Real cron handlers — delegate to `CronService` via `SharedCronService`.

use serde_json::{json, Value};

use crate::gateway::handlers::task_error;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::tasks::cron::clock::Clock;
use crate::tasks::cron::service::ops::{validate_schedule_kind, CronJobUpdates};
use crate::tasks::cron::{
    CronJob, CronJobView, FailureAlertConfig, JobChain, ScheduleKind, SessionTarget,
    SharedCronService,
};
use aleph_protocol::cron::CronJobRow;

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

/// Parse the `chain` parameter into the tri-state update convention.
///
/// Identical shape to [`parse_failure_alert`], and for the same reason: absent
/// → leave alone, explicit `null` → clear, object → replace. Malformed is an
/// error rather than a silent skip — a client that misspells `on_success`
/// would otherwise get `success` back with no chain stored, which is the exact
/// failure this whole field was added to end.
fn parse_chain(
    params: &serde_json::Map<String, Value>,
) -> Result<Option<Option<JobChain>>, String> {
    match params.get("chain") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => serde_json::from_value::<JobChain>(v.clone())
            .map(|c| Some((!c.is_empty()).then_some(c)))
            .map_err(|e| format!("Invalid chain: {e}")),
    }
}

/// The serialized token of a unit enum, taken from its own `Serialize` impl.
///
/// Not `Display`: `RunStatus` carries both, and two hand-maintained spellings
/// of one fact drift. serde's is the one that has always been on the wire.
fn wire_tag<T: serde::Serialize>(value: T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

/// Project a `CronJobView` onto the wire row every client parses.
///
/// **Constructs** [`CronJobRow`] rather than hand-writing a `json!` map, which
/// is the whole point: a hand-written map is a contract with no compiler behind
/// it, and the CLI spent its whole life parsing this response against a
/// `schedule` key that has never existed here — `serde_json::from_value`
/// returned `Err` for every non-empty job list and `aleph cron list` answered
/// "No cron jobs configured" on a server with jobs. Building the shared type
/// means a rename is a compile error on this side and a loud parse error on the
/// other, and over-sending a field with no reader is not expressible.
///
/// `parked` joins the wire here. `CronJobView` has computed it since the field
/// was introduced — precisely so a surface would stop showing a permanently
/// failed job as healthy — but it was never emitted, so no RPC client could ask
/// the question the field exists to answer.
fn job_view_to_json(view: &CronJobView) -> Value {
    let row = CronJobRow {
        id: view.id.clone(),
        name: view.name.clone(),
        enabled: view.enabled,
        parked: view.parked,
        schedule_kind: serde_json::to_value(&view.schedule_kind).unwrap_or(Value::Null),
        agent_id: view.agent_id.clone(),
        source_channel_id: view.source_channel_id.clone(),
        prompt: view.prompt.clone(),
        timezone: view.timezone.clone(),
        tags: view.tags.clone(),
        session_target: match view.session_target {
            SessionTarget::Main => "main".to_string(),
            SessionTarget::Isolated => "isolated".to_string(),
        },
        created_at: view.created_at,
        updated_at: view.updated_at,
        next_run_at: view.state.next_run_at_ms,
        running_at_ms: view.state.running_at_ms,
        last_run_at: view.state.last_run_at_ms,
        last_run_status: view.state.last_run_status.and_then(wire_tag),
        last_error: view.state.last_error.clone(),
        last_error_reason: view
            .state
            .last_error_reason
            .as_ref()
            .and_then(|r| serde_json::to_value(r).ok()),
        last_duration_ms: view.state.last_duration_ms,
        consecutive_errors: view.state.consecutive_errors,
        last_delivery_status: view.state.last_delivery_status.and_then(wire_tag),
        failure_alert: view
            .failure_alert
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok()),
        chain: view
            .chain
            .as_ref()
            .and_then(|c| serde_json::to_value(c).ok()),
        timeout_ms: view.timeout_ms,
    };
    serde_json::to_value(row).unwrap_or(Value::Null)
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
        Err(e) => task_error::respond(request.id, "Failed to list jobs", &e),
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
        Err(e) => task_error::respond(request.id, "Failed to get job", &e),
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
    match parse_chain(params) {
        // Existence / cycle / self-link are checked inside `add_job`, under the
        // store lock — the same predicate `cron_manage` gets.
        Ok(Some(chain)) => job.set_chain(chain),
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
        // The chain checks inside `add_job` land here. They are the reason
        // this round happened: a chain to a job that does not exist, a cycle
        // and a self-link are all things the caller typed, and all three used
        // to come back as `-32603 Internal error`.
        Err(e) => task_error::respond(request.id, "Failed to create job", &e),
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
    match parse_chain(params) {
        Ok(chain) => updates.chain = chain,
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
                Err(e) => return task_error::respond(request.id, "Failed to get job", &e),
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
        Err(e) => task_error::respond(request.id, "Failed to update job", &e),
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
        Err(e) => task_error::respond(request.id, "Failed to delete job", &e),
    }
}

/// Handle cron.status RPC request (real)
pub async fn handle_status(request: JsonRpcRequest, cron: SharedCronService) -> JsonRpcResponse {
    let service = cron.lock().await;
    match service.list_jobs().await {
        Ok(jobs) => {
            let enabled_count = jobs.iter().filter(|j| j.enabled).count();
            // `running` used to be the literal `true`. The timer loop's startup
            // is conditional (no execution adapter ⇒ "Cron timer loop: skipped"
            // on stdout, and in daemon mode not even that) while every `cron.*`
            // handler is registered either way — so an operator whose jobs
            // silently never fire was told the scheduler was up, by the one
            // surface that exists to answer that question. Derive it from the
            // scan the loop actually performs: alive means it scanned within
            // three intervals, which tolerates one missed wake-up without
            // reporting a healthy scheduler as dead.
            let last_tick_at_ms = service.last_tick_at_ms();
            let liveness_window_ms = (service.check_interval_secs() as i64) * 1000 * 3;
            let now_ms = chrono::Utc::now().timestamp_millis();
            let running =
                last_tick_at_ms != 0 && now_ms.saturating_sub(last_tick_at_ms) < liveness_window_ms;
            JsonRpcResponse::success(
                request.id,
                json!({
                    "running": running,
                    // Reported alongside so a client can say "last scan 4m ago"
                    // rather than only "false"; `null` = never scanned.
                    "last_tick_at_ms": (last_tick_at_ms != 0).then_some(last_tick_at_ms),
                    "job_count": jobs.len(),
                    "enabled_count": enabled_count,
                }),
            )
        }
        Err(e) => task_error::respond(request.id, "Failed to get status", &e),
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
        // "job not found", "disabled, enable it first" and "already running"
        // are three different things the caller can act on, and all three came
        // back as an internal error before this went through the classifier.
        Err(e) => task_error::respond(request.id, "Failed to run job", &e),
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
    // Through `job_runs`, not `state().store` directly: reaching past the
    // service was a second path to the same rows whose failures could not be
    // classified with the rest, and it is the path the conversational face
    // (`cron_manage`) never used.
    match service.job_runs(&job_id, limit).await {
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
        Err(e) => task_error::respond(request.id, "Failed to get runs", &e),
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
        Err(e) => task_error::respond(request.id, "Failed to toggle job", &e),
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

    #[test]
    fn chain_tri_state() {
        let object = json!({ "chain": { "on_success": "job-b", "on_failure": "job-c" } });
        let parsed = parse_chain(object.as_object().unwrap())
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(parsed.on_success.as_deref(), Some("job-b"));
        assert_eq!(parsed.on_failure.as_deref(), Some("job-c"));

        let cleared = json!({ "chain": null });
        assert!(parse_chain(cleared.as_object().unwrap())
            .unwrap()
            .unwrap()
            .is_none());

        let absent = json!({});
        assert!(parse_chain(absent.as_object().unwrap()).unwrap().is_none());

        // `{}` means "no links", which is the same state as cleared — not a
        // chain object with two `None`s that reads as "set" downstream.
        let empty = json!({ "chain": {} });
        assert!(parse_chain(empty.as_object().unwrap())
            .unwrap()
            .unwrap()
            .is_none());
    }

    /// A misspelled link key must fail loudly rather than store nothing and
    /// answer `success` — the same failure mode `failure_alert` shipped with.
    #[test]
    fn misspelled_chain_key_is_rejected() {
        let typo = json!({ "chain": { "onSuccess": "job-b" } });
        assert!(parse_chain(typo.as_object().unwrap()).is_err());
    }

    /// The chain must come back out of the same view the setters write into.
    /// A settable field with no read-back leaves the caller unable to confirm
    /// what was stored.
    #[test]
    fn the_rendered_job_carries_its_chain() {
        let mut job = CronJob::new(
            "src",
            "agent",
            "p",
            ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.set_chain(Some(crate::tasks::cron::JobChain {
            on_success: Some("job-b".to_string()),
            on_failure: None,
        }));
        let rendered = job_view_to_json(&CronJobView::from(&job));
        assert_eq!(rendered["chain"]["on_success"], "job-b");
        assert!(rendered["chain"].get("on_failure").is_none());

        job.set_chain(None);
        let rendered = job_view_to_json(&CronJobView::from(&job));
        assert!(rendered["chain"].is_null(), "no chain must render as null");
    }

    // ── Error classification over the wire ──────────────────────────
    //
    // Unit-testing `task_error::respond` proves the mapping; these prove the
    // handlers reach it. Between the two sits the thing that was actually
    // broken for a year: a `Result<_, String>` that gave every handler no
    // choice but `INTERNAL_ERROR`.

    use crate::tasks::cron::{CronConfig, CronService};

    fn live_service(dir: &tempfile::TempDir) -> SharedCronService {
        let service = CronService::new(CronConfig {
            db_path: dir.path().join("cron.db").to_string_lossy().to_string(),
            ..CronConfig::default()
        })
        .unwrap();
        std::sync::Arc::new(tokio::sync::Mutex::new(service))
    }

    fn request(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params: Some(params),
        }
    }

    fn error_of(response: JsonRpcResponse) -> (i32, String) {
        let e = response.error.expect("expected an error response");
        (e.code, e.message)
    }

    /// The leftover this round exists for.
    ///
    /// `add_job` refuses a chain whose target does not exist — the caller
    /// typed a job id that is not there and can fix it by creating that job
    /// first. It arrived as `-32603 Internal error`: retry, read the server
    /// log, the server is broken. None of that was true.
    #[tokio::test]
    async fn a_chain_to_a_missing_job_is_invalid_params_not_an_internal_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let cron = live_service(&dir);

        let response = handle_create(
            request(
                "cron.create",
                json!({
                    "name": "source",
                    "schedule_kind": { "kind": "every", "every_ms": 60_000 },
                    "chain": { "on_success": "no-such-job" },
                }),
            ),
            cron.clone(),
        )
        .await;

        let (code, message) = error_of(response);
        assert_eq!(code, INVALID_PARAMS, "message was: {message}");
        assert!(
            message.contains("no-such-job"),
            "the refusal must still name the target: {message}"
        );

        // And the refusal wrote nothing.
        let jobs = cron.lock().await.list_jobs().await.unwrap();
        assert!(jobs.is_empty());
    }

    /// A self-link and a cycle are the same class — caller-authored content
    /// the scheduler refuses — and must not be able to drift apart from the
    /// case above.
    #[tokio::test]
    async fn a_self_link_is_also_invalid_params() {
        let dir = tempfile::TempDir::new().unwrap();
        let cron = live_service(&dir);

        let created = handle_create(
            request(
                "cron.create",
                json!({ "name": "a", "schedule_kind": { "kind": "every", "every_ms": 60_000 } }),
            ),
            cron.clone(),
        )
        .await;
        let id = created.result.unwrap()["job"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let response = handle_update(
            request(
                "cron.update",
                json!({ "job_id": id, "chain": { "on_success": id } }),
            ),
            cron,
        )
        .await;

        let (code, message) = error_of(response);
        assert_eq!(code, INVALID_PARAMS, "message was: {message}");
        assert!(message.contains("itself"), "{message}");
    }

    /// An id that is not in the store is `RESOURCE_NOT_FOUND`, not "the server
    /// broke". `cron.*` is operator-gated, so there is no existence oracle to
    /// protect and a named 404 is the honest answer.
    #[tokio::test]
    async fn an_unknown_job_id_is_resource_not_found_on_every_verb_that_addresses_one() {
        use crate::gateway::protocol::RESOURCE_NOT_FOUND;

        let dir = tempfile::TempDir::new().unwrap();
        let cron = live_service(&dir);
        let missing = json!({ "job_id": "ghost" });

        for (verb, response) in [
            (
                "cron.get",
                handle_get(request("cron.get", missing.clone()), cron.clone()).await,
            ),
            (
                "cron.delete",
                handle_delete(request("cron.delete", missing.clone()), cron.clone()).await,
            ),
            (
                "cron.update",
                handle_update(
                    request("cron.update", json!({ "job_id": "ghost", "name": "x" })),
                    cron.clone(),
                )
                .await,
            ),
            (
                "cron.toggle",
                handle_toggle(request("cron.toggle", missing.clone()), cron.clone()).await,
            ),
            (
                "cron.run",
                handle_run(request("cron.run", missing.clone()), cron.clone()).await,
            ),
        ] {
            let (code, message) = error_of(response);
            assert_eq!(
                code, RESOURCE_NOT_FOUND,
                "{verb} answered {code}: {message}"
            );
            assert!(message.contains("ghost"), "{verb}: {message}");
        }
    }

    /// "the job is disabled" is a refusal the caller can act on in one call.
    /// It shares `INVALID_PARAMS` with the content refusals above on purpose —
    /// see `TaskError` for why there is no `Conflict` code with no consumer.
    #[tokio::test]
    async fn running_a_disabled_job_is_a_caller_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let cron = live_service(&dir);

        let created = handle_create(
            request(
                "cron.create",
                json!({
                    "name": "off",
                    "enabled": false,
                    "schedule_kind": { "kind": "every", "every_ms": 60_000 },
                }),
            ),
            cron.clone(),
        )
        .await;
        let id = created.result.unwrap()["job"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let (code, message) =
            error_of(handle_run(request("cron.run", json!({ "job_id": id })), cron).await);
        assert_eq!(code, INVALID_PARAMS, "message was: {message}");
        assert!(message.contains("enable it first"), "{message}");
    }

    /// What `cron.list` emits must be EXACTLY the shared row — no key more, no
    /// key less.
    ///
    /// Equality, not containment. A superset assertion is what parsing already
    /// gives you and it is structurally blind in the direction that hurt here:
    /// this handler hand-wrote a `json!` map for its whole life, the CLI parsed
    /// it against a `schedule` key that was never in it, and every CLI-side
    /// test was green because it only ever read literals it had just written.
    /// Deriving the expected key set from `CronJobRow` itself means the two
    /// sides cannot drift without one of them failing to compile.
    #[test]
    fn cron_list_row_key_set_is_exactly_the_contract() {
        let mut job = CronJob::new(
            "daily brief",
            "main",
            "summarise",
            ScheduleKind::Cron {
                expr: "0 0 8 * * *".to_string(),
                tz: Some("UTC".to_string()),
                stagger_ms: None,
            },
        );
        job.state.next_run_at_ms = Some(1_700_000_000_000);
        let view = CronJobView::from(&job);

        let emitted = job_view_to_json(&view);
        let emitted_keys: std::collections::BTreeSet<&String> = emitted
            .as_object()
            .expect("cron.list row must be a JSON object")
            .keys()
            .collect();

        // The expectation is DERIVED: serialize the contract type itself
        // rather than restating its field names here, so this guard cannot go
        // stale while claiming to be current.
        let contract = serde_json::to_value(
            serde_json::from_value::<CronJobRow>(emitted.clone())
                .expect("the emitted row must parse as CronJobRow"),
        )
        .expect("serialize contract row");
        let contract_keys: std::collections::BTreeSet<&String> = contract
            .as_object()
            .expect("contract row is an object")
            .keys()
            .collect();

        assert_eq!(
            emitted_keys, contract_keys,
            "cron.list row drifted from aleph_protocol::cron::CronJobRow"
        );
    }

    /// `parked` must reach the wire.
    ///
    /// The field has been computed by `CronJobView` since it was added —
    /// specifically so a surface would stop rendering a permanently-failed job
    /// as healthy — and it was emitted by nothing, so no RPC client could ask
    /// the question it exists to answer.
    #[test]
    fn a_parked_job_says_so_on_the_wire() {
        let mut job = CronJob::new(
            "dead job",
            "main",
            "x",
            ScheduleKind::Cron {
                expr: "0 0 8 * * *".to_string(),
                tz: None,
                stagger_ms: None,
            },
        );
        // Enabled, but nothing scheduled: it will never fire again.
        job.enabled = true;
        job.state.next_run_at_ms = None;

        let emitted = job_view_to_json(&CronJobView::from(&job));
        assert_eq!(
            emitted["parked"],
            serde_json::json!(true),
            "an enabled job with no next run is parked, and a client that only \
             sees `enabled` reports it as healthy"
        );
        assert_eq!(emitted["enabled"], serde_json::json!(true));
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
