// Skills Tab — per-agent skill toggles

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use serde_json::json;
use crate::api::agents::AgentsApi;
use crate::context::DashboardState;

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
pub fn SkillsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let agent_id = StoredValue::new(agent_id);

    let all_skills = RwSignal::new(Vec::<SkillEntry>::new());
    let agent_skills = RwSignal::new(Vec::<String>::new());
    let filter = RwSignal::new(String::new());
    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let save_message = RwSignal::new(Option::<(bool, String)>::None);

    // Load available skills and agent's current skills
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = agent_id.get_value();
        spawn_local(async move {
            if let Ok(result) = dash.rpc_call("skills.list", serde_json::Value::Null).await {
                if let Some(arr) = result.get("skills") {
                    if let Ok(skills) = serde_json::from_value::<Vec<SkillEntry>>(arr.clone()) {
                        all_skills.set(skills);
                    }
                }
            }
            if let Ok(detail) = AgentsApi::get(&dash, &id).await {
                if let Some(skills) = detail.definition.get("skills").and_then(|v| v.as_array()) {
                    let ids: Vec<String> = skills.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                    agent_skills.set(ids);
                }
            }
            is_loading.set(false);
        });
    });

    // Toggle skill
    let toggle_skill = move |skill_id: String| {
        let mut current = agent_skills.get();
        if current.contains(&skill_id) {
            current.retain(|s| s != &skill_id);
        } else {
            current.push(skill_id);
        }
        agent_skills.set(current);
    };

    view! {
        <div class="space-y-4">
            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center">"Loading skills..."</div>
                    }.into_any();
                }

                view! {
                    <div class="space-y-4">
                        <input
                            type="text"
                            placeholder="Search skills..."
                            prop:value=move || filter.get()
                            on:input=move |ev| filter.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary text-sm"
                        />

                        <div class="bg-surface-raised border border-border rounded-xl divide-y divide-border">
                            {move || {
                                let f = filter.get().to_lowercase();
                                let current = agent_skills.get();
                                all_skills.get().into_iter()
                                    .filter(|s| f.is_empty() || s.name.to_lowercase().contains(&f) || s.id.to_lowercase().contains(&f))
                                    .map(|skill| {
                                        let sid = skill.id.clone();
                                        let sid_toggle = sid.clone();
                                        let is_enabled = current.contains(&sid);
                                        view! {
                                            <div class="flex items-center justify-between p-3">
                                                <div>
                                                    <div class="text-sm font-medium text-text-primary">{skill.name.clone()}</div>
                                                    <div class="text-xs text-text-tertiary">{skill.description.clone()}</div>
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
                                    let skills = agent_skills.get();
                                    let dash = state;
                                    spawn_local(async move {
                                        match AgentsApi::update(&dash, &id, json!({"skills": skills})).await {
                                            Ok(()) => save_message.set(Some((true, "Skills saved".to_string()))),
                                            Err(e) => save_message.set(Some((false, e))),
                                        }
                                        is_saving.set(false);
                                    });
                                }
                                disabled=move || is_saving.get()
                                class="px-6 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors"
                            >
                                {move || if is_saving.get() { "Saving..." } else { "Save Skills" }}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
