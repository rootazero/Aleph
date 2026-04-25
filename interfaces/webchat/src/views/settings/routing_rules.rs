//! Routing Rules Configuration View
//!
//! Provides UI for managing routing rules:
//! - List all rules (command + keyword)
//! - Add/Edit/Delete rules
//! - Reorder rules (drag & drop or move up/down)
//! - Real-time updates via config events

use crate::api::{RoutingRuleConfig, RoutingRuleInfo, RoutingRulesApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn RoutingRulesView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State
    let rules = RwSignal::new(Vec::<RoutingRuleInfo>::new());
    let selected = RwSignal::new(Option::<usize>::None);
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load rules on mount
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
            <div class="p-6 border-b border-border">
                <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, settings.routing_rules.title)}</h1>
                <p class="mt-1 text-sm text-text-secondary">
                    {t!(i18n, settings.routing_rules.description)}
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
    let i18n = use_i18n();
    view! {
        <div class="w-80 border-r border-border flex flex-col">
            // Add button
            <div class="p-4 border-b border-border">
                <button
                    on:click=move |_| selected.set(Some(usize::MAX))
                    class="w-full px-4 py-2 bg-primary hover:bg-primary-hover text-white rounded-lg transition-colors"
                >
                    {t!(i18n, settings.routing_rules.add_rule)}
                </button>
            </div>

            // Rules list
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if loading.get() {
                        view! {
                            <div class="p-4 text-center text-text-secondary">
                                {t_string!(i18n, common.loading).to_string()}
                            </div>
                        }.into_any()
                    } else if rules.get().is_empty() {
                        view! {
                            <div class="p-4 text-center text-text-secondary">
                                {t!(i18n, settings.routing_rules.no_rules)}
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
                    "w-full p-3 bg-primary-subtle border border-primary rounded-lg text-left transition-colors"
                } else {
                    "w-full p-3 bg-surface-sunken border border-border hover:border-border-strong rounded-lg text-left transition-colors"
                }
            }
        >
            <div class="flex items-center justify-between mb-1">
                <span class="text-xs font-medium text-primary">
                    {rule_type.to_uppercase()}
                </span>
                <span class="text-xs text-text-tertiary">
                    {"#"}{index}
                </span>
            </div>
            <div class="text-sm text-text-primary font-mono truncate">
                {regex}
            </div>
            {move || {
                if let Some(prov) = provider.clone() {
                    view! {
                        <div class="mt-1 text-xs text-text-secondary">
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
    let i18n = use_i18n();

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
                if p.is_empty() {
                    None
                } else {
                    Some(p)
                }
            },
            system_prompt: {
                let s = form_system_prompt.get();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            },
            strip_prefix: None,
            intent_type: None,
            preferred_model: None,
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
                        <div class="flex items-center justify-center h-full text-text-tertiary">
                            {t!(i18n, settings.routing_rules.select_or_add)}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-8 max-w-3xl mx-auto">
                            // Header
                            <div class="mb-6">
                                <h2 class="text-2xl font-bold text-text-primary mb-2">
                                    {move || if is_new() { t_string!(i18n, settings.routing_rules.add_routing_rule).to_string() } else { t_string!(i18n, settings.routing_rules.edit_routing_rule).to_string() }}
                                </h2>
                                <p class="text-sm text-text-secondary">
                                    {t!(i18n, settings.routing_rules.configure_rules)}
                                </p>
                            </div>

                            // Error message
                            {move || {
                                if let Some(err) = error.get() {
                                    view! {
                                        <div class="mb-4 p-4 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
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
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, settings.routing_rules.rule_type)}
                                    </label>
                                    <select
                                        prop:value=move || form_rule_type.get()
                                        on:change=move |ev| form_rule_type.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                    >
                                        <option value="command">{t!(i18n, settings.routing_rules.command_type)}</option>
                                        <option value="keyword">{t!(i18n, settings.routing_rules.keyword_type)}</option>
                                    </select>
                                </div>

                                // Regex Pattern
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, settings.routing_rules.regex_pattern)}
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_regex.get()
                                        on:input=move |ev| form_regex.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono focus:outline-none focus:border-primary"
                                        placeholder="^/draw\\s+"
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        {t!(i18n, settings.routing_rules.regex_hint)}
                                    </p>
                                </div>

                                // Provider (for command rules)
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, settings.routing_rules.provider_label)}
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_provider.get()
                                        on:input=move |ev| form_provider.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder="openai, claude, gemini"
                                    />
                                    <p class="mt-1 text-xs text-text-tertiary">
                                        {t!(i18n, settings.routing_rules.provider_hint)}
                                    </p>
                                </div>

                                // System Prompt
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, settings.routing_rules.system_prompt)}
                                    </label>
                                    <textarea
                                        prop:value=move || form_system_prompt.get()
                                        on:input=move |ev| form_system_prompt.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
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
                                    class="px-6 py-2 bg-primary hover:bg-primary-hover disabled:bg-primary/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                >
                                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                </button>

                                {move || {
                                    if !is_new() {
                                        view! {
                                            <button
                                                on:click=on_delete
                                                prop:disabled=move || saving.get()
                                                class="px-6 py-2 bg-danger hover:bg-danger disabled:bg-danger/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                                            >
                                                {t!(i18n, settings.routing_rules.delete_rule)}
                                            </button>
                                        }.into_any()
                                    } else {
                                        view! { <span></span> }.into_any()
                                    }
                                }}

                                <button
                                    on:click=move |_| selected.set(None)
                                    class="px-6 py-2 bg-surface-sunken hover:bg-surface-sunken text-text-primary rounded-lg transition-colors"
                                >
                                    {t!(i18n, common.cancel)}
                                </button>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}
