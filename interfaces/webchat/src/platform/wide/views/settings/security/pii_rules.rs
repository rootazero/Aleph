//! Custom PII rule editor (sub-section + section wrapper).

use leptos::prelude::*;

use crate::api::{CustomPiiRule, CustomPiiSeverity, PiiAction, SecurityConfig};
use crate::i18n::{t, t_string, use_i18n};

use super::validate_regex;

#[component]
pub(super) fn CustomPiiRulesSubsection(
    rules: RwSignal<Vec<CustomPiiRule>>,
    pattern_errors: RwSignal<Vec<(usize, String)>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let validate_all = move || {
        let mut errors = Vec::new();
        for (i, rule) in rules.get().iter().enumerate() {
            if let Err(e) = validate_regex(&rule.pattern) {
                errors.push((i, e));
            }
        }
        let is_valid = errors.is_empty();
        pattern_errors.set(errors);
        is_valid
    };

    let add_rule = move || {
        let mut current = rules.get();
        current.push(CustomPiiRule {
            name: String::new(),
            pattern: String::new(),
            placeholder: "[CUSTOM_PII]".to_string(),
            severity: CustomPiiSeverity::Medium,
            action: PiiAction::Block,
        });
        rules.set(current);
    };

    let remove_rule = move |index: usize| {
        let mut current = rules.get();
        current.remove(index);
        rules.set(current);
        validate_all();
    };

    let update_rule = move |index: usize, field: &'static str, value: String| {
        let mut current = rules.get();
        if let Some(rule) = current.get_mut(index) {
            match field {
                "name" => rule.name = value,
                "pattern" => rule.pattern = value,
                "placeholder" => rule.placeholder = value,
                "severity" => {
                    rule.severity = match value.as_str() {
                        "low" => CustomPiiSeverity::Low,
                        "medium" => CustomPiiSeverity::Medium,
                        "high" => CustomPiiSeverity::High,
                        "critical" => CustomPiiSeverity::Critical,
                        _ => CustomPiiSeverity::Medium,
                    }
                }
                "action" => {
                    rule.action = match value.as_str() {
                        "block" => PiiAction::Block,
                        "warn" => PiiAction::Warn,
                        "off" => PiiAction::Off,
                        _ => PiiAction::Block,
                    }
                }
                _ => {}
            }
        }
        rules.set(current);
        validate_all();
    };

    view! {
        <div class="mt-6 pt-6 border-t border-border">
            <h3 class="text-sm font-semibold text-text-secondary mb-3">
                {t!(i18n, settings.security.pii_rules_title)}
            </h3>

            <div class="space-y-3">
                {move || {
                    let rule_list = rules.get();
                    rule_list.into_iter().enumerate().map(|(i, rule)| {
                        let has_err = pattern_errors.get().iter().any(|(idx, _)| *idx == i);
                        view! {
                            <div class="p-3 bg-surface-sunken rounded border border-border space-y-2">
                                <div class="flex gap-2">
                                    <input
                                        type="text"
                                        prop:value=rule.name.clone()
                                        on:input=move |ev| update_rule(i, "name", event_target_value(&ev))
                                        placeholder=move || t_string!(i18n, settings.security.pii_rules_name_placeholder).to_string()
                                        class="flex-1 px-3 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    />
                                    <button
                                        on:click=move |_| remove_rule(i)
                                        class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                    >
                                        {t!(i18n, settings.security.pii_rules_remove)}
                                    </button>
                                </div>
                                <input
                                    type="text"
                                    prop:value=rule.pattern.clone()
                                    on:input=move |ev| update_rule(i, "pattern", event_target_value(&ev))
                                    placeholder=move || t_string!(i18n, settings.security.pii_rules_regex_placeholder).to_string()
                                    class=move || format!("w-full px-3 py-1 bg-surface-raised border rounded text-sm text-text-primary {}",
                                        if has_err { "border-danger" } else { "border-border" })
                                />
                                <div class="flex gap-2">
                                    <input
                                        type="text"
                                        prop:value=rule.placeholder.clone()
                                        on:input=move |ev| update_rule(i, "placeholder", event_target_value(&ev))
                                        placeholder=move || t_string!(i18n, settings.security.pii_rules_placeholder_placeholder).to_string()
                                        class="flex-1 px-3 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    />
                                    <select
                                        prop:value=move || match rule.severity {
                                            CustomPiiSeverity::Low => "low",
                                            CustomPiiSeverity::Medium => "medium",
                                            CustomPiiSeverity::High => "high",
                                            CustomPiiSeverity::Critical => "critical",
                                        }
                                        on:change=move |ev| update_rule(i, "severity", event_target_value(&ev))
                                        class="px-2 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    >
                                        <option value="low">{t!(i18n, settings.security.pii_rules_severity_low)}</option>
                                        <option value="medium">{t!(i18n, settings.security.pii_rules_severity_medium)}</option>
                                        <option value="high">{t!(i18n, settings.security.pii_rules_severity_high)}</option>
                                        <option value="critical">{t!(i18n, settings.security.pii_rules_severity_critical)}</option>
                                    </select>
                                    <select
                                        prop:value=move || match rule.action {
                                            PiiAction::Block => "block",
                                            PiiAction::Warn => "warn",
                                            PiiAction::Off => "off",
                                        }
                                        on:change=move |ev| update_rule(i, "action", event_target_value(&ev))
                                        class="px-2 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    >
                                        <option value="block">{t!(i18n, settings.security.pii_rules_action_block)}</option>
                                        <option value="warn">{t!(i18n, settings.security.pii_rules_action_warn)}</option>
                                        <option value="off">{t!(i18n, settings.security.pii_rules_action_off)}</option>
                                    </select>
                                </div>
                            </div>
                        }
                    }).collect::<Vec<_>>()
                }}
            </div>

            <button
                on:click=move |_| add_rule()
                class="mt-3 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
            >
                {t!(i18n, settings.security.pii_rules_add)}
            </button>

            {move || {
                let errors = pattern_errors.get();
                if !errors.is_empty() {
                    Some(view! {
                        <div class="mt-2 p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                            <div class="font-semibold mb-1">{t!(i18n, settings.security.pii_rules_invalid_regex)}</div>
                            <ul class="list-disc list-inside">
                                {errors.iter().map(|(i, err)| view! {
                                    <li>{format!("#{}: {}", i + 1, err)}</li>
                                }).collect::<Vec<_>>()}
                            </ul>
                        </div>
                    })
                } else {
                    None
                }
            }}
        </div>
    }
}

#[component]
pub(super) fn CustomPiiRulesSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    let custom_rules = RwSignal::new(config.get().map(|c| c.custom_pii_rules).unwrap_or_default());
    let pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());

    // Keep the local editor in sync with the authoritative config so that an
    // empty/stale snapshot is never written back while the config is still
    // loading or after another client/CLI changed the rules.
    Effect::new(move || {
        if let Some(cfg) = config.get() {
            custom_rules.set(cfg.custom_pii_rules);
        }
    });

    let save = move |_| {
        if let Some(mut cfg) = config.get() {
            cfg.custom_pii_rules = custom_rules.get();
            config.set(Some(cfg));
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <div class="flex justify-between items-start mb-4">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary">{t!(i18n, settings.security.pii_rules_title)}</h2>
                    <p class="text-sm text-text-tertiary mt-1">
                        {t!(i18n, settings.security.pii_rules_desc)}
                    </p>
                </div>
                <button
                    on:click=save
                    class="px-4 py-1.5 text-sm bg-info text-white rounded hover:bg-primary-hover"
                >
                    {t!(i18n, settings.security.pii_rules_apply)}
                </button>
            </div>
            <CustomPiiRulesSubsection rules=custom_rules pattern_errors=pattern_errors />
        </div>
    }
}
