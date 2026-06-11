//! Left-panel list sections — Subscription / Preset / Custom.
//!
//! All three render a vertical stack of provider cards. They take the shared
//! `providers` + `selected` signals from the parent `ProvidersView` and emit
//! click handlers that mutate `selected` (preset → `__preset__<name>`,
//! configured → real name).

use crate::api::ProviderInfo;
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n, I18nLocaleTrait};
use crate::preset_data::{OAUTH_PRESETS, PRESETS};
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::canonical_oauth_name;
use crate::api::ProvidersApi;

#[component]
pub(super) fn SubscriptionLoginSection(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    // Track OAuth connection status for each OAuth preset
    let oauth_statuses: Vec<(&'static str, RwSignal<Option<bool>>)> = OAUTH_PRESETS
        .iter()
        .map(|preset| (preset.name, RwSignal::new(None::<bool>)))
        .collect();
    let oauth_statuses = std::rc::Rc::new(oauth_statuses);

    // Query OAuth status on mount and when providers change
    {
        let oauth_statuses = oauth_statuses.clone();
        Effect::new(move || {
            let _ = providers.get(); // track providers changes
            let state = expect_context::<DashboardState>();
            for (name, status_signal) in oauth_statuses.iter() {
                let name = name.to_string();
                let status_signal = *status_signal;
                spawn_local(async move {
                    match ProvidersApi::oauth_status(&state, name).await {
                        Ok(status) => status_signal.set(Some(status.connected)),
                        Err(_) => status_signal.set(Some(false)),
                    }
                });
            }
        });
    }

    let oauth_statuses_view = oauth_statuses.clone();
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                {t!(i18n, settings.providers.subscription_login)}
            </h2>
            <div class="grid grid-cols-1 gap-2">
                {OAUTH_PRESETS.iter().enumerate().map(|(idx, preset)| {
                    let name = preset.name;
                    let description = preset.description;
                    let icon_color = preset.icon_color;
                    let oauth_connected = oauth_statuses_view[idx].1;

                    // OAuth providers may be stored under canonical name (e.g. "chatgpt" for "codex")
                    let canonical = canonical_oauth_name(name);

                    let is_configured = move || {
                        providers.get().iter().any(|p| p.name == name || p.name == canonical)
                    };

                    let on_click = move || {
                        if is_configured() {
                            // Select by the name that actually exists in providers list
                            let actual_name = providers.get().iter()
                                .find(|p| p.name == name || p.name == canonical)
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| name.to_string());
                            selected.set(Some(actual_name));
                        } else {
                            selected.set(Some(format!("__preset__{name}")));
                        }
                    };

                    view! {
                        <ProviderRowCard
                            name=name.to_string()
                            icon_color=icon_color.to_string()
                            subtitle=description.to_string()
                            is_selected=move || {
                                let sel = selected.get();
                                sel.as_deref() == Some(name)
                                    || sel.as_deref() == Some(canonical)
                                    || sel.as_deref() == Some(&format!("__preset__{name}"))
                            }
                            is_configured=move || {
                                let connected = oauth_connected.get().unwrap_or(false);
                                let is_verified = providers.get().iter()
                                    .find(|p| p.name == name || p.name == canonical)
                                    .is_some_and(|p| p.verified);
                                connected || is_verified
                            }
                            dot=move || {
                                let connected = oauth_connected.get().unwrap_or(false);
                                let is_verified = providers.get().iter()
                                    .find(|p| p.name == name || p.name == canonical)
                                    .is_some_and(|p| p.verified);
                                if connected || is_verified { RowDot::Verified } else { RowDot::None }
                            }
                            badge=move || {
                                let connected = oauth_connected.get().unwrap_or(false);
                                let list = providers.get();
                                let provider = list.iter().find(|p| p.name == name || p.name == canonical);
                                let state = BadgeState {
                                    is_default: provider.is_some_and(|p| p.is_default),
                                    verified: connected || provider.is_some_and(|p| p.verified),
                                };
                                view! { <ProviderBadges state=state /> }.into_any()
                            }
                            large_icon=true
                            trailing=move || view! {
                                <svg class="w-4 h-4 text-text-tertiary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"/>
                                </svg>
                            }.into_any()
                            on_click=on_click
                        />
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

#[component]
pub(super) fn PresetGrid(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                {t!(i18n, settings.providers.quick_setup)}
            </h2>
            <div class="grid grid-cols-1 gap-2">
                {PRESETS.iter().map(|preset| {
                    let name = preset.name;
                    let description = preset.description;
                    let icon_color = preset.icon_color;

                    view! {
                        <ProviderRowCard
                            name=name.to_string()
                            icon_color=icon_color.to_string()
                            subtitle=description.to_string()
                            is_selected=move || {
                                let sel = selected.get();
                                sel.as_deref() == Some(name)
                                    || sel.as_deref() == Some(&format!("__preset__{name}"))
                            }
                            is_configured=move || providers.get().iter().any(|p| p.name == name)
                            dot=move || {
                                let list = providers.get();
                                let provider = list.iter().find(|p| p.name == name);
                                if provider.is_some_and(|p| p.verified) {
                                    RowDot::Verified
                                } else {
                                    RowDot::None
                                }
                            }
                            badge=move || {
                                let p = providers.get();
                                let entry = p.iter().find(|p| p.name == name);
                                let state = crate::components::provider_badge::BadgeState {
                                    is_default: entry.map(|p| p.is_default).unwrap_or(false),
                                    verified: entry.map(|p| p.verified).unwrap_or(false),
                                };
                                view! { <ProviderBadges state=state /> }.into_any()
                            }
                            on_click=move || {
                                if providers.get().iter().any(|p| p.name == name) {
                                    selected.set(Some(name.to_string()));
                                } else {
                                    selected.set(Some(format!("__preset__{name}")));
                                }
                            }
                        />
                    }
                }).collect_view()}
            </div>
        </div>
    }
}

#[component]
pub(super) fn CustomProvidersList(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let mut preset_names: Vec<&str> = PRESETS
        .iter()
        .chain(OAUTH_PRESETS.iter())
        .map(|p| p.name)
        .collect();
    // Also exclude canonical OAuth names (e.g. "chatgpt" for "codex")
    for preset in OAUTH_PRESETS.iter() {
        let canonical = canonical_oauth_name(preset.name);
        if !preset_names.contains(&canonical) {
            preset_names.push(canonical);
        }
    }

    view! {
        {move || {
            let list = providers.get();
            let custom: Vec<_> = list.into_iter()
                .filter(|p| !preset_names.contains(&p.name.as_str()))
                .collect();
            if custom.is_empty() {
                view! { <div></div> }.into_any()
            } else {
                view! {
                    <div>
                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                            {t!(i18n, settings.providers.custom_providers)}
                        </h2>
                        <div class="grid grid-cols-1 gap-2">
                            {custom.into_iter().map(|p| {
                                let name = p.name.clone();
                                let name_click = name.clone();
                                let name_check = name.clone();
                                let model = p.model.clone();
                                let color = p.color.clone();
                                let is_default = p.is_default;
                                let verified = p.verified;

                                view! {
                                    <ProviderRowCard
                                        name=name
                                        icon_color=color
                                        subtitle=model
                                        is_selected=move || {
                                            selected.get().as_deref() == Some(&name_check)
                                        }
                                        is_configured=|| true
                                        dot=move || if verified { RowDot::Verified } else { RowDot::Inactive }
                                        badge=move || {
                                            let state = crate::components::provider_badge::BadgeState {
                                                is_default,
                                                verified,
                                            };
                                            view! { <ProviderBadges state=state /> }.into_any()
                                        }
                                        on_click=move || selected.set(Some(name_click.clone()))
                                    />
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
            }
        }}
    }
}
