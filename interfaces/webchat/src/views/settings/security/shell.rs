//! Shell-security section: command allow-list + risk patterns.

use leptos::prelude::*;

use crate::api::{CustomRiskPattern, SecurityConfig};
use crate::i18n::*;

use super::validate_regex;

#[component]
pub(super) fn ShellSecuritySection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let pattern_errors = RwSignal::new(Vec::<(usize, String, String)>::new());

    let validate_all_patterns = move || {
        let mut errors = Vec::new();
        if let Some(cfg) = config.get() {
            for (i, p) in cfg.shell_security.custom_blocked.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "blocked".to_string(), e));
                }
            }
            for (i, p) in cfg.shell_security.custom_danger.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "danger".to_string(), e));
                }
            }
            for (i, p) in cfg.shell_security.custom_safe.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "safe".to_string(), e));
                }
            }
        }
        let is_valid = errors.is_empty();
        pattern_errors.set(errors);
        is_valid
    };

    let add_pattern = move |category: &'static str| {
        if let Some(mut cfg) = config.get() {
            let new_pattern = CustomRiskPattern {
                pattern: String::new(),
                reason: None,
            };
            match category {
                "blocked" => cfg.shell_security.custom_blocked.push(new_pattern),
                "danger" => cfg.shell_security.custom_danger.push(new_pattern),
                "safe" => cfg.shell_security.custom_safe.push(new_pattern),
                _ => {}
            }
            config.set(Some(cfg));
        }
    };

    let remove_pattern = move |category: &'static str, index: usize| {
        if let Some(mut cfg) = config.get() {
            match category {
                "blocked" => {
                    cfg.shell_security.custom_blocked.remove(index);
                }
                "danger" => {
                    cfg.shell_security.custom_danger.remove(index);
                }
                "safe" => {
                    cfg.shell_security.custom_safe.remove(index);
                }
                _ => {}
            }
            config.set(Some(cfg));
            validate_all_patterns();
        }
    };

    let update_pattern =
        move |category: &'static str, index: usize, field: &'static str, value: String| {
            if let Some(mut cfg) = config.get() {
                let pattern = match category {
                    "blocked" => cfg.shell_security.custom_blocked.get_mut(index),
                    "danger" => cfg.shell_security.custom_danger.get_mut(index),
                    "safe" => cfg.shell_security.custom_safe.get_mut(index),
                    _ => None,
                };
                if let Some(p) = pattern {
                    match field {
                        "pattern" => p.pattern = value,
                        "reason" => p.reason = if value.is_empty() { None } else { Some(value) },
                        _ => {}
                    }
                }
                config.set(Some(cfg));
                validate_all_patterns();
            }
        };

    let has_error = move |category: &'static str, index: usize| -> bool {
        pattern_errors
            .get()
            .iter()
            .any(|(i, c, _)| *i == index && c == category)
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                "Shell Command Security"
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure custom risk patterns for shell command execution."
            </p>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || {
                            config.get()
                                .map(|c| c.shell_security.enable_custom_patterns)
                                .unwrap_or(false)
                        }
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.shell_security.enable_custom_patterns =
                                    event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">
                            "Enable Custom Risk Patterns"
                        </div>
                        <div class="text-xs text-text-tertiary">
                            "When enabled, custom patterns supplement built-in security rules"
                        </div>
                    </div>
                </label>

                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">
                        "Blocked Patterns (execution denied)"
                    </h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config
                                .get()
                                .map(|c| c.shell_security.custom_blocked.clone())
                                .unwrap_or_default();
                            patterns
                                .into_iter()
                                .enumerate()
                                .map(|(i, p)| {
                                    let has_err = has_error("blocked", i);
                                    view! {
                                        <div class="flex gap-2 items-start">
                                            <div class="flex-1 space-y-1">
                                                <input
                                                    type="text"
                                                    prop:value=p.pattern.clone()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "blocked",
                                                            i,
                                                            "pattern",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Regex pattern..."
                                                    class=move || {
                                                        format!(
                                                            "w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                            if has_err {
                                                                "border-danger"
                                                            } else {
                                                                "border-border"
                                                            }
                                                        )
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    prop:value=p.reason.clone().unwrap_or_default()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "blocked",
                                                            i,
                                                            "reason",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Reason (optional)..."
                                                    class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                                />
                                            </div>
                                            <button
                                                on:click=move |_| remove_pattern("blocked", i)
                                                class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                            >
                                                "Remove"
                                            </button>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("blocked")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Blocked Pattern"
                    </button>
                </div>

                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">
                        "Danger Patterns (require approval)"
                    </h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config
                                .get()
                                .map(|c| c.shell_security.custom_danger.clone())
                                .unwrap_or_default();
                            patterns
                                .into_iter()
                                .enumerate()
                                .map(|(i, p)| {
                                    let has_err = has_error("danger", i);
                                    view! {
                                        <div class="flex gap-2 items-start">
                                            <div class="flex-1 space-y-1">
                                                <input
                                                    type="text"
                                                    prop:value=p.pattern.clone()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "danger",
                                                            i,
                                                            "pattern",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Regex pattern..."
                                                    class=move || {
                                                        format!(
                                                            "w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                            if has_err {
                                                                "border-danger"
                                                            } else {
                                                                "border-border"
                                                            }
                                                        )
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    prop:value=p.reason.clone().unwrap_or_default()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "danger",
                                                            i,
                                                            "reason",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Reason (optional)..."
                                                    class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                                />
                                            </div>
                                            <button
                                                on:click=move |_| remove_pattern("danger", i)
                                                class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                            >
                                                "Remove"
                                            </button>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("danger")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Danger Pattern"
                    </button>
                </div>

                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">
                        "Safe Patterns (auto-approved)"
                    </h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config
                                .get()
                                .map(|c| c.shell_security.custom_safe.clone())
                                .unwrap_or_default();
                            patterns
                                .into_iter()
                                .enumerate()
                                .map(|(i, p)| {
                                    let has_err = has_error("safe", i);
                                    view! {
                                        <div class="flex gap-2 items-start">
                                            <div class="flex-1 space-y-1">
                                                <input
                                                    type="text"
                                                    prop:value=p.pattern.clone()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "safe",
                                                            i,
                                                            "pattern",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Regex pattern..."
                                                    class=move || {
                                                        format!(
                                                            "w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                            if has_err {
                                                                "border-danger"
                                                            } else {
                                                                "border-border"
                                                            }
                                                        )
                                                    }
                                                />
                                                <input
                                                    type="text"
                                                    prop:value=p.reason.clone().unwrap_or_default()
                                                    on:input=move |ev| {
                                                        update_pattern(
                                                            "safe",
                                                            i,
                                                            "reason",
                                                            event_target_value(&ev),
                                                        )
                                                    }
                                                    placeholder="Reason (optional)..."
                                                    class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                                />
                                            </div>
                                            <button
                                                on:click=move |_| remove_pattern("safe", i)
                                                class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                            >
                                                "Remove"
                                            </button>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("safe")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Safe Pattern"
                    </button>
                </div>

                {move || {
                    let errors = pattern_errors.get();
                    if !errors.is_empty() {
                        Some(view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                                <ul class="list-disc list-inside">
                                    {errors.iter().map(|(i, cat, err)| view! {
                                        <li>{format!("{} #{}: {}", cat, i + 1, err)}</li>
                                    }).collect::<Vec<_>>()}
                                </ul>
                            </div>
                        })
                    } else {
                        None
                    }
                }}
            </div>
        </div>
    }
}

