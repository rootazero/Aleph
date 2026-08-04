//! AI Providers Configuration View.
//!
//! Split-pane layout matching Embedding/Generation Providers:
//! - Left panel: Preset provider grid + configured provider list
//! - Right panel: Detail/form editor for selected provider
//! - Preset quick-setup for common AI services
//!
//! ## Layout
//! - this module — top-level `ProvidersView` + shared `canonical_oauth_name`
//! - [`list`] — left-panel sections (Subscription / Preset / Custom)
//! - [`detail_panel`] — right-panel detail editor

mod detail_panel;
mod list;

use crate::api::{ProviderInfo, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::preset_data::OAUTH_PRESETS;
use leptos::prelude::*;
use leptos::task::spawn_local;

use detail_panel::ProviderDetailPanel;
use list::{CustomProvidersList, PresetGrid, SubscriptionLoginSection};

/// Map OAuth preset name to the canonical name used in config (e.g. "codex" → "chatgpt").
pub(super) fn canonical_oauth_name(name: &str) -> &'static str {
    match name {
        "codex" => "chatgpt",
        other => {
            // Return a static str — for known presets only
            OAUTH_PRESETS
                .iter()
                .find(|p| p.name == other)
                .map(|p| p.name)
                .unwrap_or("chatgpt")
        }
    }
}

#[component]
#[must_use]
pub fn ProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let providers = RwSignal::new(Vec::<ProviderInfo>::new());
    let selected = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load providers on mount
    spawn_local(async move {
        match ProvidersApi::list(&state).await {
            Ok(list) => {
                // `try_get_untracked`: this view can be disposed while the RPC
                // is in flight, and reading a disposed signal panics the whole
                // panel (see `acp_harnesses` for the reproduction).
                let Some(current) = selected.try_get_untracked() else {
                    return;
                };
                // Auto-select the default provider on first load so the detail pane
                // shows content instead of the empty placeholder (mirrors Embedding/Reranking).
                if current.is_none() {
                    if let Some(name) = list
                        .iter()
                        .find(|p| p.is_default)
                        .or_else(|| list.iter().find(|p| p.enabled))
                        .or_else(|| list.first())
                        .map(|p| p.name.clone())
                    {
                        selected.set(Some(name));
                    }
                }
                providers.set(list);
                error.set(None);
            }
            Err(e) => {
                error.set(Some(format!("Failed to load providers: {e}")));
            }
        }
        loading.set(false);
    });

    view! {
        <div class="flex h-full aleph-content-top aleph-md">
            // Left panel: Presets + Configured providers
            <div class="flex flex-col w-5/12 min-w-0 border-r border-border aleph-md-list">
                // Header
                <div class="px-6 pb-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">{t!(i18n, settings.providers.title)}</h1>
                    <p class="mt-1 text-sm text-text-tertiary">
                        {t!(i18n, settings.providers.description)}
                    </p>
                </div>

                // Scrollable content
                <div class="flex-1 overflow-y-auto p-6 space-y-6">
                    {move || error.get().filter(|e| e.contains("Failed to load")).map(|_| view! {
                        <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm flex items-center gap-2">
                            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                <circle cx="12" cy="12" r="10"/>
                                <line x1="12" y1="16" x2="12" y2="12"/>
                                <line x1="12" y1="8" x2="12.01" y2="8"/>
                            </svg>
                            {t!(i18n, settings.providers.gateway_unavailable)}
                        </div>
                    })}

                    // Subscription login section (OAuth providers)
                    <SubscriptionLoginSection providers=providers selected=selected />

                    // Preset grid (badges shown inline for configured providers)
                    <PresetGrid providers=providers selected=selected />

                    // Custom providers (not matching any preset)
                    <CustomProvidersList providers=providers selected=selected />

                    // Add Custom Provider button
                    <div class="pt-2">
                        <button
                            on:click=move |_| selected.set(Some("__new__".to_string()))
                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                        >
                            {t!(i18n, settings.providers.add_custom)}
                        </button>
                    </div>
                </div>
            </div>

            // Right panel: Detail/Editor
            <div class="w-7/12 min-w-0 overflow-y-auto aleph-md-detail">
                <ProviderDetailPanel
                    providers=providers
                    selected=selected
                    error=error
                />
            </div>
        </div>
    }
}
