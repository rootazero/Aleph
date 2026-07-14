//! Plan inspector surface — a run's task plan / scratchpad detail.
//!
//! Opened by the ↗ affordance on the inline `TodoPanel` header. Reads
//! `chat.plan` (the same `PlanView` the sticky Todo widget renders) and lays it
//! out fully in the side pane: objective, progress, and every item with its
//! status — without competing with the composer for the bottom slot. `plan` is
//! a single active-run signal, so `run_id` is carried for identity only.

use crate::i18n::{t, use_i18n};
use crate::views::chat::plan::PlanItemStatusView;
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// Per-run task-plan detail.
#[component]
pub fn PlanInspector(run_id: String) -> impl IntoView {
    // `plan` is single-current; run_id is identity only (see module doc).
    let _ = run_id;
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex items-center gap-2 pb-2 border-b border-border/60">
                <span class="text-base shrink-0">"🗒"</span>
                <span class="flex-1 text-sm text-text-primary font-medium">
                    {t!(i18n, common.inspector_plan_title)}
                </span>
            </div>
            {move || match chat.plan.get() {
                None => view! {
                    <p class="text-xs text-text-tertiary italic">
                        {t!(i18n, common.inspector_plan_empty)}
                    </p>
                }
                .into_any(),
                Some(plan) => {
                    let done = plan.done_count();
                    let total = plan.total();
                    let pct = plan.percent();
                    let objective = plan.objective.clone();
                    view! {
                        <div class="flex flex-col gap-2">
                            {objective.map(|o| view! {
                                <div class="text-xs">
                                    <span class="text-text-tertiary">
                                        {t!(i18n, common.inspector_objective)}": "
                                    </span>
                                    <span class="text-text-secondary break-words">{o}</span>
                                </div>
                            })}
                            <div class="text-[11px] text-text-tertiary tabular-nums">
                                {format!("{done}/{total} · {pct}%")}
                            </div>
                            <ul class="flex flex-col gap-1">
                                {plan.items.into_iter().map(|it| {
                                    let (glyph, cls) = match it.status {
                                        PlanItemStatusView::Completed => ("✓", "text-success"),
                                        PlanItemStatusView::InProgress => ("◗", "text-primary"),
                                        PlanItemStatusView::Pending => ("○", "text-text-tertiary"),
                                    };
                                    view! {
                                        <li class="flex items-start gap-2 text-xs">
                                            <span class=format!("shrink-0 {cls}")>{glyph}</span>
                                            <span class="text-text-secondary break-words">
                                                {it.text}
                                            </span>
                                        </li>
                                    }
                                }).collect::<Vec<_>>()}
                            </ul>
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}
