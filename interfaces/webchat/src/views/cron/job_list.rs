//! Left pane — job list, Quick Create drawer, and individual job cards.

use super::helpers::{format_relative_time, format_schedule_summary};
use crate::api::cron::{CreateCronJob, CronApi, CronJobInfo};
use crate::context::DashboardState;
use crate::i18n::{t_string, t, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

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
pub(super) fn JobList(
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
                    // next_run_at is epoch ms (backend state.next_run_at_ms);
                    // format_relative_time expects epoch seconds.
                    let relative = format_relative_time(ts / 1000, &overdue_label);
                    let next_prefix = t_string!(i18n, cron.next).to_string();
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
