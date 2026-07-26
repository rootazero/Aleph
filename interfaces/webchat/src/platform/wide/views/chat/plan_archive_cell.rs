//! `PlanArchiveCell` — a sunk (completed/superseded) scratchpad plan rendered
//! as a compact, click-to-expand capsule in the conversation flow. Pure
//! presentation (R4): the data comes from `ChatMessage.plan_archive`, projected
//! by `events.rs` from the model's scratchpad signals.

use leptos::prelude::*;

use super::plan::{archive_summary, PlanItemStatusView, PlanView};

#[component]
pub fn PlanArchiveCell(plan: PlanView) -> impl IntoView {
    let expanded = RwSignal::new(false);
    let (glyph, label) = archive_summary(&plan);
    let objective = plan.objective.clone().unwrap_or_default();
    let complete = plan.complete;
    let items = StoredValue::new(plan.items.clone());
    view! {
        <style>{ARCHIVE_CELL_CSS}</style>
        <div class="aleph-plan-cap" class:done=move || complete>
            <button class="aleph-plan-cap-head" on:click=move |_| expanded.update(|e| *e = !*e)>
                <span class="aleph-plan-cap-glyph">{glyph}</span>
                <span class="aleph-plan-cap-label">{label}</span>
                <span class="aleph-plan-cap-obj">{objective}</span>
                <span class="aleph-plan-cap-chev" class:open=move || expanded.get()>"▾"</span>
            </button>
            <Show when=move || expanded.get()>
                <ul class="aleph-plan-cap-rows">
                    <For
                        // Index, not (text, status): a plan may legitimately
                        // repeat a step's wording, and this archived list is
                        // immutable so positions never shift.
                        each=move || items.get_value().into_iter().enumerate()
                        key=|(i, _)| *i
                        let:entry
                    >
                        {
                            let it = entry.1;
                            let (cls, mark) = match it.status {
                                PlanItemStatusView::Completed => ("done", "✓"),
                                PlanItemStatusView::InProgress => ("active", "◗"),
                                PlanItemStatusView::Pending => ("pending", "·"),
                            };
                            view! {
                                <li class=format!("aleph-plan-cap-row {cls}")>
                                    <span class="aleph-plan-cap-box">{mark}</span>
                                    <span class="aleph-plan-cap-txt">{it.text.clone()}</span>
                                </li>
                            }
                        }
                    </For>
                </ul>
            </Show>
        </div>
    }
}

const ARCHIVE_CELL_CSS: &str = r#"
.aleph-plan-cap{max-width:1016px;margin:2px auto;border:1px solid var(--color-border);
  border-radius:12px;background:color-mix(in oklch,var(--color-surface-overlay) 88%,transparent);
  overflow:hidden;font-size:12.5px}
.aleph-plan-cap-head{display:flex;align-items:center;gap:8px;width:100%;padding:5px 12px;
  background:transparent;border:0;cursor:pointer;color:var(--color-text-secondary,#888);text-align:left}
.aleph-plan-cap-glyph{flex:0 0 auto;font-weight:700}
.aleph-plan-cap.done .aleph-plan-cap-glyph{color:var(--color-success)}
.aleph-plan-cap-label{flex:0 0 auto;font-weight:600;font-variant-numeric:tabular-nums}
.aleph-plan-cap-obj{flex:1 1 auto;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
  color:var(--color-text-primary);opacity:.75}
.aleph-plan-cap-chev{flex:0 0 auto;margin-left:auto;transition:transform .18s}
.aleph-plan-cap-chev.open{transform:rotate(180deg)}
.aleph-plan-cap-rows{list-style:none;margin:0;padding:2px 10px 8px}
.aleph-plan-cap-row{display:flex;align-items:flex-start;gap:8px;padding:3px 4px;line-height:1.4}
.aleph-plan-cap-row .aleph-plan-cap-box{flex:0 0 auto;width:15px;text-align:center}
.aleph-plan-cap-row.done .aleph-plan-cap-box{color:var(--color-success)}
.aleph-plan-cap-row.done .aleph-plan-cap-txt{text-decoration:line-through;opacity:.7}
"#;
