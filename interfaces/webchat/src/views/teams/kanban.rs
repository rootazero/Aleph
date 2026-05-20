//! KanbanView — sub-view that mounts a board for the currently selected team
//! and subscribes to `team.*.task.*` topic events for live refresh.

use super::components::board::KanbanBoard;
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

    let card_click = Callback::new(move |task_id: String| {
        if let Some(task) = tasks.get_untracked().into_iter().find(|t| t.id == task_id) {
            drawer.set(Some(task));
        }
    });

    let drawer_changed = Callback::new(move |()| refresh());

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
                    view! { <KanbanBoard tasks=tasks.into() on_card_click=card_click /> }.into_any()
                }
            }}
            <TaskDetailDrawer open_for=drawer on_changed=drawer_changed />
        </div>
    }
}
