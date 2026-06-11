//! `KanbanBoard` — five-column responsive layout grouping tasks by derived status.

use super::column::KanbanColumn;
use crate::api::teams::CoordTaskDto;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

#[component]
#[must_use]
pub fn KanbanBoard(
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();

    let pending = Signal::derive(move || tasks_with_status(&tasks.get(), "pending"));
    // "unsatisfiable" is a refinement of blocked (a dependency terminally
    // failed); group it under the Blocked column so these tasks stay visible.
    let blocked = Signal::derive(move || {
        tasks
            .get()
            .iter()
            .filter(|t| t.status == "blocked" || t.status == "unsatisfiable")
            .cloned()
            .collect::<Vec<_>>()
    });
    let in_progress = Signal::derive(move || tasks_with_status(&tasks.get(), "in_progress"));
    let completed = Signal::derive(move || tasks_with_status(&tasks.get(), "completed"));
    let failed = Signal::derive(move || tasks_with_status(&tasks.get(), "failed"));
    let cancelled = Signal::derive(move || tasks_with_status(&tasks.get(), "cancelled"));

    let empty_label = move || t_string!(i18n, teams.kanban.empty_state.no_tasks).to_string();

    view! {
        <div class="grid gap-3 p-3 flex-1 overflow-auto"
             style="grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); align-items: stretch; min-height: 0;">
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.pending).to_string()
                tasks=pending
                on_card_click=on_card_click
                empty_label=empty_label()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.blocked).to_string()
                tasks=blocked
                on_card_click=on_card_click
                empty_label=empty_label()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.in_progress).to_string()
                tasks=in_progress
                on_card_click=on_card_click
                empty_label=empty_label()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.completed).to_string()
                tasks=completed
                on_card_click=on_card_click
                empty_label=empty_label()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.failed).to_string()
                tasks=failed
                on_card_click=on_card_click
                empty_label=empty_label()
            />
            <KanbanColumn
                title=t_string!(i18n, teams.kanban.columns.cancelled).to_string()
                tasks=cancelled
                on_card_click=on_card_click
                empty_label=empty_label()
            />
        </div>
    }
}

fn tasks_with_status(tasks: &[CoordTaskDto], status: &str) -> Vec<CoordTaskDto> {
    tasks
        .iter()
        .filter(|t| t.status == status)
        .cloned()
        .collect()
}
