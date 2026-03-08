//! Tools Tab — per-agent tool configuration with group/tool toggles

use std::collections::HashSet;
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use crate::api::agents::{AgentsApi, ToolGroupInfo};
use crate::context::DashboardState;

#[component]
pub fn ToolsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let groups = RwSignal::new(Vec::<ToolGroupInfo>::new());
    let enabled_tools = RwSignal::new(HashSet::<String>::new());
    let original_tools = RwSignal::new(HashSet::<String>::new());
    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let error_msg = RwSignal::new(Option::<String>::None);
    let success_msg = RwSignal::new(Option::<String>::None);

    // Load tool schema + current agent config
    let id_for_load = agent_id.clone();
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = id_for_load.clone();
        spawn_local(async move {
            let schema = match AgentsApi::tools_schema(&dash).await {
                Ok(s) => s,
                Err(e) => {
                    error_msg.set(Some(format!("Failed to load tool schema: {}", e)));
                    is_loading.set(false);
                    return;
                }
            };

            let all_tools: HashSet<String> = schema.groups.iter()
                .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
                .collect();

            let (skills, blacklist) = match AgentsApi::get(&dash, &id).await {
                Ok(detail) => {
                    let def = &detail.definition;
                    let skills: Vec<String> = def.get("skills")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_else(|| vec!["*".to_string()]);
                    let blacklist: Vec<String> = def.get("skills_blacklist")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    (skills, blacklist)
                }
                Err(e) => {
                    error_msg.set(Some(format!("Failed to load agent: {}", e)));
                    is_loading.set(false);
                    return;
                }
            };

            let blacklist_set: HashSet<String> = blacklist.into_iter().collect();
            let enabled: HashSet<String> = if skills.contains(&"*".to_string()) {
                all_tools.difference(&blacklist_set).cloned().collect()
            } else {
                skills.into_iter()
                    .filter(|s| !blacklist_set.contains(s))
                    .collect()
            };

            groups.set(schema.groups);
            enabled_tools.set(enabled.clone());
            original_tools.set(enabled);
            is_loading.set(false);
        });
    });

    let toggle_tool = move |tool_name: String| {
        enabled_tools.update(|set| {
            if set.contains(&tool_name) {
                set.remove(&tool_name);
            } else {
                set.insert(tool_name);
            }
        });
        success_msg.set(None);
    };

    let toggle_group = move |tool_names: Vec<String>| {
        enabled_tools.update(|set| {
            let all_enabled = tool_names.iter().all(|t| set.contains(t));
            if all_enabled {
                for t in &tool_names {
                    set.remove(t);
                }
            } else {
                for t in tool_names {
                    set.insert(t);
                }
            }
        });
        success_msg.set(None);
    };

    let has_changes = Memo::new(move |_| {
        enabled_tools.get() != original_tools.get()
    });

    let id_for_save = StoredValue::new(agent_id.clone());
    let save = move |_| {
        let id = id_for_save.get_value();
        is_saving.set(true);
        error_msg.set(None);
        success_msg.set(None);

        spawn_local(async move {
            let enabled = enabled_tools.get();
            let all_tools: HashSet<String> = groups.get().iter()
                .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
                .collect();

            let disabled: HashSet<String> = all_tools.difference(&enabled).cloned().collect();

            let patch = if disabled.is_empty() {
                json!({ "skills": ["*"], "skills_blacklist": [] })
            } else if disabled.len() <= enabled.len() {
                let mut bl: Vec<String> = disabled.into_iter().collect();
                bl.sort();
                json!({ "skills": ["*"], "skills_blacklist": bl })
            } else {
                let mut wl: Vec<String> = enabled.iter().cloned().collect();
                wl.sort();
                json!({ "skills": wl, "skills_blacklist": [] })
            };

            match AgentsApi::update(&dash, &id, patch).await {
                Ok(()) => {
                    original_tools.set(enabled_tools.get());
                    success_msg.set(Some("Saved".to_string()));
                }
                Err(e) => {
                    error_msg.set(Some(format!("Failed to save: {}", e)));
                }
            }
            is_saving.set(false);
        });
    };

    let reset = move |_| {
        let all_tools: HashSet<String> = groups.get().iter()
            .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
            .collect();
        enabled_tools.set(all_tools);
        success_msg.set(None);
    };

    view! {
        <div class="space-y-6">
            {move || error_msg.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}
            {move || success_msg.get().map(|msg| view! {
                <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{msg}</div>
            })}

            {move || {
                if is_loading.get() {
                    return view! {
                        <div class="text-text-secondary py-8 text-center">"Loading..."</div>
                    }.into_any();
                }

                let current_groups = groups.get();

                view! {
                    <div class="space-y-4">
                        {current_groups.into_iter().map(|group| {
                            let group_tools: Vec<String> = group.tools.iter().map(|t| t.name.clone()).collect();
                            let gt_toggle = group_tools.clone();
                            let gt_check = group_tools.clone();

                            view! {
                                <div class="bg-surface-raised border border-border rounded-xl overflow-hidden">
                                    <div class="flex items-center justify-between px-5 py-3 bg-surface-sunken/50 border-b border-border">
                                        <span class="text-sm font-semibold text-text-primary">{group.name.clone()}</span>
                                        <button
                                            class="relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none"
                                            class=("bg-primary", {
                                                let gt = gt_check.clone();
                                                move || gt.iter().all(|t| enabled_tools.get().contains(t))
                                            })
                                            class=("bg-border", {
                                                let gt = gt_check.clone();
                                                move || !gt.iter().all(|t| enabled_tools.get().contains(t))
                                            })
                                            on:click=move |_| toggle_group(gt_toggle.clone())
                                        >
                                            <span
                                                class="inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform"
                                                class=("translate-x-4.5", {
                                                    let gt = gt_check.clone();
                                                    move || gt.iter().all(|t| enabled_tools.get().contains(t))
                                                })
                                                class=("translate-x-0.5", {
                                                    let gt = gt_check.clone();
                                                    move || !gt.iter().all(|t| enabled_tools.get().contains(t))
                                                })
                                            />
                                        </button>
                                    </div>
                                    <div class="divide-y divide-border/50">
                                        {group.tools.into_iter().map(|tool| {
                                            let tn = tool.name.clone();
                                            let tn_toggle = tn.clone();
                                            let tn_on = tn.clone();
                                            let tn_off = tn.clone();
                                            let tn_knob_on = tn.clone();
                                            let tn_knob_off = tn.clone();
                                            view! {
                                                <div class="flex items-center justify-between px-5 py-2.5">
                                                    <div class="flex-1 min-w-0">
                                                        <span class="text-sm font-medium text-text-primary">{tn.clone()}</span>
                                                        <p class="text-xs text-text-tertiary truncate mt-0.5">{tool.description.clone()}</p>
                                                    </div>
                                                    <button
                                                        class="relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none ml-4 flex-shrink-0"
                                                        class=("bg-primary", move || enabled_tools.get().contains(&tn_on))
                                                        class=("bg-border", move || !enabled_tools.get().contains(&tn_off))
                                                        on:click=move |_| toggle_tool(tn_toggle.clone())
                                                    >
                                                        <span
                                                            class="inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform"
                                                            class=("translate-x-4.5", move || enabled_tools.get().contains(&tn_knob_on))
                                                            class=("translate-x-0.5", move || !enabled_tools.get().contains(&tn_knob_off))
                                                        />
                                                    </button>
                                                </div>
                                            }
                                        }).collect_view()}
                                    </div>
                                </div>
                            }
                        }).collect_view()}

                        <div class="flex justify-end gap-3 pt-2">
                            <button
                                class="px-4 py-2 text-sm font-medium text-text-secondary bg-surface-raised border border-border rounded-lg hover:bg-surface-sunken transition-colors"
                                on:click=reset
                            >
                                "Reset to All"
                            </button>
                            <button
                                class="px-4 py-2 text-sm font-medium text-white bg-primary rounded-lg hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                disabled=move || !has_changes.get() || is_saving.get()
                                on:click=save
                            >
                                {move || if is_saving.get() { "Saving..." } else { "Save" }}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
