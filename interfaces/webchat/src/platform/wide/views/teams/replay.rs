//! `ReplayView` — R3 unified audit timeline.
//!
//! Left pane: task list for the active team (status dot + journal chip).
//! Right pane: when a task is selected, fetches `teams.task.trace` and
//! renders the exit journal at the top followed by a merged chronological
//! timeline (runs · comments · events · artifacts). Pure consumer of the
//! API hooks added in R3 — no new server surface.

use super::TeamsTabState;
use crate::api::teams::{CoordTaskDto, TaskExitJournalDto, TaskFilter, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

#[derive(Clone, Debug)]
struct TimelineRow {
    /// Unix seconds, used only for ordering. 0 ⇒ unknown.
    ts: u64,
    kind: &'static str,
    actor: String,
    title: String,
    body: Option<String>,
    badge_class: &'static str,
}

#[component]
#[must_use]
pub fn ReplayView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let state = expect_context::<TeamsTabState>();

    let tasks: RwSignal<Vec<CoordTaskDto>> = RwSignal::new(Vec::new());
    let journals: RwSignal<Vec<TaskExitJournalDto>> = RwSignal::new(Vec::new());
    let selected: RwSignal<Option<String>> = RwSignal::new(None);
    let trace: RwSignal<Option<Value>> = RwSignal::new(None);
    let loading_trace = RwSignal::new(false);
    let error: RwSignal<Option<String>> = RwSignal::new(None);

    let refresh_list = move || {
        let Some(team_id) = state.selected_team_id.get_untracked() else {
            tasks.set(Vec::new());
            journals.set(Vec::new());
            return;
        };
        spawn_local(async move {
            if let Ok(list) = TeamsApi::list_tasks(&dash, &team_id, TaskFilter::default()).await {
                tasks.set(list);
            }
            if let Ok(js) = TeamsApi::task_journal_list(&dash, &team_id).await {
                journals.set(js);
            }
        });
    };

    let refresh_trace = move |task_id: String| {
        loading_trace.set(true);
        error.set(None);
        spawn_local(async move {
            match TeamsApi::task_trace(&dash, &task_id).await {
                Ok(v) => trace.set(Some(v)),
                Err(e) => error.set(Some(e)),
            }
            loading_trace.set(false);
        });
    };

    // Switch teams ⇒ reload list, clear selection.
    Effect::new(move |_| {
        let _ = state.selected_team_id.get();
        refresh_list();
        selected.set(None);
        trace.set(None);
    });

    // Re-fetch trace when selection changes.
    Effect::new(move |_| {
        if let Some(tid) = selected.get() {
            refresh_trace(tid);
        }
    });

    // Live refresh on team.*.task.* topic.
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic("team.*.task.*").await;
        });
    });
    let sub_id = dash.subscribe_events(move |evt| {
        let Some(team_id) = state.selected_team_id.get_untracked() else {
            return;
        };
        let prefix = format!("team.{team_id}.task.");
        if !evt.topic.starts_with(&prefix) {
            return;
        }
        refresh_list();
        if let Some(tid) = selected.get_untracked() {
            refresh_trace(tid);
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));

    view! {
        // `aleph-md`: this is a master-detail, and it was the only one in the
        // tree that never said so — a hand-rolled `w-72` + `flex-1` pair. Under
        // 720 px (and inside `.phone-wrapped`) the shared rule stacks the panes;
        // without it the 288 px list left the trace pane ~102 px on a 390 px
        // screen, which is not "cramped", it is unreadable.
        <div class="flex-1 flex h-full overflow-hidden aleph-md">
            <TaskListPane
                tasks=tasks
                journals=journals
                selected=selected
                team_picked=Signal::derive(move || state.selected_team_id.get().is_some())
            />
            <TracePane
                selected=selected
                trace=trace
                loading=loading_trace
                error=error
                on_refresh=Callback::new(move |()| {
                    if let Some(tid) = selected.get_untracked() {
                        refresh_trace(tid);
                    }
                })
            />
        </div>
    }
}

// ─── Left pane ──────────────────────────────────────────────────────────

#[component]
fn TaskListPane(
    tasks: RwSignal<Vec<CoordTaskDto>>,
    journals: RwSignal<Vec<TaskExitJournalDto>>,
    selected: RwSignal<Option<String>>,
    team_picked: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="w-72 border-r border-border flex flex-col aleph-md-list">
            <div class="px-4 py-3 border-b border-border aleph-content-top">
                <h2 class="text-sm font-semibold text-text-primary">{t!(i18n, teams.replay_title)}</h2>
                <p class="text-xs text-text-tertiary mt-0.5">
                    {t!(i18n, teams.replay_subtitle)}
                </p>
            </div>
            <div class="flex-1 overflow-y-auto py-1">
                {move || {
                    if !team_picked.get() {
                        return view! {
                            <div class="px-4 py-6 text-xs text-text-tertiary">
                                {t!(i18n, teams.replay_pick_team)}
                            </div>
                        }.into_any();
                    }
                    let list = tasks.get();
                    if list.is_empty() {
                        return view! {
                            <div class="px-4 py-6 text-xs text-text-tertiary">
                                {t!(i18n, teams.replay_no_tasks)}
                            </div>
                        }.into_any();
                    }
                    let journal_ids: std::collections::HashSet<String> = journals
                        .get()
                        .into_iter()
                        .map(|j| j.task_id)
                        .collect();
                    list.into_iter().map(|t| {
                        let tid = t.id.clone();
                        let tid_for_active = tid.clone();
                        let has_journal = journal_ids.contains(&t.id);
                        let owner = t.owner.clone().unwrap_or_else(|| "—".to_string());
                        let subject = t.subject.clone();
                        let status = t.status;
                        view! {
                            <button
                                class=move || {
                                    let base = "w-full text-left px-4 py-2 hover:bg-sidebar-active/40 cursor-pointer transition-colors block";
                                    if selected.get().as_deref() == Some(tid_for_active.as_str()) {
                                        format!("{base} bg-sidebar-active")
                                    } else {
                                        base.to_string()
                                    }
                                }
                                on:click={
                                    let tid = tid;
                                    move |_| selected.set(Some(tid.clone()))
                                }
                            >
                                <div class="flex items-center gap-2">
                                    <StatusDot status=status />
                                    <span class="text-sm text-text-primary truncate flex-1">{subject}</span>
                                    {if has_journal {
                                        let title = t_string!(i18n, teams.replay_journal_recorded).to_string();
                                        view! {
                                            <span
                                                class="text-xs px-1.5 rounded bg-info/10 text-info"
                                                title=title
                                            >"J"</span>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }}
                                </div>
                                <div class="text-xs text-text-tertiary mt-0.5 truncate">
                                    {owner}
                                </div>
                            </button>
                        }.into_any()
                    }).collect_view().into_any()
                }}
            </div>
        </div>
    }
}

// ─── Right pane ─────────────────────────────────────────────────────────

#[component]
fn TracePane(
    selected: RwSignal<Option<String>>,
    trace: RwSignal<Option<Value>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    on_refresh: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex-1 flex flex-col overflow-hidden aleph-md-detail">
            <div class="px-6 py-4 border-b border-border flex items-center justify-between aleph-content-top">
                <div>
                    <h1 class="text-lg font-semibold text-text-primary">{t!(i18n, teams.replay_audit_title)}</h1>
                    <p class="text-xs text-text-tertiary">
                        {t!(i18n, teams.replay_audit_desc)}
                    </p>
                </div>
                <button
                    class="px-3 py-1.5 text-sm rounded-md border border-border hover:bg-surface-raised disabled:opacity-50"
                    disabled=move || loading.get() || selected.get().is_none()
                    on:click=move |_| on_refresh.run(())
                >
                    {move || if loading.get() {
                        t_string!(i18n, teams.replay_refreshing).to_string()
                    } else {
                        t_string!(i18n, teams.replay_refresh).to_string()
                    }}
                </button>
            </div>
            <div class="flex-1 overflow-y-auto p-6 space-y-4">
                {move || {
                    if let Some(err) = error.get() {
                        let prefix = t_string!(i18n, teams.replay_load_failed).to_string();
                        return view! {
                            <div class="bg-red-50 border border-red-200 text-red-800 text-sm p-3 rounded-md">
                                {format!("{prefix} {err}")}
                            </div>
                        }.into_any();
                    }
                    if selected.get().is_none() {
                        return view! {
                            <div class="text-text-tertiary text-sm">
                                {t!(i18n, teams.replay_select_task)}
                            </div>
                        }.into_any();
                    }
                    let Some(v) = trace.get() else {
                        return view! {
                            <div class="text-text-tertiary text-sm">{t!(i18n, teams.replay_loading)}</div>
                        }.into_any();
                    };
                    let journal = v.get("journal")
                        .filter(|j| !j.is_null())
                        .and_then(|j| serde_json::from_value::<TaskExitJournalDto>(j.clone()).ok());
                    let rows = flatten_trace(&v);
                    view! {
                        {journal.map(|j| view! { <JournalChip j=j /> })}
                        {if rows.is_empty() {
                            view! {
                                <div class="text-text-tertiary text-sm">
                                    {t!(i18n, teams.replay_no_activity)}
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-2">
                                    {rows.into_iter()
                                        .map(|r| view! { <TimelineRowView r=r /> }.into_any())
                                        .collect_view()}
                                </div>
                            }.into_any()
                        }}
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// ─── Sub-components ─────────────────────────────────────────────────────

#[component]
fn StatusDot(status: String) -> impl IntoView {
    let color = match status.as_str() {
        "pending" => "bg-text-tertiary",
        "blocked" => "bg-warning",
        "in_progress" => "bg-info",
        "completed" => "bg-success",
        "failed" => "bg-danger",
        "cancelled" => "bg-text-tertiary",
        "paused" => "bg-warning/70",
        "waiting_review" => "bg-primary",
        "skipped" => "bg-text-tertiary",
        _ => "bg-text-tertiary",
    };
    view! { <span class=format!("inline-block w-2 h-2 rounded-full flex-shrink-0 {color}") /> }
}

#[component]
fn JournalChip(j: TaskExitJournalDto) -> impl IntoView {
    let i18n = use_i18n();
    let agent = j.agent_id.clone();
    let summary = j.summary.clone();
    let decisions = j.decisions.clone();
    let next_steps = j.next_steps.clone();
    let artifacts = j.artifacts_ref.clone();
    let confidence = j.confidence;
    let ts_label = format_timestamp(j.created_at as i64);
    let by_label = t_string!(i18n, teams.replay_journal_by).to_string();
    view! {
        <div class="bg-surface-raised border border-border rounded-lg p-4">
            <div class="flex items-center gap-2 mb-2 flex-wrap">
                <span class="text-xs font-semibold uppercase tracking-wider text-info">
                    {t!(i18n, teams.replay_journal_chip)}
                </span>
                <span class="text-xs text-text-tertiary">
                    {format!("{by_label} {agent} · {ts_label}")}
                </span>
                {confidence.map(|c| {
                    let conf_label = t_string!(i18n, teams.replay_journal_confidence).to_string();
                    view! {
                        <span class="ml-auto text-xs px-2 py-0.5 rounded bg-info/10 text-info">
                            {format!("{conf_label} {c}%")}
                        </span>
                    }
                })}
            </div>
            <p class="text-sm text-text-primary mb-3 whitespace-pre-wrap">{summary}</p>
            {(!decisions.is_empty()).then(|| view! {
                <div class="mb-2">
                    <div class="text-xs font-medium text-text-secondary mb-1">{t!(i18n, teams.replay_journal_decisions)}</div>
                    <ul class="list-disc list-inside text-xs text-text-secondary space-y-0.5">
                        {decisions.into_iter()
                            .map(|d| view! { <li>{d}</li> }.into_any())
                            .collect_view()}
                    </ul>
                </div>
            })}
            {(!next_steps.is_empty()).then(|| view! {
                <div class="mb-2">
                    <div class="text-xs font-medium text-text-secondary mb-1">{t!(i18n, teams.replay_journal_next_steps)}</div>
                    <ul class="list-disc list-inside text-xs text-text-secondary space-y-0.5">
                        {next_steps.into_iter()
                            .map(|d| view! { <li>{d}</li> }.into_any())
                            .collect_view()}
                    </ul>
                </div>
            })}
            {(!artifacts.is_empty()).then(|| view! {
                <div>
                    <div class="text-xs font-medium text-text-secondary mb-1">{t!(i18n, teams.replay_journal_artifacts)}</div>
                    <div class="flex flex-wrap gap-1">
                        {artifacts.into_iter()
                            .map(|a| view! {
                                <code class="text-xs px-1.5 py-0.5 rounded bg-surface-sunken text-text-secondary">
                                    {a}
                                </code>
                            }.into_any())
                            .collect_view()}
                    </div>
                </div>
            })}
        </div>
    }
}

#[component]
fn TimelineRowView(r: TimelineRow) -> impl IntoView {
    let ts_label = format_timestamp(r.ts as i64);
    let kind = r.kind;
    let badge_class = r.badge_class;
    let actor = r.actor;
    let title = r.title;
    let body = r.body;
    view! {
        <div class="flex gap-3 border-l-2 border-border pl-3 py-1">
            <div class="flex-shrink-0 w-20 text-xs text-text-tertiary tabular-nums pt-0.5">
                {ts_label}
            </div>
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2 flex-wrap">
                    <span class=format!(
                        "text-xs font-medium px-1.5 py-0.5 rounded uppercase tracking-wider {badge_class}"
                    )>{kind}</span>
                    <span class="text-xs text-text-tertiary">{actor}</span>
                    <span class="text-sm text-text-primary">{title}</span>
                </div>
                {body.map(|b| view! {
                    <p class="text-xs text-text-secondary mt-1 whitespace-pre-wrap break-words">
                        {b}
                    </p>
                })}
            </div>
        </div>
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────

fn flatten_trace(v: &Value) -> Vec<TimelineRow> {
    let mut rows: Vec<TimelineRow> = Vec::new();

    if let Some(runs) = v.get("runs").and_then(|x| x.as_array()) {
        for r in runs {
            let ts = r
                .get("started_at")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let agent = r
                .get("agent_id")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            let status = r.get("status").and_then(|x| x.as_str()).unwrap_or("?");
            let summary = r.get("summary").and_then(|x| x.as_str()).map(String::from);
            let error = r.get("error").and_then(|x| x.as_str()).map(String::from);
            let body = error.or(summary);
            rows.push(TimelineRow {
                ts,
                kind: "run",
                actor: agent,
                title: format!("run · {status}"),
                body,
                badge_class: match status {
                    "completed" => "bg-success/10 text-success",
                    "failed" | "timeout" => "bg-danger/10 text-danger",
                    "running" => "bg-info/10 text-info",
                    _ => "bg-surface-sunken text-text-secondary",
                },
            });
        }
    }

    if let Some(cs) = v.get("comments").and_then(|x| x.as_array()) {
        for c in cs {
            let ts = c
                .get("created_at")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let author = c
                .get("author")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            let body = c
                .get("body")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            rows.push(TimelineRow {
                ts,
                kind: "note",
                actor: author,
                title: "comment".to_string(),
                body: Some(body),
                badge_class: "bg-primary/10 text-primary",
            });
        }
    }

    if let Some(es) = v.get("events").and_then(|x| x.as_array()) {
        for e in es {
            let ts = e
                .get("timestamp")
                .and_then(|x| x.as_str())
                .and_then(rfc3339_to_epoch)
                .unwrap_or(0);
            let agent = e
                .get("agent_id")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            let etype = e
                .get("event_type")
                .and_then(|x| x.as_str())
                .unwrap_or("event")
                .to_string();
            rows.push(TimelineRow {
                ts,
                kind: "event",
                actor: agent,
                title: etype,
                body: None,
                badge_class: "bg-info/10 text-info",
            });
        }
    }

    if let Some(arts) = v.get("artifacts").and_then(|x| x.as_array()) {
        for a in arts {
            let ts = a
                .get("created_at")
                .and_then(|x| x.as_str())
                .and_then(rfc3339_to_epoch)
                .unwrap_or(0);
            let agent = a
                .get("agent_id")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            let title = a
                .get("title")
                .and_then(|x| x.as_str())
                .unwrap_or("artifact")
                .to_string();
            let atype = a
                .get("artifact_type")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string();
            rows.push(TimelineRow {
                ts,
                kind: "artifact",
                actor: agent,
                title: format!("{atype} · {title}"),
                body: None,
                badge_class: "bg-warning/10 text-warning",
            });
        }
    }

    rows.sort_by_key(|r| r.ts);
    rows
}

fn rfc3339_to_epoch(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp().max(0) as u64)
}

/// Format a UNIX epoch (seconds) as "MM/DD HH:MM" via the browser's Date API.
fn format_timestamp(ts: i64) -> String {
    if ts <= 0 {
        return "—".to_string();
    }
    let date = js_sys::Date::new_0();
    date.set_time((ts * 1000) as f64);
    let month = date.get_month() + 1;
    let day = date.get_date();
    let hours = date.get_hours();
    let minutes = date.get_minutes();
    format!("{month:02}/{day:02} {hours:02}:{minutes:02}")
}
