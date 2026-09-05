use crate::api::{SearchBackendEntry, SearchConfig, SearchConfigApi};
use crate::components::provider_key_field::ProviderKeyField;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

// ============================================================================
// Add Custom Search Provider Panel
// ============================================================================

#[component]
pub(super) fn AddCustomSearchProviderPanel(
    config: RwSignal<SearchConfig>,
    on_added: impl Fn() + 'static + Copy,
    on_cancel: impl Fn() + 'static + Copy,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let form_name = RwSignal::new(String::new());
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(String::new());
    let form_engine_id = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    let on_add = move |_| {
        let name = form_name.get().trim().to_string();
        if name.is_empty() {
            error.set(Some("Provider name is required".to_string()));
            return;
        }

        saving.set(true);
        error.set(None);

        let mut cfg = config.get();
        // Add backend entry
        cfg.backends.push(SearchBackendEntry {
            name,
            api_key: {
                let v = form_api_key.get();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            base_url: {
                let v = form_base_url.get();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            engine_id: {
                let v = form_engine_id.get();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            engines: None,
            has_api_key: false,
            verified: false,
        });

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg.clone()).await {
                Ok(_) => {
                    config.set(cfg);
                    on_added();
                }
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to add provider: {e}")
                        }),
                    ));
                }
            }
            saving.set(false);
        });
    };

    view! {
        <div class="flex flex-col h-full">
            // Header
            <div class="px-6 py-4 border-b border-border">
                <div class="flex items-center justify-between">
                    <h2 class="text-xl font-semibold text-text-primary">{t!(i18n, settings.search.add_custom_provider)}</h2>
                    <button
                        on:click=move |_| on_cancel()
                        class="text-text-tertiary hover:text-text-primary transition-colors"
                    >
                        {t!(i18n, common.cancel)}
                    </button>
                </div>
            </div>

            // Form
            <div class="flex-1 overflow-y-auto p-6 space-y-6">
                {move || error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                })}

                <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                    <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.search.provider_details)}</h3>

                    // Provider Name
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.provider_name)}</label>
                        <input
                            type="text"
                            prop:value=move || form_name.get()
                            on:input=move |ev| form_name.set(event_target_value(&ev))
                            placeholder=t_string!(i18n, settings.search.custom_id_placeholder)
                            class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                        />
                    </div>

                    // API Key
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.api_key)}</label>
                        <ProviderKeyField
                            value=form_api_key
                            has_api_key=Signal::derive(|| false)
                            hint=t_string!(i18n, settings.search.optional_api_key).to_string()
                        />
                    </div>

                    // Base URL
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.base_url)}</label>
                        <input
                            type="text"
                            prop:value=move || form_base_url.get()
                            on:input=move |ev| form_base_url.set(event_target_value(&ev))
                            placeholder="https://api.example.com/search"
                            class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                        />
                    </div>

                    // Engine ID
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, settings.search.engine_id)}</label>
                        <input
                            type="text"
                            prop:value=move || form_engine_id.get()
                            on:input=move |ev| form_engine_id.set(event_target_value(&ev))
                            placeholder=t_string!(i18n, settings.search.api_key_optional_placeholder)
                            class="w-full px-3 py-2 border border-border rounded-lg bg-surface text-text-primary text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 font-mono"
                        />
                    </div>
                </div>

                // Add button
                <button
                    on:click=on_add
                    prop:disabled=move || saving.get() || form_name.get().trim().is_empty()
                    class="w-full px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                >
                    {move || if saving.get() { t_string!(i18n, settings.search.adding).to_string() } else { t_string!(i18n, settings.search.add_provider).to_string() }}
                </button>
            </div>
        </div>
    }
}
