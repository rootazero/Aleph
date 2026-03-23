//! Tools Tab — per-agent tool permissions with three-state segmented controls
//!
//! Each tool shows Allow / Ask / Deny. Global policy restrictions are enforced:
//! - Globally denied tools are greyed out and locked at Deny
//! - Globally "ask" tools cannot be elevated to Allow

use std::collections::HashMap;
use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::api::agents::{AgentsApi, ToolGroupInfo};
use crate::api::tool_permissions::ToolPermissionsApi;
use crate::context::DashboardState;
use crate::i18n::*;

/// Permission level constants
const ALLOW: &str = "allow";
const ASK: &str = "ask";
const DENY: &str = "deny";

#[component]
pub fn ToolsTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let groups = RwSignal::new(Vec::<ToolGroupInfo>::new());
    // Current per-tool permission state (tool_name -> "allow"|"ask"|"deny")
    let tool_perms = RwSignal::new(HashMap::<String, String>::new());
    // Snapshot at load time for change detection
    let original_perms = RwSignal::new(HashMap::<String, String>::new());
    // The agent-level default permission
    let default_perm = RwSignal::new(ALLOW.to_string());
    let original_default = RwSignal::new(ALLOW.to_string());
    // Global restrictions from policy (tool_name -> global effective permission)
    let global_effective = RwSignal::new(HashMap::<String, String>::new());
    let global_default = RwSignal::new(ALLOW.to_string());

    let is_loading = RwSignal::new(true);
    let is_saving = RwSignal::new(false);
    let error_msg = RwSignal::new(Option::<String>::None);
    let success_msg = RwSignal::new(Option::<String>::None);

    // Load tool schema + permissions
    let id_for_load = agent_id.clone();
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = id_for_load.clone();
        spawn_local(async move {
            // Load tool schema for group/tool structure
            let schema = match AgentsApi::tools_schema(&dash).await {
                Ok(s) => s,
                Err(e) => {
                    error_msg.set(Some(format!("Failed to load tool schema: {}", e)));
                    is_loading.set(false);
                    return;
                }
            };

            // Load agent-level permissions (includes effective + global info)
            let perms = match ToolPermissionsApi::get_agent(&dash, &id).await {
                Ok(p) => p,
                Err(e) => {
                    error_msg.set(Some(format!("Failed to load permissions: {}", e)));
                    is_loading.set(false);
                    return;
                }
            };

            // Build the effective per-tool map
            let eff_default = perms.effective_default.as_deref().unwrap_or(&perms.default);
            let effective_map: HashMap<String, String> = perms.effective
                .clone()
                .unwrap_or_default();

            // Collect all tool names from schema
            let all_tools: Vec<String> = schema.groups.iter()
                .flat_map(|g| g.tools.iter().map(|t| t.name.clone()))
                .collect();

            // Build current permission map from effective data
            let mut current = HashMap::new();
            for t in &all_tools {
                let perm = effective_map.get(t)
                    .cloned()
                    .unwrap_or_else(|| eff_default.to_string());
                current.insert(t.clone(), perm);
            }

            // Build global ceiling map (from global_overrides + global default via get_global)
            // We use the agent response's global_overrides field
            let g_overrides = perms.global_overrides.clone().unwrap_or_default();

            // Also load global default separately for accurate ceiling
            let g_default = match ToolPermissionsApi::get_global(&dash).await {
                Ok(gp) => {
                    // Build full global map
                    let mut gmap = HashMap::new();
                    for t in &all_tools {
                        let gperm = gp.overrides.get(t)
                            .cloned()
                            .unwrap_or_else(|| gp.default.clone());
                        gmap.insert(t.clone(), gperm);
                    }
                    global_effective.set(gmap);
                    gp.default
                }
                Err(_) => {
                    // Fallback: use global_overrides from agent response
                    let mut gmap = HashMap::new();
                    for t in &all_tools {
                        if let Some(v) = g_overrides.get(t) {
                            gmap.insert(t.clone(), v.clone());
                        }
                    }
                    global_effective.set(gmap);
                    ALLOW.to_string()
                }
            };

            global_default.set(g_default);
            default_perm.set(perms.default.clone());
            original_default.set(perms.default);
            groups.set(schema.groups);
            tool_perms.set(current.clone());
            original_perms.set(current);
            is_loading.set(false);
        });
    });

    // Set a single tool's permission
    let set_tool_perm = move |tool_name: String, level: String| {
        tool_perms.update(|map| {
            map.insert(tool_name, level);
        });
        success_msg.set(None);
    };

    // Toggle entire group to Allow or Deny (skipping globally restricted tools)
    let toggle_group = move |tool_names: Vec<String>| {
        let ge = global_effective.get();
        let gd = global_default.get();
        tool_perms.update(|map| {
            // If all non-restricted tools are Allow, switch to Deny; otherwise switch to Allow
            let non_restricted: Vec<&String> = tool_names.iter()
                .filter(|t| {
                    let g = ge.get(*t).unwrap_or(&gd);
                    g != DENY
                })
                .collect();

            let all_allowed = non_restricted.iter().all(|t| {
                map.get(*t).map(|v| v == ALLOW).unwrap_or(false)
            });

            if all_allowed {
                // Switch non-restricted to Deny
                for t in &non_restricted {
                    map.insert((*t).clone(), DENY.to_string());
                }
            } else {
                // Switch to highest allowed level for each tool
                for t in &non_restricted {
                    let g = ge.get(*t).unwrap_or(&gd);
                    if g == ALLOW {
                        map.insert((*t).clone(), ALLOW.to_string());
                    } else {
                        // global is "ask", best we can do is "ask"
                        map.insert((*t).clone(), ASK.to_string());
                    }
                }
            }
        });
        success_msg.set(None);
    };

    let has_changes = Memo::new(move |_| {
        tool_perms.get() != original_perms.get() || default_perm.get() != original_default.get()
    });

    let id_for_save = StoredValue::new(agent_id.clone());
    let save = move |_| {
        let id = id_for_save.get_value();
        is_saving.set(true);
        error_msg.set(None);
        success_msg.set(None);

        spawn_local(async move {
            let current_default = default_perm.get();
            let current_perms = tool_perms.get();

            // Build overrides: only tools that differ from the agent default
            let overrides: HashMap<String, String> = current_perms.into_iter()
                .filter(|(_, v)| *v != current_default)
                .collect();

            match ToolPermissionsApi::update_agent(&dash, &id, &current_default, &overrides).await {
                Ok(()) => {
                    original_perms.set(tool_perms.get());
                    original_default.set(default_perm.get());
                    success_msg.set(Some(t_string!(i18n, agents.tools.saved).to_string()));
                }
                Err(e) => {
                    error_msg.set(Some(format!("Failed to save: {}", e)));
                }
            }
            is_saving.set(false);
        });
    };

    let reset = move |_| {
        tool_perms.set(original_perms.get());
        default_perm.set(original_default.get());
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
                        <div class="text-text-secondary py-8 text-center">{t!(i18n, common.loading)}</div>
                    }.into_any();
                }

                let current_groups = groups.get();

                view! {
                    <div class="space-y-4">
                        // Default permission selector
                        <div class="flex items-center justify-between p-4 bg-surface-raised border border-border rounded-xl">
                            <div>
                                <span class="text-sm font-semibold text-text-primary">{t!(i18n, agents.tools.default_permission)}</span>
                                <p class="text-xs text-text-tertiary mt-0.5">{t!(i18n, agents.tools.default_permission_hint)}</p>
                            </div>
                            <PermissionSegmentedControl
                                value=Signal::derive(move || default_perm.get())
                                on_change=move |level: String| {
                                    default_perm.set(level);
                                    success_msg.set(None);
                                }
                                globally_denied=Signal::derive(move || false)
                                globally_ask=Signal::derive(move || false)
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
                                                    let ge = global_effective.get();
                                                    let gd = global_default.get();
                                                    gt.iter().all(|t| {
                                                        let g = ge.get(t).unwrap_or(&gd);
                                                        g == DENY || perms.get(t).map(|v| v == ALLOW).unwrap_or(false)
                                                    })
                                                }
                                            })
                                            class=("bg-border", {
                                                let gt = gt_check.clone();
                                                move || {
                                                    let perms = tool_perms.get();
                                                    let ge = global_effective.get();
                                                    let gd = global_default.get();
                                                    !gt.iter().all(|t| {
                                                        let g = ge.get(t).unwrap_or(&gd);
                                                        g == DENY || perms.get(t).map(|v| v == ALLOW).unwrap_or(false)
                                                    })
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
                                                        let ge = global_effective.get();
                                                        let gd = global_default.get();
                                                        gt.iter().all(|t| {
                                                            let g = ge.get(t).unwrap_or(&gd);
                                                            g == DENY || perms.get(t).map(|v| v == ALLOW).unwrap_or(false)
                                                        })
                                                    }
                                                })
                                                class=("translate-x-0.5", {
                                                    let gt = gt_check.clone();
                                                    move || {
                                                        let perms = tool_perms.get();
                                                        let ge = global_effective.get();
                                                        let gd = global_default.get();
                                                        !gt.iter().all(|t| {
                                                            let g = ge.get(t).unwrap_or(&gd);
                                                            g == DENY || perms.get(t).map(|v| v == ALLOW).unwrap_or(false)
                                                        })
                                                    }
                                                })
                                            />
                                        </button>
                                    </div>
                                    <div class="divide-y divide-border/50">
                                        {group.tools.into_iter().map(|tool| {
                                            let tn = tool.name.clone();
                                            let tn_perm = tn.clone();
                                            let tn_global = tn.clone();
                                            let tn_global_ask = tn.clone();
                                            let tn_set = tn.clone();
                                            let tn_disabled = tn.clone();

                                            view! {
                                                <div
                                                    class="flex items-center justify-between px-5 py-2.5"
                                                    class=("opacity-50", move || {
                                                        let ge = global_effective.get();
                                                        let gd = global_default.get();
                                                        ge.get(&tn_disabled).unwrap_or(&gd) == DENY
                                                    })
                                                >
                                                    <div class="flex-1 min-w-0">
                                                        <span class="text-sm font-medium text-text-primary">{tn.clone()}</span>
                                                        <p class="text-xs text-text-tertiary truncate mt-0.5">{tool.description.clone()}</p>
                                                    </div>
                                                    <div class="ml-4 flex-shrink-0">
                                                        <PermissionSegmentedControl
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
                                                            globally_denied=Signal::derive(move || {
                                                                let ge = global_effective.get();
                                                                let gd = global_default.get();
                                                                ge.get(&tn_global).unwrap_or(&gd) == DENY
                                                            })
                                                            globally_ask=Signal::derive(move || {
                                                                let ge = global_effective.get();
                                                                let gd = global_default.get();
                                                                ge.get(&tn_global_ask).unwrap_or(&gd) == ASK
                                                            })
                                                        />
                                                    </div>
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
                                {t!(i18n, agents.tools.reset)}
                            </button>
                            <button
                                class="px-4 py-2 text-sm font-medium text-white bg-primary rounded-lg hover:bg-primary/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                disabled=move || !has_changes.get() || is_saving.get()
                                on:click=save
                            >
                                {move || if is_saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, agents.tools.save).to_string() }}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// Reusable three-state segmented control: Allow / Ask / Deny
#[component]
fn PermissionSegmentedControl(
    /// Current value: "allow" | "ask" | "deny"
    value: Signal<String>,
    /// Callback when user clicks a segment
    on_change: impl Fn(String) + 'static + Clone,
    /// Whether this tool is globally denied (lock to deny, disable all buttons)
    globally_denied: Signal<bool>,
    /// Whether this tool is globally "ask" (cannot select Allow)
    globally_ask: Signal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
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
            // Allow button
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors"
                class=("bg-success", move || value.get() == ALLOW && !globally_denied.get())
                class=("text-white", move || value.get() == ALLOW && !globally_denied.get())
                class=("text-text-secondary", move || value.get() != ALLOW || globally_denied.get())
                class=("hover:bg-surface-sunken", move || value.get() != ALLOW && !globally_denied.get() && !globally_ask.get())
                class=("cursor-not-allowed", move || globally_denied.get() || globally_ask.get())
                class=("opacity-40", move || globally_denied.get())
                disabled=move || globally_denied.get() || globally_ask.get()
                on:click=on_allow
                title=move || {
                    if globally_denied.get() { t_string!(i18n, agents.tools.globally_denied_title).to_string() }
                    else if globally_ask.get() { t_string!(i18n, agents.tools.cannot_exceed_global_ask).to_string() }
                    else { t_string!(i18n, agents.tools.allow_tool_title).to_string() }
                }
            >
                {t!(i18n, agents.tools.allow)}
            </button>
            // Ask button
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors border-x border-border"
                class=("bg-warning", move || value.get() == ASK && !globally_denied.get())
                class=("text-white", move || value.get() == ASK && !globally_denied.get())
                class=("text-text-secondary", move || value.get() != ASK || globally_denied.get())
                class=("hover:bg-surface-sunken", move || value.get() != ASK && !globally_denied.get())
                class=("cursor-not-allowed", move || globally_denied.get())
                class=("opacity-40", move || globally_denied.get())
                disabled=move || globally_denied.get()
                on:click=on_ask
                title=move || {
                    if globally_denied.get() { t_string!(i18n, agents.tools.globally_denied_title).to_string() }
                    else { t_string!(i18n, agents.tools.ask_tool_title).to_string() }
                }
            >
                {t!(i18n, agents.tools.ask)}
            </button>
            // Deny button
            <button
                class="px-2 py-0.5 text-xs font-medium transition-colors"
                class=("bg-danger", move || value.get() == DENY)
                class=("text-white", move || value.get() == DENY)
                class=("text-text-secondary", move || value.get() != DENY)
                class=("hover:bg-surface-sunken", move || value.get() != DENY && !globally_denied.get())
                on:click=on_deny
                title=move || t_string!(i18n, agents.tools.deny_tool_title).to_string()
            >
                {t!(i18n, agents.tools.deny)}
            </button>
        </div>
    }
}
