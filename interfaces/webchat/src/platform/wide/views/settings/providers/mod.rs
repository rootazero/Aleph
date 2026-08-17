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
//! - [`list`] — left-panel sections (Subscription / Configured)
//! - [`picker`] — the "add a provider" disclosure: search + the rest of the
//!   catalogue, keyboard-walkable. The 56 unconfigured presets used to render
//!   as cards in the left panel, which buried the rows actually in use.
//! - [`detail_panel`] — right-panel detail editor

mod detail_panel;
mod list;
mod model_ladder;
mod picker;

use crate::api::{CatalogEntry, CatalogView, ProviderInfo, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

use detail_panel::ProviderDetailPanel;
use list::{is_configured, ConfiguredList, SubscriptionLoginSection};
use picker::CatalogPicker;

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
    // Whether the "add a provider" disclosure is expanded. Owned here because
    // the first-load seed below has to reach it.
    let picker_open = RwSignal::new(false);
    // Seed it open **once**, after the catalogue arrives, when the operator has
    // nothing configured. Otherwise a fresh install renders a left panel
    // holding one collapsed button and nothing else — and this page's whole job
    // is offering providers nobody has set up yet. A seed rather than a derived
    // predicate: a signal that recomputed would snap back open every time the
    // operator closed it while still configuring their first provider.
    let seeded = RwSignal::new(false);
    Effect::new(move |_| {
        let rows = catalog.get();
        if rows.is_empty() || seeded.get_untracked() {
            return;
        }
        seeded.set(true);
        if !rows.iter().any(is_configured) {
            picker_open.set(true);
        }
    });

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

                    // Add a provider — button + the searchable catalogue it
                    // reveals. Top of the panel because it is the action; the
                    // sections below it are the content.
                    <CatalogPicker
                        catalog=catalog
                        providers=providers
                        selected=selected
                        open=picker_open
                    />

                    // Subscription login section (auth_kind == oauth rows)
                    <SubscriptionLoginSection catalog=catalog providers=providers selected=selected />

                    // Providers the operator has actually configured.
                    <ConfiguredList catalog=catalog providers=providers selected=selected />

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
