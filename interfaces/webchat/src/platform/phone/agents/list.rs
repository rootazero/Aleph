//! Phone Agents list landing (`/agents`): mirrors the desktop `AgentsSidebar`
//! as a full-screen list — filter chips, an inline-expandable "+ New Agent"
//! form, and agent cells (emoji · name · channel badge · ★). Tapping a cell
//! drills into `/agents/{id}`. Reads the router-owned `PhoneAgentsState`;
//! reuses the agents data layer (R4).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::api::agents::{AgentSummary, AgentsApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string};
use crate::platform::phone::shell::PhoneShell;

use super::PhoneAgentsState;

/// Filter chips: (label, filter key).
const FILTERS: [(&str, &str); 3] = [
    ("All", "all"),
    ("Channel", "channel"),
    ("Standalone", "standalone"),
];

#[component]
#[must_use]
pub fn PhoneAgentsList() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let dashboard = expect_context::<DashboardState>();
    let st = expect_context::<PhoneAgentsState>();
    let navigate = use_navigate();

    // Filter + create-form state are list-local (ephemeral).
    let filter = RwSignal::new("all".to_string());
    let show_create = RwSignal::new(false);
    let new_id = RwSignal::new(String::new());
    let new_name = RwSignal::new(String::new());
    let new_archetype = RwSignal::new("assistant".to_string());
    let create_error = RwSignal::new(Option::<String>::None);

    // agents → filter (channel/standalone via the bindings map).
    let visible = move || {
        let binds = st.bindings.get();
        let f = filter.get();
        st.agents
            .get()
            .into_iter()
            .filter(|a| match f.as_str() {
                "channel" => binds.contains_key(&a.id),
                "standalone" => !binds.contains_key(&a.id),
                _ => true,
            })
            .collect::<Vec<AgentSummary>>()
    };

    let submit_create = move |_| {
        let id = new_id.get();
        if id.is_empty() {
            create_error.set(Some("Agent ID is required".to_string()));
            return;
        }
        create_error.set(None);
        let name_val = new_name.get();
        let name = (!name_val.is_empty()).then_some(name_val);
        let archetype = new_archetype.get();
        let dash = dashboard;
        spawn_local(async move {
            match AgentsApi::create(&dash, &id, name.as_deref(), None, Some(&archetype)).await {
                Ok(()) => {
                    show_create.set(false);
                    new_id.set(String::new());
                    new_name.set(String::new());
                    new_archetype.set("assistant".to_string());
                    st.reload_nonce.update(|n| *n += 1);
                }
                Err(e) => create_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    };

    view! {
        <PhoneShell title="Agents">
        // Single element child for PhoneShell (footgun).
        <div style="display:flex; flex-direction:column; gap:12px;">
            // ── Filter chips ──
            <div class="cc-hide-scroll" style="display:flex; gap:8px; overflow-x:auto; margin:0 -16px; padding:1px 16px;">
                {FILTERS.iter().map(|(label, key)| {
                    let key = *key;
                    view! {
                        <button
                            class="chip"
                            class:chip-active=move || filter.get() == key
                            style="flex:none;"
                            on:click=move |_| filter.set(key.to_string())
                        >
                            {*label}
                        </button>
                    }
                }).collect_view()}
            </div>

            // ── New Agent (inline-expandable) ──
            <div class="list">
                <div class="cell" on:click=move |_| show_create.update(|v| *v = !*v)>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title" style="color:var(--color-primary);">{t!(i18n, agents.phone_new_agent)}</div></div>
                </div>
                {move || show_create.get().then(|| view! {
                    <div style="display:flex; flex-direction:column; gap:8px; padding:12px;">
                        <input class="field" type="text" placeholder=t_string!(i18n, agents.phone_agent_id_placeholder)
                            prop:value=move || new_id.get()
                            on:input=move |ev| new_id.set(event_target_value(&ev)) />
                        <input class="field" type="text" placeholder=t_string!(i18n, agents.phone_display_name_placeholder)
                            prop:value=move || new_name.get()
                            on:input=move |ev| new_name.set(event_target_value(&ev)) />
                        <select class="field"
                            prop:value=move || new_archetype.get()
                            on:change=move |ev| new_archetype.set(event_target_value(&ev))>
                            <option value="assistant">{t!(i18n, agents.phone_persona_assistant)}</option>
                            <option value="expert">{t!(i18n, agents.phone_persona_expert)}</option>
                            <option value="maker">{t!(i18n, agents.phone_persona_maker)}</option>
                            <option value="companion">{t!(i18n, agents.phone_persona_companion)}</option>
                        </select>
                        {move || create_error.get().map(|e| view! {
                            <div class="cell-sub" style="color:var(--color-danger);">{e}</div>
                        })}
                        <button class="chip" style="align-self:flex-start;" on:click=submit_create>{t!(i18n, agents.phone_create)}</button>
                    </div>
                })}
            </div>

            // ── Agent list ──
            {move || {
                if !st.loaded.get() {
                    let label = if dashboard.is_connected.get() {
                        t_string!(i18n, common.loading)
                    } else {
                        t_string!(i18n, common.connecting)
                    };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                }
                if let Some(err) = st.error.get() {
                    return view! {
                        <div class="list">
                            <div class="cell"><div class="cell-body"><div class="cell-title">{t!(i18n, agents.phone_load_failed)}</div><div class="cell-sub">{err}</div></div></div>
                            <div class="cell" on:click=move |_| st.reload_nonce.update(|n| *n += 1)>
                                <div class="cell-body"><div class="cell-title" style="color:var(--color-primary);">{t!(i18n, common.retry)}</div></div>
                            </div>
                        </div>
                    }.into_any();
                }
                let items = visible();
                if items.is_empty() {
                    return view! { <div class="list-header">{t!(i18n, agents.phone_no_agents)}</div> }.into_any();
                }
                let binds = st.bindings.get();
                view! {
                    <div class="list">
                        {items.into_iter().map(|a| {
                            let navigate = navigate.clone();
                            let id_for_click = a.id.clone();
                            let emoji = a.emoji.clone().unwrap_or_default();
                            let name = a.name.clone().unwrap_or_else(|| a.id.clone());
                            let is_default = a.is_default;
                            let channel = binds
                                .get(&a.id)
                                .filter(|chs| !chs.is_empty())
                                .map(|chs| chs.join(", "));
                            view! {
                                <div class="cell" on:click=move |_| navigate(&format!("/agents/{}", id_for_click), NavigateOptions::default())>
                                    <span class="cell-leading" style="font-size:18px;">{emoji}</span>
                                    <div class="cell-body"><div class="cell-title">{name}</div></div>
                                    {channel.map(|ch| view! { <span class="badge badge-info" style="flex:none;">{ch}</span> })}
                                    {is_default.then(|| view! { <span class="badge badge-warning" style="flex:none;">"★"</span> })}
                                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </div>
        </PhoneShell>
    }
}
