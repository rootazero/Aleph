use std::collections::HashMap;
use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::agents::{AgentsApi, ToolGroupInfo};
use crate::api::tool_permissions::ToolPermissionsApi;
use crate::context::DashboardState;

/// Permission level constants
const ALLOW: &str = "allow";
const ASK: &str = "ask";
const DENY: &str = "deny";

#[component]
pub fn PoliciesView() -> impl IntoView {
    let content_filter = RwSignal::new(false);
    let filter_level = RwSignal::new("moderate".to_string());
    let log_conversations = RwSignal::new(true);
    let data_retention_days = RwSignal::new(90);
    let allow_analytics = RwSignal::new(false);

    // Tool permissions state
    let state = expect_context::<DashboardState>();
    let groups = RwSignal::new(Vec::<ToolGroupInfo>::new());
    let tool_perms = RwSignal::new(HashMap::<String, String>::new());
    let original_perms = RwSignal::new(HashMap::<String, String>::new());
    let default_perm = RwSignal::new(ALLOW.to_string());
    let original_default = RwSignal::new(ALLOW.to_string());
    let tp_loading = RwSignal::new(true);
    let tp_saving = RwSignal::new(false);
    let tp_error = RwSignal::new(Option::<String>::None);
    let tp_success = RwSignal::new(Option::<String>::None);

    // Load tool permissions + schema
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        spawn_local(async move {
            let schema = match AgentsApi::tools_schema(&dash).await {
                Ok(s) => s,
                Err(e) => {
                    tp_error.set(Some(format!("Failed to load tool schema: {}", e)));
                    tp_loading.set(false);
                    return;
                }
            };

            let perms = match ToolPermissionsApi::get_global(&dash).await {
                Ok(p) => p,
                Err(e) => {
                    tp_error.set(Some(format!("Failed to load permissions: {}", e)));
                    tp_loading.set(false);
                    return;
                }
            };

            let all_tools: Vec<String> = schema.groups.iter()
                .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
                .collect();

            let mut current = HashMap::new();
            for t in &all_tools {
                let perm = perms.overrides.get(t)
                    .cloned()
                    .unwrap_or_else(|| perms.default.clone());
                current.insert(t.clone(), perm);
            }

            default_perm.set(perms.default.clone());
            original_default.set(perms.default);
            groups.set(schema.groups);
            tool_perms.set(current.clone());
            original_perms.set(current);
            tp_loading.set(false);
        });
    });

    let set_tool_perm = move |tool_name: String, level: String| {
        tool_perms.update(|map| {
            map.insert(tool_name, level);
        });
        tp_success.set(None);
    };

    let toggle_group = move |tool_names: Vec<String>| {
        tool_perms.update(|map| {
            let all_allowed = tool_names.iter().all(|t| {
                map.get(t).map(|v| v == ALLOW).unwrap_or(false)
            });
            let target = if all_allowed { DENY } else { ALLOW };
            for t in &tool_names {
                map.insert(t.clone(), target.to_string());
            }
        });
        tp_success.set(None);
    };

    let tp_has_changes = Memo::new(move |_| {
        tool_perms.get() != original_perms.get() || default_perm.get() != original_default.get()
    });

    let tp_save = move |_| {
        tp_saving.set(true);
        tp_error.set(None);
        tp_success.set(None);

        spawn_local(async move {
            let current_default = default_perm.get();
            let current_perms = tool_perms.get();

            let overrides: HashMap<String, String> = current_perms.into_iter()
                .filter(|(_, v)| *v != current_default)
                .collect();

            match ToolPermissionsApi::update_global(&dash, &current_default, &overrides).await {
                Ok(()) => {
                    original_perms.set(tool_perms.get());
                    original_default.set(default_perm.get());
                    tp_success.set(Some("Saved. Changes will take effect on next agent run.".to_string()));
                }
                Err(e) => {
                    tp_error.set(Some(format!("Failed to save: {}", e)));
                }
            }
            tp_saving.set(false);
        });
    };

    let tp_reset = move |_| {
        tool_perms.set(original_perms.get());
        default_perm.set(original_default.get());
        tp_success.set(None);
    };

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-2xl space-y-6">
                // Page Header
                <div>
                    <h1 class="text-2xl font-semibold text-text-primary mb-1">
                        "Policies"
                    </h1>
                    <p class="text-sm text-text-secondary">
                        "Configure tool permissions, content moderation, and data policies"
                    </p>
                </div>

                // Tool Permissions Section
                <div class="space-y-4">
                    <div>
                        <h2 class="text-lg font-medium text-text-primary">"Tool Permissions"</h2>
                        <p class="text-xs text-text-tertiary mt-1">
                            "Global ceiling for all agents. Per-agent settings cannot exceed these limits."
                        </p>
                    </div>

                    {move || tp_error.get().map(|e| view! {
                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                    })}
                    {move || tp_success.get().map(|msg| view! {
                        <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm">{msg}</div>
                    })}

                    {move || {
                        if tp_loading.get() {
                            return view! {
                                <div class="text-text-secondary py-4 text-center text-sm">"Loading tool permissions..."</div>
                            }.into_any();
                        }

                        let current_groups = groups.get();

                        view! {
                            <div class="space-y-3">
                                // Default permission selector
                                <div class="flex items-center justify-between p-4 bg-surface-raised border border-border rounded-xl">
                                    <div>
                                        <span class="text-sm font-semibold text-text-primary">"Default Permission"</span>
                                        <p class="text-xs text-text-tertiary mt-0.5">"Applied to tools without specific overrides"</p>
                                    </div>
                                    <PolicySegmentedControl
                                        value=Signal::derive(move || default_perm.get())
                                        on_change=move |level: String| {
                                            default_perm.set(level);
                                            tp_success.set(None);
                                        }
                                    />
                                </div>

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
                                                        move || {
                                                            let perms = tool_perms.get();
                                                            gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                        }
                                                    })
                                                    class=("bg-border", {
                                                        let gt = gt_check.clone();
                                                        move || {
                                                            let perms = tool_perms.get();
                                                            !gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                        }
                                                    })
                                                    on:click=move |_| toggle_group(gt_toggle.clone())
                                                >
                                                    <span
                                                        class="inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform"
                                                        class=("translate-x-4.5", {
                                                            let gt = gt_check.clone();
                                                            move || {
                                                                let perms = tool_perms.get();
                                                                gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                            }
                                                        })
                                                        class=("translate-x-0.5", {
                                                            let gt = gt_check.clone();
                                                            move || {
                                                                let perms = tool_perms.get();
                                                                !gt.iter().all(|t| perms.get(t).map(|v| v == ALLOW).unwrap_or(false))
                                                            }
                                                        })
                                                    />
                                                </button>
                                            </div>
                                            <div class="divide-y divide-border/50">
                                                {group.tools.into_iter().map(|tool| {
                                                    let tn = tool.name.clone();
                                                    let tn_perm = tn.clone();
                                                    let tn_set = tn.clone();

                                                    view! {
                                                        <div class="flex items-center justify-between px-5 py-2.5">
                                                            <div class="flex-1 min-w-0">
                                                                <span class="text-sm font-medium text-text-primary">{tn.clone()}</span>
                                                                <p class="text-xs text-text-tertiary truncate mt-0.5">{tool.description.clone()}</p>
                                                            </div>
                                                            <div class="ml-4 flex-shrink-0">
                                                                <PolicySegmentedControl
                                                                    value=Signal::derive(move || {
                                                                        let perms = tool_perms.get();
                                                                        perms.get(&tn_perm).cloned().unwrap_or_else(|| default_perm.get())
                                                                    })
                                                                    on_change={
                                                                        let tn_c = tn_set.clone();
                                                                        move |level: String| {
                                                                            set_tool_perm(tn_c.clone(), level);
                                                                        }
                                                                    }
                                                                />
                                                            </div>
                                                        </div>
                                                    }
                                                }).collect_view()}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}

                                <p class="text-xs text-text-tertiary italic">
                                    "Changes will take effect on next agent run."
                                </p>

                                <div class="flex justify-end gap-3 pt-1">
                                    <button
                                        class="px-4 py-2 text-sm font-medium text-text-secondary bg-surface-raised border border-border rounded-lg hover:bg-surface-sunken transition-colors"
                                        on:click=tp_reset
                                    >
                                        "Reset"
                                    </button>
                                    <button
                                        class="px-4 py-2 text-sm font-medium text-white bg-primary rounded-lg hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                        disabled=move || !tp_has_changes.get() || tp_saving.get()
                                        on:click=tp_save
                                    >
                                        {move || if tp_saving.get() { "Saving..." } else { "Save" }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    }}
                </div>

                // Content Safety Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Content Safety"</h2>

                    <div class="p-4 bg-surface-raised border border-border rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Content Filter"</div>
                                <div class="text-xs text-text-secondary mt-1">
                                    "Filter potentially harmful content"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    checked=move || content_filter.get()
                                    on:change=move |ev| {
                                        content_filter.set(event_target_checked(&ev));
                                    }
                                />
                                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                            </label>
                        </div>
                    </div>

                    {move || {
                        if content_filter.get() {
                            view! {
                                <div class="p-4 bg-surface-raised border border-border rounded">
                                    <div class="flex items-center justify-between">
                                        <div>
                                            <div class="text-sm font-medium text-text-primary">"Filter Level"</div>
                                            <div class="text-xs text-text-secondary mt-1">
                                                "Strictness of content filtering"
                                            </div>
                                        </div>
                                        <select
                                            class="px-3 py-1.5 bg-surface-sunken border border-border rounded text-text-primary text-sm"
                                            on:change=move |ev| filter_level.set(event_target_value(&ev))
                                        >
                                            <option value="strict" selected=move || filter_level.get() == "strict">"Strict"</option>
                                            <option value="moderate" selected=move || filter_level.get() == "moderate">"Moderate"</option>
                                            <option value="off" selected=move || filter_level.get() == "off">"Off"</option>
                                        </select>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! { <div class="hidden"></div> }.into_any()
                        }
                    }}
                </div>

                // Data & Privacy Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Data & Privacy"</h2>

                    <div class="p-4 bg-surface-raised border border-border rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Log Conversations"</div>
                                <div class="text-xs text-text-secondary mt-1">
                                    "Save conversation history locally"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    checked=move || log_conversations.get()
                                    on:change=move |ev| {
                                        log_conversations.set(event_target_checked(&ev));
                                    }
                                />
                                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                            </label>
                        </div>
                    </div>

                    {move || {
                        if log_conversations.get() {
                            view! {
                                <div class="p-4 bg-surface-raised border border-border rounded">
                                    <div class="flex items-center justify-between">
                                        <div>
                                            <div class="text-sm font-medium text-text-primary">"Data Retention"</div>
                                            <div class="text-xs text-text-secondary mt-1">
                                                "Days to keep conversation logs"
                                            </div>
                                        </div>
                                        <div class="flex items-center gap-3 w-48">
                                            <input
                                                type="range"
                                                min="7"
                                                max="365"
                                                step="7"
                                                class="flex-1"
                                                value=move || data_retention_days.get()
                                                on:input=move |ev| {
                                                    if let Ok(val) = event_target_value(&ev).parse::<i32>() {
                                                        data_retention_days.set(val);
                                                    }
                                                }
                                            />
                                            <span class="text-xs text-text-secondary w-12 text-right font-mono">
                                                {move || format!("{}d", data_retention_days.get())}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! { <div class="hidden"></div> }.into_any()
                        }
                    }}
                </div>

                // Analytics Section
                <div class="space-y-4">
                    <h2 class="text-lg font-medium text-text-primary">"Analytics"</h2>

                    <div class="p-4 bg-surface-raised border border-border rounded">
                        <div class="flex items-center justify-between">
                            <div>
                                <div class="text-sm font-medium text-text-primary">"Allow Analytics"</div>
                                <div class="text-xs text-text-secondary mt-1">
                                    "Send anonymous usage data to improve Aleph"
                                </div>
                            </div>
                            <label class="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="sr-only peer"
                                    checked=move || allow_analytics.get()
                                    on:change=move |ev| {
                                        allow_analytics.set(event_target_checked(&ev));
                                    }
                                />
                                <div class="w-11 h-6 bg-surface-sunken peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30 rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-primary"></div>
                            </label>
                        </div>
                    </div>

                    {move || {
                        if allow_analytics.get() {
                            view! {
                                <div class="p-4 bg-primary-subtle border border-primary/20 rounded">
                                    <span class="text-sm text-info">
                                        "Analytics include: feature usage, performance metrics, and crash reports. No personal data, conversation content, or API keys are collected."
                                    </span>
                                </div>
                            }.into_any()
                        } else {
                            view! { <div class="hidden"></div> }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

/// Segmented control for global policy (no restrictions since this IS the global ceiling)
#[component]
fn PolicySegmentedControl(
    value: Signal<String>,
    on_change: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    let on_allow = {
        let cb = on_change.clone();
        move |_| cb(ALLOW.to_string())
    };
    let on_ask = {
        let cb = on_change.clone();
        move |_| cb(ASK.to_string())
    };
    let on_deny = {
        let cb = on_change.clone();
        move |_| cb(DENY.to_string())
    };

    view! {
        <div class="flex rounded-lg border border-border overflow-hidden">
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors"
                class=("bg-success", move || value.get() == ALLOW)
                class=("text-white", move || value.get() == ALLOW)
                class=("text-text-secondary", move || value.get() != ALLOW)
                class=("hover:bg-surface-sunken", move || value.get() != ALLOW)
                on:click=on_allow
            >
                "Allow"
            </button>
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors border-x border-border"
                class=("bg-warning", move || value.get() == ASK)
                class=("text-white", move || value.get() == ASK)
                class=("text-text-secondary", move || value.get() != ASK)
                class=("hover:bg-surface-sunken", move || value.get() != ASK)
                on:click=on_ask
            >
                "Ask"
            </button>
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors"
                class=("bg-danger", move || value.get() == DENY)
                class=("text-white", move || value.get() == DENY)
                class=("text-text-secondary", move || value.get() != DENY)
                class=("hover:bg-surface-sunken", move || value.get() != DENY)
                on:click=on_deny
            >
                "Deny"
            </button>
        </div>
    }
}
