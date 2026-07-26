//! Bottom task strip for team chat: one tappable pill showing the most-salient
//! team task (`● tasks · {subject} · {status}  +N`). Hidden when the team has no
//! tasks. Lives in the composer's floating stack so it sits just above the
//! input box and is covered by the same `--composer-clearance` measurement.
//! Tapping toggles `TaskDrawerOpen` (consumed by `TeamTaskDrawer`).

use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::views::chat::state::ChatState;
use crate::views::chat::team_task_logic::{extra_task_count, most_salient_task, task_status_color};
// Single localized `CoordTaskStatus` → text table, shared with the kanban.
use crate::views::teams::components::board_columns::column_label;

/// Shared open-state for the team task drawer (set by the strip, read by the
/// drawer). Provided by `ChatView` (view.rs) — a common ancestor of both the
/// strip and the drawer — so `expect_context` resolves in both subtrees.
#[derive(Clone, Copy)]
pub struct TaskDrawerOpen(pub RwSignal<bool>);

#[component]
#[must_use]
pub fn TeamTaskStrip() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    // Open-state lives in ChatView's context (Step 4) so the sibling
    // TeamTaskDrawer reads the same signal. The strip only sets it.
    let TaskDrawerOpen(drawer_open) = expect_context::<TaskDrawerOpen>();

    view! {
        <Show when=move || {
            chat.team_id.get().is_some() && !chat.team_tasks.get().is_empty()
        }>
            <button
                type="button"
                class="w-full mb-2 flex items-center gap-2 px-3 py-1.5 rounded-full \
                       text-xs bg-surface-raised/70 backdrop-blur border border-border/60 \
                       hover:bg-surface-raised/90 transition-colors text-left"
                on:click=move |_| drawer_open.set(true)
            >
                {move || {
                    let tasks = chat.team_tasks.get();
                    let Some(top) = most_salient_task(&tasks) else {
                        return view! { <span></span> }.into_any();
                    };
                    let dot = task_status_color(&top.status);
                    let label = column_label(i18n, &top.status);
                    let subject = top.subject.clone();
                    let extra = extra_task_count(tasks.len());
                    view! {
                        <span style=format!("color: {dot};")>"●"</span>
                        <span class="opacity-60">{t_string!(i18n, common.team_tasks).to_string()}</span>
                        <span class="opacity-40">"·"</span>
                        <span class="font-medium truncate aleph-task-strip-subject">{subject}</span>
                        <span class="opacity-40">"·"</span>
                        <span class="opacity-70">{label}</span>
                        {extra.map(|n| view! {
                            <span class="ml-auto text-[10px] px-1.5 py-0.5 rounded-full \
                                         bg-border/40 opacity-70">{format!("+{n}")}</span>
                        })}
                    }
                    .into_any()
                }}
            </button>
        </Show>
    }
}

#[component]
#[must_use]
pub fn TeamTaskDrawer() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let TaskDrawerOpen(open) = expect_context::<TaskDrawerOpen>();

    view! {
        <Show when=move || open.get()>
            // Backdrop catcher — click outside closes.
            <div class="fixed inset-0 z-[80] bg-black/20" on:click=move |_| open.set(false)></div>
            // Slide-over panel.
            <div class="fixed top-0 right-0 bottom-0 z-[81] w-[320px] max-w-[85vw] \
                        bg-surface-raised/95 backdrop-blur border-l border-border \
                        shadow-xl flex flex-col aleph-no-drag"
                 data-tauri-drag-region="false">
                <div class="flex items-center justify-between px-4 py-3 border-b border-border">
                    <span class="text-sm font-semibold">
                        {move || t_string!(i18n, chat.team_tasks_title).to_string()}
                    </span>
                    <button
                        type="button"
                        class="text-xs opacity-60 hover:opacity-100"
                        on:click=move |_| open.set(false)
                    >"✕"</button>
                </div>
                <div class="flex-1 overflow-y-auto p-2 space-y-1">
                    {move || {
                        let tasks = chat.team_tasks.get();
                        if tasks.is_empty() {
                            return view! {
                                <div class="text-xs opacity-50 px-2 py-4 text-center">
                                    {t_string!(i18n, common.team_no_tasks).to_string()}
                                </div>
                            }.into_any();
                        }
                        tasks
                            .into_iter()
                            .map(|t| {
                                let dot = task_status_color(&t.status);
                                let label = column_label(i18n, &t.status);
                                view! {
                                    <div class="flex items-center gap-2 px-2 py-2 rounded \
                                                hover:bg-surface-sunken/40 text-xs">
                                        <span style=format!("color: {dot};")>"●"</span>
                                        <span class="flex-1 truncate">{t.subject}</span>
                                        <span class="text-[10px] opacity-60 shrink-0">{label}</span>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                            .into_any()
                    }}
                </div>
            </div>
        </Show>
    }
}
