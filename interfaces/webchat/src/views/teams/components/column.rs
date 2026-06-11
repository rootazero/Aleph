//! KanbanColumn — a single status column rendering a vertical list of task cards.

use super::task_card::TaskCard;
use crate::api::teams::CoordTaskDto;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn KanbanColumn(
    /// Column title — already localized by the caller.
    #[prop(into)]
    title: String,
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
    #[prop(into)] empty_label: String,
) -> impl IntoView {
    let count = move || tasks.get().len();

    view! {
        <div class="flex flex-col w-full min-w-0 bg-surface-sunken border border-border rounded-lg overflow-hidden">
            <div class="px-3 py-2 border-b border-border flex items-center justify-between">
                <h3 class="text-xs font-semibold text-text-secondary uppercase tracking-wider">
                    {title}
                </h3>
                <span class="text-xs text-text-tertiary">{count}</span>
            </div>
            <div class="flex-1 overflow-y-auto p-2 space-y-2 min-h-0">
                {move || {
                    let list = tasks.get();
                    if list.is_empty() {
                        view! {
                            <div class="text-xs text-text-tertiary text-center py-6">
                                {empty_label.clone()}
                            </div>
                        }.into_any()
                    } else {
                        list.into_iter()
                            .map(|task| view! { <TaskCard task=task on_click=on_card_click /> })
                            .collect_view()
                            .into_any()
                    }
                }}
            </div>
        </div>
    }
}
