// Skills Tab — per-agent skill toggles.
//
// Semantics: every skill is enabled by default. Turning a skill OFF adds its id
// to the agent's `skills_blacklist` (→ runtime `tool_blacklist`), which denies
// just that skill-tool without touching the agent's tool whitelist. This is the
// only non-destructive way to express "disable this one skill": writing the
// `skills` whitelist instead would lock the agent to ONLY the listed ids and
// silently strip every built-in / MCP tool.

use crate::api::agents::AgentsApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, Deserialize)]
struct SkillEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

#[component]
#[must_use]
pub fn SkillsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let agent_id = StoredValue::new(agent_id);

    let all_skills = RwSignal::new(Vec::<SkillEntry>::new());
    // The agent's skills_blacklist — skills explicitly turned OFF for this agent.
    let disabled_skills = RwSignal::new(Vec::<String>::new());
    let filter = RwSignal::new(String::new());
    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let save_message = RwSignal::new(Option::<(bool, String)>::None);

    // Load available skills (skills.status) and the agent's current blacklist.
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() {
            return;
        }
        let id = agent_id.get_value();
        spawn_local(async move {
            if let Ok(result) = dash
                .rpc_call("skills.status", serde_json::Value::Null)
                .await
            {
                if let Some(arr) = result.get("skills") {
                    if let Ok(skills) = serde_json::from_value::<Vec<SkillEntry>>(arr.clone()) {
                        all_skills.set(skills);
                    }
                }
            }
            if let Ok(detail) = AgentsApi::get(&dash, &id).await {
                if let Some(blacklist) = detail
                    .definition
                    .get("skills_blacklist")
                    .and_then(|v| v.as_array())
                {
                    let ids: Vec<String> = blacklist
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    disabled_skills.set(ids);
                }
            }
            is_loading.set(false);
        });
    });

    // Toggle a skill: flip its presence in the blacklist.
    let toggle_skill = move |skill_id: String| {
        let mut current = disabled_skills.get();
        if current.contains(&skill_id) {
            current.retain(|s| s != &skill_id);
        } else {
            current.push(skill_id);
        }
        disabled_skills.set(current);
        save_message.set(None);
    };

    view! {
        <div class="space-y-4">
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center">{t!(i18n, agents.skills.loading)}</div>
                    }.into_any();
                }

                view! {
                    <div class="space-y-4">
                        <input
                            type="text"
                            placeholder=move || t_string!(i18n, agents.skills.search_placeholder).to_string()
                            prop:value=move || filter.get()
                            on:input=move |ev| filter.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm"
                        />

                        <div class="bg-surface-raised border border-border rounded-xl divide-y divide-border">
                            {move || {
                                let f = filter.get().to_lowercase();
                                let disabled = disabled_skills.get();
                                all_skills.get().into_iter()
                                    .filter(|s| f.is_empty() || s.name.to_lowercase().contains(&f) || s.id.to_lowercase().contains(&f))
                                    .map(|skill| {
                                        let sid = skill.id.clone();
                                        let sid_toggle = sid.clone();
                                        let is_enabled = !disabled.contains(&sid);
                                        view! {
                                            <div class="flex items-center justify-between p-3">
                                                <div>
                                                    <div class="text-sm font-medium text-text-primary">{skill.name.clone()}</div>
                                                    <div class="text-xs text-text-tertiary">{skill.description}</div>
                                                </div>
                                                <input
                                                    type="checkbox"
                                                    checked=is_enabled
                                                    on:change=move |_| toggle_skill(sid_toggle.clone())
                                                    class="w-4 h-4"
                                                />
                                            </div>
                                        }
                                    }).collect_view()
                            }}
                        </div>

                        {move || save_message.get().map(|(ok, msg)| {
                            let cls = if ok {
                                "p-3 bg-success-subtle border border-success/30 rounded-lg text-success text-sm"
                            } else {
                                "p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm"
                            };
                            view! { <div class=cls>{msg}</div> }
                        })}

                        <div class="flex justify-end">
                            <button
                                on:click=move |_| {
                                    is_saving.set(true);
                                    save_message.set(None);
                                    let id = agent_id.get_value();
                                    let blacklist = disabled_skills.get();
                                    let dash = state;
                                    spawn_local(async move {
                                        match AgentsApi::update(&dash, &id, json!({"skills_blacklist": blacklist})).await {
                                            Ok(_) => save_message.set(Some((true, t_string!(i18n, agents.skills.saved).to_string()))),
                                            // `agents.` is admin-gated, so a member's Save comes
                                            // back refused. The guard in `admin_refusal` cannot
                                            // see this site — its receiver-name heuristic only
                                            // recognises signals called `*err*` — which is why
                                            // it stood while 154 named ones were fixed.
                                            Err(e) => save_message.set(Some((
                                                false,
                                                admin_refusal::settings_write_error(i18n, &e, |detail| {
                                                    format!("{}: {detail}", t_string!(i18n, agents.skills.save))
                                                }),
                                            ))),
                                        }
                                        is_saving.set(false);
                                    });
                                }
                                disabled=move || is_saving.get()
                                class="px-6 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors"
                            >
                                {move || if is_saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, agents.skills.save).to_string() }}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
