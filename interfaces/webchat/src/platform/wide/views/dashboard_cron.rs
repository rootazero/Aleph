//! Dashboard Cron sparkline — live 12-cell success/fail history per scheduled
//! job. Pulls `cron.list` once when the gateway connects, then `cron.runs`
//! per job (limit = `SPARK_CELLS`). No subscription churn: a fresh list reads
//! on every reconnect.

use crate::api::cron::{CronApi, CronJobInfo, JobRunInfo};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use std::collections::HashMap;

const SPARK_CELLS: usize = 12;
const MAX_VISIBLE_JOBS: usize = 8;

/// Render up to [`MAX_VISIBLE_JOBS`] cron jobs, each followed by a 12-cell
/// success/fail history sparkline (newest on the right).
#[component]
#[must_use]
pub fn CronSparklines() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let jobs = RwSignal::new(Vec::<CronJobInfo>::new());
    let runs_by_job: RwSignal<HashMap<String, Vec<JobRunInfo>>> = RwSignal::new(HashMap::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);

    Effect::new(move || {
        if !state.is_connected.get() {
            jobs.set(Vec::new());
            runs_by_job.set(HashMap::new());
            loading.set(true);
            error.set(None);
            return;
        }
        leptos::task::spawn_local(async move {
            loading.set(true);
            match CronApi::list(&state).await {
                Ok(list) => {
                    let visible: Vec<CronJobInfo> =
                        list.into_iter().take(MAX_VISIBLE_JOBS).collect();
                    let mut runs_map: HashMap<String, Vec<JobRunInfo>> = HashMap::new();
                    for job in &visible {
                        if let Ok(runs) = CronApi::runs(&state, &job.id, SPARK_CELLS as i32).await {
                            runs_map.insert(job.id.clone(), runs);
                        }
                    }
                    runs_by_job.set(runs_map);
                    jobs.set(visible);
                    error.set(None);
                }
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
            loading.set(false);
        });
    });

    view! {
        <div class="bg-surface-raised border border-border rounded-2xl overflow-hidden">
            <div class="p-4 border-b border-border bg-surface-sunken flex items-center justify-between">
                <span class="text-sm font-medium text-text-secondary">"Scheduled job pulse"</span>
                <span class="text-[10px] font-mono text-text-tertiary uppercase tracking-wider">
                    {format!("Last {SPARK_CELLS} runs")}
                </span>
            </div>
            <div class="p-4">
                {move || {
                    if !state.is_connected.get() {
                        return view! {
                            <p class="text-text-tertiary text-sm py-4 text-center">
                                {t!(i18n, dashboard_cron.gateway_disconnected)}
                            </p>
                        }.into_any();
                    }
                    if loading.get() {
                        return view! {
                            <p class="text-text-tertiary text-sm py-4 text-center">"Loading…"</p>
                        }.into_any();
                    }
                    if let Some(msg) = error.get() {
                        return view! {
                            <p class="text-danger text-sm py-4 text-center">{msg}</p>
                        }.into_any();
                    }
                    let list = jobs.get();
                    if list.is_empty() {
                        return view! {
                            <p class="text-text-tertiary text-sm py-4 text-center">
                                {t!(i18n, dashboard_cron.no_jobs)}
                            </p>
                        }.into_any();
                    }
                    view! {
                        <ul class="space-y-1">
                            {list.into_iter().map(|job| {
                                let runs = runs_by_job.with(|m| m.get(&job.id).cloned().unwrap_or_default());
                                view! { <SparklineRow job=job runs=runs /> }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

#[component]
fn SparklineRow(job: CronJobInfo, runs: Vec<JobRunInfo>) -> impl IntoView {
    let i18n = use_i18n();
    // Newest at the right edge. `runs` is newest-first from cron.runs.
    let mut cells: Vec<Option<JobRunInfo>> = vec![None; SPARK_CELLS];
    for (idx, run) in runs.into_iter().enumerate().take(SPARK_CELLS) {
        cells[SPARK_CELLS - 1 - idx] = Some(run);
    }

    let enabled_class = if job.enabled {
        "bg-success"
    } else {
        "bg-text-tertiary/40"
    };
    let consec_errs = job.consecutive_errors.unwrap_or(0);
    let job_title = job.name.clone();

    view! {
        <li class="flex items-center gap-3 px-2 py-1.5 rounded hover:bg-surface-sunken/60 transition-colors">
            <span
                class=format!("w-1.5 h-1.5 rounded-full flex-shrink-0 {}", enabled_class)
                title=if job.enabled { "Enabled" } else { "Disabled" }
            ></span>
            <span class="flex-1 min-w-0 text-sm text-text-primary truncate" title=job_title>
                {job.name}
            </span>
            <div class="flex items-center gap-[2px] flex-shrink-0">
                {cells.into_iter().map(render_cell).collect_view()}
            </div>
            {(consec_errs > 0).then(|| view! {
                <span class="text-[10px] font-mono text-danger flex-shrink-0" title=move || t_string!(i18n, dashboard_cron.consecutive_failures).to_string()>
                    {format!("\u{00d7} {consec_errs}")}
                </span>
            })}
        </li>
    }
}

/// Render a single sparkline cell. `None` = no run in that slot.
fn render_cell(maybe: Option<JobRunInfo>) -> impl IntoView {
    let (cls, tooltip): (&'static str, String) = match maybe {
        None => ("bg-text-tertiary/20", "\u{2014}".to_string()),
        Some(r) => match r.status.as_str() {
            "success" | "ok" | "completed" => (
                "bg-success",
                format!("success \u{00b7} {}ms", r.duration_ms),
            ),
            "failed" | "error" => ("bg-danger", r.error.unwrap_or_else(|| "failed".to_string())),
            "skipped" | "missed" => ("bg-warning", format!("skipped @ {}", r.started_at)),
            "running" => ("bg-primary animate-pulse", "running".to_string()),
            other => ("bg-text-tertiary/50", other.to_string()),
        },
    };
    view! {
        <span
            class=format!("inline-block w-1.5 h-4 rounded-[1px] {}", cls)
            title=tooltip
        ></span>
    }
}
