//! Embedding Providers settings view.
//!
//! ## Layout
//! - this module — top-level `EmbeddingProvidersView` (left list + right pane router) + `EmptyState`
//! - [`detail_panel`] — `ProviderDetailPanel` for selected provider (embeds [`reembed_card`])
//! - [`reembed_card`] — `ReembedMigrationCard` driving the memory.reembed.* event subscription
//! - [`add_panel`] — `AddProviderPanel` for custom (non-preset) endpoints

mod add_panel;
mod detail_panel;
mod reembed_card;

use crate::api::{
    EmbeddingPresetEntry, EmbeddingProviderConfig, EmbeddingProviderEntry, EmbeddingProvidersApi,
};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

use add_panel::AddProviderPanel;
use detail_panel::ProviderDetailPanel;

#[component]
#[must_use]
pub fn EmbeddingProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State signals
    let (providers, set_providers) = signal(Vec::<EmbeddingProviderEntry>::new());
    let (presets, set_presets) = signal(Vec::<EmbeddingPresetEntry>::new());
    let (is_loading, set_is_loading) = signal(true);
    let (error_message, set_error_message) = signal(Option::<String>::None);
    let (selected_provider_id, set_selected_provider_id) = signal(Option::<String>::None);
    let (show_add_form, set_show_add_form) = signal(false);

    // Load providers and presets on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                set_is_loading.set(true);
                let providers_result = EmbeddingProvidersApi::list(&state).await;
                let presets_result = EmbeddingProvidersApi::presets(&state).await;

                match (providers_result, presets_result) {
                    (Ok(list), Ok(preset_list)) => {
                        // Auto-select the active provider on first load.
                        // Post-`.await` — same shape, same hazard, and the same
                        // fix as the `providers` view (see
                        // `crate::disposed_reads`).
                        let Some(current) = selected_provider_id.try_get_untracked() else {
                            return;
                        };
                        if current.is_none() {
                            if let Some(active) = list.iter().find(|p| p.is_active) {
                                set_selected_provider_id.set(Some(active.id.clone()));
                            }
                        }
                        set_providers.set(list);
                        set_presets.set(preset_list);
                        set_is_loading.set(false);
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        set_error_message.set(Some(
                            crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                                format!("Failed to load: {e}")
                            }),
                        ));
                        set_is_loading.set(false);
                    }
                }
            });
        } else {
            set_is_loading.set(false);
        }
    });

    // Reload helper
    let reload = move || {
        spawn_local(async move {
            if let Ok(list) = EmbeddingProvidersApi::list(&state).await {
                set_providers.set(list);
            }
        });
    };

    view! {
        <div class="flex h-full aleph-content-top aleph-md">
            // Left panel - Provider list
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border aleph-md-list">
                // Header
                <div class="px-6 pb-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">
                        {t!(i18n, settings.embedding.title)}
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        {t!(i18n, settings.embedding.description)}
                    </p>
                </div>

                // Content
                <div class="flex-1 overflow-auto">
                    {move || {
                        if is_loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">{t!(i18n, settings.embedding.loading)}</div>
                                </div>
                            }.into_any()
                        } else if let Some(error) = error_message.get() {
                            view! {
                                <div class="p-6">
                                    <div class="p-4 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{error}</div>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="p-6 space-y-4">
                                    // Preset Grid
                                    <div>
                                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                            {t!(i18n, settings.embedding.providers_section)}
                                        </h2>
                                        <div class="grid grid-cols-1 gap-2">
                                            {move || {
                                                let preset_list = presets.get();
                                                let provider_list = providers.get();
                                                preset_list.into_iter().map(|preset| {
                                                    let preset_id = preset.id.clone();
                                                    let preset_name = preset.name.clone();
                                                    let preset_label = preset.preset.clone();
                                                    let model = preset.model.clone();
                                                    let dims = preset.dimensions;

                                                    // Check if this preset is configured
                                                    let configured_provider = provider_list.iter().find(|p| p.preset == preset_label);
                                                    let is_configured = configured_provider.is_some();
                                                    let is_active = configured_provider.is_some_and(|p| p.is_active);
                                                    let is_verified = configured_provider.is_some_and(|p| p.verified);
                                                    let configured_id = configured_provider.map(|p| p.id.clone());

                                                    let sel_id = configured_id.unwrap_or(preset_id.clone());
                                                    let sel_id_click = sel_id.clone();
                                                    let sel_id_check = sel_id;

                                                    let icon_color = match preset_label.as_str() {
                                                        "silicon_flow" => "#6C5CE7",
                                                        "open_ai" => "#10A37F",
                                                        "ollama" => "#1D1D1F",
                                                        _ => "#808080",
                                                    };

                                                    let preset_for_add = if !is_configured {
                                                        Some(EmbeddingProviderConfig {
                                                            id: preset_id,
                                                            name: preset_name.clone(),
                                                            preset: preset_label.clone(),
                                                            api_base: preset.api_base,
                                                            api_key_env: None,
                                                            api_key: None,
                                                            model: model.clone(),
                                                            dimensions: dims,
                                                            batch_size: 32,
                                                            timeout_ms: 10000,
                                                            enabled: true,
                                                        })
                                                    } else {
                                                        None
                                                    };

                                                    view! {
                                                        <ProviderRowCard
                                                            name=preset_name
                                                            icon_color=icon_color.to_string()
                                                            subtitle=format!("{} · {}d", model, dims)
                                                            is_selected=move || {
                                                                selected_provider_id.get().as_deref() == Some(&sel_id_check)
                                                            }
                                                            is_configured=move || is_configured
                                                            dot=|| RowDot::None
                                                            badge=move || view! {
                                                                <ProviderBadges state=BadgeState { is_default: is_active, verified: is_verified } />
                                                            }.into_any()
                                                            on_click=move || {
                                                                if let Some(ref config) = preset_for_add {
                                                                    let config = config.clone();
                                                                    let id = config.id.clone();
                                                                    let state = expect_context::<DashboardState>();
                                                                    spawn_local(async move {
                                                                        match EmbeddingProvidersApi::add(&state, config).await {
                                                                            Ok(_) => {
                                                                                reload();
                                                                                set_selected_provider_id.set(Some(id));
                                                                                set_show_add_form.set(false);
                                                                            }
                                                                            Err(e) => {
                                                                                web_sys::console::error_1(&format!("Failed to add preset: {e}").into());
                                                                            }
                                                                        }
                                                                    });
                                                                } else {
                                                                    set_selected_provider_id.set(Some(sel_id_click.clone()));
                                                                    set_show_add_form.set(false);
                                                                }
                                                            }
                                                        />
                                                    }
                                                }).collect_view()
                                            }}
                                        </div>
                                    </div>

                                    // Custom providers (not matching any preset)
                                    {move || {
                                        let provider_list = providers.get();
                                        let preset_labels: Vec<String> = presets.get().iter().map(|p| p.preset.clone()).collect();
                                        let custom_providers: Vec<_> = provider_list.into_iter()
                                            .filter(|p| !preset_labels.contains(&p.preset))
                                            .collect();
                                        if custom_providers.is_empty() {
                                            view! { <div></div> }.into_any()
                                        } else {
                                            view! {
                                                <div class="pt-2">
                                                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                                        {t!(i18n, settings.embedding.custom_providers)}
                                                    </h2>
                                                    <div class="grid grid-cols-1 gap-2">
                                                        {custom_providers.into_iter().map(|cp| {
                                                            let cp_name = cp.name.clone();
                                                            let cp_model = cp.model.clone();
                                                            let cp_dims = cp.dimensions;
                                                            let cp_is_active = cp.is_active;
                                                            let cp_verified = cp.verified;
                                                            let sel_id = cp.id.clone();
                                                            let sel_id_check = cp.id;

                                                            view! {
                                                                <ProviderRowCard
                                                                    name=cp_name
                                                                    icon_color="#808080".to_string()
                                                                    subtitle=format!("{} · {}d", cp_model, cp_dims)
                                                                    is_selected=move || {
                                                                        selected_provider_id.get().as_deref() == Some(&sel_id_check)
                                                                    }
                                                                    is_configured=|| true
                                                                    dot=|| RowDot::None
                                                                    badge=move || view! {
                                                                        <ProviderBadges state=BadgeState { is_default: cp_is_active, verified: cp_verified } />
                                                                    }.into_any()
                                                                    on_click=move || {
                                                                        set_selected_provider_id.set(Some(sel_id.clone()));
                                                                        set_show_add_form.set(false);
                                                                    }
                                                                />
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                </div>
                                            }.into_any()
                                        }
                                    }}

                                    // Add Custom Provider button
                                    <div class="pt-2">
                                        <button
                                            on:click=move |_| {
                                                set_show_add_form.set(true);
                                                set_selected_provider_id.set(None);
                                            }
                                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                                        >
                                            {t!(i18n, settings.embedding.add_custom)}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>

            // Right panel - Detail / Add form
            <div class="w-7/12 min-w-[320px] bg-surface aleph-md-detail">
                {move || {
                    if show_add_form.get() {
                        view! {
                            <AddProviderPanel
                                on_added=move || {
                                    set_show_add_form.set(false);
                                    reload();
                                }
                                on_cancel=move || set_show_add_form.set(false)
                            />
                        }.into_any()
                    } else if let Some(provider_id) = selected_provider_id.get() {
                        let provider = providers.get().into_iter().find(|p| p.id == provider_id);
                        if let Some(provider) = provider {
                            view! {
                                <ProviderDetailPanel
                                    provider=provider
                                    on_reload=move || reload()
                                />
                            }.into_any()
                        } else {
                            view! { <EmptyState /> }.into_any()
                        }
                    } else {
                        view! { <EmptyState /> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn EmptyState() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex items-center justify-center h-full">
            <div class="text-center text-text-secondary">
                <p class="text-lg">{t!(i18n, settings.embedding.select_to_view)}</p>
                <p class="text-sm text-text-tertiary mt-1">{t!(i18n, settings.embedding.add_new)}</p>
            </div>
        </div>
    }
}
