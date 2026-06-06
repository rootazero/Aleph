//! Cron Job Management View
//!
//! Provides UI for managing scheduled tasks:
//! - List all cron jobs with status indicators
//! - Create/Edit/Delete jobs with form editor
//! - View execution history for each job
//! - Trigger immediate runs

use crate::api::agents::{AgentsApi, AgentSummary};
use crate::api::cron::{CreateCronJob, CronApi, CronJobInfo, JobRunInfo, UpdateCronJob};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// Helper Functions
// ============================================================================

/// Format a schedule into a human-readable summary.
/// Prefers the structured `schedule_kind` JSON object when available,
/// falling back to legacy `kind` + `schedule` string fields.
fn format_schedule_summary(
    schedule_kind_obj: &Option<serde_json::Value>,
    kind: &str,
    schedule: &str,
) -> String {
    // Try structured schedule_kind JSON first
    if let Some(obj) = schedule_kind_obj {
        if let Some(k) = obj.get("kind").and_then(|v| v.as_str()) {
            match k {
                "every" => {
                    if let Some(ms) = obj.get("every_ms").and_then(|v| v.as_u64()) {
                        return format_ms_interval(ms);
                    }
                }
                "cron" => {
                    // Backend serializes as "expr", panel historically used "expression"
                    if let Some(expr) = obj
                        .get("expr")
                        .or_else(|| obj.get("expression"))
                        .and_then(|v| v.as_str())
                    {
                        return expr.to_string();
                    }
                }
                "at" => {
                    if let Some(dt) = obj.get("datetime").and_then(|v| v.as_str()) {
                        return format!("At {}", dt);
                    }
                    // Backend stores ms; convert to seconds for display
                    if let Some(ts_ms) = obj
                        .get("at")
                        .or_else(|| obj.get("at_ms"))
                        .and_then(|v| v.as_i64())
                    {
                        return format!("At {}", format_timestamp(ts_ms / 1000));
                    }
                }
                _ => {}
            }
        }
    }

    // Fallback to legacy string fields
    match kind {
        "every" => {
            let trimmed = schedule.trim();
            if let Some(rest) = trimmed.strip_suffix('m') {
                format!("Every {}min", rest)
            } else if let Some(rest) = trimmed.strip_suffix('h') {
                format!("Every {}h", rest)
            } else if let Some(rest) = trimmed.strip_suffix('s') {
                format!("Every {}s", rest)
            } else {
                format!("Every {}", trimmed)
            }
        }
        "at" => format!("At {}", schedule),
        _ => schedule.to_string(),
    }
}

/// Format milliseconds into a human-readable interval string.
fn format_ms_interval(ms: u64) -> String {
    if ms < 60_000 {
        format!("Every {}s", ms / 1000)
    } else if ms < 3_600_000 {
        format!("Every {}min", ms / 60_000)
    } else if ms < 86_400_000 {
        format!("Every {}h", ms / 3_600_000)
    } else {
        format!("Every {}d", ms / 86_400_000)
    }
}

/// Format a UNIX timestamp (seconds) as a relative time string.
/// e.g. "5min", "2h", "3d", or the provided overdue_label.
fn format_relative_time(ts: i64, overdue_label: &str) -> String {
    let now_ms = js_sys::Date::now();
    let now_s = (now_ms / 1000.0) as i64;
    let diff = ts - now_s;

    if diff < 0 {
        return overdue_label.to_string();
    }

    let minutes = diff / 60;
    let hours = diff / 3600;
    let days = diff / 86400;

    if minutes < 1 {
        format!("{}s", diff)
    } else if hours < 1 {
        format!("{}min", minutes)
    } else if days < 1 {
        format!("{}h", hours)
    } else {
        format!("{}d", days)
    }
}

/// Format a UNIX timestamp (seconds) as "MM/DD HH:MM".
fn format_timestamp(ts: i64) -> String {
    let date = js_sys::Date::new_0();
    date.set_time((ts * 1000) as f64);

    let month = date.get_month() + 1; // 0-indexed
    let day = date.get_date();
    let hours = date.get_hours();
    let minutes = date.get_minutes();

    format!("{:02}/{:02} {:02}:{:02}", month, day, hours, minutes)
}

/// Format a duration in milliseconds to a human-readable string.
/// e.g. "200ms", "1.5s", "2.1min".
fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.1}min", ms as f64 / 60_000.0)
    }
}

/// Parse a string into an optional i64, returning None if empty or invalid.
fn parse_optional_i64(s: &str) -> Option<i64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        trimmed.parse().ok()
    }
}

/// Extract schedule type and value from the `schedule_kind` JSON object returned by backend.
///
/// Backend returns tagged enums like:
///   `{"kind":"cron","expr":"0 0 11 * * *","tz":null,"stagger_ms":null}`
///   `{"kind":"every","every_ms":3600000,"anchor_ms":null}`
///   `{"kind":"at","at":1711944000000,"delete_after_run":true}`
///
/// Returns (kind_str, schedule_str, anchor_ms_str, stagger_ms_str).
fn extract_schedule_from_kind(
    schedule_kind: &Option<serde_json::Value>,
) -> (String, String, Option<String>, Option<String>) {
    let Some(obj) = schedule_kind else {
        return ("cron".to_string(), String::new(), None, None);
    };
    let kind = obj.get("kind").and_then(|v| v.as_str()).unwrap_or("cron");
    match kind {
        "cron" => {
            let expr = obj.get("expr").and_then(|v| v.as_str()).unwrap_or("");
            let stagger = obj
                .get("stagger_ms")
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string());
            (kind.to_string(), expr.to_string(), None, stagger)
        }
        "every" => {
            let every_ms = obj.get("every_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let anchor = obj
                .get("anchor_ms")
                .and_then(|v| v.as_i64())
                .map(|v| v.to_string());
            (kind.to_string(), every_ms.to_string(), anchor, None)
        }
        "at" => {
            // Backend stores ms; convert to local datetime string for the form
            let at_ms = obj.get("at").and_then(|v| v.as_i64()).unwrap_or(0);
            let dt_str = ms_to_datetime_local(at_ms);
            (kind.to_string(), dt_str, None, None)
        }
        _ => ("cron".to_string(), String::new(), None, None),
    }
}

/// Build a `schedule_kind` JSON object from form fields.
/// Returns the tagged JSON like `{"kind":"every","every_ms":60000}`.
fn build_schedule_kind_json(
    kind: &str,
    schedule: &str,
    anchor_ms_str: &str,
    stagger_ms_str: &str,
) -> Option<serde_json::Value> {
    match kind {
        "every" => {
            // Try to parse schedule as milliseconds directly, or as interval string
            let every_ms = parse_interval_to_ms(schedule)?;
            let mut obj = serde_json::json!({
                "kind": "every",
                "every_ms": every_ms,
            });
            if let Some(anchor) = parse_optional_i64(anchor_ms_str) {
                obj["anchor_ms"] = serde_json::json!(anchor);
            }
            Some(obj)
        }
        "cron" => {
            let mut obj = serde_json::json!({
                "kind": "cron",
                "expr": schedule,
            });
            if let Some(stagger) = parse_optional_i64(stagger_ms_str) {
                obj["stagger_ms"] = serde_json::json!(stagger);
            }
            Some(obj)
        }
        "at" => {
            // User enters local datetime (YYYY-MM-DDTHH:MM); convert to epoch ms
            let at_ms = datetime_local_to_ms(schedule.trim())?;
            Some(serde_json::json!({
                "kind": "at",
                "at": at_ms,
                "delete_after_run": true,
            }))
        }
        _ => None,
    }
}

/// Convert epoch milliseconds to a `YYYY-MM-DDTHH:MM` string in local timezone.
/// This format is used by `<input type="datetime-local">`.
fn ms_to_datetime_local(ms: i64) -> String {
    let date = js_sys::Date::new_0();
    date.set_time(ms as f64);
    let y = date.get_full_year();
    let m = date.get_month() + 1;
    let d = date.get_date();
    let hh = date.get_hours();
    let mm = date.get_minutes();
    format!("{:04}-{:02}-{:02}T{:02}:{:02}", y, m, d, hh, mm)
}

/// Convert a `YYYY-MM-DDTHH:MM` local datetime string to epoch milliseconds.
fn datetime_local_to_ms(s: &str) -> Option<i64> {
    // <input type="datetime-local"> produces "YYYY-MM-DDTHH:MM"
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(s));
    let ms = date.get_time();
    if ms.is_nan() {
        None
    } else {
        Some(ms as i64)
    }
}

/// Parse a human interval string (e.g. "5m", "2h", "30s") to milliseconds.
fn parse_interval_to_ms(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Try direct number (assume ms)
    if let Ok(ms) = trimmed.parse::<u64>() {
        return Some(ms);
    }
    if let Some(rest) = trimmed.strip_suffix('s') {
        rest.parse::<u64>().ok().map(|v| v * 1000)
    } else if let Some(rest) = trimmed.strip_suffix('m') {
        rest.parse::<u64>().ok().map(|v| v * 60_000)
    } else if let Some(rest) = trimmed.strip_suffix('h') {
        rest.parse::<u64>().ok().map(|v| v * 3_600_000)
    } else if let Some(rest) = trimmed.strip_suffix('d') {
        rest.parse::<u64>().ok().map(|v| v * 86_400_000)
    } else {
        None
    }
}

/// Returns the id to render as a "(deleted)" placeholder option when the job's
/// currently-bound agent is no longer in the available list. `None` when the
/// current id is empty or still present.
fn stale_agent_option(current: &str, available: &[crate::api::agents::AgentSummary]) -> Option<String> {
    if current.is_empty() || available.iter().any(|a| a.id == current) {
        None
    } else {
        Some(current.to_string())
    }
}

// ============================================================================
// CronView — Main Container
// ============================================================================

#[component]
pub fn CronView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State
    let jobs = RwSignal::new(Vec::<CronJobInfo>::new());
    let selected = RwSignal::new(Option::<usize>::None);
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Closure that re-loads the job list. Reused by initial mount, by the
    // `cron.job.changed` push handler (drops the old 5s polling pattern), and
    // by manual refresh actions further down.
    let refresh_jobs = move || {
        spawn_local(async move {
            match CronApi::list(&state).await {
                Ok(list) => {
                    jobs.set(list);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load jobs: {}", e)));
                    loading.set(false);
                }
            }
        });
    };

    // Initial load on mount.
    refresh_jobs();

    // Live push: subscribe to `cron.job.changed` and re-fetch whenever a
    // CRUD or scheduler-tick mutation lands. Server-side emit lives in
    // `CronService` (see [[tasks-openclaw-parity]] spec D2).
    Effect::new(move |_| {
        let dash = state;
        spawn_local(async move {
            let _ = dash.subscribe_topic("cron.job.changed").await;
        });
    });
    let sub_id = state.subscribe_events(move |evt| {
        if evt.topic == "cron.job.changed" {
            refresh_jobs();
        }
    });
    on_cleanup(move || state.unsubscribe_events(sub_id));

    view! {
        <div class="flex flex-col h-full aleph-content-top">
            // Header
            <div class="p-6 border-b border-border">
                <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, cron.title)}</h1>
                <p class="mt-1 text-sm text-text-secondary">
                    {t!(i18n, cron.description)}
                </p>
            </div>

            // Content
            <div class="flex-1 flex overflow-hidden">
                <JobList jobs=jobs selected=selected loading=loading />
                <JobEditor jobs=jobs selected=selected saving=saving error=error />
            </div>
        </div>
    }
}

// ============================================================================
// JobList — Left Pane
// ============================================================================

/// A preset cron-job template for the Quick Create drawer. Each entry maps
/// to a [`CreateCronJob`] payload via `to_create_cron_job`. The drawer
/// stays panel-local — there is no backend list of templates, so adding /
/// removing presets here does not need a server change.
#[derive(Clone)]
struct QuickCreatePreset {
    label: &'static str,
    description: &'static str,
    schedule_kind: serde_json::Value,
    sample_prompt: &'static str,
}

fn cron_quick_create_presets() -> Vec<QuickCreatePreset> {
    vec![
        QuickCreatePreset {
            label: "Daily 9 AM digest",
            description: "Runs every day at 09:00 local time",
            // Six-field cron: sec min hour dom mon dow
            schedule_kind: serde_json::json!({"kind": "cron", "expr": "0 0 9 * * *"}),
            sample_prompt: "Summarize my emails, calendar, and tasks for today.",
        },
        QuickCreatePreset {
            label: "Hourly health check",
            description: "Pings on the hour and reports anomalies",
            schedule_kind: serde_json::json!({"kind": "every", "every_ms": 3_600_000_i64}),
            sample_prompt: "Check system health and notify on anything unusual.",
        },
        QuickCreatePreset {
            label: "Weekly Monday review",
            description: "Mondays at 10:00",
            schedule_kind: serde_json::json!({"kind": "cron", "expr": "0 0 10 * * MON"}),
            sample_prompt:
                "Prepare a weekly review of last week's accomplishments and this week's plan.",
        },
        QuickCreatePreset {
            label: "Every 5 minutes monitor",
            description: "Tight-loop monitoring (use sparingly)",
            schedule_kind: serde_json::json!({"kind": "every", "every_ms": 300_000_i64}),
            sample_prompt: "Run a lightweight check and alert me only on changes.",
        },
    ]
}

#[component]
fn JobList(
    jobs: RwSignal<Vec<CronJobInfo>>,
    selected: RwSignal<Option<usize>>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let quick_open = RwSignal::new(false);
    let presets = cron_quick_create_presets();

    view! {
        <div class="w-80 border-r border-border flex flex-col">
            // Add button + Quick Create toggle
            <div class="p-4 border-b border-border space-y-2">
                <button
                    on:click=move |_| selected.set(Some(usize::MAX))
                    class="w-full px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors"
                >
                    {t!(i18n, cron.new_job_btn)}
                </button>
                <button
                    on:click=move |_| quick_open.update(|v| *v = !*v)
                    class="w-full px-4 py-2 bg-surface-sunken hover:bg-surface-raised text-text-primary border border-border rounded-lg transition-colors text-sm"
                >
                    {move || if quick_open.get() { "Hide Quick Create ▲" } else { "Quick Create ▼" }}
                </button>
                {move || {
                    if !quick_open.get() {
                        view! { <div></div> }.into_any()
                    } else {
                        let cards = presets.clone();
                        view! {
                            <div class="space-y-1 pt-2 border-t border-border">
                                {cards.into_iter().map(|preset| {
                                    let label = preset.label;
                                    let desc = preset.description;
                                    let schedule_kind = preset.schedule_kind.clone();
                                    let prompt = preset.sample_prompt;
                                    view! {
                                        <button
                                            on:click=move |_| {
                                                let payload = CreateCronJob {
                                                    name: label.to_string(),
                                                    schedule: String::new(),
                                                    schedule_kind: Some(schedule_kind.clone()),
                                                    agent_id: "main".to_string(),
                                                    prompt: prompt.to_string(),
                                                    enabled: true,
                                                    timezone: None,
                                                    tags: vec!["quick-create".to_string()],
                                                    session_target: Some("isolated".to_string()),
                                                    failure_alert: None,
                                                };
                                                spawn_local(async move {
                                                    let _ = CronApi::create(&state, payload).await;
                                                });
                                                quick_open.set(false);
                                            }
                                            class="w-full p-2 text-left bg-surface-sunken hover:bg-surface-raised border border-border rounded-lg transition-colors"
                                        >
                                            <div class="text-xs font-medium text-text-primary">{label}</div>
                                            <div class="text-xs text-text-tertiary">{desc}</div>
                                        </button>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            // Jobs list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if loading.get() {
                        view! {
                            <div class="p-4 text-center text-text-secondary">
                                {t!(i18n, cron.loading)}
                            </div>
                        }.into_any()
                    } else if jobs.get().is_empty() {
                        view! {
                            <div class="flex flex-col items-center justify-center p-8 text-text-tertiary">
                                // Clock SVG icon
                                <svg class="w-12 h-12 mb-3 opacity-40" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <circle cx="12" cy="12" r="10" stroke-width="1.5"/>
                                    <path d="M12 6v6l4 2" stroke-width="1.5" stroke-linecap="round"/>
                                </svg>
                                <span class="text-sm">{t!(i18n, cron.no_tasks)}</span>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-2 space-y-2">
                                {move || {
                                    jobs.get().iter().enumerate().map(|(idx, job)| {
                                        let job = job.clone();
                                        let is_selected = Signal::derive(move || selected.get() == Some(idx));
                                        view! {
                                            <JobListItem job=job index=idx is_selected=is_selected selected=selected />
                                        }
                                    }).collect::<Vec<_>>()
                                }}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// JobListItem — Individual Card in List
// ============================================================================

#[component]
fn JobListItem(
    job: CronJobInfo,
    index: usize,
    is_selected: Signal<bool>,
    selected: RwSignal<Option<usize>>,
) -> impl IntoView {
    let i18n = use_i18n();

    let name = job.name.clone();
    let enabled = job.enabled;
    let schedule_kind_str = job.schedule_kind_str.clone();
    let schedule_kind_obj = job.schedule_kind.clone();
    let schedule = job.schedule.clone();
    let next_run_at = job.next_run_at;
    let is_running = job.running_at_ms.is_some();
    let has_errors = job.consecutive_errors.unwrap_or(0) > 0;

    let summary = format_schedule_summary(&schedule_kind_obj, &schedule_kind_str, &schedule);

    view! {
        <button
            on:click=move |_| selected.set(Some(index))
            class=move || {
                if is_selected.get() {
                    "w-full p-3 bg-primary-subtle border border-primary rounded-lg text-left transition-colors"
                } else {
                    "w-full p-3 bg-surface-sunken border border-border hover:border-border-strong rounded-lg text-left transition-colors"
                }
            }
        >
            <div class="flex items-center gap-2 mb-1">
                // Status dot: blue pulsing = running, green = enabled, gray = disabled
                <span class=move || {
                    if is_running {
                        "w-2 h-2 rounded-full bg-blue-500 animate-pulse flex-shrink-0"
                    } else if enabled {
                        "w-2 h-2 rounded-full bg-success flex-shrink-0"
                    } else {
                        "w-2 h-2 rounded-full bg-text-tertiary flex-shrink-0"
                    }
                }></span>
                <span class="text-sm font-medium text-text-primary truncate">
                    {name}
                </span>
                // Error badge
                {if has_errors {
                    view! {
                        <span class="ml-auto w-2 h-2 rounded-full bg-danger flex-shrink-0"
                              title="Has consecutive errors"></span>
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
            </div>
            <div class="ml-4 text-xs text-text-secondary">
                {summary}
            </div>
            {move || {
                if is_running {
                    view! {
                        <div class="ml-4 mt-1 text-xs text-blue-500">
                            {t!(i18n, cron.running)}
                        </div>
                    }.into_any()
                } else if let Some(ts) = next_run_at {
                    let overdue_label = t_string!(i18n, cron.overdue).to_string();
                    let relative = format_relative_time(ts, &overdue_label);
                    let next_prefix = t_string!(i18n, cron.next).to_string();
                    view! {
                        <div class="ml-4 mt-1 text-xs text-text-tertiary">
                            {format!("{}{}", next_prefix, relative)}
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
        </button>
    }
}

// ============================================================================
// JobEditor — Right Pane (form + actions + history)
// ============================================================================

#[component]
fn JobEditor(
    jobs: RwSignal<Vec<CronJobInfo>>,
    selected: RwSignal<Option<usize>>,
    saving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Form state
    let form_name = RwSignal::new(String::new());
    let form_schedule_kind = RwSignal::new(String::from("cron"));
    let form_schedule = RwSignal::new(String::new());
    let form_agent_id = RwSignal::new(String::new());
    // Available agents for the selector + default id.
    let agents = RwSignal::new(Vec::<AgentSummary>::new());
    let default_agent_id = RwSignal::new(String::from("main"));
    let form_prompt = RwSignal::new(String::new());
    let form_timezone = RwSignal::new(String::new());
    let form_tags = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);
    let form_anchor_ms = RwSignal::new(String::new());
    let form_stagger_ms = RwSignal::new(String::new());
    let form_session_target = RwSignal::new(String::new());
    // Failure alert sub-fields
    let form_alert_after = RwSignal::new(String::from("2"));
    let form_alert_cooldown = RwSignal::new(String::from("1h"));
    let form_alert_kind = RwSignal::new(String::from("announce"));
    let form_alert_channel = RwSignal::new(String::new());
    let form_alert_expanded = RwSignal::new(false);

    let runs = RwSignal::new(Vec::<JobRunInfo>::new());
    let run_success = RwSignal::new(Option::<String>::None);

    let is_new = move || selected.get() == Some(usize::MAX);
    let is_editing = move || selected.get().is_some();

    // Load available agents once connected, for the agent selector.
    {
        let dash = state;
        Effect::new(move || {
            if !dash.is_connected.get() {
                return;
            }
            spawn_local(async move {
                if let Ok(resp) = AgentsApi::list(&dash).await {
                    default_agent_id.set(resp.default_id.clone());
                    agents.set(resp.agents);
                }
            });
        });
    }

    // Populate form when selection changes
    Effect::new(move || {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                // Reset form for new job
                form_name.set(String::new());
                form_schedule_kind.set(String::from("cron"));
                form_schedule.set(String::new());
                form_agent_id.set(default_agent_id.get_untracked());
                form_prompt.set(String::new());
                form_timezone.set(String::new());
                form_tags.set(String::new());
                form_enabled.set(true);
                form_anchor_ms.set(String::new());
                form_stagger_ms.set(String::new());
                form_session_target.set(String::new());
                form_alert_after.set("2".to_string());
                form_alert_cooldown.set("1h".to_string());
                form_alert_kind.set("announce".to_string());
                form_alert_channel.set(String::new());
                form_alert_expanded.set(false);
                runs.set(Vec::new());
            } else {
                // Load existing job data
                if let Some(job) = jobs.get().get(idx) {
                    form_name.set(job.name.clone());

                    // Extract schedule type and value from schedule_kind JSON object
                    // Backend returns: {"kind":"cron","expr":"..."} or {"kind":"every","every_ms":...} etc.
                    let (sk_type, sk_val, sk_anchor, sk_stagger) =
                        extract_schedule_from_kind(&job.schedule_kind);
                    form_schedule_kind.set(sk_type);
                    form_schedule.set(sk_val);

                    form_agent_id.set(job.agent_id.clone());
                    form_prompt.set(job.prompt.clone());
                    form_timezone.set(job.timezone.clone().unwrap_or_default());
                    form_tags.set(job.tags.join(", "));
                    form_enabled.set(job.enabled);
                    form_anchor_ms.set(
                        sk_anchor
                            .or(job.anchor_ms.map(|v| v.to_string()))
                            .unwrap_or_default(),
                    );
                    form_stagger_ms.set(
                        sk_stagger
                            .or(job.stagger_ms.map(|v| v.to_string()))
                            .unwrap_or_default(),
                    );
                    form_session_target.set(job.session_target.clone().unwrap_or_default());

                    // Populate failure alert from JSON
                    if let Some(ref alert) = job.failure_alert {
                        form_alert_after.set(
                            alert
                                .get("after_n")
                                .and_then(|v| v.as_u64())
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "2".to_string()),
                        );
                        form_alert_cooldown.set(
                            alert
                                .get("cooldown")
                                .and_then(|v| v.as_str())
                                .unwrap_or("1h")
                                .to_string(),
                        );
                        form_alert_kind.set(
                            alert
                                .get("kind")
                                .and_then(|v| v.as_str())
                                .unwrap_or("announce")
                                .to_string(),
                        );
                        form_alert_channel.set(
                            alert
                                .get("channel")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        );
                        form_alert_expanded.set(true);
                    } else {
                        form_alert_after.set("2".to_string());
                        form_alert_cooldown.set("1h".to_string());
                        form_alert_kind.set("announce".to_string());
                        form_alert_channel.set(String::new());
                        form_alert_expanded.set(false);
                    }

                    // Load run history
                    let job_id = job.id.clone();
                    spawn_local(async move {
                        match CronApi::runs(&state, &job_id, 20).await {
                            Ok(list) => runs.set(list),
                            Err(_) => runs.set(Vec::new()),
                        }
                    });
                }
            }
        }
    });

    // Handle save
    let on_save = move |_| {
        let name = form_name.get();
        if name.trim().is_empty() {
            error.set(Some(t_string!(i18n, cron.error_name_required).to_string()));
            return;
        }

        saving.set(true);
        error.set(None);

        let schedule_kind = form_schedule_kind.get();
        let schedule = form_schedule.get();
        let agent_id = form_agent_id.get();
        let prompt = form_prompt.get();
        let timezone = {
            let tz = form_timezone.get();
            if tz.trim().is_empty() {
                None
            } else {
                Some(tz)
            }
        };
        let tags: Vec<String> = form_tags
            .get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let enabled = form_enabled.get();

        // Build schedule_kind JSON object from form fields
        let schedule_kind_obj = build_schedule_kind_json(
            &schedule_kind,
            &schedule,
            &form_anchor_ms.get(),
            &form_stagger_ms.get(),
        );

        if schedule_kind_obj.is_none() {
            let hint = match schedule_kind.as_str() {
                "every" => "e.g. 5m, 2h, 30s, or 60000 (ms)",
                "at" => "a valid date and time",
                _ => "cron expression, e.g. 0 0 9 * * *",
            };
            error.set(Some(format!(
                "Invalid schedule value for type '{}'. Expected: {}",
                schedule_kind, hint
            )));
            saving.set(false);
            return;
        }

        let session_target = {
            let s = form_session_target.get();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        };

        // Build failure_alert JSON if channel is specified
        let failure_alert = {
            let ch = form_alert_channel.get();
            if ch.trim().is_empty() {
                None
            } else {
                Some(serde_json::json!({
                    "after_n": form_alert_after.get().parse::<u32>().unwrap_or(2),
                    "cooldown": form_alert_cooldown.get(),
                    "kind": form_alert_kind.get(),
                    "channel": ch,
                }))
            }
        };

        if is_new() {
            let create = CreateCronJob {
                name,
                schedule,
                schedule_kind: schedule_kind_obj,
                agent_id,
                prompt,
                enabled,
                timezone,
                tags,
                session_target,
                failure_alert,
            };

            spawn_local(async move {
                match CronApi::create(&state, create).await {
                    Ok(_) => {
                        error.set(None);
                        if let Ok(list) = CronApi::list(&state).await {
                            jobs.set(list);
                        }
                        selected.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to create job: {}", e)));
                    }
                }
                saving.set(false);
            });
        } else if let Some(idx) = selected.get() {
            if let Some(job) = jobs.get().get(idx).cloned() {
                let patch = UpdateCronJob {
                    job_id: job.id.clone(),
                    name: Some(name),
                    schedule_kind: schedule_kind_obj,
                    agent_id: Some(agent_id),
                    prompt: Some(prompt),
                    enabled: Some(enabled),
                    timezone,
                    tags: Some(tags),
                    session_target,
                    failure_alert,
                };

                spawn_local(async move {
                    match CronApi::update(&state, patch).await {
                        Ok(_) => {
                            error.set(None);
                            if let Ok(list) = CronApi::list(&state).await {
                                jobs.set(list);
                            }
                            selected.set(None);
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to update job: {}", e)));
                        }
                    }
                    saving.set(false);
                });
            }
        }
    };

    // Two-step delete confirmation
    let confirm_delete = RwSignal::new(false);

    // Reset confirmation when selection changes
    Effect::new(move || {
        let _ = selected.get();
        confirm_delete.set(false);
    });

    let on_delete = move |_| {
        if !confirm_delete.get() {
            // First click: show confirmation
            confirm_delete.set(true);
            return;
        }

        // Second click: actually delete
        confirm_delete.set(false);
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                return;
            }

            if let Some(job) = jobs.get().get(idx).cloned() {
                saving.set(true);
                error.set(None);

                let job_id = job.id.clone();
                spawn_local(async move {
                    match CronApi::delete(&state, &job_id).await {
                        Ok(()) => {
                            error.set(None);
                            if let Ok(list) = CronApi::list(&state).await {
                                jobs.set(list);
                            }
                            selected.set(None);
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to delete job: {}", e)));
                        }
                    }
                    saving.set(false);
                });
            }
        }
    };

    // Handle run now
    let on_run_now = move |_| {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                return;
            }

            if let Some(job) = jobs.get().get(idx).cloned() {
                let job_id = job.id.clone();
                let state_for_spawn = state;
                let runs_for_spawn = runs;
                let error_for_spawn = error;
                let success_for_spawn = run_success;
                spawn_local(async move {
                    match CronApi::run_now(&state_for_spawn, &job_id).await {
                        Ok(_) => {
                            error_for_spawn.set(None);
                            success_for_spawn
                                .set(Some(t_string!(i18n, cron.run_triggered).to_string()));
                            // Reload runs after triggering
                            if let Ok(list) = CronApi::runs(&state_for_spawn, &job_id, 20).await {
                                runs_for_spawn.set(list);
                            }
                            gloo_timers::future::sleep(std::time::Duration::from_secs(3)).await;
                            success_for_spawn.set(None);
                        }
                        Err(e) => {
                            error_for_spawn.set(Some(format!("Failed to trigger run: {}", e)));
                        }
                    }
                });
            }
        }
    };

    // Dynamic placeholder for schedule input
    let schedule_placeholder = move || match form_schedule_kind.get().as_str() {
        "every" => "5m, 2h, 30s",
        "at" => "1711944000",
        _ => "*/5 * * * *",
    };

    view! {
        <div class="flex-1 overflow-y-auto">
            {move || {
                if !is_editing() {
                    // Empty state — no selection
                    view! {
                        <div class="flex flex-col items-center justify-center h-full text-text-tertiary">
                            <svg class="w-16 h-16 mb-4 opacity-30" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <circle cx="12" cy="12" r="10" stroke-width="1.5"/>
                                <path d="M12 6v6l4 2" stroke-width="1.5" stroke-linecap="round"/>
                            </svg>
                            <span class="text-sm">{t!(i18n, cron.select_or_create)}</span>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-8 max-w-3xl mx-auto">
                            // Header
                            <div class="mb-6">
                                <h2 class="text-2xl font-bold text-text-primary mb-2">
                                    {move || if is_new() { t_string!(i18n, cron.new_task_title) } else { t_string!(i18n, cron.edit_task_title) }}
                                </h2>
                                <p class="text-sm text-text-secondary">
                                    {t!(i18n, cron.form_description)}
                                </p>
                            </div>

                            // Error message
                            {move || {
                                if let Some(err) = error.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                                            {err}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            {move || {
                                if let Some(msg) = run_success.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">
                                            {msg}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            // Form
                            <div class="space-y-6">
                                // Name
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_name)}
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_name.get()
                                        on:input=move |ev| form_name.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder=move || t_string!(i18n, cron.placeholder_name).to_string()
                                    />
                                </div>

                                // Schedule Type + Schedule (grid 1/3 + 2/3)
                                <div class="grid grid-cols-3 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_schedule_type)}
                                        </label>
                                        <select
                                            prop:value=move || form_schedule_kind.get()
                                            on:change=move |ev| form_schedule_kind.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        >
                                            <option value="cron">{t!(i18n, cron.schedule_type_cron)}</option>
                                            <option value="every">{t!(i18n, cron.schedule_type_every)}</option>
                                            <option value="at">{t!(i18n, cron.schedule_type_at)}</option>
                                        </select>
                                    </div>
                                    <div class="col-span-2">
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_schedule)}
                                        </label>
                                        {move || {
                                            if form_schedule_kind.get() == "at" {
                                                view! {
                                                    <input
                                                        type="datetime-local"
                                                        prop:value=move || form_schedule.get()
                                                        on:input=move |ev| form_schedule.set(event_target_value(&ev))
                                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    />
                                                    <p class="mt-1.5 text-xs text-text-tertiary">
                                                        {t!(i18n, cron.at_hint)}
                                                    </p>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <input
                                                        type="text"
                                                        prop:value=move || form_schedule.get()
                                                        on:input=move |ev| form_schedule.set(event_target_value(&ev))
                                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-primary"
                                                        placeholder=schedule_placeholder
                                                    />
                                                    <p class="mt-1.5 text-xs text-text-tertiary">
                                                        {move || match form_schedule_kind.get().as_str() {
                                                            "cron" => t_string!(i18n, cron.cron_hint),
                                                            "every" => t_string!(i18n, cron.every_hint),
                                                            _ => "",
                                                        }}
                                                    </p>
                                                }.into_any()
                                            }
                                        }}
                                    </div>
                                </div>

                                // Anchor / Stagger (conditional on schedule type)
                                {move || {
                                    let kind = form_schedule_kind.get();
                                    match kind.as_str() {
                                        "every" => view! {
                                            <div>
                                                <label class="block text-sm font-medium text-text-secondary mb-2">
                                                    {t!(i18n, cron.field_anchor_ms)}
                                                </label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_anchor_ms.get()
                                                    on:input=move |ev| form_anchor_ms.set(event_target_value(&ev))
                                                    class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    placeholder=move || t_string!(i18n, cron.anchor_placeholder).to_string()
                                                />
                                            </div>
                                        }.into_any(),
                                        "cron" => view! {
                                            <div>
                                                <label class="block text-sm font-medium text-text-secondary mb-2">
                                                    {t!(i18n, cron.field_stagger_ms)}
                                                </label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_stagger_ms.get()
                                                    on:input=move |ev| form_stagger_ms.set(event_target_value(&ev))
                                                    class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    placeholder=move || t_string!(i18n, cron.stagger_placeholder).to_string()
                                                />
                                            </div>
                                        }.into_any(),
                                        _ => view! { <div></div> }.into_any(),
                                    }
                                }}

                                // Agent
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_agent)}
                                    </label>
                                    <select
                                        on:change=move |ev| form_agent_id.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                    >
                                        {move || {
                                            let current = form_agent_id.get();
                                            let list = agents.get();
                                            let deleted_suffix =
                                                t_string!(i18n, cron.agent_deleted_suffix).to_string();
                                            let stale = stale_agent_option(&current, &list);
                                            let mut opts = list
                                                .into_iter()
                                                .map(|a| {
                                                    let id = a.id.clone();
                                                    let label = a.name.clone().unwrap_or_else(|| a.id.clone());
                                                    let sel = id == current;
                                                    view! {
                                                        <option value=id selected=sel>{label}</option>
                                                    }
                                                    .into_any()
                                                })
                                                .collect::<Vec<_>>();
                                            if let Some(stale_id) = stale {
                                                let label = format!("{stale_id}{deleted_suffix}");
                                                opts.push(view! {
                                                    <option value=stale_id selected=true disabled=true>
                                                        {label}
                                                    </option>
                                                }.into_any());
                                            }
                                            opts
                                        }}
                                    </select>
                                </div>

                                // Prompt
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_prompt)}
                                    </label>
                                    <textarea
                                        prop:value=move || form_prompt.get()
                                        on:input=move |ev| form_prompt.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        rows="3"
                                        placeholder=move || t_string!(i18n, cron.placeholder_prompt).to_string()
                                    ></textarea>
                                </div>

                                // Timezone + Tags (grid 1/2 + 1/2)
                                <div class="grid grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_timezone)}
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || form_timezone.get()
                                            on:input=move |ev| form_timezone.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder=move || t_string!(i18n, cron.placeholder_timezone).to_string()
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            {t!(i18n, cron.field_tags)}
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || form_tags.get()
                                            on:input=move |ev| form_tags.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder=move || t_string!(i18n, cron.placeholder_tags).to_string()
                                        />
                                    </div>
                                </div>

                                // Enabled toggle
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_status)}
                                    </label>
                                    <button
                                        on:click=move |_| form_enabled.set(!form_enabled.get())
                                        class=move || {
                                            if form_enabled.get() {
                                                "px-4 py-2 bg-success/20 border border-success text-success rounded-lg transition-colors text-sm font-medium"
                                            } else {
                                                "px-4 py-2 bg-surface-sunken border border-border text-text-tertiary rounded-lg transition-colors text-sm font-medium"
                                            }
                                        }
                                    >
                                        {move || if form_enabled.get() { t_string!(i18n, cron.enabled) } else { t_string!(i18n, cron.disabled) }}
                                    </button>
                                </div>

                                // Session Target
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_session_target)}
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_session_target.get()
                                        on:input=move |ev| form_session_target.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder=move || t_string!(i18n, cron.placeholder_session_target).to_string()
                                    />
                                </div>

                                // Failure Alert (collapsible)
                                <div class="border border-border rounded-lg">
                                    <button
                                        on:click=move |_| form_alert_expanded.set(!form_alert_expanded.get())
                                        class="w-full px-4 py-3 flex items-center gap-2 text-sm font-medium text-text-secondary hover:text-text-primary transition-colors"
                                    >
                                        <span>{move || if form_alert_expanded.get() { "\u{25BC}" } else { "\u{25B6}" }}</span>
                                        {t!(i18n, cron.field_failure_alert)}
                                    </button>
                                    {move || {
                                        if form_alert_expanded.get() {
                                            view! {
                                                <div class="px-4 pb-4 space-y-4 border-t border-border pt-3">
                                                    <div class="grid grid-cols-2 gap-4">
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_after_n)}
                                                            </label>
                                                            <input
                                                                type="number"
                                                                prop:value=move || form_alert_after.get()
                                                                on:input=move |ev| form_alert_after.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                                placeholder="2"
                                                            />
                                                        </div>
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_cooldown)}
                                                            </label>
                                                            <input
                                                                type="text"
                                                                prop:value=move || form_alert_cooldown.get()
                                                                on:input=move |ev| form_alert_cooldown.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                                placeholder="1h"
                                                            />
                                                        </div>
                                                    </div>
                                                    <div class="grid grid-cols-2 gap-4">
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_to)}
                                                            </label>
                                                            <select
                                                                prop:value=move || form_alert_kind.get()
                                                                on:change=move |ev| form_alert_kind.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                            >
                                                                <option value="announce">{t!(i18n, cron.alert_announce)}</option>
                                                                <option value="webhook">{t!(i18n, cron.alert_webhook)}</option>
                                                            </select>
                                                        </div>
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                {t!(i18n, cron.alert_channel_url)}
                                                            </label>
                                                            <input
                                                                type="text"
                                                                prop:value=move || form_alert_channel.get()
                                                                on:input=move |ev| form_alert_channel.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                                placeholder=move || t_string!(i18n, cron.alert_channel_placeholder).to_string()
                                                            />
                                                        </div>
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! { <div></div> }.into_any()
                                        }
                                    }}
                                </div>
                            </div>

                            // Actions
                            <div class="mt-8 flex items-center gap-3">
                                <button
                                    on:click=on_save
                                    prop:disabled=move || saving.get()
                                    class="px-6 py-2 bg-primary hover:bg-primary-hover disabled:bg-primary/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                >
                                    {move || if saving.get() { t_string!(i18n, cron.btn_saving) } else { t_string!(i18n, cron.btn_save) }}
                                </button>

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_run_now
                                                prop:disabled=move || saving.get()
                                                class="px-6 py-2 bg-success/80 hover:bg-success disabled:bg-success/40 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                {t!(i18n, cron.btn_run_now)}
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_delete
                                                prop:disabled=move || saving.get()
                                                class=move || {
                                                    if confirm_delete.get() {
                                                        "px-6 py-2 bg-danger hover:bg-danger/80 disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed ring-2 ring-danger/50 ring-offset-1 ring-offset-surface animate-pulse"
                                                    } else {
                                                        "px-6 py-2 bg-danger/80 hover:bg-danger disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                                    }
                                                }
                                            >
                                                {move || if confirm_delete.get() { t_string!(i18n, cron.btn_confirm_delete) } else { t_string!(i18n, cron.btn_delete) }}
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                <button
                                    on:click=move |_| selected.set(None)
                                    class="px-6 py-2 bg-surface-sunken hover:bg-surface-sunken text-text-primary rounded-lg transition-colors"
                                >
                                    {t!(i18n, cron.btn_cancel)}
                                </button>
                            </div>

                            // Run History (existing jobs only)
                            {move || {
                                if !is_new() {
                                    view! {
                                        <div class="mt-10">
                                            <RunHistory runs=runs />
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

// ============================================================================
// RunHistory — Execution History Table
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::agents::AgentSummary;

    fn agent(id: &str) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: Some(id.to_string()),
            emoji: None,
            description: None,
            model: None,
            is_default: id == "main",
        }
    }

    #[test]
    fn stale_option_none_when_current_in_list() {
        let list = vec![agent("main"), agent("research")];
        assert_eq!(stale_agent_option("research", &list), None);
    }

    #[test]
    fn stale_option_none_when_current_empty() {
        let list = vec![agent("main")];
        assert_eq!(stale_agent_option("", &list), None);
    }

    #[test]
    fn stale_option_some_when_current_deleted() {
        let list = vec![agent("main")];
        assert_eq!(stale_agent_option("gone", &list), Some("gone".to_string()));
    }
}

#[component]
fn RunHistory(runs: RwSignal<Vec<JobRunInfo>>) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="border border-border rounded-lg">
            <div class="px-4 py-3 border-b border-border flex items-center justify-between gap-3">
                <h3 class="text-sm font-semibold text-text-primary">{t!(i18n, cron.history_title)}</h3>
                // Sparkline of the last 12 run outcomes (newest on the right).
                // Pure CSS divs — no chart library (R3 core-lightweight).
                {move || {
                    let run_list = runs.get();
                    if run_list.is_empty() {
                        view! { <span></span> }.into_any()
                    } else {
                        let take = 12usize;
                        // RunHistory is sorted newest-first by the backend;
                        // reverse a windowed copy so the newest sits on the right.
                        let mut window: Vec<_> = run_list.iter().take(take).cloned().collect();
                        window.reverse();
                        view! {
                            <div class="flex items-center gap-0.5"
                                 title="Recent runs — newest on right (green=ok, red=failed, yellow=timeout, gray=other)">
                                {window.into_iter().map(|run| {
                                    let cls = match run.status.as_str() {
                                        "success" => "w-1.5 h-4 rounded-sm bg-success",
                                        "failed"  => "w-1.5 h-4 rounded-sm bg-danger",
                                        "timeout" => "w-1.5 h-4 rounded-sm bg-warning",
                                        _         => "w-1.5 h-4 rounded-sm bg-text-tertiary opacity-40",
                                    };
                                    view! { <div class=cls></div> }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            {move || {
                let run_list = runs.get();
                if run_list.is_empty() {
                    view! {
                        <div class="p-6 text-center text-sm text-text-tertiary">
                            {t!(i18n, cron.history_no_records)}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b border-border text-text-secondary">
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_status)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_time)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_duration)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_delivery)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_error)}</th>
                                </tr>
                            </thead>
                            <tbody>
                                {run_list.into_iter().map(|run| {
                                    let (icon, color) = match run.status.as_str() {
                                        "success" => ("\u{2713}", "text-success"),
                                        "failed" => ("\u{2717}", "text-danger"),
                                        "timeout" => ("\u{23F1}", "text-warning"),
                                        "running" => ("\u{23F1}", "text-primary"),
                                        _ => ("?", "text-text-tertiary"),
                                    };
                                    let time_str = format_timestamp(run.started_at);
                                    let duration_str = format_duration(run.duration_ms);

                                    // Delivery status with icon
                                    let delivery_str = run.delivery_status.clone().unwrap_or_default();
                                    let delivery_icon = match delivery_str.as_str() {
                                        "delivered" => "\u{2713}",
                                        "deduped" => "\u{2261}",
                                        "failed" => "\u{2717}",
                                        _ => "",
                                    };

                                    // Combine error_reason prefix with error
                                    let error_str = match (&run.error_reason, &run.error) {
                                        (Some(reason), Some(err)) => {
                                            let reason_str = reason.get("message")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| reason.to_string());
                                            format!("[{}] {}", reason_str, err)
                                        }
                                        (Some(reason), None) => {
                                            reason.get("message")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| reason.to_string())
                                        }
                                        (None, Some(err)) => err.clone(),
                                        (None, None) => String::new(),
                                    };

                                    view! {
                                        <tr class="border-b border-border last:border-b-0 hover:bg-surface-sunken/50">
                                            <td class=format!("px-4 py-2 {}", color)>
                                                {icon}
                                            </td>
                                            <td class="px-4 py-2 text-text-primary">
                                                {time_str}
                                            </td>
                                            <td class="px-4 py-2 text-text-secondary">
                                                {duration_str}
                                            </td>
                                            <td class="px-4 py-2 text-text-secondary">
                                                {delivery_icon}" "{delivery_str}
                                            </td>
                                            <td class="px-4 py-2 text-text-tertiary truncate max-w-xs">
                                                {error_str}
                                            </td>
                                        </tr>
                                    }
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            }}
        </div>
    }
}
