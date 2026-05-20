//! TaskDetailDrawer — slide-out detail panel with status-transition actions.

use crate::api::teams::{CoordTaskDto, TaskPatch, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn TaskDetailDrawer(
    open_for: RwSignal<Option<CoordTaskDto>>,
    #[prop(into)] on_changed: Callback<()>,
) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let close = move |_| open_for.set(None);

    let patch_status = move |new_status: &'static str| {
        let Some(task) = open_for.get_untracked() else { return; };
        let id = task.id.clone();
        spawn_local(async move {
            let _ = TeamsApi::update_task(
                &dash,
                &id,
                TaskPatch {
                    status: Some(new_status.to_string()),
                    ..Default::default()
                },
            )
            .await;
            on_changed.run(());
            open_for.set(None);
        });
    };

    view! {
        {move || match open_for.get() {
            None => ().into_any(),
            Some(task) => {
                let status = task.status.clone();
                let owner = task.owner.clone().unwrap_or_default();
                let priority = task.priority.clone();
                let description = task.description.clone();
                let result = task.result.clone();
                let subject = task.subject.clone();
                let status_label = t_string!(i18n, teams.kanban.field.status).to_string();
                let owner_label = t_string!(i18n, teams.kanban.field.owner).to_string();
                let priority_label = t_string!(i18n, teams.kanban.field.priority).to_string();
                let desc_label = t_string!(i18n, teams.kanban.field.description).to_string();
                let result_label = t_string!(i18n, teams.kanban.field.result).to_string();
                let start_label = t_string!(i18n, teams.kanban.actions.start).to_string();
                let complete_label = t_string!(i18n, teams.kanban.actions.complete).to_string();
                let fail_label = t_string!(i18n, teams.kanban.actions.fail).to_string();
                let cancel_label = t_string!(i18n, teams.kanban.actions.cancel).to_string();
                let st_for_btns = status.clone();
                view! {
                    <div class="fixed inset-0 z-40 flex justify-end">
                        <div class="absolute inset-0 bg-black/30" on:click=close></div>
                        <aside class="relative w-96 h-full bg-surface border-l border-border shadow-xl flex flex-col">
                            <header class="px-4 py-3 border-b border-border flex items-center justify-between">
                                <h3 class="text-sm font-semibold text-text-primary">{subject}</h3>
                                <button class="text-text-tertiary hover:text-text-primary" on:click=close>
                                    "✕"
                                </button>
                            </header>
                            <div class="flex-1 overflow-y-auto p-4 space-y-3 text-sm">
                                <FieldRow label=status_label value=status />
                                <FieldRow label=owner_label value=owner />
                                <FieldRow label=priority_label value=priority />
                                <div>
                                    <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                                        {desc_label}
                                    </div>
                                    <div class="text-text-secondary whitespace-pre-wrap">
                                        {description}
                                    </div>
                                </div>
                                {
                                    if let Some(r) = result.filter(|s| !s.is_empty()) {
                                        view! {
                                            <div>
                                                <div class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1">
                                                    {result_label}
                                                </div>
                                                <div class="text-text-secondary whitespace-pre-wrap">{r}</div>
                                            </div>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }
                                }
                            </div>
                            <footer class="px-4 py-3 border-t border-border flex gap-2 flex-wrap">
                                <ActionButton
                                    label=start_label
                                    disabled=matches!(st_for_btns.as_str(), "in_progress" | "completed" | "failed" | "cancelled")
                                    on_click=move |_| patch_status("in_progress")
                                />
                                <ActionButton
                                    label=complete_label
                                    disabled=matches!(st_for_btns.as_str(), "completed" | "failed" | "cancelled")
                                    on_click=move |_| patch_status("completed")
                                />
                                <ActionButton
                                    label=fail_label
                                    disabled=matches!(st_for_btns.as_str(), "completed" | "failed" | "cancelled")
                                    on_click=move |_| patch_status("failed")
                                />
                                <ActionButton
                                    label=cancel_label
                                    disabled=matches!(st_for_btns.as_str(), "completed" | "failed" | "cancelled")
                                    on_click=move |_| patch_status("cancelled")
                                />
                            </footer>
                        </aside>
                    </div>
                }.into_any()
            }
        }}
    }
}

#[component]
fn FieldRow(label: String, value: String) -> impl IntoView {
    view! {
        <div class="flex items-baseline gap-2">
            <span class="text-xs text-text-tertiary uppercase tracking-wider w-20 flex-shrink-0">{label}</span>
            <span class="text-text-secondary">{value}</span>
        </div>
    }
}

#[component]
fn ActionButton(
    label: String,
    disabled: bool,
    on_click: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    let class = if disabled {
        "px-3 py-1.5 rounded text-xs bg-surface-sunken text-text-tertiary cursor-not-allowed"
    } else {
        "px-3 py-1.5 rounded text-xs bg-primary/10 text-primary hover:bg-primary/20 cursor-pointer"
    };
    view! {
        <button class=class on:click=on_click disabled=disabled>
            {label}
        </button>
    }
}
