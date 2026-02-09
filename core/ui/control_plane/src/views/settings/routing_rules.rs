//! Routing Rules Configuration View
//!
//! Provides UI for managing routing rules:
//! - List all rules (command + keyword)
//! - Add/Edit/Delete rules
//! - Reorder rules (drag & drop or move up/down)
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{RoutingRulesApi, RoutingRuleInfo, RoutingRuleConfig};

#[component]
pub fn RoutingRulesView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let rules = RwSignal::new(Vec::<RoutingRuleInfo>::new());
    let selected = RwSignal::new(Option::<usize>::None);
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load rules on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                match RoutingRulesApi::list(&state).await {
                    Ok(list) => {
                        rules.set(list);
                        loading.set(false);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load rules: {}", e)));
                        loading.set(false);
                    }
                }
            });
        }
    });

    // Subscribe to config events
    Effect::new(move || {
        if state.is_connected.get() {
            // TODO: Subscribe to config.routing_rules.* events
            // and reload rules when changes occur
        }
    });

    view! {
        <div class="flex flex-col h-full">
            // Header
            <div class="p-6 border-b border-slate-700">
                <h1 class="text-2xl font-bold text-slate-200">"Routing Rules"</h1>
                <p class="mt-1 text-sm text-slate-400">
                    "Configure AI routing rules for commands and keywords"
                </p>
            </div>

            // Content
            <div class="flex-1 flex overflow-hidden">
                <RulesList rules=rules selected=selected loading=loading />
                <RuleEditor rules=rules selected=selected saving=saving error=error />
            </div>
        </div>
    }
}

// ============================================================================
// Rules List Component
// ============================================================================

#[component]
fn RulesList(
    rules: RwSignal<Vec<RoutingRuleInfo>>,
    selected: RwSignal<Option<usize>>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <div class="w-80 border-r border-slate-700 flex flex-col">
            // Add button
            <div class="p-4 border-b border-slate-700">
                <button
                    on:click=move |_| selected.set(Some(usize::MAX))
                    class="w-full px-4 py-2 bg-indigo-600 hover:bg-indigo-700 text-white rounded-lg transition-colors"
                >
                    "+ Add Rule"
                </button>
            </div>

            // Rules list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if loading.get() {
                        view! {
                            <div class="p-4 text-center text-slate-400">
                                "Loading..."
                            </div>
                        }.into_any()
                    } else if rules.get().is_empty() {
                        view! {
                            <div class="p-4 text-center text-slate-400">
                                "No routing rules configured"
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-2 space-y-2">
                                {move || {
                                    rules.get().iter().enumerate().map(|(idx, rule)| {
                                        let rule = rule.clone();
                                        let is_selected = Signal::derive(move || selected.get() == Some(idx));
                                        view! {
                                            <RuleCard rule=rule index=idx is_selected=is_selected selected=selected />
                                        }
                                    }).collect::<Vec<_>>()
                                }}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ============================================================================
// Rule Card Component
// ============================================================================

#[component]
fn RuleCard(
    rule: RoutingRuleInfo,
    index: usize,
    is_selected: Signal<bool>,
    selected: RwSignal<Option<usize>>,
) -> impl IntoView {
    let regex = rule.regex.clone();
    let rule_type = rule.rule_type.clone();
    let provider = rule.provider.clone();

    view! {
        <button
            on:click=move |_| selected.set(Some(index))
            class=move || {
                if is_selected.get() {
                    "w-full p-3 bg-indigo-600/20 border border-indigo-500/50 rounded-lg text-left transition-colors"
                } else {
                    "w-full p-3 bg-slate-800 border border-slate-700 hover:border-slate-600 rounded-lg text-left transition-colors"
                }
            }
        >
            <div class="flex items-center justify-between mb-1">
                <span class="text-xs font-medium text-indigo-400">
                    {rule_type.to_uppercase()}
                </span>
                <span class="text-xs text-slate-500">
                    {"#"}{index}
                </span>
            </div>
            <div class="text-sm text-slate-200 font-mono truncate">
                {regex}
            </div>
            {move || {
                if let Some(prov) = provider.clone() {
                    view! {
                        <div class="mt-1 text-xs text-slate-400">
                            {prov}
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}
        </button>
    }
}

// ============================================================================
// Rule Editor Component
// ============================================================================

#[component]
fn RuleEditor(
    rules: RwSignal<Vec<RoutingRuleInfo>>,
    selected: RwSignal<Option<usize>>,
    saving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Form state
    let form_rule_type = RwSignal::new(String::from("command"));
    let form_regex = RwSignal::new(String::new());
    let form_provider = RwSignal::new(String::new());
    let form_system_prompt = RwSignal::new(String::new());

    let is_new = move || selected.get() == Some(usize::MAX);
    let is_editing = move || selected.get().is_some();

    // Load rule data when selection changes
    Effect::new(move || {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                // Reset form for new rule
                form_rule_type.set(String::from("command"));
                form_regex.set(String::new());
                form_provider.set(String::new());
                form_system_prompt.set(String::new());
            } else {
                // Load existing rule
                if let Some(rule) = rules.get().get(idx) {
                    form_rule_type.set(rule.rule_type.clone());
                    form_regex.set(rule.regex.clone());
                    form_provider.set(rule.provider.clone().unwrap_or_default());
                    form_system_prompt.set(rule.system_prompt.clone().unwrap_or_default());
                }
            }
        }
    });

    // Handle save
    let on_save = move |_| {
        let regex = form_regex.get();
        if regex.is_empty() {
            error.set(Some("Regex pattern is required".to_string()));
            return;
        }

        saving.set(true);
        error.set(None);

        let rule_config = RoutingRuleConfig {
            rule_type: Some(form_rule_type.get()),
            regex: regex.clone(),
            provider: {
                let p = form_provider.get();
                if p.is_empty() { None } else { Some(p) }
            },
            system_prompt: {
                let s = form_system_prompt.get();
                if s.is_empty() { None } else { Some(s) }
            },
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            preferred_model: None,
            context_format: None,
            icon: None,
        };

        spawn_local(async move {
            let result = if is_new() {
                RoutingRulesApi::create(&state, rule_config).await
            } else if let Some(idx) = selected.get() {
                RoutingRulesApi::update(&state, idx, rule_config).await
            } else {
                Err("No rule selected".to_string())
            };

            match result {
                Ok(()) => {
                    error.set(None);
                    // Reload rules list
                    if let Ok(list) = RoutingRulesApi::list(&state).await {
                        rules.set(list);
                    }
                    // Clear selection
                    selected.set(None);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to save: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    // Handle delete
    let on_delete = move |_| {
        if let Some(idx) = selected.get() {
            if idx == usize::MAX {
                return;
            }

            saving.set(true);
            error.set(None);

            spawn_local(async move {
                match RoutingRulesApi::delete(&state, idx).await {
                    Ok(()) => {
                        error.set(None);
                        // Reload rules list
                        if let Ok(list) = RoutingRulesApi::list(&state).await {
                            rules.set(list);
                        }
                        // Clear selection
                        selected.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to delete: {}", e)));
                    }
                }
                saving.set(false);
            });
        }
    };

    view! {
        <div class="flex-1 overflow-y-auto">
            {move || {
                if !is_editing() {
                    view! {
                        <div class="flex items-center justify-center h-full text-slate-500">
                            "Select a rule to edit or add a new one"
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-8 max-w-3xl mx-auto">
                            // Header
                            <div class="mb-6">
                                <h2 class="text-2xl font-bold text-slate-200 mb-2">
                                    {move || if is_new() { "Add Routing Rule" } else { "Edit Routing Rule" }}
                                </h2>
                                <p class="text-sm text-slate-400">
                                    "Configure routing rules for AI provider selection"
                                </p>
                            </div>

                            // Error message
                            {move || {
                                if let Some(err) = error.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-red-500/10 border border-red-500/20 rounded-lg text-red-400 text-sm">
                                            {err}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div></div> }.into_any()
                                }
                            }}

                            // Form
                            <div class="space-y-6">
                                // Rule Type
                                <div>
                                    <label class="block text-sm font-medium text-slate-300 mb-2">
                                        "Rule Type"
                                    </label>
                                    <select
                                        prop:value=move || form_rule_type.get()
                                        on:change=move |ev| form_rule_type.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-slate-800 border border-slate-700 rounded-lg text-slate-200 focus:outline-none focus:border-indigo-500"
                                    >
                                        <option value="command">"Command"</option>
                                        <option value="keyword">"Keyword"</option>
                                    </select>
                                </div>

                                // Regex Pattern
                                <div>
                                    <label class="block text-sm font-medium text-slate-300 mb-2">
                                        "Regex Pattern"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_regex.get()
                                        on:input=move |ev| form_regex.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-slate-800 border border-slate-700 rounded-lg text-slate-200 font-mono focus:outline-none focus:border-indigo-500"
                                        placeholder="^/draw\\s+"
                                    />
                                    <p class="mt-1 text-xs text-slate-500">
                                        "Regular expression to match user input"
                                    </p>
                                </div>

                                // Provider (for command rules)
                                <div>
                                    <label class="block text-sm font-medium text-slate-300 mb-2">
                                        "Provider"
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_provider.get()
                                        on:input=move |ev| form_provider.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-slate-800 border border-slate-700 rounded-lg text-slate-200 focus:outline-none focus:border-indigo-500"
                                        placeholder="openai, claude, gemini"
                                    />
                                    <p class="mt-1 text-xs text-slate-500">
                                        "Required for command rules, ignored for keyword rules"
                                    </p>
                                </div>

                                // System Prompt
                                <div>
                                    <label class="block text-sm font-medium text-slate-300 mb-2">
                                        "System Prompt"
                                    </label>
                                    <textarea
                                        prop:value=move || form_system_prompt.get()
                                        on:input=move |ev| form_system_prompt.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-slate-800 border border-slate-700 rounded-lg text-slate-200 focus:outline-none focus:border-indigo-500"
                                        rows="4"
                                        placeholder="You are a helpful assistant..."
                                    ></textarea>
                                </div>
                            </div>

                            // Actions
                            <div class="mt-8 flex items-center gap-3">
                                <button
                                    on:click=on_save
                                    prop:disabled=move || saving.get()
                                    class="px-6 py-2 bg-indigo-600 hover:bg-indigo-700 disabled:bg-indigo-600/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                >
                                    {move || if saving.get() { "Saving..." } else { "Save" }}
                                </button>

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_delete
                                                prop:disabled=move || saving.get()
                                                class="px-6 py-2 bg-red-600 hover:bg-red-700 disabled:bg-red-600/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                "Delete"
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                <button
                                    on:click=move |_| selected.set(None)
                                    class="px-6 py-2 bg-slate-700 hover:bg-slate-600 text-slate-200 rounded-lg transition-colors"
                                >
                                    "Cancel"
                                </button>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}
