use crate::api::SearchConfig;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;

// ============================================================================
// Global Settings
// ============================================================================

#[component]
pub(super) fn GlobalSettings(
    config: RwSignal<SearchConfig>,
    loading: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div>
            <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                {t!(i18n, settings.search.global_settings)}
            </h2>
            {move || {
                if loading.get() {
                    view! {
                        <div class="text-center py-4 text-text-tertiary text-sm">{t_string!(i18n, common.loading).to_string()}</div>
                    }.into_any()
                } else {
                    let cfg = config.get();
                    let provider_display = if cfg.default_provider.is_empty() {
                        "None".to_string()
                    } else {
                        cfg.default_provider.clone()
                    };
                    view! {
                        <div class="bg-surface-raised rounded-lg border border-border p-4 space-y-3">
                            <div class="flex items-center justify-between">
                                <div>
                                    <div class="text-sm font-medium text-text-primary">{t!(i18n, settings.search.web_search)}</div>
                                    <div class="text-xs text-text-tertiary">{t!(i18n, settings.search.web_search_desc)}</div>
                                </div>
                                <div class=move || {
                                    if config.get().enabled {
                                        "px-2 py-0.5 bg-success-subtle text-success text-xs font-medium rounded"
                                    } else {
                                        "px-2 py-0.5 bg-surface-sunken text-text-tertiary text-xs font-medium rounded"
                                    }
                                }>
                                    {move || if config.get().enabled { t_string!(i18n, settings.search.enabled).to_string() } else { t_string!(i18n, settings.search.disabled).to_string() }}
                                </div>
                            </div>

                            <div class="flex items-center gap-4 text-xs text-text-tertiary">
                                <span>"Max Results: " {cfg.max_results}</span>
                                <span>"\u{00B7}"</span>
                                <span>"Timeout: " {cfg.timeout_seconds} "s"</span>
                                <span>"\u{00B7}"</span>
                                <span>"Provider: " {provider_display}</span>
                            </div>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}
