//! `TaskCard` — compact card rendered inside a `KanbanColumn`.
//!
//! Three concerns beyond display:
//! - **drag source**: draggable when the task has any legal move, publishing a
//!   [`DragCard`] into the shared [`KanbanDnd`] context on drag start;
//! - **priority / status affordances**: left accent bar, in-progress live dot,
//!   owner avatar, dependency + review badges;
//! - **hover quick-actions**: the one or two primary lifecycle moves, routed
//!   through the same [`DropRequest`] path as a drop so confirm-on-destructive
//!   applies uniformly.

use super::board::{DragCard, DropRequest, KanbanDnd};
use super::board_columns::column_label;
use super::format_relative_time;
use super::lifecycle::{action_label, is_draggable, primary_actions};
use crate::api::teams::CoordTaskDto;
use crate::i18n::use_i18n;
use leptos::prelude::*;

/// 1–2 char avatar initials for an owner id (e.g. "code-reviewer" → "CR").
fn initials(name: &str) -> String {
    let parts: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    match parts.as_slice() {
        [] => "?".to_string(),
        [one] => one.chars().take(2).collect::<String>().to_uppercase(),
        [first, second, ..] => {
            let a = first.chars().next().unwrap_or('?');
            let b = second.chars().next().unwrap_or('?');
            format!("{a}{b}").to_uppercase()
        }
    }
}

/// Pill background/text classes for a priority.
fn priority_pill_class(priority: &str) -> &'static str {
    match priority {
        "critical" => "bg-danger/20 text-danger",
        "high" => "bg-warning/20 text-warning",
        "low" => "bg-text-tertiary/20 text-text-tertiary",
        _ => "bg-info/20 text-info",
    }
}

/// Left accent-bar colour class for a priority (drives the card's `border-l`).
fn priority_accent_class(priority: &str) -> &'static str {
    match priority {
        "critical" => "border-l-danger",
        "high" => "border-l-warning",
        "low" => "border-l-text-tertiary",
        _ => "border-l-info",
    }
}

#[component]
#[must_use]
pub fn TaskCard(task: CoordTaskDto, #[prop(into)] on_click: Callback<String>) -> impl IntoView {
    let dnd = expect_context::<KanbanDnd>();
    let i18n = use_i18n();

    let status = task.status.clone();
    let priority = task.priority.clone();
    let subject = task.subject.clone();
    let owner = task.owner.clone();
    let created_at = task.created_at;
    let dep_count = task.dependencies.len();
    let is_review = status == "waiting_review";
    let is_running = status == "in_progress";
    let draggable = is_draggable(&status);

    let body_preview = if task.description.chars().count() > 80 {
        let head: String = task.description.chars().take(80).collect();
        format!("{head}…")
    } else {
        task.description.clone()
    };

    let accent = priority_accent_class(&priority);
    let priority_class = priority_pill_class(&priority);

    // --- Drag source ---------------------------------------------------------
    let drag_id = task.id.clone();
    let drag_status = status.clone();
    let drag_subject = subject.clone();
    let on_dragstart = move |ev: web_sys::DragEvent| {
        if let Some(dt) = ev.data_transfer() {
            dt.set_effect_allowed("move");
            let _ = dt.set_data("text/plain", &drag_id);
        }
        dnd.dragging.set(Some(DragCard {
            id: drag_id.clone(),
            status: drag_status.clone(),
            subject: drag_subject.clone(),
        }));
    };
    let on_dragend = move |_ev: web_sys::DragEvent| {
        dnd.dragging.set(None);
        dnd.drag_over.set(None);
    };

    // Dim the card while it is the one being dragged.
    let self_id = task.id.clone();
    let is_being_dragged = move || dnd.dragging.get().as_ref().is_some_and(|d| d.id == self_id);

    let card_class = move || {
        let base = "group relative p-3 rounded-lg border border-l-4 bg-surface hover:border-border-strong hover:shadow-sm cursor-pointer transition-all";
        if is_being_dragged() {
            format!("{base} {accent} opacity-40")
        } else {
            format!("{base} {accent}")
        }
    };

    let click_id = task.id.clone();

    // --- Hover quick-actions -------------------------------------------------
    let quick = primary_actions(&status);
    let quick_id = task.id.clone();
    let quick_subject = subject.clone();

    view! {
        <div
            class=card_class
            draggable=if draggable { "true" } else { "false" }
            on:dragstart=on_dragstart
            on:dragend=on_dragend
            on:click=move |_| on_click.run(click_id.clone())
        >
            <div class="flex items-start gap-1.5 mb-2">
                {is_running.then(|| view! {
                    <span class="w-1.5 h-1.5 mt-1 rounded-full bg-info animate-pulse flex-shrink-0"
                          title="in_progress"></span>
                })}
                <div class="text-sm font-medium text-text-primary min-w-0">{subject}</div>
            </div>
            <div class="flex items-center gap-2 flex-wrap text-xs">
                {owner.map(|o| {
                    let chip = initials(&o);
                    let title = o.clone();
                    view! {
                        <span class="flex items-center gap-1" title=title>
                            <span class="w-4 h-4 rounded bg-primary/15 text-primary text-[9px] font-semibold flex items-center justify-center flex-shrink-0">
                                {chip}
                            </span>
                            <span class="text-text-secondary truncate max-w-[7rem]">{o}</span>
                        </span>
                    }
                })}
                <span class=format!("px-2 py-0.5 rounded {priority_class}")>
                    {priority}
                </span>
                {is_review.then(|| view! {
                    <span class="px-2 py-0.5 rounded bg-primary/10 text-primary">"review"</span>
                })}
                {(dep_count > 0).then(|| view! {
                    <span class="px-1.5 py-0.5 rounded bg-surface-sunken text-text-tertiary"
                          title="dependencies">
                        {format!("⿻ {dep_count}")}
                    </span>
                })}
                <span class="text-text-tertiary ml-auto whitespace-nowrap">
                    {format_relative_time(created_at)}
                </span>
            </div>
            {(!body_preview.is_empty()).then(|| view! {
                <div class="mt-2 text-xs text-text-secondary">{body_preview}</div>
            })}
            {(!quick.is_empty()).then(|| {
                let qid = quick_id.clone();
                let qsubject = quick_subject.clone();
                view! {
                    <div class="mt-2 flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                        {quick.into_iter().map(|action| {
                            let label = action_label(i18n, action);
                            let target = action.target_column();
                            let to_label = column_label(i18n, target);
                            let qid = qid.clone();
                            let qsubject = qsubject.clone();
                            view! {
                                <button
                                    class="px-2 py-0.5 rounded text-[10px] bg-primary/10 text-primary hover:bg-primary/20 cursor-pointer"
                                    on:click=move |ev: web_sys::MouseEvent| {
                                        ev.stop_propagation();
                                        dnd.on_drop.run(DropRequest {
                                            task_id: qid.clone(),
                                            subject: qsubject.clone(),
                                            to_col: target,
                                            to_label: to_label.clone(),
                                            mv: action.to_move(),
                                            destructive: action.is_destructive(),
                                        });
                                    }
                                >
                                    {label}
                                </button>
                            }
                        }).collect_view()}
                    </div>
                }
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::initials;

    #[test]
    fn initials_handles_common_shapes() {
        assert_eq!(initials("code-reviewer"), "CR");
        assert_eq!(initials("alice"), "AL");
        assert_eq!(initials("a"), "A");
        assert_eq!(initials(""), "?");
        assert_eq!(initials("security_review_bot"), "SR");
    }
}
