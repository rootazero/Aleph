//! Reranking Providers settings view.
//!
//! ## Layout
//! - this module — top-level `RerankingProvidersView` + shared `RERANK_PRESETS`
//! - [`detail_panel`] — `ProviderDetailPanel` for preset selection
//! - [`add_custom`] — `AddCustomProviderPanel` for vLLM-compatible custom endpoints

mod add_custom;
mod detail_panel;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{RerankConfig, RerankConfigApi};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};

use add_custom::AddCustomProviderPanel;
use detail_panel::ProviderDetailPanel;

/// Preset reranking provider metadata
pub(super) struct RerankPreset {
    pub key: &'static str,
    pub name: &'static str,
    pub icon_color: &'static str,
    pub default_api_base: &'static str,
    pub default_model: &'static str,
}

pub(super) const RERANK_PRESETS: &[RerankPreset] = &[
    RerankPreset {
        key: "jina",
        name: "Jina AI",
        icon_color: "#FF6B6B",
        default_api_base: "https://api.jina.ai/v1",
        default_model: "jina-reranker-v2-base-multilingual",
    },
    RerankPreset {
        key: "siliconflow",
        name: "SiliconFlow",
        icon_color: "#6C5CE7",
        default_api_base: "https://api.siliconflow.cn/v1",
        default_model: "BAAI/bge-reranker-v2-m3",
    },
    RerankPreset {
        key: "voyage",
        name: "Voyage AI",
        icon_color: "#00B4D8",
        default_api_base: "https://api.voyageai.com/v1",
        default_model: "rerank-2",
    },
    RerankPreset {
        key: "pinecone",
        name: "Pinecone",
        icon_color: "#1DB954",
        default_api_base: "https://api.pinecone.io",
        default_model: "pinecone-rerank-v0",
    },
    RerankPreset {
        key: "vllm",
        name: "vLLM",
        icon_color: "#FF9F1C",
        default_api_base: "http://localhost:8000/v1",
        default_model: "BAAI/bge-reranker-v2-m3",
    },
    RerankPreset {
        key: "cohere",
        name: "Cohere",
        icon_color: "#A78BFA",
        default_api_base: "https://api.cohere.com/v2",
        default_model: "rerank-v3.5",
    },
];

#[component]
#[must_use]
pub fn RerankingProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let config = RwSignal::new(Option::<RerankConfig>::None);
    let loading = RwSignal::new(true);
    let selected_provider = RwSignal::new(Option::<String>::None);
    let (show_add_form, set_show_add_form) = signal(false);

    // Load config on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                match RerankConfigApi::get(&state).await {
                    Ok(cfg) => {
                        // Auto-select the current provider. `try_get_untracked`:
                        // this view can be disposed while the RPC is in flight,
                        // and reading a disposed signal panics the whole panel
                        // (see `acp_harnesses` for the reproduction).
                        let current = cfg.provider.as_str().to_string();
                        let Some(picked) = selected_provider.try_get_untracked() else {
                            return;
                        };
                        if picked.is_none() {
                            selected_provider.set(Some(current));
                        }
                        config.set(Some(cfg));
                    }
                    Err(_) => {
                        config.set(None);
                    }
                }
                loading.set(false);
            });
        }
    });

    view! {
        <div class="flex h-full aleph-content-top aleph-md">
            // Left panel — provider list
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border aleph-md-list">
                <div class="px-6 pb-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">
                        {t!(i18n, settings.reranking.title)}
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        {t!(i18n, settings.reranking.description)}
                    </p>
                </div>

                <div class="flex-1 overflow-auto">
                    {move || {
                        if loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">"Loading reranking providers..."</div>
                                </div>
                            }.into_any()
                        } else {
                            let current_provider = config.get()
                                .map(|c| c.provider.as_str().to_string())
                                .unwrap_or_else(|| "jina".to_string());
                            let is_enabled = config.get().map(|c| c.enabled).unwrap_or(false);
                            let is_verified = config.get().map(|c| c.verified).unwrap_or(false);

                            view! {
                                <div class="p-6 space-y-4">
                                    // Preset providers
                                    <div>
                                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                            {t!(i18n, settings.reranking.providers_section)}
                                        </h2>
                                        <div class="grid grid-cols-1 gap-2">
                                            {RERANK_PRESETS.iter().map(|preset| {
                                                let key = preset.key.to_string();
                                                let name = preset.name.to_string();
                                                let icon_color = preset.icon_color.to_string();
                                                let default_model = preset.default_model.to_string();

                                                let is_active_provider = current_provider == key;
                                                let key_click = key.clone();
                                                let key_check = key;
                                                let key_check_for_class = key_check;

                                                view! {
                                                    <ProviderRowCard
                                                        name=name
                                                        icon_color=icon_color
                                                        subtitle=default_model
                                                        is_selected=move || {
                                                            selected_provider.get().as_deref() == Some(&key_check_for_class)
                                                                && !show_add_form.get()
                                                        }
                                                        is_configured=move || is_active_provider
                                                        dot=|| RowDot::None
                                                        badge=move || view! {
                                                            <ProviderBadges state=BadgeState {
                                                                is_default: is_active_provider && is_enabled,
                                                                verified: is_active_provider && is_verified,
                                                            } />
                                                        }.into_any()
                                                        on_click=move || {
                                                            selected_provider.set(Some(key_click.clone()));
                                                            set_show_add_form.set(false);
                                                        }
                                                    />
                                                }
                                            }).collect_view()}
                                        </div>
                                    </div>

                                    // Add Custom Provider button
                                    <div class="pt-2">
                                        <button
                                            on:click=move |_| {
                                                set_show_add_form.set(true);
                                                selected_provider.set(None);
                                            }
                                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                                        >
                                            {t!(i18n, settings.reranking.add_custom)}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>

            // Right panel — detail / add form
            <div class="flex-1 flex flex-col overflow-hidden aleph-md-detail">
                {move || {
                    if loading.get() {
                        view! { <div></div> }.into_any()
                    } else if show_add_form.get() {
                        view! {
                            <AddCustomProviderPanel
                                config=config
                                on_saved=move || {
                                    set_show_add_form.set(false);
                                    // Select the newly saved custom provider
                                    if let Some(cfg) = config.get() {
                                        selected_provider.set(Some(cfg.provider.as_str().to_string()));
                                    }
                                }
                                on_cancel=move || {
                                    set_show_add_form.set(false);
                                    // Re-select current provider
                                    if let Some(cfg) = config.get() {
                                        selected_provider.set(Some(cfg.provider.as_str().to_string()));
                                    }
                                }
                            />
                        }.into_any()
                    } else if let Some(sel_key) = selected_provider.get() {
                        if config.get().is_some() {
                            view! {
                                <ProviderDetailPanel
                                    provider_key=sel_key
                                    config=config
                                />
                            }.into_any()
                        } else {
                            view! { <div></div> }.into_any()
                        }
                    } else {
                        view! {
                            <div class="flex items-center justify-center h-full text-text-tertiary">
                                <div class="text-center">
                                    <p class="text-lg">{t!(i18n, settings.reranking.select_to_configure)}</p>
                                    <p class="text-sm text-text-tertiary mt-1">{t!(i18n, settings.reranking.select_or_add)}</p>
                                </div>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
