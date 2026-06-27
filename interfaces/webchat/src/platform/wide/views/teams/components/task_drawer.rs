//! `TaskDetailDrawer` — slide-out detail panel with status-transition actions
//! and 3 collapsible read-only sections (Runs / Comments / Events) that
//! surface the audit history wired in `coord_task_runs`, `coord_task_comments`,
//! and `team_events`.

use super::format_relative_time;
use crate::api::teams::{
    CoordTaskDto, TaskCommentDto, TaskEventDto, TaskPatch, TaskRunDto, TeamsApi,
};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// A lifecycle action the drawer can offer for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskAction {
    Start,
    Complete,
    Fail,
    Cancel,
    Pause,
    Resume,
    Skip,
    Retry,
    Approve,
    Reject,
}

/// The dedicated side-effecting backend verbs (everything except the four
/// plain status writes, which go through `update_task`, and Reject, which
/// needs a reason and is handled separately).
#[derive(Debug, Clone, Copy)]
enum Verb {
    Pause,
    Resume,
    Skip,
    Retry,
    Approve,
}

/// Which lifecycle actions the drawer offers for a task in `status`.
///
/// Pure + total so it is unit-testable without a DOM. The backend remains the
/// final authority on transition validity; this only hides the obviously
/// invalid actions so the UI never offers a guaranteed no-op. Terminal and
/// unknown statuses expose nothing.
fn actions_for_status(status: &str) -> Vec<TaskAction> {
    use TaskAction::*;
    match status {
        "pending" => vec![Start, Pause, Skip, Cancel],
        "blocked" | "unsatisfiable" => vec![Pause, Skip, Cancel],
        "in_progress" => vec![Complete, Fail, Pause, Cancel],
        "waiting_review" => vec![Approve, Reject, Skip, Cancel],
        "paused" => vec![Resume, Cancel],
        "failed" => vec![Retry],
        _ => vec![],
    }
}

#[component]
#[must_use]
pub fn TaskDetailDrawer(
    open_for: RwSignal<Option<CoordTaskDto>>,
    #[prop(into)] on_changed: Callback<()>,
) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Transient state for the in-flight status mutation.
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let busy = RwSignal::new(false);

    // History subview state — fetched lazily when the task changes.
    let runs: RwSignal<Vec<TaskRunDto>> = RwSignal::new(Vec::new());
    let comments: RwSignal<Vec<TaskCommentDto>> = RwSignal::new(Vec::new());
    let events: RwSignal<Vec<TaskEventDto>> = RwSignal::new(Vec::new());
    let new_comment: RwSignal<String> = RwSignal::new(String::new());
    let comment_busy = RwSignal::new(false);

    // Reject flow: clicking Reject reveals an inline optional-reason input.
    let reject_open = RwSignal::new(false);
    let reject_reason: RwSignal<String> = RwSignal::new(String::new());

    // Reset transient action state whenever the drawer target changes so a
    // stale error from a previous task never bleeds into the next one.
    Effect::new(move |_| {
        let task_opt = open_for.get();
        error.set(None);
        busy.set(false);
        comment_busy.set(false);
        new_comment.set(String::new());
        reject_open.set(false);
        reject_reason.set(String::new());
        // Clear the previous task's history immediately so the user doesn't
        // see stale rows from another card while the fetch is in flight.
        runs.set(Vec::new());
        comments.set(Vec::new());
        events.set(Vec::new());
        if let Some(task) = task_opt {
            let id = task.id;
            let dash_runs = dash;
            let dash_comments = dash;
            let dash_events = dash;
            let id_runs = id.clone();
            let id_comments = id.clone();
            let id_events = id;
            spawn_local(async move {
                if let Ok(rs) = TeamsApi::list_task_runs(&dash_runs, &id_runs).await {
                    runs.set(rs);
                }
            });
            spawn_local(async move {
                if let Ok(cs) = TeamsApi::list_task_comments(&dash_comments, &id_comments).await {
                    comments.set(cs);
                }
            });
            spawn_local(async move {
                if let Ok(es) = TeamsApi::list_task_events(&dash_events, &id_events).await {
                    events.set(es);
                }
            });
        }
    });

    let close = move |_| open_for.set(None);

    let submit_comment = move |_ev: web_sys::MouseEvent| {
        if comment_busy.get_untracked() {
            return;
        }
        let Some(task) = open_for.get_untracked() else {
            return;
        };
        let body = new_comment.get_untracked().trim().to_string();
        if body.is_empty() {
            return;
        }
        let id = task.id;
        // Panel doesn't track a per-user identity yet; comments authored from
        // the drawer are attributed to a synthetic "panel" actor. Worker-
        // initiated comments (via the dedicated builtin tool) carry the real
        // agent id.
        let author = "panel".to_string();
        comment_busy.set(true);
        spawn_local(async move {
            match TeamsApi::add_task_comment(&dash, &id, &author, &body).await {
                Ok(c) => {
                    let mut cur = comments.get_untracked();
                    cur.push(c);
                    comments.set(cur);
                    new_comment.set(String::new());
                    comment_busy.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    comment_busy.set(false);
                }
            }
        });
    };

    let patch_status = move |new_status: &'static str| {
        if busy.get_untracked() {
            return;
        }
        let Some(task) = open_for.get_untracked() else {
            return;
        };
        let id = task.id;
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match TeamsApi::update_task(
                &dash,
                &id,
                TaskPatch {
                    status: Some(new_status.to_string()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    busy.set(false);
                    on_changed.run(());
                    open_for.set(None);
                }
                Err(e) => {
                    // Keep the drawer open so the user sees what failed.
                    busy.set(false);
                    error.set(Some(e));
                }
            }
        });
    };

    // Generic runner for the dedicated side-effecting verbs (Result<(), String>).
    // Mirrors `patch_status`'s busy-lock + success/error handling.
    let run_verb = move |verb: Verb| {
        if busy.get_untracked() {
            return;
        }
        let Some(task) = open_for.get_untracked() else {
            return;
        };
        let id = task.id;
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let res = match verb {
                Verb::Pause => TeamsApi::task_pause(&dash, &id).await,
                Verb::Resume => TeamsApi::task_resume(&dash, &id).await,
                Verb::Skip => TeamsApi::task_skip(&dash, &id).await,
                Verb::Retry => TeamsApi::task_retry(&dash, &id).await,
                Verb::Approve => TeamsApi::task_approve(&dash, &id).await,
            };
            match res {
                Ok(()) => {
                    busy.set(false);
                    on_changed.run(());
                    open_for.set(None);
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e));
                }
            }
        });
    };

    // Reject needs an optional reason captured from the reveal-on-click input.
    let submit_reject = move |_ev: web_sys::MouseEvent| {
        if busy.get_untracked() {
            return;
        }
        let Some(task) = open_for.get_untracked() else {
            return;
        };
        let id = task.id;
        let reason = reject_reason.get_untracked().trim().to_string();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let reason_opt = if reason.is_empty() { None } else { Some(reason.as_str()) };
            match TeamsApi::task_reject(&dash, &id, reason_opt).await {
                Ok(()) => {
                    busy.set(false);
                    on_changed.run(());
                    open_for.set(None);
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e));
                }
            }
        });
    };

    view! {
        {move || match open_for.get() {
            None => ().into_any(),
            Some(task) => {
                let status = task.status.clone();
                let owner = task.owner.clone().unwrap_or_default();
                let priority = task.priority.clone();
                let description = task.description.clone();
                let result = task.result.clone();
                let subject = task.subject.clone();
                let task_id = task.id.clone();
                let created = format_relative_time(task.created_at);
                let started = task.started_at.map(format_relative_time);
                let completed = task.completed_at.map(format_relative_time);
                let dep_count = task.dependencies.len();

                let status_label = t_string!(i18n, teams.kanban.field.status).to_string();
                let owner_label = t_string!(i18n, teams.kanban.field.owner).to_string();
                let priority_label = t_string!(i18n, teams.kanban.field.priority).to_string();
                let desc_label = t_string!(i18n, teams.kanban.field.description).to_string();
                let result_label = t_string!(i18n, teams.kanban.field.result).to_string();
                let created_label = t_string!(i18n, teams.kanban.field.created).to_string();
                let started_label = t_string!(i18n, teams.kanban.field.started).to_string();
                let completed_label = t_string!(i18n, teams.kanban.field.completed).to_string();
                let deps_label = t_string!(i18n, teams.kanban.field.dependencies).to_string();
                let err_prefix = t_string!(i18n, teams.kanban.error.update_failed).to_string();

                view! {
                    <div class="fixed inset-0 z-40 flex justify-end">
                        <div class="aleph-scrim absolute inset-0 bg-black/30" on:click=close></div>
                        <aside class="glass relative w-96 h-full bg-surface-overlay/85 border-l border-border shadow-xl flex flex-col">
                            <header class="px-4 py-3 border-b border-border flex items-start justify-between gap-2">
                                <div class="min-w-0">
                                    <h3 class="text-sm font-semibold text-text-primary truncate">{subject}</h3>
                                    <p class="text-[10px] font-mono text-text-tertiary truncate mt-0.5">{task_id}</p>
                                </div>
                                <button class="text-text-tertiary hover:text-text-primary flex-shrink-0" on:click=close>
                                    "✕"
                                </button>
                            </header>
                            <div class="flex-1 overflow-y-auto p-4 space-y-3 text-sm">
                                <FieldRow label=status_label value=status.clone() />
                                <FieldRow label=owner_label value=owner />
                                <FieldRow label=priority_label value=priority />
                                <FieldRow label=created_label value=created />
                                {started.map(|v| view! { <FieldRow label=started_label value=v /> })}
                                {completed.map(|v| view! { <FieldRow label=completed_label value=v /> })}
                                {(dep_count > 0)
                                    .then(|| view! { <FieldRow label=deps_label value=dep_count.to_string() /> })}
                                <div>
                                    <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                                        {desc_label}
                                    </div>
                                    <div class="text-text-secondary whitespace-pre-wrap">
                                        {description}
                                    </div>
                                </div>
                                {
                                    if let Some(r) = result.filter(|s| !s.is_empty()) {
                                        view! {
                                            <div>
                                                <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                                                    {result_label}
                                                </div>
                                                <div class="text-text-secondary whitespace-pre-wrap">{r}</div>
                                            </div>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }
                                }

                                // --- Audit history sections (Runs / Comments / Events) ---
                                <RunsSection runs=runs />
                                <CommentsSection
                                    comments=comments
                                    new_comment=new_comment
                                    busy=comment_busy
                                    submit=Callback::new(submit_comment)
                                />
                                <EventsSection events=events />
                            </div>
                            {move || error.get().map(|e| view! {
                                <div class="mx-4 mb-2 px-3 py-2 rounded bg-danger/10 border border-danger/20 text-xs text-danger">
                                    <strong>{err_prefix.clone()}{": "}</strong>{e}
                                </div>
                            })}
                            <footer class="px-4 py-3 border-t border-border flex flex-col gap-2">
                                {move || reject_open.get().then(|| view! {
                                    <div class="flex gap-1.5 items-start">
                                        <textarea
                                            class="flex-1 text-xs p-1.5 rounded border border-border bg-surface-sunken resize-y min-h-[2.5rem]"
                                            placeholder=move || t_string!(i18n, teams.kanban.actions.reject_reason_placeholder).to_string()
                                            prop:value=move || reject_reason.get()
                                            on:input=move |ev| reject_reason.set(event_target_value(&ev))
                                        />
                                        <button
                                            class="px-2 py-1 text-xs rounded bg-danger/10 text-danger hover:bg-danger/20 cursor-pointer"
                                            disabled=move || busy.get()
                                            on:click=move |ev| submit_reject(ev)
                                        >
                                            {t_string!(i18n, teams.kanban.actions.reject).to_string()}
                                        </button>
                                    </div>
                                })}
                                <div class="flex gap-2 flex-wrap">
                                    {actions_for_status(&status).into_iter().map(|action| {
                                        let label = match action {
                                            TaskAction::Start => t_string!(i18n, teams.kanban.actions.start).to_string(),
                                            TaskAction::Complete => t_string!(i18n, teams.kanban.actions.complete).to_string(),
                                            TaskAction::Fail => t_string!(i18n, teams.kanban.actions.fail).to_string(),
                                            TaskAction::Cancel => t_string!(i18n, teams.kanban.actions.cancel).to_string(),
                                            TaskAction::Pause => t_string!(i18n, teams.kanban.actions.pause).to_string(),
                                            TaskAction::Resume => t_string!(i18n, teams.kanban.actions.resume).to_string(),
                                            TaskAction::Skip => t_string!(i18n, teams.kanban.actions.skip).to_string(),
                                            TaskAction::Retry => t_string!(i18n, teams.kanban.actions.retry).to_string(),
                                            TaskAction::Approve => t_string!(i18n, teams.kanban.actions.approve).to_string(),
                                            TaskAction::Reject => t_string!(i18n, teams.kanban.actions.reject).to_string(),
                                        };
                                        view! {
                                            <ActionButton
                                                label=label
                                                disabled=Signal::derive(move || busy.get())
                                                on_click=move |_| match action {
                                                    TaskAction::Start => patch_status("in_progress"),
                                                    TaskAction::Complete => patch_status("completed"),
                                                    TaskAction::Fail => patch_status("failed"),
                                                    TaskAction::Cancel => patch_status("cancelled"),
                                                    TaskAction::Pause => run_verb(Verb::Pause),
                                                    TaskAction::Resume => run_verb(Verb::Resume),
                                                    TaskAction::Skip => run_verb(Verb::Skip),
                                                    TaskAction::Retry => run_verb(Verb::Retry),
                                                    TaskAction::Approve => run_verb(Verb::Approve),
                                                    TaskAction::Reject => reject_open.set(true),
                                                }
                                            />
                                        }
                                    }).collect_view()}
                                </div>
                            </footer>
                        </aside>
                    </div>
                }.into_any()
            }
        }}
    }
}

#[component]
fn FieldRow(label: String, value: String) -> impl IntoView {
    view! {
        <div class="flex items-baseline gap-2">
            <span class="text-xs text-text-tertiary uppercase tracking-wider w-20 flex-shrink-0">{label}</span>
            <span class="text-text-secondary">{value}</span>
        </div>
    }
}

#[component]
fn ActionButton(
    label: String,
    disabled: Signal<bool>,
    on_click: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    let class = move || {
        if disabled.get() {
            "px-3 py-1.5 rounded text-xs bg-surface-sunken text-text-tertiary cursor-not-allowed"
        } else {
            "px-3 py-1.5 rounded text-xs bg-primary/10 text-primary hover:bg-primary/20 cursor-pointer"
        }
    };
    view! {
        <button class=class on:click=on_click disabled=move || disabled.get()>
            {label}
        </button>
    }
}

// =============================================================================
// Audit-history subviews
// =============================================================================

/// Status pill colour for a run record. Mirrors the kanban-column palette so
/// the eye matches "`in_progress"/"completed"/"failed`" hues across the panel.
fn run_status_class(status: &str) -> &'static str {
    match status {
        "running" => "bg-info/10 text-info",
        "completed" => "bg-success/10 text-success",
        "failed" => "bg-danger/10 text-danger",
        "timeout" => "bg-warning/10 text-warning",
        _ => "bg-surface-sunken text-text-tertiary",
    }
}

#[component]
fn RunsSection(runs: RwSignal<Vec<TaskRunDto>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div>
            <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                {t!(i18n, teams.drawer_runs)} " (" {move || runs.get().len()} ")"
            </div>
            {move || {
                let rs = runs.get();
                if rs.is_empty() {
                    view! {
                        <div class="text-text-tertiary text-xs italic">{t!(i18n, teams.drawer_no_runs)}</div>
                    }.into_any()
                } else {
                    let items = rs.into_iter().map(|r| {
                        let badge = run_status_class(&r.status);
                        let duration = r.ended_at
                            .map(|e| format!("{}s", e.saturating_sub(r.started_at)))
                            .unwrap_or_else(|| "—".to_string());
                        let detail = r.error.clone()
                            .filter(|s| !s.is_empty())
                            .or_else(|| r.summary.clone().filter(|s| !s.is_empty()));
                        view! {
                            <li class="border border-border rounded p-2 text-xs space-y-1">
                                <div class="flex items-center gap-2">
                                    <span class=format!("px-1.5 py-0.5 rounded text-[10px] {badge}")>{r.status}</span>
                                    <span class="text-text-tertiary">{format_relative_time(r.started_at)}</span>
                                    <span class="ml-auto text-text-tertiary">{duration}</span>
                                </div>
                                <div class="text-text-tertiary text-[10px]">{r.agent_id}</div>
                                {detail.map(|d| view! {
                                    <div class="text-text-secondary whitespace-pre-wrap break-words">{d}</div>
                                })}
                            </li>
                        }
                    }).collect_view();
                    view! { <ul class="space-y-1.5">{items}</ul> }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn CommentsSection(
    comments: RwSignal<Vec<TaskCommentDto>>,
    new_comment: RwSignal<String>,
    busy: RwSignal<bool>,
    submit: Callback<web_sys::MouseEvent>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div>
            <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                {t!(i18n, teams.drawer_comments)} " (" {move || comments.get().len()} ")"
            </div>
            {move || {
                let cs = comments.get();
                if cs.is_empty() {
                    view! {
                        <div class="text-text-tertiary text-xs italic">{t!(i18n, teams.drawer_no_comments)}</div>
                    }.into_any()
                } else {
                    let items = cs.into_iter().map(|c| view! {
                        <li class="border border-border rounded p-2 text-xs space-y-1">
                            <div class="flex items-center gap-2">
                                <span class="font-medium text-text-primary">{c.author}</span>
                                <span class="text-text-tertiary text-[10px] ml-auto">{format_relative_time(c.created_at)}</span>
                            </div>
                            <div class="text-text-secondary whitespace-pre-wrap break-words">{c.body}</div>
                        </li>
                    }).collect_view();
                    view! { <ul class="space-y-1.5">{items}</ul> }.into_any()
                }
            }}
            <div class="mt-2 flex gap-1.5 items-start">
                <textarea
                    class="flex-1 text-xs p-1.5 rounded border border-border bg-surface-sunken resize-y min-h-[2.5rem]"
                    placeholder=move || t_string!(i18n, teams.drawer_add_comment_placeholder).to_string()
                    prop:value=move || new_comment.get()
                    on:input=move |ev| new_comment.set(event_target_value(&ev))
                />
                <button
                    class=move || if busy.get() || new_comment.get().trim().is_empty() {
                        "px-2 py-1 text-xs rounded bg-surface-sunken text-text-tertiary cursor-not-allowed"
                    } else {
                        "px-2 py-1 text-xs rounded bg-primary/10 text-primary hover:bg-primary/20 cursor-pointer"
                    }
                    disabled=move || busy.get() || new_comment.get().trim().is_empty()
                    on:click=move |ev| submit.run(ev)
                >
                    {t!(i18n, teams.drawer_post)}
                </button>
            </div>
        </div>
    }
}

#[component]
fn EventsSection(events: RwSignal<Vec<TaskEventDto>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div>
            <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                {t!(i18n, teams.drawer_events)} " (" {move || events.get().len()} ")"
            </div>
            {move || {
                let es = events.get();
                if es.is_empty() {
                    view! {
                        <div class="text-text-tertiary text-xs italic">{t!(i18n, teams.drawer_no_events)}</div>
                    }.into_any()
                } else {
                    let items = es.into_iter().rev().take(20).collect::<Vec<_>>().into_iter().map(|e| view! {
                        <li class="flex items-baseline gap-2 text-xs">
                            <span class="text-text-tertiary text-[10px] w-28 flex-shrink-0">{e.timestamp}</span>
                            <span class="font-mono text-primary">{e.event_type}</span>
                            <span class="text-text-tertiary text-[10px]">{e.agent_id}</span>
                        </li>
                    }).collect_view();
                    view! { <ul class="space-y-0.5">{items}</ul> }.into_any()
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{actions_for_status, TaskAction};

    #[test]
    fn gating_matches_lifecycle_rules() {
        use TaskAction::*;
        assert_eq!(actions_for_status("pending"), vec![Start, Pause, Skip, Cancel]);
        assert_eq!(actions_for_status("blocked"), vec![Pause, Skip, Cancel]);
        // "unsatisfiable" (derived blocked) mirrors blocked exactly.
        assert_eq!(actions_for_status("unsatisfiable"), actions_for_status("blocked"));
        assert_eq!(actions_for_status("in_progress"), vec![Complete, Fail, Pause, Cancel]);
        assert_eq!(actions_for_status("waiting_review"), vec![Approve, Reject, Skip, Cancel]);
        assert_eq!(actions_for_status("paused"), vec![Resume, Cancel]);
        assert_eq!(actions_for_status("failed"), vec![Retry]);
        // Terminal + unknown statuses expose no actions.
        for s in ["completed", "skipped", "cancelled", "garbage"] {
            assert!(actions_for_status(s).is_empty(), "{s} must be terminal/inert");
        }
    }
}
