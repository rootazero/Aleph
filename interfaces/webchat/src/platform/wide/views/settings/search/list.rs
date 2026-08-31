use super::presentation::presets;
use crate::api::SearchConfig;
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;

// ============================================================================
// Preset Grid
// ============================================================================

#[component]
pub(super) fn PresetGrid(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    show_add_form: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                {t!(i18n, settings.search.providers_section)}
            </h2>
            <div class="grid grid-cols-1 gap-2">
                {presets().map(|preset| {
                    let name = preset.name;
                    let display_name = preset.display_name;
                    let description = preset.description;
                    let icon_color = preset.icon_color;
                    let first_char = preset.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();

                    let is_active = move || {
                        let dp = config.get().default_provider;
                        !dp.is_empty() && dp == name
                    };

                    let on_click = move |_| {
                        selected.set(Some(name.to_string()));
                        show_add_form.set(false);
                    };

                    view! {
                        <button
                            on:click=on_click
                            class=move || {
                                let base = "text-left p-3 rounded-lg border transition-all";
                                let sel = selected.get();
                                let is_sel = sel.as_deref() == Some(name);
                                if is_sel {
                                    format!("{base} bg-primary-subtle border-primary")
                                } else if is_active() {
                                    format!("{base} bg-surface-raised border-border hover:border-primary/40")
                                } else {
                                    format!("{base} bg-surface-sunken border-border hover:border-border-strong")
                                }
                            }
                        >
                            <div class="flex items-center gap-3">
                                <div
                                    class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                    style=format!("background-color: {}", icon_color)
                                >
                                    {first_char}
                                </div>
                                <div class="min-w-0">
                                    <div class="flex items-center gap-2">
                                        <span class="font-medium text-text-primary text-sm truncate">
                                            {display_name}
                                        </span>
                                        {move || {
                                            let cfg = config.get();
                                            let is_default = !cfg.default_provider.is_empty() && cfg.default_provider == name;
                                            let backend_verified = cfg.backends.iter().find(|b| b.name == name).is_some_and(|b| b.verified);
                                            view! {
                                                <ProviderBadges state=BadgeState {
                                                    is_default,
                                                    verified: backend_verified,
                                                } />
                                            }
                                        }}
                                    </div>
                                    <div class="text-xs text-text-tertiary truncate">
                                        {description}
                                    </div>
                                </div>
                            </div>
                        </button>
                    }
                }).collect_view()}
            </div>
        </div>
    }
}


// ============================================================================
// Custom Search Providers List (non-preset providers)
// ============================================================================

#[component]
pub(super) fn CustomSearchProvidersList(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    show_add_form: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let preset_names: Vec<&str> = presets().map(|p| p.name).collect();

    view! {
        {move || {
            let cfg = config.get();
            let custom: Vec<_> = cfg.backends.iter()
                .filter(|b| !preset_names.contains(&b.name.as_str()))
                .cloned()
                .collect();
            if custom.is_empty() {
                view! { <div></div> }.into_any()
            } else {
                view! {
                    <div>
                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                            {t!(i18n, settings.search.custom_providers)}
                        </h2>
                        <div class="grid grid-cols-1 gap-2">
                            {custom.into_iter().map(|backend| {
                                let name = backend.name.clone();
                                let name_click = name.clone();
                                let name_check = name.clone();
                                let is_default = !cfg.default_provider.is_empty() && cfg.default_provider == name;
                                let verified = backend.verified;
                                let first_char = name.chars().next().unwrap_or('?').to_uppercase().to_string();

                                view! {
                                    <button
                                        on:click=move |_| {
                                            selected.set(Some(name_click.clone()));
                                            show_add_form.set(false);
                                        }
                                        class=move || {
                                            let base = "text-left p-3 rounded-lg border transition-all";
                                            let is_sel = selected.get().as_deref() == Some(&name_check);
                                            if is_sel {
                                                format!("{base} bg-primary-subtle border-primary")
                                            } else {
                                                format!("{base} bg-surface-raised border-border hover:border-primary/40")
                                            }
                                        }
                                    >
                                        <div class="flex items-center gap-3">
                                            <div
                                                class="w-8 h-8 rounded-lg flex items-center justify-center text-white text-sm font-bold shrink-0"
                                                style="background-color: #808080"
                                            >
                                                {first_char}
                                            </div>
                                            <div class="min-w-0">
                                                <div class="flex items-center gap-2">
                                                    <span class="font-medium text-text-primary text-sm truncate">
                                                        {name}
                                                    </span>
                                                    <ProviderBadges state=BadgeState {
                                                        is_default,
                                                        verified,
                                                    } />
                                                </div>
                                                <div class="text-xs text-text-tertiary truncate">
                                                    {t!(i18n, settings.search.custom_search_provider)}
                                                </div>
                                            </div>
                                        </div>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
            }
        }}
    }
}

