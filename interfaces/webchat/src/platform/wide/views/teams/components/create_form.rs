//! `KanbanCreateForm` — modal dialog for creating a task on the active board.
//!
//! The store emits a `team.<id>.task.created` topic on success, so the board
//! refreshes itself; `on_created` is still invoked for an immediate refresh in
//! case the event is missed.

use super::super::TeamsTabState;
use crate::api::teams::{NewTaskInput, TeamsApi};
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
#[must_use]
pub fn KanbanCreateForm(
    open: RwSignal<bool>,
    #[prop(into)] on_created: Callback<()>,
) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let state = expect_context::<TeamsTabState>();

    let subject = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let owner = RwSignal::new(String::new());
    let priority = RwSignal::new("normal".to_string());
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let busy = RwSignal::new(false);

    let reset = move || {
        subject.set(String::new());
        description.set(String::new());
        owner.set(String::new());
        priority.set("normal".to_string());
        error.set(None);
        busy.set(false);
    };

    let close = move || {
        reset();
        open.set(false);
    };

    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        let subj = subject.get_untracked().trim().to_string();
        if subj.is_empty() {
            error.set(Some(
                t_string!(i18n, teams.kanban.form.subject_required).to_string(),
            ));
            return;
        }
        // The toolbar only shows this form when a team is selected; bail
        // quietly if that invariant is somehow broken.
        let Some(team_id) = state.selected_team_id.get_untracked() else {
            return;
        };

        let input = NewTaskInput {
            subject: subj,
            description: description.get_untracked().trim().to_string(),
            owner: {
                let o = owner.get_untracked().trim().to_string();
                (!o.is_empty()).then_some(o)
            },
            priority: Some(priority.get_untracked()),
        };

        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match TeamsApi::create_task(&dash, &team_id, input).await {
                Ok(_) => {
                    reset();
                    open.set(false);
                    on_created.run(());
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                }
            }
        });
    };

    view! {
        {move || {
            if !open.get() {
                return ().into_any();
            }

            let title = t_string!(i18n, teams.kanban.form.title).to_string();
            let subject_label = t_string!(i18n, teams.kanban.form.subject).to_string();
            let subject_ph = t_string!(i18n, teams.kanban.form.subject_placeholder).to_string();
            let desc_label = t_string!(i18n, teams.kanban.form.description).to_string();
            let desc_ph = t_string!(i18n, teams.kanban.form.description_placeholder).to_string();
            let owner_label = t_string!(i18n, teams.kanban.form.owner).to_string();
            let owner_ph = t_string!(i18n, teams.kanban.form.owner_placeholder).to_string();
            let priority_label = t_string!(i18n, teams.kanban.form.priority).to_string();
            let create_label = t_string!(i18n, teams.kanban.form.create).to_string();
            let cancel_label = t_string!(i18n, teams.kanban.form.cancel).to_string();
            let err_prefix = t_string!(i18n, teams.kanban.error.create_failed).to_string();
            let p_low = t_string!(i18n, teams.kanban.priority.low).to_string();
            let p_normal = t_string!(i18n, teams.kanban.priority.normal).to_string();
            let p_high = t_string!(i18n, teams.kanban.priority.high).to_string();
            let p_critical = t_string!(i18n, teams.kanban.priority.critical).to_string();

            let input_class =
                "w-full px-2 py-1.5 rounded bg-surface-sunken border border-border text-sm \
                 text-text-primary focus:outline-none focus:border-border-strong";
            let field_label_class =
                "block text-xs font-medium text-text-tertiary uppercase tracking-wider mb-1";

            view! {
                <div class="fixed inset-0 z-50 flex items-center justify-center">
                    <div class="aleph-scrim absolute inset-0 bg-black/40" on:click=move |_| close()></div>
                    <div class="glass relative w-[26rem] max-w-[92vw] max-h-[90vh] bg-surface-overlay/85 border border-border rounded-xl shadow-xl flex flex-col">
                        <header class="px-4 py-3 border-b border-border flex items-center justify-between">
                            <h3 class="text-sm font-semibold text-text-primary">{title}</h3>
                            <button
                                class="text-text-tertiary hover:text-text-primary"
                                on:click=move |_| close()
                            >
                                "✕"
                            </button>
                        </header>
                        <div class="flex-1 overflow-y-auto p-4 space-y-3">
                            <div>
                                <label class=field_label_class>{subject_label}</label>
                                <input
                                    class=input_class
                                    type="text"
                                    placeholder=subject_ph
                                    prop:value=move || subject.get()
                                    on:input=move |ev| subject.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class=field_label_class>{desc_label}</label>
                                <textarea
                                    class=format!("{input_class} resize-y")
                                    rows="3"
                                    placeholder=desc_ph
                                    prop:value=move || description.get()
                                    on:input=move |ev| description.set(event_target_value(&ev))
                                ></textarea>
                            </div>
                            <div>
                                <label class=field_label_class>{owner_label}</label>
                                <input
                                    class=input_class
                                    type="text"
                                    placeholder=owner_ph
                                    prop:value=move || owner.get()
                                    on:input=move |ev| owner.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class=field_label_class>{priority_label}</label>
                                <select
                                    class=input_class
                                    prop:value=move || priority.get()
                                    on:change=move |ev| priority.set(event_target_value(&ev))
                                >
                                    <option value="low">{p_low}</option>
                                    <option value="normal">{p_normal}</option>
                                    <option value="high">{p_high}</option>
                                    <option value="critical">{p_critical}</option>
                                </select>
                            </div>
                            {move || error.get().map(|e| view! {
                                <div class="px-3 py-2 rounded bg-danger/10 border border-danger/20 text-xs text-danger">
                                    <strong>{err_prefix.clone()}{": "}</strong>{e}
                                </div>
                            })}
                        </div>
                        <footer class="px-4 py-3 border-t border-border flex justify-end gap-2">
                            <button
                                class="px-3 py-1.5 rounded text-xs bg-surface-sunken text-text-secondary hover:text-text-primary cursor-pointer"
                                on:click=move |_| close()
                            >
                                {cancel_label}
                            </button>
                            <button
                                class=move || if busy.get() {
                                    "px-3 py-1.5 rounded text-xs bg-surface-sunken text-text-tertiary cursor-not-allowed"
                                } else {
                                    "px-3 py-1.5 rounded text-xs bg-primary text-white hover:bg-primary/90 cursor-pointer"
                                }
                                disabled=move || busy.get()
                                on:click=move |_| submit()
                            >
                                {create_label}
                            </button>
                        </footer>
                    </div>
                </div>
            }.into_any()
        }}
    }
}
