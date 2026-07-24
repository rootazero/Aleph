//! `KanbanColumn` — a single status column: a toned header (dot + title +
//! count) over a vertical list of task cards, and a drop zone for drag-and-drop
//! status transitions.

use super::board::{DropRequest, KanbanDnd};
use super::board_columns::Tone;
use super::lifecycle::resolve_move;
use super::task_card::TaskCard;
use crate::api::teams::CoordTaskDto;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn KanbanColumn(
    /// Stored status key this column groups (matches `CoordTaskStatus::as_str`).
    col_status: &'static str,
    /// Colour tone for the header dot.
    tone: Tone,
    /// Column title — already localized by the caller.
    #[prop(into)]
    title: String,
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
    #[prop(into)] empty_label: String,
) -> impl IntoView {
    let dnd = expect_context::<KanbanDnd>();
    let count = move || tasks.get().len();

    // The confirm prompt wants the localized target label; keep a copy the drop
    // handler can move while `title` flows into the header view.
    let drop_label = title.clone();

    // Whether the in-flight drag (if any) can legally drop into this column.
    let can_drop = move || {
        dnd.dragging
            .get()
            .as_ref()
            .is_some_and(|d| resolve_move(&d.status, col_status).is_some())
    };
    let is_over = move || dnd.drag_over.get() == Some(col_status) && can_drop();

    let on_dragover = move |ev: web_sys::DragEvent| {
        if !can_drop() {
            return;
        }
        ev.prevent_default(); // mandatory for `drop` to fire
        if let Some(dt) = ev.data_transfer() {
            dt.set_drop_effect("move");
        }
        if dnd.drag_over.get_untracked() != Some(col_status) {
            dnd.drag_over.set(Some(col_status));
        }
    };
    let on_dragleave = move |_ev: web_sys::DragEvent| {
        if dnd.drag_over.get_untracked() == Some(col_status) {
            dnd.drag_over.set(None);
        }
    };
    let on_drop = move |ev: web_sys::DragEvent| {
        ev.prevent_default();
        dnd.drag_over.set(None);
        let Some(card) = dnd.dragging.get_untracked() else {
            return;
        };
        dnd.dragging.set(None);
        let Some(mv) = resolve_move(&card.status, col_status) else {
            return;
        };
        dnd.on_drop.run(DropRequest {
            task_id: card.id.clone(),
            subject: card.subject.clone(),
            to_col: col_status,
            to_label: drop_label.clone(),
            mv,
            destructive: mv.is_destructive(),
        });
    };

    // Base + drop-highlight classes. A droppable-but-not-hovered column shows a
    // dashed hint ring while a drag is in flight; the hovered one lifts.
    let container_class = move || {
        let base = "flex flex-col w-full min-w-0 border rounded-lg overflow-hidden transition-colors";
        if is_over() {
            format!("{base} bg-primary/5 border-primary")
        } else if can_drop() {
            format!("{base} bg-surface-sunken border-dashed border-border-strong")
        } else {
            format!("{base} bg-surface-sunken border-border")
        }
    };

    view! {
        <div
            class=container_class
            on:dragover=on_dragover
            on:dragleave=on_dragleave
            on:drop=on_drop
        >
            <div class="px-3 py-2 border-b border-border flex items-center gap-2">
                <span class=format!("w-2 h-2 rounded-full flex-shrink-0 {}", tone.dot_class())></span>
                <h3 class="text-xs font-semibold text-text-secondary uppercase tracking-wider truncate">
                    {title}
                </h3>
                <span class="text-xs text-text-tertiary ml-auto tabular-nums">{count}</span>
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
