//! KanbanView — sub-view that mounts a board for the currently selected team,
//! a toolbar (search + create), and subscribes to `team.*.task.*` topic events
//! for live refresh.

use super::components::board::KanbanBoard;
use super::components::create_form::KanbanCreateForm;
use super::components::task_drawer::TaskDetailDrawer;
use super::TeamsTabState;
use crate::api::teams::{CoordTaskDto, TaskFilter, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn KanbanView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();

    let tasks: RwSignal<Vec<CoordTaskDto>> = RwSignal::new(Vec::new());
    let drawer: RwSignal<Option<CoordTaskDto>> = RwSignal::new(None);
    let search = RwSignal::new(String::new());
    let show_create = RwSignal::new(false);

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

    // Re-fetch whenever the active team changes.
    Effect::new(move |_| {
        let _ = state.selected_team_id.get();
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

    // Client-side filter: case-insensitive substring match on subject or owner.
    let filtered = Signal::derive(move || {
        let query = search.get().trim().to_lowercase();
        let all = tasks.get();
        if query.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|t| {
                t.subject.to_lowercase().contains(&query)
                    || t
                        .owner
                        .as_deref()
                        .is_some_and(|o| o.to_lowercase().contains(&query))
            })
            .collect()
    });

    let card_click = Callback::new(move |task_id: String| {
        if let Some(task) = tasks.get_untracked().into_iter().find(|t| t.id == task_id) {
            drawer.set(Some(task));
        }
    });

    let on_changed = Callback::new(move |()| refresh());

    view! {
        <div class="flex flex-col h-full">
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
                            <div class="flex items-center gap-2 px-3 pt-3">
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
                            <KanbanBoard tasks=filtered on_card_click=card_click />
                        </div>
                    }.into_any()
                }
            }}
            <TaskDetailDrawer open_for=drawer on_changed=on_changed />
            <KanbanCreateForm open=show_create on_created=on_changed />
        </div>
    }
}
