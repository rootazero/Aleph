//! Cron Job Management View
//!
//! Provides UI for managing scheduled tasks:
//! - List all cron jobs with status indicators
//! - Create/Edit/Delete jobs with form editor
//! - View execution history for each job
//! - Trigger immediate runs

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::cron::{CronApi, CronJobInfo, CreateCronJob, UpdateCronJob, JobRunInfo};

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
                    if let Some(expr) = obj.get("expression").and_then(|v| v.as_str()) {
                        return expr.to_string();
                    }
                }
                "at" => {
                    if let Some(dt) = obj.get("datetime").and_then(|v| v.as_str()) {
                        return format!("At {}", dt);
                    }
                    if let Some(ts) = obj.get("at_ms").and_then(|v| v.as_i64()) {
                        return format!("At {}", format_timestamp(ts / 1000));
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
/// e.g. "5min", "2h", "3d", or "overdue".
fn format_relative_time(ts: i64) -> String {
    let now_ms = js_sys::Date::now();
    let now_s = (now_ms / 1000.0) as i64;
    let diff = ts - now_s;

    if diff < 0 {
        return "overdue".to_string();
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
                "expression": schedule,
            });
            if let Some(stagger) = parse_optional_i64(stagger_ms_str) {
                obj["stagger_ms"] = serde_json::json!(stagger);
            }
            Some(obj)
        }
        "at" => {
            Some(serde_json::json!({
                "kind": "at",
                "datetime": schedule,
            }))
        }
        _ => None,
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

// ============================================================================
// CronView — Main Container
// ============================================================================

#[component]
pub fn CronView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let jobs = RwSignal::new(Vec::<CronJobInfo>::new());
    let selected = RwSignal::new(Option::<usize>::None);
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load jobs on mount
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

    view! {
        <div class="flex flex-col h-full">
            // Header
            <div class="p-6 border-b border-border">
                <h1 class="text-2xl font-bold text-text-primary">"Scheduled Tasks"</h1>
                <p class="mt-1 text-sm text-text-secondary">
                    "Manage cron jobs and scheduled automation tasks"
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

#[component]
fn JobList(
    jobs: RwSignal<Vec<CronJobInfo>>,
    selected: RwSignal<Option<usize>>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <div class="w-80 border-r border-border flex flex-col">
            // Add button
            <div class="p-4 border-b border-border">
                <button
                    on:click=move |_| selected.set(Some(usize::MAX))
                    class="w-full px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors"
                >
                    "+ New Job"
                </button>
            </div>

            // Jobs list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if loading.get() {
                        view! {
                            <div class="p-4 text-center text-text-secondary">
                                "Loading..."
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
                                <span class="text-sm">"No scheduled tasks yet"</span>
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
                            "running..."
                        </div>
                    }.into_any()
                } else if let Some(ts) = next_run_at {
                    let relative = format_relative_time(ts);
                    view! {
                        <div class="ml-4 mt-1 text-xs text-text-tertiary">
                            "Next: "{relative}
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

    // Form state
    let form_name = RwSignal::new(String::new());
    let form_schedule_kind = RwSignal::new(String::from("cron"));
    let form_schedule = RwSignal::new(String::new());
    let form_agent_id = RwSignal::new(String::new());
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

    // Run history for existing jobs
    let runs = RwSignal::new(Vec::<JobRunInfo>::new());

    let is_new = move || selected.get() == Some(usize::MAX);
    let is_editing = move || selected.get().is_some();

    // Populate form when selection changes
    Effect::new(move || {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                // Reset form for new job
                form_name.set(String::new());
                form_schedule_kind.set(String::from("cron"));
                form_schedule.set(String::new());
                form_agent_id.set(String::new());
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
                    form_schedule_kind.set(job.schedule_kind_str.clone());
                    form_schedule.set(job.schedule.clone());
                    form_agent_id.set(job.agent_id.clone());
                    form_prompt.set(job.prompt.clone());
                    form_timezone.set(job.timezone.clone().unwrap_or_default());
                    form_tags.set(job.tags.join(", "));
                    form_enabled.set(job.enabled);
                    form_anchor_ms.set(job.anchor_ms.map(|v| v.to_string()).unwrap_or_default());
                    form_stagger_ms.set(job.stagger_ms.map(|v| v.to_string()).unwrap_or_default());
                    form_session_target.set(job.session_target.clone().unwrap_or_default());

                    // Populate failure alert from JSON
                    if let Some(ref alert) = job.failure_alert {
                        form_alert_after.set(
                            alert.get("after_n").and_then(|v| v.as_u64())
                                .map(|v| v.to_string()).unwrap_or_else(|| "2".to_string())
                        );
                        form_alert_cooldown.set(
                            alert.get("cooldown").and_then(|v| v.as_str())
                                .unwrap_or("1h").to_string()
                        );
                        form_alert_kind.set(
                            alert.get("kind").and_then(|v| v.as_str())
                                .unwrap_or("announce").to_string()
                        );
                        form_alert_channel.set(
                            alert.get("channel").and_then(|v| v.as_str())
                                .unwrap_or("").to_string()
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
            error.set(Some("Job name is required".to_string()));
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
            if tz.trim().is_empty() { None } else { Some(tz) }
        };
        let tags: Vec<String> = form_tags.get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let enabled = form_enabled.get();

        // Build schedule_kind JSON object from form fields
        let schedule_kind_obj = build_schedule_kind_json(
            &schedule_kind, &schedule,
            &form_anchor_ms.get(), &form_stagger_ms.get(),
        );

        let anchor_ms = parse_optional_i64(&form_anchor_ms.get());
        let stagger_ms = parse_optional_i64(&form_stagger_ms.get());
        let session_target = {
            let s = form_session_target.get();
            if s.trim().is_empty() { None } else { Some(s) }
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
                schedule_kind,
                agent_id,
                prompt,
                enabled,
                timezone,
                tags,
                schedule_kind_obj,
                anchor_ms,
                stagger_ms,
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
                    schedule: Some(schedule),
                    schedule_kind: Some(schedule_kind),
                    agent_id: Some(agent_id),
                    prompt: Some(prompt),
                    enabled: Some(enabled),
                    timezone,
                    tags: Some(tags),
                    schedule_kind_obj,
                    anchor_ms,
                    stagger_ms,
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

    // Handle delete
    let on_delete = move |_| {
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
                spawn_local(async move {
                    match CronApi::run_now(&state, &job_id).await {
                        Ok(_) => {
                            // Reload runs after triggering
                            if let Ok(list) = CronApi::runs(&state, &job_id, 20).await {
                                runs.set(list);
                            }
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to trigger run: {}", e)));
                        }
                    }
                });
            }
        }
    };

    // Dynamic placeholder for schedule input
    let schedule_placeholder = move || {
        match form_schedule_kind.get().as_str() {
            "every" => "5m, 2h, 30s",
            "at" => "09:00, 14:30",
            _ => "*/5 * * * *",
        }
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
                            <span class="text-sm">"Select a job to edit or create a new one"</span>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-8 max-w-3xl mx-auto">
                            // Header
                            <div class="mb-6">
                                <h2 class="text-2xl font-bold text-text-primary mb-2">
                                    {move || if is_new() { "New Scheduled Task" } else { "Edit Scheduled Task" }}
                                </h2>
                                <p class="text-sm text-text-secondary">
                                    "Configure the job schedule and execution parameters"
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

                            // Form
                            <div class="space-y-6">
                                // Name
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Name"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_name.get()
                                        on:input=move |ev| form_name.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="Daily report generation"
                                    />
                                </div>

                                // Schedule Type + Schedule (grid 1/3 + 2/3)
                                <div class="grid grid-cols-3 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            "Schedule Type"
                                        </label>
                                        <select
                                            prop:value=move || form_schedule_kind.get()
                                            on:change=move |ev| form_schedule_kind.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        >
                                            <option value="cron">"Cron"</option>
                                            <option value="every">"Every"</option>
                                            <option value="at">"At"</option>
                                        </select>
                                    </div>
                                    <div class="col-span-2">
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            "Schedule"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || form_schedule.get()
                                            on:input=move |ev| form_schedule.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-primary"
                                            placeholder=schedule_placeholder
                                        />
                                    </div>
                                </div>

                                // Anchor / Stagger (conditional on schedule type)
                                {move || {
                                    let kind = form_schedule_kind.get();
                                    match kind.as_str() {
                                        "every" => view! {
                                            <div>
                                                <label class="block text-sm font-medium text-text-secondary mb-2">
                                                    "Anchor (ms)"
                                                </label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_anchor_ms.get()
                                                    on:input=move |ev| form_anchor_ms.set(event_target_value(&ev))
                                                    class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    placeholder="Leave empty to use creation time"
                                                />
                                            </div>
                                        }.into_any(),
                                        "cron" => view! {
                                            <div>
                                                <label class="block text-sm font-medium text-text-secondary mb-2">
                                                    "Stagger (ms)"
                                                </label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_stagger_ms.get()
                                                    on:input=move |ev| form_stagger_ms.set(event_target_value(&ev))
                                                    class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                                    placeholder="Spread window, e.g. 5000"
                                                />
                                            </div>
                                        }.into_any(),
                                        _ => view! { <div></div> }.into_any(),
                                    }
                                }}

                                // Agent
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Agent"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_agent_id.get()
                                        on:input=move |ev| form_agent_id.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="default"
                                    />
                                </div>

                                // Prompt
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Prompt"
                                    </label>
                                    <textarea
                                        prop:value=move || form_prompt.get()
                                        on:input=move |ev| form_prompt.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        rows="3"
                                        placeholder="Generate a daily summary of..."
                                    ></textarea>
                                </div>

                                // Timezone + Tags (grid 1/2 + 1/2)
                                <div class="grid grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            "Timezone"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || form_timezone.get()
                                            on:input=move |ev| form_timezone.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder="Asia/Shanghai"
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-text-secondary mb-2">
                                            "Tags"
                                        </label>
                                        <input
                                            type="text"
                                            prop:value=move || form_tags.get()
                                            on:input=move |ev| form_tags.set(event_target_value(&ev))
                                            class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                            placeholder="report, daily"
                                        />
                                    </div>
                                </div>

                                // Enabled toggle
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Status"
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
                                        {move || if form_enabled.get() { "Enabled" } else { "Disabled" }}
                                    </button>
                                </div>

                                // Session Target
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        "Session Target"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_session_target.get()
                                        on:input=move |ev| form_session_target.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="e.g. new, reuse, reuse:session-id"
                                    />
                                </div>

                                // Failure Alert (collapsible)
                                <div class="border border-border rounded-lg">
                                    <button
                                        on:click=move |_| form_alert_expanded.set(!form_alert_expanded.get())
                                        class="w-full px-4 py-3 flex items-center gap-2 text-sm font-medium text-text-secondary hover:text-text-primary transition-colors"
                                    >
                                        <span>{move || if form_alert_expanded.get() { "\u{25BC}" } else { "\u{25B6}" }}</span>
                                        "Failure Alert"
                                    </button>
                                    {move || {
                                        if form_alert_expanded.get() {
                                            view! {
                                                <div class="px-4 pb-4 space-y-4 border-t border-border pt-3">
                                                    <div class="grid grid-cols-2 gap-4">
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                "After N failures"
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
                                                                "Cooldown"
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
                                                                "Alert to"
                                                            </label>
                                                            <select
                                                                prop:value=move || form_alert_kind.get()
                                                                on:change=move |ev| form_alert_kind.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                            >
                                                                <option value="announce">"Announce"</option>
                                                                <option value="webhook">"Webhook"</option>
                                                            </select>
                                                        </div>
                                                        <div>
                                                            <label class="block text-xs font-medium text-text-secondary mb-1">
                                                                "Channel / URL"
                                                            </label>
                                                            <input
                                                                type="text"
                                                                prop:value=move || form_alert_channel.get()
                                                                on:input=move |ev| form_alert_channel.set(event_target_value(&ev))
                                                                class="w-full px-3 py-1.5 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:border-primary"
                                                                placeholder="channel name or webhook URL"
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
                                    {move || if saving.get() { "Saving..." } else { "Save" }}
                                </button>

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_run_now
                                                prop:disabled=move || saving.get()
                                                class="px-6 py-2 bg-success/80 hover:bg-success disabled:bg-success/40 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                "Run Now"
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
                                                class="px-6 py-2 bg-danger hover:bg-danger disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                "Delete"
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
                                    "Cancel"
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

#[component]
fn RunHistory(
    runs: RwSignal<Vec<JobRunInfo>>,
) -> impl IntoView {
    view! {
        <div class="border border-border rounded-lg">
            <div class="px-4 py-3 border-b border-border">
                <h3 class="text-sm font-semibold text-text-primary">"Execution History"</h3>
            </div>

            {move || {
                let run_list = runs.get();
                if run_list.is_empty() {
                    view! {
                        <div class="p-6 text-center text-sm text-text-tertiary">
                            "No execution records yet"
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b border-border text-text-secondary">
                                    <th class="px-4 py-2 text-left font-medium">"Status"</th>
                                    <th class="px-4 py-2 text-left font-medium">"Time"</th>
                                    <th class="px-4 py-2 text-left font-medium">"Duration"</th>
                                    <th class="px-4 py-2 text-left font-medium">"Delivery"</th>
                                    <th class="px-4 py-2 text-left font-medium">"Error"</th>
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
                                        (Some(reason), Some(err)) => format!("[{}] {}", reason, err),
                                        (Some(reason), None) => reason.clone(),
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
