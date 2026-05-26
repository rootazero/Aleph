//! Secret-protection section: leak patterns + vault virtual keys.

use leptos::prelude::*;

use crate::api::{CustomLeakPattern, SecurityConfig, VirtualKeyEntry};
use crate::i18n::*;

use super::validate_regex;

#[component]
pub(super) fn SecretProtectionSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let leak_pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());

    let validate_leak_patterns = move || {
        let mut errors = Vec::new();
        if let Some(cfg) = config.get() {
            for (i, p) in cfg
                .secrets_protection
                .custom_leak_patterns
                .iter()
                .enumerate()
            {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, e));
                }
            }
        }
        let is_valid = errors.is_empty();
        leak_pattern_errors.set(errors);
        is_valid
    };

    let add_virtual_key = move || {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.virtual_keys.push(VirtualKeyEntry {
                alias: String::new(),
                secret_name: String::new(),
            });
            config.set(Some(cfg));
        }
    };

    let remove_virtual_key = move |index: usize| {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.virtual_keys.remove(index);
            config.set(Some(cfg));
        }
    };

    let update_virtual_key = move |index: usize, field: &'static str, value: String| {
        if let Some(mut cfg) = config.get() {
            if let Some(entry) = cfg.secrets_protection.virtual_keys.get_mut(index) {
                match field {
                    "alias" => entry.alias = value,
                    "secret_name" => entry.secret_name = value,
                    _ => {}
                }
            }
            config.set(Some(cfg));
        }
    };

    let add_leak_pattern = move || {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection
                .custom_leak_patterns
                .push(CustomLeakPattern {
                    name: String::new(),
                    pattern: String::new(),
                });
            config.set(Some(cfg));
        }
    };

    let remove_leak_pattern = move |index: usize| {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.custom_leak_patterns.remove(index);
            config.set(Some(cfg));
            validate_leak_patterns();
        }
    };

    let update_leak_pattern = move |index: usize, field: &'static str, value: String| {
        if let Some(mut cfg) = config.get() {
            if let Some(pattern) = cfg.secrets_protection.custom_leak_patterns.get_mut(index) {
                match field {
                    "name" => pattern.name = value,
                    "pattern" => pattern.pattern = value,
                    _ => {}
                }
            }
            config.set(Some(cfg));
            validate_leak_patterns();
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                "Secret Protection"
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure virtual key aliases and custom leak detection patterns."
            </p>

            <div class="space-y-6">
                <div>
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Virtual Key Aliases"</h3>
                    <div class="space-y-2">
                        {move || {
                            let keys = config.get().map(|c| c.secrets_protection.virtual_keys.clone()).unwrap_or_default();
                            keys.into_iter().enumerate().map(|(i, entry)| {
                                view! {
                                    <div class="flex gap-2 items-center">
                                        <input
                                            type="text"
                                            prop:value=entry.alias.clone()
                                            on:input=move |ev| update_virtual_key(i, "alias", event_target_value(&ev))
                                            placeholder="Alias (e.g., openai)"
                                            class="flex-1 px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                        />
                                        <span class="text-text-tertiary">"→"</span>
                                        <input
                                            type="text"
                                            prop:value=entry.secret_name.clone()
                                            on:input=move |ev| update_virtual_key(i, "secret_name", event_target_value(&ev))
                                            placeholder="Secret name (e.g., OPENAI_API_KEY)"
                                            class="flex-1 px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                        />
                                        <button
                                            on:click=move |_| remove_virtual_key(i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_virtual_key()
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Virtual Key"
                    </button>
                </div>

                <div class="pt-4 border-t border-border">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Custom Leak Detection Patterns"</h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config.get().map(|c| c.secrets_protection.custom_leak_patterns.clone()).unwrap_or_default();
                            patterns.into_iter().enumerate().map(|(i, p)| {
                                let has_err = leak_pattern_errors.get().iter().any(|(idx, _)| *idx == i);
                                view! {
                                    <div class="flex gap-2 items-start">
                                        <div class="flex-1 space-y-1">
                                            <input
                                                type="text"
                                                prop:value=p.name.clone()
                                                on:input=move |ev| update_leak_pattern(i, "name", event_target_value(&ev))
                                                placeholder="Pattern name..."
                                                class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                            />
                                            <input
                                                type="text"
                                                prop:value=p.pattern.clone()
                                                on:input=move |ev| update_leak_pattern(i, "pattern", event_target_value(&ev))
                                                placeholder="Regex pattern..."
                                                class=move || format!("w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                    if has_err { "border-danger" } else { "border-border" })
                                            />
                                        </div>
                                        <button
                                            on:click=move |_| remove_leak_pattern(i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_leak_pattern()
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Leak Pattern"
                    </button>

                    {move || {
                        let errors = leak_pattern_errors.get();
                        if !errors.is_empty() {
                            Some(view! {
                                <div class="mt-2 p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                    <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                                    <ul class="list-disc list-inside">
                                        {errors.iter().map(|(i, err)| view! {
                                            <li>{format!("Pattern #{}: {}", i + 1, err)}</li>
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
        </div>
    }
}

