//! `KanbanBoard` — responsive column layout grouping tasks by stored status.
//!
//! The column list, tones, and status→label mapping all come from
//! [`board_columns`], so the board and the stats chip bar can never drift.
//! Every stored `CoordTaskStatus` maps to exactly one column so no task is
//! silently dropped (`unsatisfiable` folds into Blocked).
//!
//! Cards are draggable; dropping onto another column routes through
//! [`super::lifecycle::resolve_move`] and hands a [`DropRequest`] up to the
//! view, which either applies it immediately or (for destructive moves)
//! confirms first. Drag state is shared with columns and cards via
//! [`KanbanDnd`] context to avoid prop-drilling.

use super::board_columns::{column_label, tasks_for_column, BOARD_COLUMNS};
use super::column::KanbanColumn;
use super::lifecycle::TaskMove;
use crate::api::teams::CoordTaskDto;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

/// The card currently being dragged. Held in `KanbanDnd::dragging` so columns
/// can test droppability and the drop handler can resolve the move without
/// round-tripping through `dataTransfer` (unreliable across the WASM boundary).
#[derive(Clone)]
pub struct DragCard {
    pub id: String,
    pub status: String,
    pub subject: String,
}

/// A resolved drop the board hands up to the view to apply (or confirm first).
#[derive(Clone)]
pub struct DropRequest {
    pub task_id: String,
    pub subject: String,
    pub to_col: &'static str,
    /// Localized target-column label, for the confirm prompt.
    pub to_label: String,
    pub mv: TaskMove,
    pub destructive: bool,
}

/// Shared drag-and-drop state, provided by the board and consumed by columns
/// and cards via context.
#[derive(Clone, Copy)]
pub struct KanbanDnd {
    pub dragging: RwSignal<Option<DragCard>>,
    /// Stored status of the column currently under the pointer (for highlight).
    pub drag_over: RwSignal<Option<&'static str>>,
    pub on_drop: Callback<DropRequest>,
}

#[component]
#[must_use]
pub fn KanbanBoard(
    tasks: Signal<Vec<CoordTaskDto>>,
    #[prop(into)] on_card_click: Callback<String>,
    #[prop(into)] on_move: Callback<DropRequest>,
) -> impl IntoView {
    let i18n = use_i18n();

    let dragging = RwSignal::new(None::<DragCard>);
    let drag_over = RwSignal::new(None::<&'static str>);
    provide_context(KanbanDnd {
        dragging,
        drag_over,
        on_drop: on_move,
    });

    let empty_label = move || t_string!(i18n, teams.kanban.empty_state.no_tasks).to_string();

    view! {
        <div class="grid gap-3 p-3 flex-1 overflow-auto"
             style="grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); align-items: stretch; min-height: 0;">
            {BOARD_COLUMNS.iter().map(|col| {
                let status = col.status;
                let tone = col.tone;
                let title = column_label(i18n, status);
                let col_tasks = Signal::derive(move || tasks_for_column(&tasks.get(), status));
                view! {
                    <KanbanColumn
                        col_status=status
                        tone=tone
                        title=title
                        tasks=col_tasks
                        on_card_click=on_card_click
                        empty_label=empty_label()
                    />
                }
            }).collect_view()}
        </div>
    }
}
