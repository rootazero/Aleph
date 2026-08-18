//! Sidebar "Projects" section (P2 Task 8) — lists the project rooms the caller
//! is a member of. `projects.list` already filters to the caller's roster
//! server-side (`gateway::handlers::projects::handle_list` — spec §6.3's
//! no-oracle contract), so no client-side filtering is needed here.
//!
//! Deliberately separate from the Chat sidebar's "enter project workspace"
//! picker (`views::chat::project_menu::ProjectMenu`) — spec §6.1 keeps the
//! two unmixed even though both read rows out of the same `projects` table
//! (server doc: "one table, two views"). That picker stays untouched by
//! this task.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::i18n::{t, t_string};

use crate::api::projects::{ProjectInfo, ProjectsApi};
use crate::components::project_page::ProjectsTabState;
use crate::context::DashboardState;

#[component]
#[must_use]
pub fn ProjectsSidebar() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let dash = expect_context::<DashboardState>();
    let tab_state = expect_context::<ProjectsTabState>();

    let projects: RwSignal<Vec<ProjectInfo>> = RwSignal::new(Vec::new());
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let creating = RwSignal::new(false);
    let new_name = RwSignal::new(String::new());

    // There is no `projects.*` push topic yet (unlike sessions/teams), so a
    // manual `refresh()` after every mutation is the sole freshness signal —
    // matching the pre-P2 picker's own model, which has the same gap.
    let refresh = move || {
        spawn_local(async move {
            match ProjectsApi::list(&dash).await {
                Ok(list) => {
                    projects.set(list);
                    error.set(None);
                }
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| e.to_string(),
                ))),
            }
        });
    };

    Effect::new(move |_| {
        if dash.is_connected.get() {
            refresh();
        } else {
            projects.set(Vec::new());
        }
    });

    let do_create = move || {
        let name = new_name.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        spawn_local(async move {
            match ProjectsApi::create(&dash, &name).await {
                Ok(project) => {
                    new_name.set(String::new());
                    creating.set(false);
                    tab_state.selected_project_id.set(Some(project.id));
                    refresh();
                }
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    };

    let select = move |id: String| {
        tab_state.selected_project_id.set(Some(id.clone()));
        spawn_local(async move {
            // Best-effort — reordering the list is a courtesy, not a
            // correctness requirement, so a failure here is silently ignored
            // (mirrors `project_menu.rs`'s own `touch` call).
            let _ = ProjectsApi::touch(&dash, &id).await;
        });
    };

    view! {
        <div class="flex flex-col h-full">
            <div class="px-3 py-3 flex items-center justify-between">
                <h3 class="text-xs font-medium text-text-tertiary uppercase tracking-wider">
                    {t!(i18n, nav.projects)}
                </h3>
                <button
                    type="button"
                    class="text-text-tertiary hover:text-text-primary text-base leading-none w-5 h-5 flex items-center justify-center rounded hover:bg-surface-sunken"
                    title=move || t_string!(i18n, project_room.sidebar_new).to_string()
                    on:click=move |_| creating.update(|v| *v = !*v)
                >
                    "+"
                </button>
            </div>

            <Show when=move || creating.get()>
                <div class="px-3 pb-2 flex items-center gap-1.5">
                    <input
                        type="text"
                        placeholder=move || t_string!(i18n, project_room.sidebar_name_placeholder).to_string()
                        class="flex-1 min-w-0 px-2 py-1 rounded-md bg-surface-sunken border border-border text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-primary/60"
                        prop:value=move || new_name.get()
                        on:input=move |ev| new_name.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                do_create();
                            }
                        }
                    />
                    <button
                        type="button"
                        class="px-2 py-1 rounded-md text-xs bg-primary/15 text-primary hover:bg-primary/25"
                        on:click=move |_| do_create()
                    >
                        {t!(i18n, project_room.sidebar_create)}
                    </button>
                </div>
            </Show>

            <nav class="flex-1 overflow-y-auto px-3 space-y-0.5">
                <Show
                    when=move || !projects.get().is_empty()
                    fallback=move || view! {
                        <p class="px-1 py-2 text-xs text-text-tertiary">
                            {t!(i18n, project_room.sidebar_empty)}
                        </p>
                    }
                >
                    <For
                        each=move || projects.get()
                        key=|p| p.id.clone()
                        children=move |p: ProjectInfo| {
                            let id = p.id.clone();
                            let id_for_click = id.clone();
                            let is_active = {
                                let id = id.clone();
                                move || tab_state.selected_project_id.get().as_deref() == Some(id.as_str())
                            };
                            let subtitle = p
                                .workspace_path
                                .clone()
                                .unwrap_or_else(|| {
                                    t_string!(i18n, project_room.workspace_unbound_short).to_string()
                                });
                            view! {
                                <button
                                    type="button"
                                    class=move || {
                                        if is_active() {
                                            "nav-tile-active w-full flex flex-col items-start px-3 py-2 rounded-lg text-left"
                                        } else {
                                            "nav-tile w-full flex flex-col items-start px-3 py-2 rounded-lg text-left"
                                        }
                                    }
                                    on:click=move |_| select(id_for_click.clone())
                                >
                                    <span class="text-sm font-medium truncate w-full">{p.name.clone()}</span>
                                    <span class="text-[11px] text-text-tertiary truncate w-full">{subtitle}</span>
                                </button>
                            }
                        }
                    />
                </Show>

                <Show when=move || error.get().is_some()>
                    <p class="px-1 py-2 text-xs text-danger">
                        {move || error.get().unwrap_or_default()}
                    </p>
                </Show>
            </nav>
        </div>
    }
}
