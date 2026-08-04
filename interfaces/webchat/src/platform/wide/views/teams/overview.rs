//! Teams Overview sub-view.
//!
//! Displays all teams (active and disbanded) with collapsible cards.
//! Each card shows summary info when collapsed and full detail when expanded.
//! Migrated from the former /dashboard/teams route into the new /teams tab.

use crate::api::teams::{TeamDetail, TeamSummary, TeamsApi, TemplateMeta};
use crate::components::ui::{Button, ButtonVariant, Card, ConfirmButton};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

// ============================================================================
// OverviewView — Teams tab "Overview" sub-view
// ============================================================================

#[component]
#[must_use]
pub fn OverviewView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let teams: RwSignal<Vec<TeamSummary>> = RwSignal::new(Vec::new());
    let expanded_id: RwSignal<Option<String>> = RwSignal::new(None);
    let expanded_detail: RwSignal<Option<TeamDetail>> = RwSignal::new(None);
    let is_loading = RwSignal::new(false);
    let error_msg: RwSignal<Option<String>> = RwSignal::new(None);

    // ---- From-Template modal state ----
    let from_template_open: RwSignal<bool> = RwSignal::new(false);
    let templates: RwSignal<Vec<TemplateMeta>> = RwSignal::new(Vec::new());
    let templates_loading: RwSignal<bool> = RwSignal::new(false);
    let ft_selected: RwSignal<String> = RwSignal::new(String::new());
    let ft_team_name: RwSignal<String> = RwSignal::new(String::new());
    let ft_goal: RwSignal<String> = RwSignal::new(String::new());
    let ft_submitting: RwSignal<bool> = RwSignal::new(false);
    let ft_error: RwSignal<Option<String>> = RwSignal::new(None);

    // --- reload ---
    let reload = move || {
        let state = state;
        spawn_local(async move {
            is_loading.set(true);
            error_msg.set(None);
            match TeamsApi::list(&state).await {
                Ok(list) => teams.set(list),
                Err(e) => error_msg.set(Some(e)),
            }
            is_loading.set(false);
        });
    };

    // Auto-load when connected
    let reload_on_connect = reload;
    Effect::new(move || {
        if state.is_connected.get() {
            reload_on_connect();
        } else {
            teams.set(Vec::new());
        }
    });

    // Live cross-view sync: any surface that creates / renames / disbands /
    // deletes a team makes the gateway emit `team.changed`. Re-fetch so this
    // tab never lags behind the group-chat sidebar (e.g. a team disbanded from
    // the sidebar must immediately show "disbanded" here, not "active").
    let reload_on_event = reload;
    let sub_id = state.subscribe_events(move |event| {
        if event.topic == "team.changed" {
            reload_on_event();
        }
    });
    // Ensure the gateway pushes the topic to this client even if the sidebar
    // (which also subscribes) is not mounted.
    let topic_state = state;
    spawn_local(async move {
        for _ in 0..50 {
            if topic_state.is_connected.get_untracked() {
                break;
            }
            gloo_timers::future::TimeoutFuture::new(100).await;
        }
        if let Err(e) = topic_state.subscribe_topic("team.changed").await {
            web_sys::console::error_1(&format!("Failed to subscribe to team.changed: {e}").into());
        }
    });
    on_cleanup(move || {
        state.unsubscribe_events(sub_id);
    });

    // --- toggle_expand ---
    let toggle_expand = move |team_id: String| {
        let current = expanded_id.get_untracked();
        if current.as_deref() == Some(&team_id) {
            // Collapse
            expanded_id.set(None);
            expanded_detail.set(None);
        } else {
            expanded_id.set(Some(team_id.clone()));
            expanded_detail.set(None);
            let state = state;
            spawn_local(async move {
                match TeamsApi::get(&state, &team_id).await {
                    Ok(detail) => expanded_detail.set(Some(detail)),
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to load team detail: {e}").into(),
                        );
                    }
                }
            });
        }
    };

    // --- handle_disband ---
    let handle_disband = move |team_id: String| {
        let state = state;
        spawn_local(async move {
            if let Err(e) = TeamsApi::disband(&state, &team_id).await {
                web_sys::console::error_1(&format!("Disband failed: {e}").into());
                error_msg.set(Some(format!("解散失败: {e}")));
                return;
            }
            error_msg.set(None);
            // Collapse if this team was expanded. Post-`.await`, so the view may
            // be gone (see `crate::disposed_reads`); `.flatten()` keeps the
            // comparison — and the control flow — byte-identical.
            if expanded_id.try_get_untracked().flatten().as_deref() == Some(&team_id) {
                expanded_id.set(None);
                expanded_detail.set(None);
            }
            is_loading.set(true);
            match TeamsApi::list(&state).await {
                Ok(list) => teams.set(list),
                Err(e) => error_msg.set(Some(e)),
            }
            is_loading.set(false);
        });
    };

    // --- handle_delete ---
    let handle_delete = move |team_id: String| {
        let state = state;
        spawn_local(async move {
            if let Err(e) = TeamsApi::delete(&state, &team_id).await {
                web_sys::console::error_1(&format!("Delete failed: {e}").into());
                error_msg.set(Some(format!("删除失败: {e}")));
                return;
            }
            error_msg.set(None);
            // Same shape as `handle_disband` above.
            if expanded_id.try_get_untracked().flatten().as_deref() == Some(&team_id) {
                expanded_id.set(None);
                expanded_detail.set(None);
            }
            is_loading.set(true);
            match TeamsApi::list(&state).await {
                Ok(list) => teams.set(list),
                Err(e) => error_msg.set(Some(e)),
            }
            is_loading.set(false);
        });
    };

    // ---- From-template handlers ----
    let open_from_template = move || {
        ft_selected.set(String::new());
        ft_team_name.set(String::new());
        ft_goal.set(String::new());
        ft_error.set(None);
        from_template_open.set(true);
        let state = state;
        spawn_local(async move {
            templates_loading.set(true);
            match TeamsApi::list_templates(&state).await {
                Ok(list) => {
                    if let Some(first) = list.first() {
                        ft_selected.set(first.name.clone());
                    }
                    templates.set(list);
                }
                Err(e) => ft_error.set(Some(e)),
            }
            templates_loading.set(false);
        });
    };

    let close_from_template = move || {
        from_template_open.set(false);
    };

    let submit_from_template = move || {
        let template = ft_selected.get_untracked();
        let team_name = ft_team_name.get_untracked();
        let goal_raw = ft_goal.get_untracked();
        if template.is_empty() || team_name.trim().is_empty() {
            ft_error.set(Some(
                t_string!(i18n, teams.from_template.required_fields).to_string(),
            ));
            return;
        }
        let goal = if goal_raw.trim().is_empty() {
            None
        } else {
            Some(goal_raw.trim().to_string())
        };
        let state = state;
        spawn_local(async move {
            ft_submitting.set(true);
            ft_error.set(None);
            let result = TeamsApi::create_from_template(
                &state,
                &template,
                team_name.trim(),
                goal.as_deref(),
            )
            .await;
            ft_submitting.set(false);
            match result {
                Ok(_team_id) => {
                    from_template_open.set(false);
                    // Reload the list so the new team appears immediately.
                    is_loading.set(true);
                    error_msg.set(None);
                    match TeamsApi::list(&state).await {
                        Ok(list) => teams.set(list),
                        Err(e) => error_msg.set(Some(e)),
                    }
                    is_loading.set(false);
                }
                Err(e) => ft_error.set(Some(e)),
            }
        });
    };

    view! {
        <div class="px-6 pb-8 aleph-content-top space-y-6">
            // ---- Header ----
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                        <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" attr:class="w-8 h-8 text-primary">
                            <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" />
                            <circle cx="9" cy="7" r="4" />
                            <path d="M23 21v-2a4 4 0 0 0-3-3.87" />
                            <path d="M16 3.13a4 4 0 0 1 0 7.75" />
                        </svg>
                        {t!(i18n, teams.title)}
                    </h2>
                    <p class="text-text-secondary">
                        {move || {
                            let list = teams.get();
                            let active = list.iter().filter(|t| t.status == "active").count();
                            let disbanded = list.iter().filter(|t| t.status != "active").count();
                            format!(
                                "{} {} · {} {}",
                                active,
                                t_string!(i18n, teams.stat_active),
                                disbanded,
                                t_string!(i18n, teams.stat_disbanded),
                            )
                        }}
                    </p>
                </div>
                <div class="flex items-center gap-2">
                    <Button
                        on:click=move |_| open_from_template()
                        variant=ButtonVariant::Primary
                        disabled=Signal::derive(move || !state.is_connected.get())
                    >
                        {t!(i18n, teams.from_template.button)}
                    </Button>
                    <Button
                        on:click=move |_| reload()
                        variant=ButtonVariant::Secondary
                        disabled=Signal::derive(move || is_loading.get() || !state.is_connected.get())
                    >
                        {move || if is_loading.get() {
                            t_string!(i18n, common.loading).to_string()
                        } else {
                            t_string!(i18n, teams.refresh).to_string()
                        }}
                    </Button>
                </div>
            </header>

            // ---- Error banner ----
            {move || error_msg.get().map(|e| view! {
                <div class="bg-danger-subtle border border-danger/20 rounded-xl p-4 text-sm text-danger">
                    <strong>{t!(i18n, common.error)}{": "}</strong>{e}
                </div>
            })}

            // ---- Not connected ----
            {move || {
                if !state.is_connected.get() {
                    view! {
                        <Card class="p-12 text-center">
                            <p class="text-text-tertiary">{t!(i18n, teams.connect_to_view)}</p>
                        </Card>
                    }.into_any()
                } else if teams.get().is_empty() && !is_loading.get() {
                    view! {
                        <Card class="p-12 text-center">
                            <p class="text-text-tertiary">{t!(i18n, teams.no_teams)}</p>
                        </Card>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-3">
                            {move || {
                                teams.get().into_iter().map(|team| {
                                    let team_id = team.id.clone();
                                    let team_id_expand = team_id.clone();
                                    let team_id_disband = team_id.clone();
                                    let team_id_delete = team_id.clone();
                                    let is_active = team.status == "active";
                                    let team_id_check = team_id.clone();
                                    let team_id_check2 = team_id;
                                    let confirming = RwSignal::new(false);

                                    view! {
                                        <div class="bg-surface-raised border border-border rounded-xl overflow-hidden transition-all">
                                            // ---- Collapsed row ----
                                            <div
                                                class="flex items-center gap-3 px-4 py-3 cursor-pointer hover:bg-surface-sunken/50 select-none"
                                                on:click=move |_| toggle_expand(team_id_expand.clone())
                                            >
                                                // Expand chevron
                                                <span class="text-text-tertiary w-4 flex-shrink-0 text-xs">
                                                    {move || if expanded_id.get().as_deref() == Some(&team_id_check) { "▼" } else { "▶" }}
                                                </span>

                                                // Team name
                                                <span class="flex-1 font-medium text-text-primary truncate">
                                                    {team.name.clone()}
                                                </span>

                                                // Status badge
                                                {if is_active {
                                                    view! {
                                                        <span class="px-2 py-0.5 rounded-full text-xs font-medium bg-success/20 text-success flex-shrink-0">
                                                            {t!(i18n, teams.status_active)}
                                                        </span>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <span class="px-2 py-0.5 rounded-full text-xs font-medium bg-surface-sunken text-text-tertiary flex-shrink-0">
                                                            {t!(i18n, teams.status_disbanded)}
                                                        </span>
                                                    }.into_any()
                                                }}

                                                // Member count
                                                <span class="text-xs text-text-secondary flex-shrink-0">
                                                    {team.member_count.to_string()}{" "}{t!(i18n, teams.members)}
                                                </span>

                                                // Action button — two-step inline confirm; stop propagation
                                                // so the click doesn't toggle the row's expand state.
                                                {if is_active {
                                                    let on_confirm_disband = move || handle_disband(team_id_disband.clone());
                                                    view! {
                                                        {move || if confirming.get() {
                                                            view! {
                                                                <ConfirmButton
                                                                    confirming=confirming
                                                                    on_confirm=on_confirm_disband.clone()
                                                                    label=Signal::derive(move || t_string!(i18n, common.confirm_dissolve).to_string())
                                                                    width_class="flex-shrink-0"
                                                                    size_class="px-3 py-1 text-xs"
                                                                    stop_propagation=true
                                                                />
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <button
                                                                    class="px-3 py-1 rounded-lg text-xs font-medium bg-warning/10 text-warning hover:bg-warning/20 border border-warning/20 transition-all flex-shrink-0"
                                                                    on:click=move |ev| {
                                                                        ev.stop_propagation();
                                                                        confirming.set(true);
                                                                    }
                                                                >
                                                                    {t!(i18n, teams.disband)}
                                                                </button>
                                                            }.into_any()
                                                        }}
                                                    }.into_any()
                                                } else {
                                                    let on_confirm_delete = move || handle_delete(team_id_delete.clone());
                                                    view! {
                                                        {move || if confirming.get() {
                                                            view! {
                                                                <ConfirmButton
                                                                    confirming=confirming
                                                                    on_confirm=on_confirm_delete.clone()
                                                                    width_class="flex-shrink-0"
                                                                    size_class="px-3 py-1 text-xs"
                                                                    stop_propagation=true
                                                                />
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <button
                                                                    class="px-3 py-1 rounded-lg text-xs font-medium bg-danger/10 text-danger hover:bg-danger/20 border border-danger/20 transition-all flex-shrink-0"
                                                                    on:click=move |ev| {
                                                                        ev.stop_propagation();
                                                                        confirming.set(true);
                                                                    }
                                                                >
                                                                    {t!(i18n, teams.delete)}
                                                                </button>
                                                            }.into_any()
                                                        }}
                                                    }.into_any()
                                                }}
                                            </div>

                                            // ---- Expanded detail ----
                                            {move || {
                                                if expanded_id.get().as_deref() != Some(&team_id_check2) {
                                                    return ().into_any();
                                                }
                                                match expanded_detail.get() {
                                                    None => view! {
                                                        <div class="border-t border-border px-6 py-4 text-sm text-text-secondary">
                                                            {t!(i18n, common.loading)}
                                                        </div>
                                                    }.into_any(),
                                                    Some(detail) => view! {
                                                        <TeamDetailPanel detail=detail />
                                                    }.into_any(),
                                                }
                                            }}
                                        </div>
                                    }
                                }).collect_view()
                            }}
                        </div>
                    }.into_any()
                }
            }}

            // ---- From-Template modal ----
            {move || {
                if !from_template_open.get() {
                    return ().into_any();
                }
                view! {
                    <div
                        class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 aleph-scrim p-4"
                        on:click=move |_| close_from_template()
                    >
                        <div
                            class="glass bg-surface-overlay/85 border border-border rounded-2xl shadow-xl w-full max-w-lg p-6 space-y-4"
                            on:click=|ev| ev.stop_propagation()
                        >
                            <h3 class="text-lg font-semibold text-text-primary">
                                {t!(i18n, teams.from_template.title)}
                            </h3>
                            <p class="text-sm text-text-secondary">
                                {t!(i18n, teams.from_template.subtitle)}
                            </p>

                            // Template dropdown
                            <div class="space-y-1.5">
                                <label class="text-xs font-medium text-text-tertiary uppercase tracking-wider">
                                    {t!(i18n, teams.from_template.template_label)}
                                </label>
                                {move || {
                                    if templates_loading.get() {
                                        view! {
                                            <p class="text-sm text-text-tertiary">{t!(i18n, common.loading)}</p>
                                        }.into_any()
                                    } else {
                                        let list = templates.get();
                                        if list.is_empty() {
                                            view! {
                                                <p class="text-sm text-text-tertiary">
                                                    {t!(i18n, teams.from_template.no_templates)}
                                                </p>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <select
                                                    class="w-full bg-surface-sunken border border-border rounded-lg px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary"
                                                    on:change=move |ev| {
                                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok());
                                                        if let Some(sel) = target {
                                                            ft_selected.set(sel.value());
                                                        }
                                                    }
                                                >
                                                    {list.into_iter().map(|tpl| {
                                                        let value = tpl.name.clone();
                                                        let label = format!(
                                                            "{} — {} ({} members, {} tasks)",
                                                            tpl.name,
                                                            tpl.description,
                                                            tpl.member_count,
                                                            tpl.task_count,
                                                        );
                                                        let selected = ft_selected.get_untracked() == value;
                                                        view! {
                                                            <option value=value selected=selected>{label}</option>
                                                        }
                                                    }).collect_view()}
                                                </select>
                                            }.into_any()
                                        }
                                    }
                                }}
                            </div>

                            // Team name input (required)
                            <div class="space-y-1.5">
                                <label class="text-xs font-medium text-text-tertiary uppercase tracking-wider">
                                    {t!(i18n, teams.from_template.team_name_label)}
                                    <span class="text-danger ml-1">{"*"}</span>
                                </label>
                                <input
                                    type="text"
                                    class="w-full bg-surface-sunken border border-border rounded-lg px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary"
                                    placeholder=move || t_string!(i18n, teams.from_template.team_name_placeholder).to_string()
                                    prop:value=move || ft_team_name.get()
                                    on:input=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                        if let Some(input) = target {
                                            ft_team_name.set(input.value());
                                        }
                                    }
                                />
                            </div>

                            // Goal textarea (optional)
                            <div class="space-y-1.5">
                                <label class="text-xs font-medium text-text-tertiary uppercase tracking-wider">
                                    {t!(i18n, teams.from_template.goal_label)}
                                </label>
                                <textarea
                                    rows="3"
                                    class="w-full bg-surface-sunken border border-border rounded-lg px-3 py-2 text-sm text-text-primary focus:outline-none focus:ring-1 focus:ring-primary resize-y"
                                    placeholder=move || t_string!(i18n, teams.from_template.goal_placeholder).to_string()
                                    prop:value=move || ft_goal.get()
                                    on:input=move |ev| {
                                        let target = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok());
                                        if let Some(input) = target {
                                            ft_goal.set(input.value());
                                        }
                                    }
                                />
                            </div>

                            // Error banner
                            {move || ft_error.get().map(|e| view! {
                                <div class="bg-danger-subtle border border-danger/20 rounded-lg p-3 text-sm text-danger">
                                    {e}
                                </div>
                            })}

                            // Footer buttons
                            <div class="flex items-center justify-end gap-2 pt-2">
                                <Button
                                    on:click=move |_| close_from_template()
                                    variant=ButtonVariant::Secondary
                                    disabled=Signal::derive(move || ft_submitting.get())
                                >
                                    {t!(i18n, common.cancel)}
                                </Button>
                                <Button
                                    on:click=move |_| submit_from_template()
                                    variant=ButtonVariant::Primary
                                    disabled=Signal::derive(move ||
                                        ft_submitting.get()
                                        || templates_loading.get()
                                        || templates.get().is_empty()
                                    )
                                >
                                    {move || if ft_submitting.get() {
                                        t_string!(i18n, common.saving).to_string()
                                    } else {
                                        t_string!(i18n, teams.from_template.submit).to_string()
                                    }}
                                </Button>
                            </div>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

// ============================================================================
// TeamDetailPanel — expanded detail section
// ============================================================================

#[component]
fn TeamDetailPanel(detail: TeamDetail) -> impl IntoView {
    let i18n = use_i18n();
    let max_tasks = 5usize;
    let tasks: Vec<_> = detail.tasks.into_iter().take(max_tasks).collect();
    let members = detail.members;
    let leader_id = detail.team.leader_id;

    view! {
        <div class="border-t border-border px-6 py-4 space-y-4 text-sm">
            // Leader
            <div class="flex items-center gap-2">
                <span class="text-xs font-medium text-text-tertiary uppercase tracking-wider w-20">
                    {t!(i18n, teams.detail_leader)}
                </span>
                <span class="text-text-primary font-mono text-xs bg-surface-sunken px-2 py-0.5 rounded">
                    {leader_id}
                </span>
            </div>

            // Members
            {if !members.is_empty() {
                view! {
                    <div class="flex items-start gap-2">
                        <span class="text-xs font-medium text-text-tertiary uppercase tracking-wider w-20 pt-0.5">
                            {t!(i18n, teams.detail_members)}
                        </span>
                        <div class="flex flex-wrap gap-2">
                            {members.into_iter().map(|m| {
                                let role_class = match m.role.as_str() {
                                    "leader" => "bg-primary/10 text-primary border-primary/20",
                                    "member" => "bg-surface-sunken text-text-secondary border-border",
                                    _ => "bg-surface-sunken text-text-tertiary border-border",
                                };
                                view! {
                                    <span class=format!(
                                        "inline-flex items-center gap-1 px-2 py-0.5 rounded border text-xs font-medium {}",
                                        role_class
                                    )>
                                        <span class="font-mono">{m.agent_id}</span>
                                        <span class="opacity-60">{"·"}</span>
                                        <span class="capitalize">{m.role}</span>
                                    </span>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}

            // Recent tasks
            {if !tasks.is_empty() {
                view! {
                    <div class="flex items-start gap-2">
                        <span class="text-xs font-medium text-text-tertiary uppercase tracking-wider w-20 pt-0.5">
                            {t!(i18n, teams.detail_tasks)}
                        </span>
                        <div class="flex flex-col gap-1.5 flex-1 min-w-0">
                            {tasks.into_iter().map(|task| {
                                let (dot_class, status_text) = match task.status.as_str() {
                                    "completed" => ("bg-success", t_string!(i18n, teams.task_completed).to_string()),
                                    "running" | "in_progress" => ("bg-warning animate-pulse", t_string!(i18n, teams.task_running).to_string()),
                                    "failed" => ("bg-danger", t_string!(i18n, teams.task_failed).to_string()),
                                    "unsatisfiable" => ("bg-danger", task.status.clone()),
                                    _ => ("bg-text-tertiary", task.status.clone()),
                                };
                                let owner = task.owner.clone().unwrap_or_else(|| "—".into());
                                view! {
                                    <div class="flex items-center gap-2 text-xs">
                                        // Status dot
                                        <span class=format!("w-2 h-2 rounded-full flex-shrink-0 {}", dot_class) />
                                        // Owner agent
                                        <span class="font-mono text-text-tertiary flex-shrink-0">{owner}</span>
                                        <span class="text-text-tertiary">{"→"}</span>
                                        // Subject
                                        <span class="text-text-primary truncate flex-1">{task.subject}</span>
                                        // Status label
                                        <span class="text-text-tertiary flex-shrink-0">{status_text}</span>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
        </div>
    }
}
