//! AI Providers Configuration View.
//!
//! Split-pane layout matching Embedding/Generation Providers:
//! - Left panel: provider list (from `providers.catalog`), grouped and searchable
//! - Right panel: Detail/form editor for selected provider
//!
//! ## Where the preset list comes from
//!
//! `providers.catalog` with `view: "all"`, not a table in this crate. The
//! hand-written one carried 13 of the core's 56 presets, defaulted two of them
//! to models the core marks retired, and named two providers (`anthropic`,
//! `ollama`) that do not resolve in the core registry at all — the same drift
//! `preset_providers.rs` had already killed for generation providers by
//! fetching. Which rows are subscription logins is `auth_kind` on the row, so
//! there is no second list of "which of these are OAuth" to go stale either.
//!
//! ## Layout
//! - this module — top-level `ProvidersView`, owns the two fetches
//! - [`list`] — left-panel sections (Subscription / Configured / Quick setup)
//! - [`detail_panel`] — right-panel detail editor

mod detail_panel;
mod list;

use crate::api::{CatalogEntry, CatalogView, ProviderInfo, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

use detail_panel::ProviderDetailPanel;
use list::{PresetGrid, SubscriptionLoginSection};

#[component]
#[must_use]
pub fn ProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let providers = RwSignal::new(Vec::<ProviderInfo>::new());
    // Every preset the core ships, credential state included. `All` rather
    // than `Configured`: this page's whole job is offering providers nobody
    // has set up yet.
    let catalog = RwSignal::new(Vec::<CatalogEntry>::new());
    let selected = RwSignal::new(Option::<String>::None);
    let error = RwSignal::new(Option::<String>::None);
    // Filter term for the left-panel list. Applied to rows the server already
    // sent, through the shared ranker (R4) — never a query parameter.
    let search = RwSignal::new(String::new());

    // Load providers + catalog on mount. `rpc_call` parks on a bounded
    // readiness wait, so a cold load (direct URL / refresh) does not have to
    // race the handshake here — see `DashboardState::await_gateway_ready`.
    spawn_local(async move {
        match ProvidersApi::catalog(&state, CatalogView::All).await {
            Ok(items) => catalog.set(items),
            Err(e) => {
                error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load providers: {e}"),
                )));
            }
        }
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
            }
            Err(e) => {
                error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load providers: {e}"),
                )));
            }
        }
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

                    // Search — filters the catalogue rows already in hand.
                    <div>
                        <input
                            type="text"
                            prop:value=move || search.get()
                            on:input=move |ev| search.set(event_target_value(&ev))
                            placeholder=move || t_string!(i18n, settings.providers.search_placeholder).to_string()
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    // Subscription login section (auth_kind == oauth rows)
                    <SubscriptionLoginSection catalog=catalog providers=providers selected=selected search=search />

                    // Configured providers, then the rest of the catalogue.
                    <PresetGrid catalog=catalog providers=providers selected=selected search=search />

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
                    catalog=catalog
                    selected=selected
                    error=error
                />
            </div>
        </div>
    }
}
