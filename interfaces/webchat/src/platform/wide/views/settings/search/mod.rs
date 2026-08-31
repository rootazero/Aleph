//! Search Settings View.
//!
//! ## Layout
//! - this module — top-level `SearchView` (left list + right pane router)
//! - [`presentation`] — the identity⋈styling join: which backends exist comes
//!   from `aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS`, how they
//!   look comes from this crate
//! - [`picker`] — which rows the panel lists vs. which the disclosure offers
//! - [`list`] — the single configured-backend card list
//! - [`detail_panel`] — `ProviderDetailPanel`, one component covering both the
//!   configured and the not-yet-configured state of a backend
//! - [`add_custom`] — `AddCustomSearchProviderPanel`
//! - [`global_settings`] — enable/max_results/timeout/PII
//! - [`fetch_section`] — `FetchProvidersSection` (crawl4ai + shared Firecrawl)

mod add_custom;
mod detail_panel;
mod fetch_section;
mod global_settings;
mod list;
mod picker;
mod presentation;

use add_custom::AddCustomSearchProviderPanel;
use detail_panel::ProviderDetailPanel;
use fetch_section::FetchProvidersSection;
use global_settings::GlobalSettings;
use list::ConfiguredList;
use picker::SearchPicker;

use crate::api::{SearchConfig, SearchConfigApi};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// Main View
// ============================================================================

#[component]
#[must_use]
pub fn SearchView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let config = RwSignal::new(SearchConfig {
        enabled: false,
        default_provider: String::new(),
        max_results: 5,
        timeout_seconds: 10,
        pii_enabled: false,
        pii_scrub_email: true,
        pii_scrub_phone: true,
        pii_scrub_ssn: true,
        pii_scrub_credit_card: true,
        backends: Vec::new(),
    });
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let selected = RwSignal::new(Option::<String>::None);
    let show_add_form = RwSignal::new(false);
    let picker_open = RwSignal::new(false);
    // Seed it open **once**, after the first load: an instance with nothing
    // configured should not land on a collapsed button. A seed rather than a
    // derived predicate -- a signal that recomputed would snap back open
    // every time the operator closed it while still configuring their first
    // provider.
    let seeded = RwSignal::new(false);
    Effect::new(move |_| {
        if loading.get() || seeded.get_untracked() {
            return;
        }
        seeded.set(true);
        if config.get_untracked().backends.is_empty() {
            picker_open.set(true);
        }
    });

    // Load config on mount
    spawn_local(async move {
        match SearchConfigApi::get(&state).await {
            Ok(cfg) => {
                // Only auto-select if there's an active provider
                if !cfg.default_provider.is_empty() {
                    selected.set(Some(cfg.default_provider.clone()));
                }
                config.set(cfg);
                error.set(None);
            }
            Err(e) => {
                error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load config: {e}"),
                )));
            }
        }
        loading.set(false);
    });

    view! {
        <div class="flex h-full aleph-content-top aleph-md">
            // Left panel: Presets + Settings
            <div class="flex flex-col w-5/12 min-w-0 border-r border-border aleph-md-list">
                // Header
                <div class="px-6 pb-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">{t!(i18n, settings.search.title)}</h1>
                    <p class="mt-1 text-sm text-text-tertiary">
                        {t!(i18n, settings.search.description)}
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
                            {t!(i18n, settings.search.gateway_unavailable)}
                        </div>
                    })}

                    // Configured backends -- one list, preset and custom alike.
                    <ConfiguredList config=config selected=selected show_add_form=show_add_form />

                    // Catalogue: nine presets plus the custom endpoint, behind one button.
                    <SearchPicker
                        config=config
                        selected=selected
                        show_add_form=show_add_form
                        open=picker_open
                    />

                    // Global search settings
                    <GlobalSettings config=config loading=loading />

                    // Fetch providers (crawl4ai + shared Firecrawl)
                    <FetchProvidersSection />
                </div>
            </div>

            // Right panel: Detail or Add form
            <div class="w-7/12 min-w-0 overflow-y-auto aleph-md-detail">
                {move || {
                    if show_add_form.get() {
                        view! {
                            <AddCustomSearchProviderPanel
                                config=config
                                on_added=move || {
                                    show_add_form.set(false);
                                }
                                on_cancel=move || show_add_form.set(false)
                            />
                        }.into_any()
                    } else {
                        view! {
                            <ProviderDetailPanel config=config selected=selected error=error />
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
