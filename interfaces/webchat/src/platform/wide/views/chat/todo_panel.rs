//! `TodoPanel` — single-chat sticky Todo widget (progress-ring card).
//!
//! Renders `ChatState.plan` as a collapsed progress-ring header (default) that
//! expands into a checklist; each completed item draws a ✓ and flashes. Hidden
//! when there is no active plan. Pure presentation (R4) — the plan is produced
//! by the LLM via the `scratchpad` tool (R7/R8).

use leptos::prelude::*;

use super::plan::{PlanItemStatusView, PlanView};
use super::state::ChatState;
use crate::i18n::{t_string, use_i18n};
use crate::state::inspector::InspectorTarget;
use crate::state::layout::WorkspaceState;

#[component]
pub fn TodoPanel() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    // Optional (absent in storybook): drives the ↗ open-in-panel affordance.
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();
    let expanded = RwSignal::new(false);

    let visible = move || {
        chat.plan
            .with(|p| p.as_ref().is_some_and(PlanView::has_content))
    };

    view! {
        <style>{TODO_PANEL_CSS}</style>
        <Show when=visible>
            {move || {
                let plan = chat.plan.get().expect("visible implies Some");
                let pct = plan.percent();
                let done = plan.done_count();
                let total = plan.total();
                let complete = plan.complete;
                let ring_style = format!(
                    "background: conic-gradient(var(--color-success) {pct}%, var(--color-border-subtle) 0);"
                );
                // A list between steps used to read "not started" at 4/5 done,
                // because only an explicit `[~]` produced a label. Fall back to
                // the next unfinished step.
                let header_label = if complete {
                    format!("\u{2713} {}", t_string!(i18n, chat.todo_done))
                } else if let Some(step) = plan.current_step() {
                    format!("{}{step}", t_string!(i18n, chat.todo_current))
                } else if let Some(step) = plan.focus_step() {
                    format!("{}{step}", t_string!(i18n, chat.todo_next))
                } else {
                    t_string!(i18n, chat.todo_not_started).to_string()
                };
                let title = t_string!(i18n, chat.todo_title).to_string();
                // The strip lives in the ResizeObserver-measured composer stack,
                // whose height pads the transcript. Without a cap a 20-item plan
                // shoves the whole conversation off-screen when expanded.
                let run_active = chat.active_run_id.get().is_some();
                view! {
                    <div
                        class="aleph-todo-wrap"
                        class:done=move || complete
                        // Freeze the in-progress pulse once the run is over:
                        // an unfinished plan stays mounted on purpose, but a
                        // perpetually animating "current step" reads as live
                        // work that is not happening.
                        class:settled=move || !run_active
                    >
                        // ── header row (always visible): toggle + ↗ open-in-panel ──
                        // Slim single line: 18px ring (same size as ContextGauge)
                        // + percentage to its right + one-line summary that
                        // ellipsis-truncates its tail. No 36px ring, no two-row meta.
                        <div style="display:flex;align-items:center">
                        <button
                            class="aleph-todo-head"
                            style="flex:1 1 auto;min-width:0"
                            on:click=move |_| expanded.update(|e| *e = !*e)
                        >
                            <span class="aleph-todo-ring" style=ring_style>
                                <span class="aleph-todo-ring-inner"></span>
                            </span>
                            <span class="aleph-todo-pct">{format!("{pct}%")}</span>
                            <span class="aleph-todo-line">
                                {format!("{title} · {done}/{total} · {header_label}")}
                            </span>
                            <span class="aleph-todo-chev" class:open=move || expanded.get()>"▾"</span>
                        </button>
                        // ↗ opens the plan inspector surface in the right pane.
                        {workspace.is_some().then(|| view! {
                            <button
                                style="flex:0 0 auto;padding:5px 10px;background:transparent;\
                                       border:0;cursor:pointer;color:var(--color-text-secondary);\
                                       font-size:13px"
                                title=move || t_string!(i18n, common.inspector_open).to_string()
                                on:click=move |_: web_sys::MouseEvent| {
                                    if let Some(ws) = workspace {
                                        let run_id = chat.active_run_id.get_untracked()
                                            .unwrap_or_default();
                                        ws.inspect(InspectorTarget::Plan { run_id });
                                    }
                                }
                            >
                                "\u{2197}"
                            </button>
                        })}
                        </div>
                        // ── checklist (expanded only) ──
                        <Show when=move || expanded.get()>
                            <ul class="aleph-todo-rows">
                                <For
                                    // Keyed by position, not (text, status):
                                    // a plan may legitimately repeat a step's
                                    // wording, and duplicate keys make a keyed
                                    // <For> drop rows.
                                    each=move || chat.plan.get()
                                        .map(|p| p.items)
                                        .unwrap_or_default()
                                        .into_iter()
                                        .enumerate()
                                    key=|(i, it)| (*i, it.status)
                                    let:entry
                                >
                                    {
                                        let it = entry.1;
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
.aleph-todo-wrap{margin:0 auto 6px;max-width:1016px;border:1px solid var(--color-border);
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
.aleph-todo-rows{list-style:none;margin:0;padding:4px 8px 8px;max-height:38vh;overflow-y:auto;
  overscroll-behavior:contain}
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
.aleph-todo-wrap.settled .aleph-todo-row.active .aleph-todo-box::after{animation:none;opacity:.55}
.aleph-todo-row.active .aleph-todo-txt{font-weight:600}
@keyframes aleph-todo-draw{from{transform:scale(.5);opacity:.3}to{transform:scale(1);opacity:1}}
@keyframes aleph-todo-flash{0%{background:var(--color-success-subtle)}100%{background:transparent}}
@keyframes aleph-todo-pulse{0%,100%{opacity:.4;transform:scale(.7)}50%{opacity:1;transform:scale(1)}}
"#;
