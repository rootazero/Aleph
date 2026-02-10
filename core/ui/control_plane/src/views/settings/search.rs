//! Search Settings View
//!
//! Provides UI for managing search configuration.

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{SearchConfig, SearchConfigApi};

#[component]
pub fn SearchView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // State
    let config = RwSignal::new(SearchConfig {
        enabled: false,
        default_provider: "tavily".to_string(),
        max_results: 5,
        timeout_seconds: 10,
        pii_enabled: false,
        pii_scrub_email: true,
        pii_scrub_phone: true,
        pii_scrub_ssn: true,
        pii_scrub_credit_card: true,
    });
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load config on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                match SearchConfigApi::get(&state).await {
                    Ok(cfg) => {
                        config.set(cfg);
                        loading.set(false);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load config: {}", e)));
                        loading.set(false);
                    }
                }
            });
        }
    });

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-slate-900">"Search Settings"</h1>
                <p class="mt-1 text-sm text-slate-600">
                    "Configure search functionality and PII scrubbing"
                </p>
            </div>

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-slate-500">"Loading..."</div>
                        </div>
                    }.into_any()
                } else if let Some(err) = error.get() {
                    view! {
                        <div class="p-4 bg-red-50 border border-red-200 rounded text-red-700">
                            {err}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-6">
                            <BasicSettingsSection config=config />
                            <PIISection config=config />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn BasicSettingsSection(config: RwSignal<SearchConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let enabled = RwSignal::new(config.get().enabled);
    let default_provider = RwSignal::new(config.get().default_provider.clone());
    let max_results = RwSignal::new(config.get().max_results);
    let timeout_seconds = RwSignal::new(config.get().timeout_seconds);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.enabled = enabled.get();
        cfg.default_provider = default_provider.get();
        cfg.max_results = max_results.get();
        cfg.timeout_seconds = timeout_seconds.get();

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(
                        move || {
                            save_success.set(false);
                        },
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    saving.set(false);
                    save_error.set(Some(e));
                }
            }
        });
    });

    view! {
        <div class="bg-white rounded-lg border border-slate-200 p-6">
            <h2 class="text-lg font-semibold text-slate-900 mb-4">"Basic Settings"</h2>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || enabled.get()
                        on:change=move |ev| enabled.set(event_target_checked(&ev))
                        class="w-4 h-4 text-indigo-600 focus:ring-indigo-500 rounded"
                    />
                    <div>
                        <div class="font-medium text-slate-900">"Enable Search"</div>
                        <div class="text-sm text-slate-500">"Allow AI to search the web"</div>
                    </div>
                </label>

                <div>
                    <label class="block text-sm font-medium text-slate-700 mb-2">
                        "Default Provider"
                    </label>
                    <select
                        prop:value=move || default_provider.get()
                        on:change=move |ev| default_provider.set(event_target_value(&ev))
                        class="w-full px-3 py-2 border border-slate-300 rounded focus:outline-none focus:ring-2 focus:ring-indigo-500"
                    >
                        <option value="tavily">"Tavily"</option>
                        <option value="searxng">"SearXNG"</option>
                        <option value="brave">"Brave"</option>
                        <option value="google">"Google"</option>
                        <option value="bing">"Bing"</option>
                        <option value="exa">"Exa"</option>
                    </select>
                </div>

                <div>
                    <div class="flex items-center justify-between mb-2">
                        <label class="block text-sm font-medium text-slate-700">
                            "Max Results: " {move || max_results.get()}
                        </label>
                    </div>
                    <input
                        type="range"
                        min="1"
                        max="20"
                        step="1"
                        value=move || max_results.get()
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u64>() {
                                max_results.set(val);
                            }
                        }
                        class="w-full h-2 bg-slate-200 rounded-lg appearance-none cursor-pointer accent-indigo-600"
                    />
                </div>

                <div>
                    <div class="flex items-center justify-between mb-2">
                        <label class="block text-sm font-medium text-slate-700">
                            "Timeout: " {move || timeout_seconds.get()} " seconds"
                        </label>
                    </div>
                    <input
                        type="range"
                        min="5"
                        max="60"
                        step="5"
                        value=move || timeout_seconds.get()
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u64>() {
                                timeout_seconds.set(val);
                            }
                        }
                        class="w-full h-2 bg-slate-200 rounded-lg appearance-none cursor-pointer accent-indigo-600"
                    />
                </div>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-red-50 border border-red-200 rounded text-red-700 text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-green-50 border border-green-200 rounded text-green-700 text-sm">
                                "Saved successfully"
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-indigo-600 text-white rounded hover:bg-indigo-700 disabled:opacity-50"
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>
        </div>
    }
}

#[component]
fn PIISection(config: RwSignal<SearchConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let pii_enabled = RwSignal::new(config.get().pii_enabled);
    let scrub_email = RwSignal::new(config.get().pii_scrub_email);
    let scrub_phone = RwSignal::new(config.get().pii_scrub_phone);
    let scrub_ssn = RwSignal::new(config.get().pii_scrub_ssn);
    let scrub_credit_card = RwSignal::new(config.get().pii_scrub_credit_card);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.pii_enabled = pii_enabled.get();
        cfg.pii_scrub_email = scrub_email.get();
        cfg.pii_scrub_phone = scrub_phone.get();
        cfg.pii_scrub_ssn = scrub_ssn.get();
        cfg.pii_scrub_credit_card = scrub_credit_card.get();

        spawn_local(async move {
            match SearchConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(
                        move || {
                            save_success.set(false);
                        },
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    saving.set(false);
                    save_error.set(Some(e));
                }
            }
        });
    });

    view! {
        <div class="bg-white rounded-lg border border-slate-200 p-6">
            <h2 class="text-lg font-semibold text-slate-900 mb-4">"PII Scrubbing"</h2>
            <p class="text-sm text-slate-600 mb-4">
                "Automatically remove personally identifiable information from search results"
            </p>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || pii_enabled.get()
                        on:change=move |ev| pii_enabled.set(event_target_checked(&ev))
                        class="w-4 h-4 text-indigo-600 focus:ring-indigo-500 rounded"
                    />
                    <div>
                        <div class="font-medium text-slate-900">"Enable PII Scrubbing"</div>
                        <div class="text-sm text-slate-500">"Remove sensitive information from results"</div>
                    </div>
                </label>

                <div class="ml-7 space-y-2 border-l-2 border-slate-200 pl-4">
                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_email.get()
                            on:change=move |ev| scrub_email.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-indigo-600 focus:ring-indigo-500 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-slate-700">"Email addresses"</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_phone.get()
                            on:change=move |ev| scrub_phone.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-indigo-600 focus:ring-indigo-500 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-slate-700">"Phone numbers"</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_ssn.get()
                            on:change=move |ev| scrub_ssn.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-indigo-600 focus:ring-indigo-500 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-slate-700">"Social Security Numbers"</span>
                    </label>

                    <label class="flex items-center space-x-2 cursor-pointer">
                        <input
                            type="checkbox"
                            checked=move || scrub_credit_card.get()
                            on:change=move |ev| scrub_credit_card.set(event_target_checked(&ev))
                            disabled=move || !pii_enabled.get()
                            class="w-4 h-4 text-indigo-600 focus:ring-indigo-500 rounded disabled:opacity-50"
                        />
                        <span class="text-sm text-slate-700">"Credit card numbers"</span>
                    </label>
                </div>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-red-50 border border-red-200 rounded text-red-700 text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-green-50 border border-green-200 rounded text-green-700 text-sm">
                                "Saved successfully"
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-indigo-600 text-white rounded hover:bg-indigo-700 disabled:opacity-50"
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </button>
            </div>
        </div>
    }
}
