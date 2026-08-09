//! Cron Job Management View
//!
//! Provides UI for managing scheduled tasks:
//! - List all cron jobs with status indicators
//! - Create/Edit/Delete jobs with form editor
//! - View execution history for each job
//! - Trigger immediate runs

mod helpers;
mod job_editor;
mod job_list;
mod run_history;

use crate::api::cron::{CronApi, CronJobInfo};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use job_editor::JobEditor;
use job_list::JobList;
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// CronView — Main Container
// ============================================================================

#[component]
#[must_use]
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
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to load jobs: {e}"),
                    )));
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
        <div class="flex flex-col h-full">
            // Header
            <div class="p-6 border-b border-border aleph-content-top">
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
