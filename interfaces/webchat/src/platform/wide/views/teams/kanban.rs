//! `KanbanView` — sub-view that mounts a board for the currently selected team,
//! a toolbar (search + create + status-filter chips), and subscribes to
//! `team.*.task.*` topic events for live refresh. Owns the drag-drop apply
//! path: it turns a board [`DropRequest`] into a backend move via the shared
//! `lifecycle::apply_move`, confirming first for destructive drops.

use super::components::board::{DropRequest, KanbanBoard};
use super::components::board_columns::{
    column_label, column_matches, count_for_column, BOARD_COLUMNS,
};
use super::components::create_form::KanbanCreateForm;
use super::components::lifecycle::apply_move;
use super::components::task_drawer::TaskDetailDrawer;
use super::TeamsTabState;
use crate::api::teams::{CoordTaskDto, TaskFilter, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
#[must_use]
pub fn KanbanView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();

    let tasks: RwSignal<Vec<CoordTaskDto>> = RwSignal::new(Vec::new());
    let drawer: RwSignal<Option<CoordTaskDto>> = RwSignal::new(None);
    let search = RwSignal::new(String::new());
    let show_create = RwSignal::new(false);
    // P2: click a stats chip to filter the board to that status column.
    let status_filter: RwSignal<Option<&'static str>> = RwSignal::new(None);
    // Destructive drops park here awaiting an in-app confirm.
    let pending_confirm: RwSignal<Option<DropRequest>> = RwSignal::new(None);
    // Surfaces a failed drag/quick-action move without touching the drawer.
    let move_error: RwSignal<Option<String>> = RwSignal::new(None);

    // Fetch tasks for the currently-selected team.
    let refresh = move || {
        let Some(team_id) = state.selected_team_id.get_untracked() else {
            tasks.set(Vec::new());
            return;
        };
        spawn_local(async move {
            if let Ok(list) = TeamsApi::list_tasks(&dash, &team_id, TaskFilter::default()).await {
                tasks.set(list);
            }
        });
    };

    // Apply a resolved move to the backend, then refresh. Shared by the
    // non-destructive drop path and the confirm dialog.
    let apply_req = move |req: DropRequest| {
        let task_id = req.task_id.clone();
        let mv = req.mv;
        spawn_local(async move {
            match apply_move(&dash, &task_id, mv).await {
                Ok(()) => refresh(),
                Err(e) => move_error.set(Some(e)),
            }
        });
    };

    // Board → view drop handler: confirm destructive moves, apply the rest.
    let on_move = Callback::new(move |req: DropRequest| {
        move_error.set(None);
        if req.destructive {
            pending_confirm.set(Some(req));
        } else {
            apply_req(req);
        }
    });

    // Re-fetch whenever the active team changes.
    Effect::new(move |_| {
        let _ = state.selected_team_id.get();
        status_filter.set(None);
        refresh();
    });

    // Ask the gateway to push us `team.*.task.*` events.
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic("team.*.task.*").await;
        });
    });

    // React to topic events for the active team.
    let sub_id = dash.subscribe_events(move |evt| {
        let Some(active) = state.selected_team_id.get_untracked() else {
            return;
        };
        let topic = evt.topic.as_str();
        if topic.starts_with("team.") && topic.contains(".task.") {
            let parts: Vec<&str> = topic.splitn(4, '.').collect();
            if parts.len() >= 3 && parts[1] == active {
                refresh();
            }
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));

    // Client-side filter: text (subject/owner) AND the optional status chip.
    let filtered = Signal::derive(move || {
        let query = search.get().trim().to_lowercase();
        let sf = status_filter.get();
        tasks
            .get()
            .into_iter()
            .filter(|t| {
                let status_ok = sf.is_none_or(|s| column_matches(&t.status, s));
                let query_ok = query.is_empty()
                    || t.subject.to_lowercase().contains(&query)
                    || t.owner
                        .as_deref()
                        .is_some_and(|o| o.to_lowercase().contains(&query));
                status_ok && query_ok
            })
            .collect::<Vec<_>>()
    });

    let card_click = Callback::new(move |task_id: String| {
        if let Some(task) = tasks.get_untracked().into_iter().find(|t| t.id == task_id) {
            drawer.set(Some(task));
        }
    });

    let on_changed = Callback::new(move |()| refresh());

    view! {
        <div class="flex flex-col h-full aleph-content-top">
            {move || {
                if state.teams.get().is_empty() {
                    view! {
                        <div class="flex-1 flex items-center justify-center text-text-tertiary text-sm">
                            {t_string!(i18n, teams.kanban.empty_state.no_teams).to_string()}
                        </div>
                    }.into_any()
                } else if state.selected_team_id.get().is_none() {
                    view! {
                        <div class="flex-1 flex items-center justify-center text-text-tertiary text-sm">
                            {t_string!(i18n, teams.kanban.empty_state.pick_a_team).to_string()}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="flex flex-col flex-1 min-h-0">
                            <div class="flex items-center gap-2 px-3 pt-0">
                                <input
                                    class="flex-1 px-2 py-1.5 rounded bg-surface-sunken border border-border text-sm text-text-primary focus:outline-none focus:border-border-strong"
                                    type="text"
                                    placeholder=move || t_string!(i18n, teams.kanban.search_placeholder).to_string()
                                    prop:value=move || search.get()
                                    on:input=move |ev| search.set(event_target_value(&ev))
                                />
                                <button
                                    class="px-3 py-1.5 rounded text-xs font-medium bg-primary text-white hover:bg-primary/90 cursor-pointer flex-shrink-0"
                                    on:click=move |_| show_create.set(true)
                                >
                                    {move || t_string!(i18n, teams.kanban.actions.new_task).to_string()}
                                </button>
                            </div>
                            <StatsChips tasks=tasks status_filter=status_filter />
                            {move || move_error.get().map(|e| view! {
                                <div class="mx-3 mb-2 px-3 py-2 rounded bg-danger/10 border border-danger/20 text-xs text-danger flex items-center gap-2">
                                    <span class="flex-1">{e}</span>
                                    <button
                                        class="text-danger/70 hover:text-danger cursor-pointer"
                                        on:click=move |_| move_error.set(None)
                                    >"✕"</button>
                                </div>
                            })}
                            <KanbanBoard tasks=filtered on_card_click=card_click on_move=on_move />
                        </div>
                    }.into_any()
                }
            }}
            <TaskDetailDrawer open_for=drawer on_changed=on_changed />
            <KanbanCreateForm open=show_create on_created=on_changed />
            <ConfirmMoveDialog
                pending=pending_confirm
                on_confirm=Callback::new(move |req: DropRequest| {
                    apply_req(req);
                    pending_confirm.set(None);
                })
            />
        </div>
    }
}

// ---------------------------------------------------------------------------
// ConfirmMoveDialog — in-app confirmation for a destructive drag/quick-action
// (completed / failed / cancelled / skip). Avoids `window.confirm`'s blocking
// modal, matching the create-form overlay pattern.
// ---------------------------------------------------------------------------

#[component]
fn ConfirmMoveDialog(
    pending: RwSignal<Option<DropRequest>>,
    #[prop(into)] on_confirm: Callback<DropRequest>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || pending.get().map(|req| {
            let subject = req.subject.clone();
            let to_label = req.to_label.clone();
            let req_apply = req.clone();
            view! {
                <div class="fixed inset-0 z-50 flex items-center justify-center">
                    <div class="absolute inset-0 bg-black/30" on:click=move |_| pending.set(None)></div>
                    <div class="glass relative w-80 max-w-[90vw] rounded-lg border border-border bg-surface-overlay/90 shadow-xl p-4 flex flex-col gap-3">
                        <p class="text-sm text-text-primary">
                            {t_string!(i18n, teams.kanban.confirm.move_prompt).to_string()}
                        </p>
                        <div class="text-xs text-text-secondary">
                            <span class="font-medium text-text-primary">{subject}</span>
                            " → "
                            <span class="font-medium text-text-primary">{to_label}</span>
                        </div>
                        <div class="flex justify-end gap-2 mt-1">
                            <button
                                class="px-3 py-1.5 rounded text-xs bg-surface-sunken text-text-secondary hover:bg-surface cursor-pointer"
                                on:click=move |_| pending.set(None)
                            >
                                {t_string!(i18n, teams.kanban.form.cancel).to_string()}
                            </button>
                            <button
                                class="px-3 py-1.5 rounded text-xs bg-danger/15 text-danger hover:bg-danger/25 cursor-pointer"
                                on:click=move |_| on_confirm.run(req_apply.clone())
                            >
                                {t_string!(i18n, teams.kanban.confirm.confirm).to_string()}
                            </button>
                        </div>
                    </div>
                </div>
            }
        })}
    }
}

// ---------------------------------------------------------------------------
// StatsChips — per-status count bar above the board. Iterates the SAME
// `BOARD_COLUMNS` the grid renders (single source), so counts can never miss a
// status again. Each chip toggles the board's status filter.
// ---------------------------------------------------------------------------

#[component]
fn StatsChips(
    tasks: RwSignal<Vec<CoordTaskDto>>,
    status_filter: RwSignal<Option<&'static str>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex gap-2 px-3 py-2 overflow-x-auto text-xs">
            {move || {
                let snapshot = tasks.get();
                let total = snapshot.len();
                let active = status_filter.get();
                let mut nodes: Vec<_> = BOARD_COLUMNS.iter().map(|col| {
                    let status = col.status;
                    let count = count_for_column(&snapshot, status);
                    let label = column_label(i18n, status);
                    let title = label.clone();
                    let is_active = active == Some(status);
                    let base = col.tone.chip_class();
                    let ring = if is_active { " ring-1 ring-inset ring-current" } else { "" };
                    view! {
                        <button
                            class=format!("px-2 py-1 rounded flex items-center gap-1.5 flex-shrink-0 cursor-pointer {base}{ring}")
                            title=title
                            on:click=move |_| {
                                status_filter.update(|f| *f = if *f == Some(status) { None } else { Some(status) });
                            }
                        >
                            <span class="font-semibold tabular-nums">{count}</span>
                            <span class="opacity-75">{label}</span>
                        </button>
                    }.into_any()
                }).collect();
                nodes.push(view! {
                    <div class="px-2 py-1 rounded flex items-center gap-1.5 flex-shrink-0 bg-primary/10 text-primary ml-auto">
                        <span class="font-semibold tabular-nums">{total}</span>
                        <span class="opacity-75">"total"</span>
                    </div>
                }.into_any());
                nodes
            }}
        </div>
    }
}
