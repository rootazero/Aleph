//! `TodoPanel` — single-chat sticky Todo widget (progress-ring card).
//!
//! Renders `ChatState.plan` as a collapsed progress-ring header (default) that
//! expands into a checklist; each completed item draws a ✓ and flashes. Hidden
//! when there is no active plan. Pure presentation (R4) — the plan is produced
//! by the LLM via the `scratchpad` tool (R7/R8).

use leptos::prelude::*;

use super::plan::{PlanItemStatusView, PlanView};
use super::state::ChatState;

#[component]
pub fn TodoPanel() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let expanded = RwSignal::new(false);

    let visible = move || chat.plan.with(|p| p.as_ref().is_some_and(PlanView::has_content));

    view! {
        <style>{TODO_PANEL_CSS}</style>
        <Show when=visible>
            {move || {
                let plan = chat.plan.get().expect("visible implies Some");
                let pct = plan.percent();
                let done = plan.done_count();
                let total = plan.total();
                let current = plan.current_step().map(str::to_string);
                let complete = plan.complete;
                let ring_style = format!(
                    "background: conic-gradient(var(--color-success) {pct}%, var(--color-border-subtle) 0);"
                );
                let header_label = current
                    .clone()
                    .map(|c| format!("正在：{c}"))
                    .unwrap_or_else(|| if complete { "已完成".into() } else { "待开始".into() });
                view! {
                    <div class="aleph-todo-wrap" class:done=move || complete>
                        // ── header (always visible) — click to toggle ──
                        // Slim single line: 18px ring (same size as ContextGauge)
                        // + percentage to its right + one-line summary that
                        // ellipsis-truncates its tail. No 36px ring, no two-row meta.
                        <button
                            class="aleph-todo-head"
                            on:click=move |_| expanded.update(|e| *e = !*e)
                        >
                            <span class="aleph-todo-ring" style=ring_style>
                                <span class="aleph-todo-ring-inner"></span>
                            </span>
                            <span class="aleph-todo-pct">{format!("{pct}%")}</span>
                            <span class="aleph-todo-line">
                                {format!("任务计划 · {done}/{total} · {header_label}")}
                            </span>
                            <span class="aleph-todo-chev" class:open=move || expanded.get()>"▾"</span>
                        </button>
                        // ── checklist (expanded only) ──
                        <Show when=move || expanded.get()>
                            <ul class="aleph-todo-rows">
                                <For
                                    each=move || chat.plan.get().map(|p| p.items).unwrap_or_default()
                                    key=|it| (it.text.clone(), it.status.clone())
                                    let:it
                                >
                                    {
                                        let (cls, glyph) = match it.status {
                                            PlanItemStatusView::Completed => ("done", "✓"),
                                            PlanItemStatusView::InProgress => ("active", ""),
                                            PlanItemStatusView::Pending => ("pending", ""),
                                        };
                                        view! {
                                            <li class=format!("aleph-todo-row {cls}")>
                                                <span class="aleph-todo-box">{glyph}</span>
                                                <span class="aleph-todo-txt">{it.text.clone()}</span>
                                            </li>
                                        }
                                    }
                                </For>
                            </ul>
                        </Show>
                    </div>
                }
            }}
        </Show>
    }
}

/// Self-contained styles (OKLCH design tokens; check-draw + flash animations).
const TODO_PANEL_CSS: &str = r#"
.aleph-todo-wrap{margin:0 auto 6px;max-width:760px;border:1px solid var(--color-border);
  border-radius:14px;background:color-mix(in oklch,var(--color-surface-overlay) 92%,transparent);
  backdrop-filter:blur(8px);overflow:hidden;font-size:13px}
.aleph-todo-head{display:flex;align-items:center;gap:8px;width:100%;padding:5px 12px;
  background:transparent;border:0;cursor:pointer;color:var(--color-text-primary);text-align:left;font-size:13px}
.aleph-todo-ring{flex:0 0 auto;width:18px;height:18px;border-radius:50%;display:grid;place-items:center}
.aleph-todo-ring-inner{width:12px;height:12px;border-radius:50%;background:var(--color-surface-raised)}
.aleph-todo-pct{flex:0 0 auto;font-size:11px;font-weight:700;font-variant-numeric:tabular-nums;
  color:var(--color-text-secondary,#888)}
.aleph-todo-line{flex:1 1 auto;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
  color:var(--color-text-primary)}
.aleph-todo-chev{flex:0 0 auto;margin-left:auto;font-size:11px;transition:transform .18s;
  color:var(--color-text-secondary,#888)}
.aleph-todo-chev.open{transform:rotate(180deg)}
.aleph-todo-rows{list-style:none;margin:0;padding:4px 8px 8px}
.aleph-todo-row{display:flex;align-items:flex-start;gap:10px;padding:6px 8px;border-radius:9px;line-height:1.45}
.aleph-todo-box{flex:0 0 auto;width:17px;height:17px;border-radius:6px;border:1.6px solid var(--color-border);
  display:grid;place-items:center;margin-top:1px;font-size:11px;color:#fff}
.aleph-todo-row.done .aleph-todo-box{background:var(--color-success);border-color:var(--color-success);
  animation:aleph-todo-draw .4s ease-out}
.aleph-todo-row.done{animation:aleph-todo-flash 1.1s ease-out}
.aleph-todo-row.done .aleph-todo-txt{color:var(--color-text-secondary,#888);text-decoration:line-through}
.aleph-todo-row.active{background:var(--color-primary-subtle)}
.aleph-todo-row.active .aleph-todo-box{border-color:var(--color-primary)}
.aleph-todo-row.active .aleph-todo-box::after{content:"";width:9px;height:9px;border-radius:3px;
  background:var(--color-primary);animation:aleph-todo-pulse 1.2s ease-in-out infinite}
.aleph-todo-row.active .aleph-todo-txt{font-weight:600}
@keyframes aleph-todo-draw{from{transform:scale(.5);opacity:.3}to{transform:scale(1);opacity:1}}
@keyframes aleph-todo-flash{0%{background:var(--color-success-subtle)}100%{background:transparent}}
@keyframes aleph-todo-pulse{0%,100%{opacity:.4;transform:scale(.7)}50%{opacity:1;transform:scale(1)}}
"#;
