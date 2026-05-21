//! TaskDetailDrawer — slide-out detail panel with status-transition actions.

use super::format_relative_time;
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

    // Transient state for the in-flight status mutation.
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let busy = RwSignal::new(false);

    // Reset transient action state whenever the drawer target changes so a
    // stale error from a previous task never bleeds into the next one.
    Effect::new(move |_| {
        let _ = open_for.get();
        error.set(None);
        busy.set(false);
    });

    let close = move |_| open_for.set(None);

    let patch_status = move |new_status: &'static str| {
        if busy.get_untracked() {
            return;
        }
        let Some(task) = open_for.get_untracked() else {
            return;
        };
        let id = task.id.clone();
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match TeamsApi::update_task(
                &dash,
                &id,
                TaskPatch {
                    status: Some(new_status.to_string()),
                    ..Default::default()
                },
            )
            .await
            {
                Ok(_) => {
                    busy.set(false);
                    on_changed.run(());
                    open_for.set(None);
                }
                Err(e) => {
                    // Keep the drawer open so the user sees what failed.
                    busy.set(false);
                    error.set(Some(e));
                }
            }
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
                let task_id = task.id.clone();
                let created = format_relative_time(task.created_at);
                let started = task.started_at.map(format_relative_time);
                let completed = task.completed_at.map(format_relative_time);
                let dep_count = task.dependencies.len();

                let status_label = t_string!(i18n, teams.kanban.field.status).to_string();
                let owner_label = t_string!(i18n, teams.kanban.field.owner).to_string();
                let priority_label = t_string!(i18n, teams.kanban.field.priority).to_string();
                let desc_label = t_string!(i18n, teams.kanban.field.description).to_string();
                let result_label = t_string!(i18n, teams.kanban.field.result).to_string();
                let created_label = t_string!(i18n, teams.kanban.field.created).to_string();
                let started_label = t_string!(i18n, teams.kanban.field.started).to_string();
                let completed_label = t_string!(i18n, teams.kanban.field.completed).to_string();
                let deps_label = t_string!(i18n, teams.kanban.field.dependencies).to_string();
                let start_label = t_string!(i18n, teams.kanban.actions.start).to_string();
                let complete_label = t_string!(i18n, teams.kanban.actions.complete).to_string();
                let fail_label = t_string!(i18n, teams.kanban.actions.fail).to_string();
                let cancel_label = t_string!(i18n, teams.kanban.actions.cancel).to_string();
                let err_prefix = t_string!(i18n, teams.kanban.error.update_failed).to_string();

                // The drawer only offers forward transitions; terminal states
                // lock every button. `busy` additionally locks them all while a
                // mutation is in flight.
                let st = status.clone();
                let start_locked =
                    matches!(st.as_str(), "in_progress" | "completed" | "failed" | "cancelled");
                let terminal_locked =
                    matches!(st.as_str(), "completed" | "failed" | "cancelled");
                let start_disabled = Signal::derive(move || busy.get() || start_locked);
                let complete_disabled = Signal::derive(move || busy.get() || terminal_locked);
                let fail_disabled = Signal::derive(move || busy.get() || terminal_locked);
                let cancel_disabled = Signal::derive(move || busy.get() || terminal_locked);

                view! {
                    <div class="fixed inset-0 z-40 flex justify-end">
                        <div class="absolute inset-0 bg-black/30" on:click=close></div>
                        <aside class="relative w-96 h-full bg-surface border-l border-border shadow-xl flex flex-col">
                            <header class="px-4 py-3 border-b border-border flex items-start justify-between gap-2">
                                <div class="min-w-0">
                                    <h3 class="text-sm font-semibold text-text-primary truncate">{subject}</h3>
                                    <p class="text-[10px] font-mono text-text-tertiary truncate mt-0.5">{task_id}</p>
                                </div>
                                <button class="text-text-tertiary hover:text-text-primary flex-shrink-0" on:click=close>
                                    "✕"
                                </button>
                            </header>
                            <div class="flex-1 overflow-y-auto p-4 space-y-3 text-sm">
                                <FieldRow label=status_label value=status />
                                <FieldRow label=owner_label value=owner />
                                <FieldRow label=priority_label value=priority />
                                <FieldRow label=created_label value=created />
                                {started.map(|v| view! { <FieldRow label=started_label value=v /> })}
                                {completed.map(|v| view! { <FieldRow label=completed_label value=v /> })}
                                {(dep_count > 0)
                                    .then(|| view! { <FieldRow label=deps_label value=dep_count.to_string() /> })}
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
                            {move || error.get().map(|e| view! {
                                <div class="mx-4 mb-2 px-3 py-2 rounded bg-danger/10 border border-danger/20 text-xs text-danger">
                                    <strong>{err_prefix.clone()}{": "}</strong>{e}
                                </div>
                            })}
                            <footer class="px-4 py-3 border-t border-border flex gap-2 flex-wrap">
                                <ActionButton
                                    label=start_label
                                    disabled=start_disabled
                                    on_click=move |_| patch_status("in_progress")
                                />
                                <ActionButton
                                    label=complete_label
                                    disabled=complete_disabled
                                    on_click=move |_| patch_status("completed")
                                />
                                <ActionButton
                                    label=fail_label
                                    disabled=fail_disabled
                                    on_click=move |_| patch_status("failed")
                                />
                                <ActionButton
                                    label=cancel_label
                                    disabled=cancel_disabled
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
    disabled: Signal<bool>,
    on_click: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    let class = move || {
        if disabled.get() {
            "px-3 py-1.5 rounded text-xs bg-surface-sunken text-text-tertiary cursor-not-allowed"
        } else {
            "px-3 py-1.5 rounded text-xs bg-primary/10 text-primary hover:bg-primary/20 cursor-pointer"
        }
    };
    view! {
        <button class=class on:click=on_click disabled=move || disabled.get()>
            {label}
        </button>
    }
}
