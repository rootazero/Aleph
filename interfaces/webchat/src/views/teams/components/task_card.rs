//! TaskCard — compact card rendered inside a KanbanColumn.

use crate::api::teams::CoordTaskDto;
use leptos::prelude::*;

#[component]
pub fn TaskCard(task: CoordTaskDto, #[prop(into)] on_click: Callback<String>) -> impl IntoView {
    let task_id = task.id.clone();
    let priority_class = match task.priority.as_str() {
        "critical" => "bg-danger/20 text-danger",
        "high" => "bg-warning/20 text-warning",
        "low" => "bg-text-tertiary/20 text-text-tertiary",
        _ => "bg-info/20 text-info",
    };
    let body_preview = if task.description.chars().count() > 80 {
        let head: String = task.description.chars().take(80).collect();
        format!("{head}…")
    } else {
        task.description.clone()
    };
    let owner_view = task.owner.clone();
    let priority_view = task.priority.clone();
    let subject_view = task.subject.clone();
    let created_at = task.created_at;

    view! {
        <div
            class="p-3 rounded-lg border border-border bg-surface hover:border-border-strong hover:shadow-sm cursor-pointer transition-all"
            on:click=move |_| on_click.run(task_id.clone())
        >
            <div class="text-sm font-medium text-text-primary mb-2">
                {subject_view}
            </div>
            <div class="flex items-center gap-2 flex-wrap text-xs">
                {
                    if let Some(owner) = owner_view {
                        view! {
                            <span class="px-2 py-0.5 rounded bg-primary/10 text-primary">
                                {owner}
                            </span>
                        }.into_any()
                    } else {
                        ().into_any()
                    }
                }
                <span class=format!("px-2 py-0.5 rounded {priority_class}")>
                    {priority_view}
                </span>
                <span class="text-text-tertiary ml-auto">
                    {format_relative_time(created_at)}
                </span>
            </div>
            {
                if !body_preview.is_empty() {
                    view! { <div class="mt-2 text-xs text-text-secondary">{body_preview}</div> }.into_any()
                } else {
                    ().into_any()
                }
            }
        </div>
    }
}

/// Format a unix-epoch (seconds) timestamp as a coarse human delta.
fn format_relative_time(epoch_secs: u64) -> String {
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let delta = now.saturating_sub(epoch_secs);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{} min ago", delta / 60)
    } else if delta < 86_400 {
        format!("{} h ago", delta / 3600)
    } else {
        format!("{} d ago", delta / 86_400)
    }
}
