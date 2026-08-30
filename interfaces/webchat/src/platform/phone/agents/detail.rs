//! Phone single-agent detail (`/agents/{id}/{tab}`): a full-screen drill from
//! the Agents list. Reuses the desktop tab content components
//! (Overview/Files/Skills/Channels/Teams) under phone chrome — a header
//! (emoji · name · ★ · Set-default · Delete) and a horizontally-scrollable tab
//! bar. Reads the router-owned `PhoneAgentsState` to resolve the current agent.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::NavigateOptions;

use crate::api::agents::AgentsApi;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::platform::phone::shell::PhoneShell;
use crate::views::agents::channels::ChannelsTab;
use crate::views::agents::files::FilesTab;
use crate::views::agents::overview::OverviewTab;
use crate::views::agents::skills::SkillsTab;
use crate::views::agents::teams::TeamsTab;

use super::{parse_detail_path, AgentDetailTab, PhoneAgentsState, DETAIL_TABS};

#[component]
#[must_use]
pub fn PhoneAgentDetail() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let st = expect_context::<PhoneAgentsState>();
    let location = use_location();
    let navigate = use_navigate();

    // (agent_id, active_tab) from the URL.
    let parsed = Memo::new(move |_| parse_detail_path(&location.pathname.get()));

    // Tap-to-confirm guard for the destructive Delete (parity with the desktop
    // ConfirmButton). Disarmed on every route change (agent or tab switch).
    let confirming = RwSignal::new(false);
    Effect::new(move || {
        parsed.get();
        confirming.set(false);
    });

    view! {
        <PhoneShell title="Agent" back="/agents" back_label="Agents">
        <div style="display:flex; flex-direction:column; gap:14px;">
            {move || {
                let Some((agent_id, tab)) = parsed.get() else {
                    return view! { <div class="list-header">"No agent selected"</div> }.into_any();
                };
                // Resolve the summary (emoji/name/default) from the loaded list.
                let Some(agent) = st.agents.get().into_iter().find(|a| a.id == agent_id) else {
                    let label = if st.loaded.get() {
                        t_string!(i18n, agents.not_found)
                    } else {
                        t_string!(i18n, common.loading)
                    };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                };
                let emoji = agent.emoji.clone().unwrap_or_default();
                let name = agent.name.clone().unwrap_or_else(|| agent.id.clone());
                let is_default = agent.is_default;

                // Header actions. The Delete chip is a two-step tap-to-confirm
                // (first tap arms, second deletes) — parity with the desktop
                // ConfirmButton. `set_default` is built only in the non-default
                // branch below to avoid an unused binding.
                let delete_or_arm = {
                    let id = agent_id.clone();
                    let navigate = navigate.clone();
                    move |_| {
                        if !confirming.get_untracked() {
                            confirming.set(true);
                            return;
                        }
                        let id = id.clone();
                        let navigate = navigate.clone();
                        let dash = dashboard;
                        spawn_local(async move {
                            if AgentsApi::delete(&dash, &id).await.is_ok() {
                                st.reload_nonce.update(|n| *n += 1);
                                navigate("/agents", NavigateOptions::default());
                            }
                        });
                    }
                };

                let id_for_tabs = agent_id.clone();
                let tab_content = match tab {
                    AgentDetailTab::Overview => view! { <OverviewTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Files => view! { <FilesTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Skills => view! { <SkillsTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Channels => view! { <ChannelsTab agent_id=agent_id.clone() /> }.into_any(),
                    AgentDetailTab::Teams => view! { <TeamsTab agent_id=agent_id.clone() /> }.into_any(),
                };

                // Default badge (when default) OR a "Set default" button (else).
                let default_action = if is_default {
                    view! { <span class="badge badge-warning" style="flex:none;">"★ Default"</span> }.into_any()
                } else {
                    let set_default = {
                        let id = agent_id.clone();
                        move |_| {
                            let id = id.clone();
                            let dash = dashboard;
                            spawn_local(async move {
                                if AgentsApi::set_default(&dash, &id).await.is_ok() {
                                    st.reload_nonce.update(|n| *n += 1);
                                }
                            });
                        }
                    };
                    view! { <button class="chip" style="flex:none;" on:click=set_default>"Set default"</button> }.into_any()
                };

                view! {
                    <div style="display:flex; flex-direction:column; gap:14px;">
                        // Header
                        <div style="display:flex; align-items:center; gap:10px;">
                            <span style="font-size:26px;">{emoji}</span>
                            <div style="flex:1; min-width:0;">
                                <div style="font-size:19px; font-weight:700; color:var(--color-text-primary); overflow:hidden; text-overflow:ellipsis; white-space:nowrap;">{name}</div>
                            </div>
                            {default_action}
                            <button
                                class="chip"
                                style=move || if is_default {
                                    "flex:none; opacity:0.4; pointer-events:none;".to_string()
                                } else if confirming.get() {
                                    "flex:none; color:var(--color-text-inverse); background:var(--color-danger); border-color:var(--color-danger);".to_string()
                                } else {
                                    "flex:none; color:var(--color-danger);".to_string()
                                }
                                on:click=delete_or_arm
                            >
                                {move || if confirming.get() { "Confirm?" } else { "Delete" }}
                            </button>
                        </div>

                        // Tab bar (horizontal scroll)
                        <div class="cc-hide-scroll" style="display:flex; gap:8px; overflow-x:auto; margin:0 -16px; padding:1px 16px;">
                            {DETAIL_TABS.iter().map(|t| {
                                let t = *t;
                                let navigate = navigate.clone();
                                let id = id_for_tabs.clone();
                                let is_active = t == tab;
                                view! {
                                    <button
                                        class="chip"
                                        class:chip-active=move || is_active
                                        style="flex:none;"
                                        on:click=move |_| navigate(&format!("/agents/{}/{}", id, t.slug()), NavigateOptions::default())
                                    >
                                        {t.label()}
                                    </button>
                                }
                            }).collect_view()}
                        </div>

                        // Tab content (reused desktop component)
                        <div>{tab_content}</div>
                    </div>
                }.into_any()
            }}
        </div>
        </PhoneShell>
    }
}
