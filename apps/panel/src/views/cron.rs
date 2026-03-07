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
/// e.g. "every 5min", "every 2h", or the raw cron expression.
fn format_schedule_summary(kind: &str, schedule: &str) -> String {
    match kind {
        "every" => {
            // Parse simple interval strings like "5m", "2h", "30s"
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
        _ => schedule.to_string(), // cron expression shown as-is
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
    let schedule_kind = job.schedule_kind.clone();
    let schedule = job.schedule.clone();
    let next_run_at = job.next_run_at;

    let summary = format_schedule_summary(&schedule_kind, &schedule);

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
                // Status dot
                <span class=move || {
                    if enabled {
                        "w-2 h-2 rounded-full bg-success flex-shrink-0"
                    } else {
                        "w-2 h-2 rounded-full bg-text-tertiary flex-shrink-0"
                    }
                }></span>
                <span class="text-sm font-medium text-text-primary truncate">
                    {name}
                </span>
            </div>
            <div class="ml-4 text-xs text-text-secondary">
                {summary}
            </div>
            {move || {
                if let Some(ts) = next_run_at {
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
                runs.set(Vec::new());
            } else {
                // Load existing job data
                if let Some(job) = jobs.get().get(idx) {
                    form_name.set(job.name.clone());
                    form_schedule_kind.set(job.schedule_kind.clone());
                    form_schedule.set(job.schedule.clone());
                    form_agent_id.set(job.agent_id.clone());
                    form_prompt.set(job.prompt.clone());
                    form_timezone.set(job.timezone.clone().unwrap_or_default());
                    form_tags.set(job.tags.join(", "));
                    form_enabled.set(job.enabled);

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
                                    let error_str = run.error.clone().unwrap_or_default();

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
