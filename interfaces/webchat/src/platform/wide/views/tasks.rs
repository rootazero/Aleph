//! Unified Tasks View — Cron + Heartbeat tabs
//!
//! Combines scheduled cron jobs and heartbeat monitoring tasks
//! into a single tabbed view accessible at /dashboard/tasks.

use crate::api::heartbeat::{
    CreateHeartbeatTask, HeartbeatApi, HeartbeatRunInfo, HeartbeatTaskInfo, UpdateHeartbeatTask,
};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::cron::CronView;
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// Helper Functions
// ============================================================================

/// Format milliseconds to human-readable interval string.
fn format_interval(ms: u64) -> String {
    if ms < 60_000 {
        format!("{}s", ms / 1000)
    } else if ms < 3_600_000 {
        format!("{}min", ms / 60_000)
    } else if ms < 86_400_000 {
        format!("{}h", ms / 3_600_000)
    } else {
        format!("{}d", ms / 86_400_000)
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
        rest.parse::<u64>().ok().and_then(|v| v.checked_mul(1000))
    } else if let Some(rest) = trimmed.strip_suffix('m') {
        rest.parse::<u64>().ok().and_then(|v| v.checked_mul(60_000))
    } else if let Some(rest) = trimmed.strip_suffix('h') {
        rest.parse::<u64>()
            .ok()
            .and_then(|v| v.checked_mul(3_600_000))
    } else if let Some(rest) = trimmed.strip_suffix('d') {
        rest.parse::<u64>()
            .ok()
            .and_then(|v| v.checked_mul(86_400_000))
    } else {
        None
    }
}

/// Format a UNIX timestamp (seconds) as a relative time string.
fn format_relative_time_hb(ts_ms: i64) -> String {
    let now_ms = js_sys::Date::now() as i64;
    let diff_ms = ts_ms - now_ms;

    if diff_ms < 0 {
        return "overdue".to_string();
    }

    let diff_s = diff_ms / 1000;
    let minutes = diff_s / 60;
    let hours = diff_s / 3600;
    let days = diff_s / 86400;

    if minutes < 1 {
        format!("{diff_s}s")
    } else if hours < 1 {
        format!("{minutes}min")
    } else if days < 1 {
        format!("{hours}h")
    } else {
        format!("{days}d")
    }
}

/// Format epoch milliseconds as "MM/DD HH:MM".
fn format_timestamp_ms(ts_ms: i64) -> String {
    let date = js_sys::Date::new_0();
    date.set_time(ts_ms as f64);

    let month = date.get_month() + 1;
    let day = date.get_date();
    let hours = date.get_hours();
    let minutes = date.get_minutes();

    format!("{month:02}/{day:02} {hours:02}:{minutes:02}")
}

// ============================================================================
// TasksView — Top-level tabbed container
// ============================================================================

/// Unified Tasks view with Cron and Heartbeat tabs.
/// Accessible at /dashboard/tasks (and /dashboard/cron for backward compat).
#[component]
#[must_use]
pub fn TasksView() -> impl IntoView {
    let (active_tab, set_active_tab) = signal("cron".to_string());
    let i18n = use_i18n();

    view! {
        <div class="flex flex-col h-full aleph-content-top">
            // Tab bar
            <div class="flex items-center border-b border-border px-6 gap-1 flex-shrink-0">
                <button
                    on:click=move |_| set_active_tab.set("cron".to_string())
                    class=move || {
                        if active_tab.get() == "cron" {
                            "px-4 py-3 text-sm font-medium border-b-2 border-primary text-primary transition-colors"
                        } else {
                            "px-4 py-3 text-sm font-medium border-b-2 border-transparent text-text-secondary hover:text-text-primary transition-colors"
                        }
                    }
                >
                    {move || t_string!(i18n, cron.title).to_string()}
                </button>
                <button
                    on:click=move |_| set_active_tab.set("heartbeat".to_string())
                    class=move || {
                        if active_tab.get() == "heartbeat" {
                            "px-4 py-3 text-sm font-medium border-b-2 border-primary text-primary transition-colors"
                        } else {
                            "px-4 py-3 text-sm font-medium border-b-2 border-transparent text-text-secondary hover:text-text-primary transition-colors"
                        }
                    }
                >
                    {move || t_string!(i18n, heartbeat.title).to_string()}
                </button>
            </div>

            // Tab content
            <div class="flex-1 overflow-hidden">
                {move || {
                    if active_tab.get() == "heartbeat" {
                        view! { <HeartbeatView /> }.into_any()
                    } else {
                        view! { <CronView /> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// HeartbeatView — Main Container
// ============================================================================

#[component]
fn HeartbeatView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State
    let tasks = RwSignal::new(Vec::<HeartbeatTaskInfo>::new());
    let selected = RwSignal::new(Option::<usize>::None);
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    let refresh_tasks = move || {
        spawn_local(async move {
            match HeartbeatApi::list(&state).await {
                Ok(list) => {
                    tasks.set(list);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to load tasks: {e}"),
                    )));
                    loading.set(false);
                }
            }
        });
    };

    // Initial load on mount.
    refresh_tasks();

    // Live push: subscribe to `heartbeat.task.changed`. Mirrors the cron-view
    // wiring — server-side emit lives in `HeartbeatService::emit_change`.
    Effect::new(move |_| {
        let dash = state;
        spawn_local(async move {
            let _ = dash.subscribe_topic("heartbeat.task.changed").await;
        });
    });
    let sub_id = state.subscribe_events(move |evt| {
        if evt.topic == "heartbeat.task.changed" {
            refresh_tasks();
        }
    });
    on_cleanup(move || state.unsubscribe_events(sub_id));

    view! {
        <div class="flex flex-col h-full">
            // Header
            <div class="p-6 border-b border-border">
                <h1 class="text-2xl font-bold text-text-primary">{move || t_string!(i18n, heartbeat.title).to_string()}</h1>
                <p class="mt-1 text-sm text-text-secondary">
                    {move || t_string!(i18n, heartbeat.description).to_string()}
                </p>
            </div>

            // Content: left list + right editor
            <div class="flex-1 flex overflow-hidden">
                <HeartbeatList tasks=tasks selected=selected loading=loading />
                <HeartbeatEditor tasks=tasks selected=selected saving=saving error=error />
            </div>
        </div>
    }
}

// ============================================================================
// HeartbeatList — Left Pane
// ============================================================================

#[component]
fn HeartbeatList(
    tasks: RwSignal<Vec<HeartbeatTaskInfo>>,
    selected: RwSignal<Option<usize>>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="w-80 border-r border-border flex flex-col">
            // Add button
            <div class="p-4 border-b border-border">
                <button
                    on:click=move |_| selected.set(Some(usize::MAX))
                    class="w-full px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors"
                >
                    {move || t_string!(i18n, heartbeat.new_task_btn).to_string()}
                </button>
            </div>

            // Task list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if loading.get() {
                        view! {
                            <div class="p-4 text-center text-text-secondary">
                                {t!(i18n, heartbeat.loading)}
                            </div>
                        }.into_any()
                    } else if tasks.get().is_empty() {
                        view! {
                            <div class="flex flex-col items-center justify-center p-8 text-text-tertiary">
                                // Heartbeat / pulse icon
                                <svg class="w-12 h-12 mb-3 opacity-40" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                                </svg>
                                <span class="text-sm">{t!(i18n, heartbeat.no_tasks)}</span>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-2 space-y-2">
                                {move || {
                                    tasks.get().iter().enumerate().map(|(idx, task)| {
                                        let task = task.clone();
                                        let is_selected = Signal::derive(move || selected.get() == Some(idx));
                                        view! {
                                            <HeartbeatListItem task=task index=idx is_selected=is_selected selected=selected />
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
// HeartbeatListItem
// ============================================================================

#[component]
fn HeartbeatListItem(
    task: HeartbeatTaskInfo,
    index: usize,
    is_selected: Signal<bool>,
    selected: RwSignal<Option<usize>>,
) -> impl IntoView {
    let i18n = use_i18n();

    let name = task.name.clone();
    let enabled = task.enabled;
    let interval_ms = task.interval_ms;
    let next_due_ms = task.state.next_due_ms;
    let has_errors = task.state.consecutive_errors > 0;
    let last_l2 = task.state.last_l2_status.unwrap_or_default();

    let interval_str = format_interval(interval_ms);

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
                // Status dot: green = enabled, gray = disabled
                <span class=move || {
                    if enabled {
                        "w-2 h-2 rounded-full bg-success flex-shrink-0"
                    } else {
                        "w-2 h-2 rounded-full bg-text-tertiary flex-shrink-0"
                    }
                }></span>
                <span class="text-sm font-medium text-text-primary truncate flex-1">
                    {name}
                </span>
                // Error badge
                {if has_errors {
                    view! {
                        <span class="w-2 h-2 rounded-full bg-danger flex-shrink-0"
                              title="Has consecutive errors"></span>
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
            </div>
            // Interval + agent
            <div class="ml-4 text-xs text-text-secondary">
                {interval_str}
                {if !last_l2.is_empty() {
                    view! {
                        <span class="ml-2 px-1.5 py-0.5 rounded text-xs bg-surface-sunken text-text-tertiary">
                            {last_l2}
                        </span>
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
            </div>
            // Next due
            {move || {
                if let Some(ts_ms) = next_due_ms {
                    let relative = format_relative_time_hb(ts_ms);
                    let next_prefix = t_string!(i18n, heartbeat.next).to_string();
                    view! {
                        <div class="ml-4 mt-1 text-xs text-text-tertiary">
                            {format!("{next_prefix}{relative}")}
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
// HeartbeatEditor — Right Pane
// ============================================================================

#[component]
fn HeartbeatEditor(
    tasks: RwSignal<Vec<HeartbeatTaskInfo>>,
    selected: RwSignal<Option<usize>>,
    saving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Form state
    let form_name = RwSignal::new(String::new());
    let form_agent_id = RwSignal::new(String::new());
    let form_interval = RwSignal::new(String::new());
    let form_probe_tool = RwSignal::new(String::new());
    let form_trigger_condition = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);

    // Run history for existing tasks
    let runs = RwSignal::new(Vec::<HeartbeatRunInfo>::new());

    // Confirm delete state
    let confirm_delete = RwSignal::new(false);

    let is_new = move || selected.get() == Some(usize::MAX);
    let is_editing = move || selected.get().is_some();

    // Populate form when selection changes
    Effect::new(move || {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                // Reset form for new task
                form_name.set(String::new());
                form_agent_id.set(String::new());
                form_interval.set(String::new());
                form_probe_tool.set(String::new());
                form_trigger_condition.set(String::new());
                form_enabled.set(true);
                runs.set(Vec::new());
                confirm_delete.set(false);
            } else if let Some(task) = tasks.get().get(idx) {
                // Load existing task data
                form_name.set(task.name.clone());
                form_agent_id.set(task.agent_id.clone());
                form_interval.set(format_interval(task.interval_ms));
                form_probe_tool.set(task.probe.tool_name.clone());
                form_trigger_condition.set(
                    task.probe
                        .trigger_condition
                        .as_ref()
                        .map(crate::api::heartbeat::trigger_condition_to_form)
                        .unwrap_or_default(),
                );
                form_enabled.set(task.enabled);
                confirm_delete.set(false);

                // Load run history
                let task_id = task.id.clone();
                spawn_local(async move {
                    match HeartbeatApi::runs(&state, &task_id, 20).await {
                        Ok(list) => runs.set(list),
                        Err(_) => runs.set(Vec::new()),
                    }
                });
            }
        }
    });

    // Handle save
    let on_save = move |_| {
        let name = form_name.get();
        if name.trim().is_empty() {
            error.set(Some(
                t_string!(i18n, heartbeat.error_name_required).to_string(),
            ));
            return;
        }

        let interval = form_interval.get();
        if interval.trim().is_empty() {
            error.set(Some(
                t_string!(i18n, heartbeat.error_interval_required).to_string(),
            ));
            return;
        }

        let probe_tool = form_probe_tool.get();
        if probe_tool.trim().is_empty() {
            error.set(Some(
                t_string!(i18n, heartbeat.error_probe_tool_required).to_string(),
            ));
            return;
        }

        // Validate interval
        if parse_interval_to_ms(&interval).is_none() {
            error.set(Some(
                "Invalid interval format. Use e.g. 5m, 30s, 2h".to_string(),
            ));
            return;
        }

        saving.set(true);
        error.set(None);

        let agent_id = form_agent_id.get();
        let trigger_condition = {
            let s = form_trigger_condition.get();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        };
        let enabled = form_enabled.get();

        let idx = selected.get().unwrap_or(usize::MAX);

        if idx == usize::MAX {
            // Create new task
            let req = CreateHeartbeatTask {
                name,
                agent_id,
                interval,
                probe_tool_name: probe_tool,
                probe_trigger_condition: trigger_condition,
            };
            spawn_local(async move {
                match HeartbeatApi::create(&state, req).await {
                    Ok(task) => {
                        tasks.update(|list| list.push(task));
                        let new_idx = tasks.get().len() - 1;
                        selected.set(Some(new_idx));
                        saving.set(false);
                    }
                    Err(e) => {
                        error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Failed to create task: {e}")
                            }),
                        ));
                        saving.set(false);
                    }
                }
            });
        } else if let Some(existing) = tasks.get().get(idx) {
            // Update existing task
            let task_id = existing.id.clone();
            let patch = UpdateHeartbeatTask {
                task_id,
                name: Some(name),
                interval: Some(interval),
                enabled: Some(enabled),
                probe_tool_name: Some(probe_tool),
                probe_trigger_condition: trigger_condition,
            };
            spawn_local(async move {
                match HeartbeatApi::update(&state, patch).await {
                    Ok(updated) => {
                        tasks.update(|list| {
                            if let Some(item) = list.get_mut(idx) {
                                *item = updated;
                            }
                        });
                        saving.set(false);
                    }
                    Err(e) => {
                        error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Failed to update task: {e}")
                            }),
                        ));
                        saving.set(false);
                    }
                }
            });
        }
    };

    // Handle toggle enable/disable
    let on_toggle = move |_| {
        let Some(idx) = selected.get() else {
            return;
        };
        if idx == usize::MAX {
            return;
        }
        let task_list = tasks.get();
        let Some(task) = task_list.get(idx) else {
            return;
        };
        let task_id = task.id.clone();
        let new_enabled = !task.enabled;
        spawn_local(async move {
            match HeartbeatApi::toggle(&state, &task_id, new_enabled).await {
                Ok(updated) => {
                    tasks.update(|list| {
                        if let Some(item) = list.get_mut(idx) {
                            *item = updated;
                        }
                    });
                    form_enabled.set(new_enabled);
                }
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to toggle task: {e}")
                        }),
                    ));
                }
            }
        });
    };

    // Handle wake (immediate probe)
    let on_wake = move |_| {
        let Some(idx) = selected.get() else {
            return;
        };
        if idx == usize::MAX {
            return;
        }
        let task_list = tasks.get();
        let Some(task) = task_list.get(idx) else {
            return;
        };
        let task_id = task.id.clone();
        spawn_local(async move {
            if let Err(e) = HeartbeatApi::wake(&state, &task_id).await {
                error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to wake task: {e}")
                    }),
                ));
            }
        });
    };

    // Handle delete
    let on_delete = move |_| {
        if !confirm_delete.get() {
            confirm_delete.set(true);
            return;
        }

        let Some(idx) = selected.get() else {
            return;
        };
        if idx == usize::MAX {
            return;
        }
        let task_list = tasks.get();
        let Some(task) = task_list.get(idx) else {
            return;
        };
        let task_id = task.id.clone();
        spawn_local(async move {
            match HeartbeatApi::delete(&state, &task_id).await {
                Ok(()) => {
                    tasks.update(|list| {
                        if idx < list.len() {
                            list.remove(idx);
                        }
                    });
                    selected.set(None);
                    confirm_delete.set(false);
                }
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to delete task: {e}")
                        }),
                    ));
                    confirm_delete.set(false);
                }
            }
        });
    };

    view! {
        <div class="flex-1 overflow-y-auto">
            {move || {
                if !is_editing() {
                    // No selection — placeholder
                    view! {
                        <div class="flex items-center justify-center h-full text-text-tertiary">
                            <div class="text-center">
                                <svg class="w-16 h-16 mx-auto mb-4 opacity-20" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
                                </svg>
                                <p class="text-sm">{t!(i18n, heartbeat.select_or_create)}</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-6 max-w-2xl">
                            // Title
                            <h2 class="text-xl font-semibold text-text-primary mb-1">
                                {move || {
                                    if is_new() {
                                        t_string!(i18n, heartbeat.new_task_title).to_string()
                                    } else {
                                        t_string!(i18n, heartbeat.edit_task_title).to_string()
                                    }
                                }}
                            </h2>

                            // Error banner
                            {move || {
                                if let Some(err) = error.get() {
                                    view! {
                                        <div class="mt-3 p-3 bg-danger/10 border border-danger/30 rounded-lg text-sm text-danger">
                                            {err}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            // Form
                            <div class="mt-6 space-y-4">
                                // Name
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                        {t!(i18n, heartbeat.field_name)}
                                    </label>
                                    <input
                                        type="text"
                                        class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                                        placeholder=move || t_string!(i18n, heartbeat.placeholder_name).to_string()
                                        prop:value=move || form_name.get()
                                        on:input=move |ev| form_name.set(event_target_value(&ev))
                                    />
                                </div>

                                // Agent ID
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                        {t!(i18n, heartbeat.field_agent)}
                                    </label>
                                    <input
                                        type="text"
                                        class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                                        placeholder=move || t_string!(i18n, heartbeat.placeholder_agent).to_string()
                                        prop:value=move || form_agent_id.get()
                                        on:input=move |ev| form_agent_id.set(event_target_value(&ev))
                                    />
                                </div>

                                // Interval
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                        {t!(i18n, heartbeat.field_interval)}
                                    </label>
                                    <input
                                        type="text"
                                        class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                                        placeholder="5m"
                                        prop:value=move || form_interval.get()
                                        on:input=move |ev| form_interval.set(event_target_value(&ev))
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        {t!(i18n, heartbeat.interval_hint)}
                                    </p>
                                </div>

                                // Probe Tool
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                        {t!(i18n, heartbeat.field_probe_tool)}
                                    </label>
                                    <input
                                        type="text"
                                        class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                                        placeholder=move || t_string!(i18n, heartbeat.placeholder_probe_tool).to_string()
                                        prop:value=move || form_probe_tool.get()
                                        on:input=move |ev| form_probe_tool.set(event_target_value(&ev))
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        {t!(i18n, heartbeat.probe_tool_hint)}
                                    </p>
                                </div>

                                // Trigger Condition
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-1">
                                        {t!(i18n, heartbeat.field_trigger_condition)}
                                    </label>
                                    <input
                                        type="text"
                                        class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                                        placeholder=move || t_string!(i18n, heartbeat.placeholder_condition).to_string()
                                        prop:value=move || form_trigger_condition.get()
                                        on:input=move |ev| form_trigger_condition.set(event_target_value(&ev))
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        {t!(i18n, heartbeat.trigger_condition_hint)}
                                    </p>
                                </div>

                                // Status toggle (show only for existing tasks)
                                {move || {
                                    if !is_new() {
                                        view! {
                                            <div class="flex items-center justify-between py-2">
                                                <span class="text-sm font-medium text-text-secondary">
                                                    {t!(i18n, heartbeat.field_status)}
                                                </span>
                                                <button
                                                    on:click=on_toggle
                                                    class=move || {
                                                        if form_enabled.get() {
                                                            "px-3 py-1 text-xs rounded-full bg-success/20 text-success border border-success/30 hover:bg-success/30 transition-colors"
                                                        } else {
                                                            "px-3 py-1 text-xs rounded-full bg-surface-sunken text-text-tertiary border border-border hover:bg-surface transition-colors"
                                                        }
                                                    }
                                                >
                                                    {move || {
                                                        if form_enabled.get() {
                                                            t_string!(i18n, heartbeat.enabled).to_string()
                                                        } else {
                                                            t_string!(i18n, heartbeat.disabled).to_string()
                                                        }
                                                    }}
                                                </button>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }
                                }}
                            </div>

                            // Action buttons
                            <div class="mt-6 flex items-center gap-3">
                                // Save
                                <button
                                    on:click=on_save
                                    disabled=move || saving.get()
                                    class="px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg text-sm transition-colors disabled:opacity-50"
                                >
                                    {move || {
                                        if saving.get() {
                                            t_string!(i18n, heartbeat.btn_saving).to_string()
                                        } else {
                                            t_string!(i18n, heartbeat.btn_save).to_string()
                                        }
                                    }}
                                </button>

                                // Wake (existing only)
                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_wake
                                                class="px-4 py-2 bg-surface border border-border hover:bg-surface-sunken text-text-primary rounded-lg text-sm transition-colors"
                                            >
                                                {t!(i18n, heartbeat.btn_wake)}
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                // Delete (existing only)
                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_delete
                                                class=move || {
                                                    if confirm_delete.get() {
                                                        "px-4 py-2 bg-danger text-white rounded-lg text-sm transition-colors ml-auto"
                                                    } else {
                                                        "px-4 py-2 bg-surface border border-border hover:bg-danger/10 hover:border-danger/30 hover:text-danger text-text-secondary rounded-lg text-sm transition-colors ml-auto"
                                                    }
                                                }
                                            >
                                                {move || {
                                                    if confirm_delete.get() {
                                                        t_string!(i18n, heartbeat.btn_confirm_delete).to_string()
                                                    } else {
                                                        t_string!(i18n, heartbeat.btn_delete).to_string()
                                                    }
                                                }}
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}
                            </div>

                            // Run History (existing tasks only)
                            {move || {
                                if !is_new() {
                                    view! {
                                        <div class="mt-8">
                                            <h3 class="text-sm font-semibold text-text-primary mb-3">
                                                {t!(i18n, heartbeat.history_title)}
                                            </h3>
                                            <HeartbeatRunHistory runs=runs />
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
// HeartbeatRunHistory — Run table
// ============================================================================

#[component]
fn HeartbeatRunHistory(runs: RwSignal<Vec<HeartbeatRunInfo>>) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        {move || {
            let run_list = runs.get();
            if run_list.is_empty() {
                view! {
                    <p class="text-sm text-text-tertiary">{t!(i18n, heartbeat.history_no_records)}</p>
                }.into_any()
            } else {
                view! {
                    <div class="overflow-x-auto">
                        <table class="w-full text-xs text-text-secondary">
                            <thead>
                                <tr class="border-b border-border">
                                    <th class="text-left py-2 pr-4 font-medium">{t!(i18n, heartbeat.col_l1)}</th>
                                    <th class="text-left py-2 pr-4 font-medium">{t!(i18n, heartbeat.col_l2)}</th>
                                    <th class="text-left py-2 pr-4 font-medium">{t!(i18n, heartbeat.col_trigger)}</th>
                                    <th class="text-left py-2 pr-4 font-medium">{t!(i18n, heartbeat.col_time)}</th>
                                    <th class="text-left py-2 font-medium">{t!(i18n, heartbeat.col_error)}</th>
                                </tr>
                            </thead>
                            <tbody>
                                {run_list.into_iter().map(|run| {
                                    let l1 = run.l1_status.clone();
                                    let l2 = run.l2_status.clone().unwrap_or_default();
                                    let trigger = run.trigger_source.clone();
                                    let ts = format_timestamp_ms(run.created_at);
                                    let err = run.error.unwrap_or_default();

                                    // Clone for use in closures vs text nodes
                                    let l1_cls = l1.clone();
                                    let l2_cls = l2.clone();
                                    let err_title = err.clone();

                                    view! {
                                        <tr class="border-b border-border/50 hover:bg-surface-sunken/50">
                                            <td class="py-2 pr-4">
                                                <span class=move || {
                                                    if l1_cls == "ok" || l1_cls == "pass" {
                                                        "text-success font-medium"
                                                    } else if l1_cls.is_empty() {
                                                        "text-text-tertiary"
                                                    } else {
                                                        "text-danger font-medium"
                                                    }
                                                }>{l1}</span>
                                            </td>
                                            <td class="py-2 pr-4">
                                                <span class=move || {
                                                    if l2_cls == "ok" || l2_cls == "pass" || l2_cls == "normal" {
                                                        "text-success"
                                                    } else if l2_cls.is_empty() {
                                                        "text-text-tertiary"
                                                    } else {
                                                        "text-warning"
                                                    }
                                                }>{l2}</span>
                                            </td>
                                            <td class="py-2 pr-4 text-text-tertiary">{trigger}</td>
                                            <td class="py-2 pr-4 text-text-tertiary">{ts}</td>
                                            <td class="py-2 text-danger truncate max-w-xs" title=err_title>{err}</td>
                                        </tr>
                                    }
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    </div>
                }.into_any()
            }
        }}
    }
}
