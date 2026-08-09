//! Right-pane detail editor for a selected provider.
//!
//! Three modes driven by `selected`:
//! - `__new__` → blank custom form
//! - `__preset__<name>` → preset hydration (read-only protocol, editable `api_key` etc.)
//! - any other → existing provider edit (full form, including OAuth section for
//!   `auth_type == "oauth"` presets such as Codex/ChatGPT)

use crate::api::{OAuthStatus, ProviderConfig, ProviderInfo, ProvidersApi, TestResult};
use crate::components::provider_key_field::ProviderKeyField;
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::preset_data::find_preset;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub(super) fn ProviderDetailPanel(
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Form state
    let form_name = RwSignal::new(String::new());
    let form_protocol = RwSignal::new(String::from("openai"));
    let form_model = RwSignal::new(String::new());
    let form_api_key = RwSignal::new(String::new());
    let form_base_url = RwSignal::new(String::new());
    let form_enabled = RwSignal::new(true);
    let form_timeout = RwSignal::new(300u64);
    let form_max_tokens = RwSignal::new(String::new());
    let form_temperature = RwSignal::new(String::new());

    let saving = RwSignal::new(false);
    let testing = RwSignal::new(false);
    let test_result = RwSignal::new(Option::<TestResult>::None);
    let oauth_status = RwSignal::new(Option::<OAuthStatus>::None);
    let oauth_loading = RwSignal::new(false);

    let is_new = move || {
        let sel = selected.get();
        sel.as_deref() == Some("__new__")
            || sel
                .as_ref()
                .map(|s| s.starts_with("__preset__"))
                .unwrap_or(false)
    };

    // Load form when selection changes
    Effect::new(move || {
        test_result.set(None);
        error.set(None);

        if let Some(sel) = selected.get() {
            if sel == "__new__" {
                form_name.set(String::new());
                form_protocol.set("openai".to_string());
                form_model.set(String::new());
                form_api_key.set(String::new());
                form_base_url.set(String::new());
                form_enabled.set(true);
                form_timeout.set(300);
                form_max_tokens.set(String::new());
                form_temperature.set(String::new());
            } else if let Some(preset_name) = sel.strip_prefix("__preset__") {
                if let Some(preset) = find_preset(preset_name) {
                    form_name.set(preset.name.to_string());
                    form_protocol.set(preset.protocol.to_string());
                    form_model.set(preset.model.to_string());
                    form_api_key.set(String::new());
                    form_base_url.set(preset.base_url.to_string());
                    form_enabled.set(true);
                    form_timeout.set(300);
                    form_max_tokens.set(String::new());
                    form_temperature.set(String::new());
                }
            } else {
                // Existing provider — populate form with actual values.
                // Read untracked so a background `providers.set(list)` refresh does
                // not re-run this Effect and clobber in-progress edits; the Effect
                // only re-hydrates when the SELECTED identity changes.
                if let Some(provider) = providers.get_untracked().iter().find(|p| p.name == sel) {
                    form_name.set(provider.name.clone());
                    form_protocol.set(
                        provider
                            .provider_type
                            .clone()
                            .unwrap_or_else(|| provider.name.clone()),
                    );
                    form_model.set(provider.model.clone());
                    // Never pre-fill the stored secret; empty submit = keep existing key.
                    form_api_key.set(String::new());
                    form_enabled.set(provider.enabled);
                    form_base_url.set(provider.base_url.clone().unwrap_or_default());
                    form_timeout.set(provider.timeout_seconds);
                    form_max_tokens.set(
                        provider
                            .max_tokens
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    );
                    form_temperature.set(
                        provider
                            .temperature
                            .map(|v| v.to_string())
                            .unwrap_or_default(),
                    );
                }
            }
        }
    });

    // Check OAuth status when an OAuth provider is selected
    Effect::new(move || {
        let sel = selected.get();
        let provider_name = sel
            .as_deref()
            .and_then(|s| s.strip_prefix("__preset__").or(Some(s)))
            .and_then(|name| {
                if name.starts_with("__") {
                    None
                } else {
                    Some(name.to_string())
                }
            });

        if let Some(name) = provider_name {
            if find_preset(&name)
                .map(|p| p.auth_type == "oauth")
                .unwrap_or(false)
            {
                oauth_loading.set(true);
                let state = expect_context::<DashboardState>();
                spawn_local(async move {
                    match ProvidersApi::oauth_status(&state, name).await {
                        Ok(status) => oauth_status.set(Some(status)),
                        Err(_) => oauth_status.set(Some(OAuthStatus {
                            connected: false,
                            expires_in_seconds: None,
                            error: None,
                        })),
                    }
                    oauth_loading.set(false);
                });
                return;
            }
        }
        oauth_status.set(None);
    });

    // Build config from form
    let build_config = move || -> ProviderConfig {
        ProviderConfig {
            protocol: Some(form_protocol.get()),
            enabled: form_enabled.get(),
            model: form_model.get(),
            api_key: {
                let key = form_api_key.get();
                if key.is_empty() {
                    None
                } else {
                    Some(key)
                }
            },
            base_url: {
                let url = form_base_url.get();
                if url.is_empty() {
                    None
                } else {
                    Some(url)
                }
            },
            color: None,
            timeout_seconds: Some(form_timeout.get()),
            max_tokens: {
                let t = form_max_tokens.get();
                if t.is_empty() {
                    None
                } else {
                    t.parse().ok()
                }
            },
            temperature: {
                let t = form_temperature.get();
                if t.is_empty() {
                    None
                } else {
                    t.parse().ok()
                }
            },
            top_p: None,
            top_k: None,
        }
    };

    let on_save = move |_| {
        let name = form_name.get();
        if name.is_empty() {
            error.set(Some("Provider name is required".to_string()));
            return;
        }
        if form_model.get().is_empty() {
            error.set(Some("Model is required".to_string()));
            return;
        }

        saving.set(true);
        error.set(None);
        let config = build_config();

        spawn_local(async move {
            let result = if is_new() {
                ProvidersApi::create(&state, name.clone(), config).await
            } else {
                ProvidersApi::update(&state, name.clone(), config).await
            };

            match result {
                Ok(()) => {
                    if let Ok(list) = ProvidersApi::list(&state).await {
                        providers.set(list);
                    }
                    selected.set(Some(name));
                }
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to save: {e}")
                    }),
                )),
            }
            saving.set(false);
        });
    };

    let on_test = move |_| {
        testing.set(true);
        test_result.set(None);
        let config = build_config();
        let provider_name = selected.get();

        spawn_local(async move {
            match ProvidersApi::test_connection(&state, provider_name.as_deref(), config).await {
                Ok(r) => {
                    test_result.set(Some(r));
                    // Refetch so a persisted `verified` flip lights the badge
                    // without a manual reload.
                    if let Ok(list) = ProvidersApi::list(&state).await {
                        providers.set(list);
                    }
                }
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Test failed: {e}"),
                ))),
            }
            testing.set(false);
        });
    };

    let on_set_default = move |_| {
        if let Some(name) = selected.get() {
            if name.starts_with("__") {
                return;
            }
            saving.set(true);
            spawn_local(async move {
                match ProvidersApi::set_default(&state, name).await {
                    Ok(()) => {
                        if let Ok(list) = ProvidersApi::list(&state).await {
                            providers.set(list);
                        }
                    }
                    Err(e) => error.set(Some(
                        crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                            format!("Failed: {e}")
                        }),
                    )),
                }
                saving.set(false);
            });
        }
    };

    let confirming = RwSignal::new(false);
    let on_confirm_delete = move || {
        if let Some(name) = selected.get() {
            if name.starts_with("__") {
                return;
            }
            saving.set(true);
            spawn_local(async move {
                match ProvidersApi::delete(&state, name).await {
                    Ok(()) => {
                        if let Ok(list) = ProvidersApi::list(&state).await {
                            providers.set(list);
                        }
                        selected.set(None);
                    }
                    Err(e) => error.set(Some(
                        crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                            format!("Failed: {e}")
                        }),
                    )),
                }
                saving.set(false);
            });
        }
    };

    view! {
        <div class="flex flex-col h-full">
            {move || {
                let sel = selected.get();
                if sel.is_none() {
                    return view! {
                        <div class="flex flex-col items-center justify-center flex-1 text-text-tertiary">
                            <svg class="w-12 h-12 mb-3 opacity-30" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.5"
                                    d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
                            </svg>
                            <span class="text-sm">{t!(i18n, settings.providers.select_to_view)}</span>
                        </div>
                    }.into_any();
                }

                let sel = sel.unwrap();
                let preset_name = if sel.starts_with("__preset__") {
                    sel.strip_prefix("__preset__").map(std::string::ToString::to_string)
                } else {
                    None
                };
                let title = if sel == "__new__" {
                    t_string!(i18n, settings.providers.custom_provider).to_string()
                } else if let Some(ref pn) = preset_name {
                    format!("{} {}", t_string!(i18n, settings.providers.setup_prefix), pn)
                } else {
                    sel.clone()
                };

                let preset_info = preset_name.as_deref()
                    .or(if !sel.starts_with("__") { Some(sel.as_str()) } else { None })
                    .and_then(find_preset);

                view! {
                    <div class="flex flex-col h-full">
                        // Header
                        <div class="px-6 py-4 border-b border-border">
                            <div class="flex items-center gap-3">
                                {if let Some(preset) = preset_info {
                                    let ch = preset.name.chars().next().unwrap_or('?').to_uppercase().to_string();
                                    view! {
                                        <div
                                            class="w-10 h-10 rounded-xl flex items-center justify-center text-white font-bold"
                                            style=format!("background-color: {}", preset.icon_color)
                                        >
                                            {ch}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="w-10 h-10 rounded-xl bg-surface-sunken flex items-center justify-center text-text-tertiary font-bold">
                                            "?"
                                        </div>
                                    }.into_any()
                                }}
                                <div class="flex-1">
                                    <h2 class="text-lg font-semibold text-text-primary capitalize">{title}</h2>
                                    {if let Some(preset) = preset_info {
                                        view! { <p class="text-xs text-text-tertiary">{preset.description}</p> }.into_any()
                                    } else {
                                        view! { <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.custom_provider_desc)}</p> }.into_any()
                                    }}
                                </div>
                            </div>
                        </div>

                        // Scrollable content
                        <div class="flex-1 overflow-y-auto p-6 space-y-6">
                            // Error
                            {move || error.get().filter(|e| !e.contains("Failed to load")).map(|e| view! {
                                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
                            })}

                            // OAuth login section (for subscription providers like Codex)
                            {if preset_info.map(|p| p.auth_type == "oauth").unwrap_or(false) {
                                view! {
                                    <div class="space-y-6">
                                        // Connection Status card (reactive)
                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.providers.connection_status)}</h3>
                                            {move || {
                                                let status = oauth_status.get();
                                                let is_connected = status.as_ref().map(|s| s.connected).unwrap_or(false);
                                                let loading = oauth_loading.get();

                                                if loading {
                                                    view! {
                                                        <div class="flex items-center gap-3">
                                                            <div class="w-3 h-3 rounded-full bg-text-tertiary animate-pulse"></div>
                                                            <span class="text-sm text-text-tertiary">{t!(i18n, settings.providers.checking)}</span>
                                                        </div>
                                                    }.into_any()
                                                } else if is_connected {
                                                    let expires = status.as_ref()
                                                        .and_then(|s| s.expires_in_seconds)
                                                        .map(|secs| {
                                                            let hours = secs / 3600;
                                                            let mins = (secs % 3600) / 60;
                                                            if hours > 0 {
                                                                format!("Expires in {hours}h {mins}m")
                                                            } else {
                                                                format!("Expires in {mins}m")
                                                            }
                                                        });
                                                    view! {
                                                        <div>
                                                            <div class="flex items-center gap-3">
                                                                <div class="w-3 h-3 rounded-full bg-success"></div>
                                                                <span class="text-sm text-success font-medium">{t!(i18n, settings.providers.connected)}</span>
                                                            </div>
                                                            {expires.map(|e| view! {
                                                                <p class="mt-1 text-xs text-text-tertiary">{e}</p>
                                                            })}
                                                        </div>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <div class="flex items-center gap-3">
                                                            <div class="w-3 h-3 rounded-full bg-text-tertiary"></div>
                                                            <span class="text-sm text-text-secondary">{t!(i18n, settings.providers.not_connected)}</span>
                                                        </div>
                                                    }.into_any()
                                                }
                                            }}
                                            <p class="text-xs text-text-tertiary">
                                                {t!(i18n, settings.providers.codex_info)}
                                            </p>
                                            // Login / Logout button (reactive)
                                            {move || {
                                                let is_connected = oauth_status.get().as_ref().map(|s| s.connected).unwrap_or(false);
                                                if is_connected {
                                                    view! {
                                                        <button
                                                            on:click=move |_| {
                                                                let provider_name = "codex".to_string();
                                                                let state = expect_context::<DashboardState>();
                                                                spawn_local(async move {
                                                                    match ProvidersApi::oauth_logout(&state, provider_name).await {
                                                                        Ok(()) => {
                                                                            oauth_status.set(Some(OAuthStatus {
                                                                                connected: false,
                                                                                expires_in_seconds: None,
                                                                                error: None,
                                                                            }));
                                                                            // Refresh providers list
                                                                            if let Ok(list) = ProvidersApi::list(&state).await {
                                                                                providers.set(list);
                                                                            }
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(crate::components::admin_refusal::settings_load_error(
                                                                                i18n,
                                                                                &e,
                                                                                |e| format!("Logout failed: {e}"),
                                                                            )));
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                            class="w-full px-4 py-2.5 bg-surface-sunken border border-border text-text-secondary text-sm font-medium rounded-xl hover:bg-surface-raised transition-colors"
                                                        >
                                                            {t!(i18n, settings.providers.logout)}
                                                        </button>
                                                    }.into_any()
                                                } else {
                                                    view! {
                                                        <button
                                                            on:click=move |_| {
                                                                let provider_name = "codex".to_string();
                                                                oauth_loading.set(true);
                                                                let state = expect_context::<DashboardState>();
                                                                spawn_local(async move {
                                                                    match ProvidersApi::oauth_login(&state, provider_name).await {
                                                                        Ok(status) => {
                                                                            oauth_status.set(Some(status));
                                                                            // Refresh providers list and switch to the actual provider
                                                                            if let Ok(list) = ProvidersApi::list(&state).await {
                                                                                // Find the actual provider name (e.g. "chatgpt")
                                                                                let actual = list.iter()
                                                                                    .find(|p| p.name == "chatgpt" || p.name == "codex")
                                                                                    .map(|p| p.name.clone());
                                                                                providers.set(list);
                                                                                if let Some(name) = actual {
                                                                                    selected.set(Some(name));
                                                                                }
                                                                            }
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                                                i18n,
                                                                                &e,
                                                                                |e| format!("OAuth login failed: {e}"),
                                                                            )));
                                                                        }
                                                                    }
                                                                    oauth_loading.set(false);
                                                                });
                                                            }
                                                            prop:disabled=move || oauth_loading.get()
                                                            class="w-full px-4 py-3 bg-[#10A37F] hover:bg-[#0d8c6d] disabled:opacity-50 text-white text-sm font-semibold rounded-xl transition-colors flex items-center justify-center gap-2"
                                                        >
                                                            <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 24 24">
                                                                <path d="M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364l2.0201-1.1638a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.4091-.6765zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0974-2.3616l2.603-1.5018 2.6029 1.5018v3.0036l-2.6029 1.5018-2.603-1.5018z"/>
                                                            </svg>
                                                            {move || if oauth_loading.get() { t_string!(i18n, settings.providers.logging_in).to_string() } else { t_string!(i18n, settings.providers.login_with_chatgpt).to_string() }}
                                                        </button>
                                                    }.into_any()
                                                }
                                            }}
                                        </div>

                                        // Model configuration card (simplified for OAuth)
                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.providers.configuration)}</h3>
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.model)}</label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_model.get()
                                                    on:input=move |ev| form_model.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                    placeholder="e.g. gpt-4o, gpt-4o-mini"
                                                />
                                                <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.providers.model_hint)}</p>
                                            </div>
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.timeout)}</label>
                                                <input
                                                    type="number"
                                                    prop:value=move || form_timeout.get()
                                                    on:input=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { form_timeout.set(v); } }
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                />
                                            </div>
                                        </div>

                                        // Save + Set Default + Delete
                                        <div class="space-y-2">
                                            <button
                                                on:click=on_save
                                                prop:disabled=move || saving.get()
                                                class="w-full px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                            >
                                                {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                            </button>

                                            {move || {
                                                let s = selected.get();
                                                let is_existing = s.as_ref().map(|s| !s.starts_with("__")).unwrap_or(false);
                                                if is_existing {
                                                    view! {
                                                        <button
                                                            on:click=on_set_default
                                                            prop:disabled=move || saving.get() || !selected.get()
                                                                .and_then(|s| providers.get().into_iter().find(|p| p.name == s))
                                                                .map(|p| p.verified).unwrap_or(false)
                                                            class="w-full px-4 py-2.5 bg-success-subtle border border-success/20 text-success text-sm font-medium rounded-lg hover:bg-success-subtle/80 disabled:opacity-50"
                                                        >
                                                            {t!(i18n, settings.providers.set_default)}
                                                        </button>
                                                        {move || {
                                                            let not_verified = !selected.get()
                                                                .and_then(|s| providers.get().into_iter().find(|p| p.name == s))
                                                                .map(|p| p.verified).unwrap_or(false);
                                                            not_verified.then(|| view! {
                                                                <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.verify_before_default)}</p>
                                                            })
                                                        }}
                                                    }.into_any()
                                                } else {
                                                    view! { <div></div> }.into_any()
                                                }
                                            }}
                                        </div>
                                    </div>
                                }.into_any()
                            } else {
                                // Standard API key provider view
                                view! {
                                    <div class="space-y-6">
                                        // Configuration form card
                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.providers.configuration)}</h3>

                                            // Name (editable only for new custom)
                                            {move || if sel == "__new__" {
                                                view! {
                                                    <div>
                                                        <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.name)}</label>
                                                        <input
                                                            type="text"
                                                            prop:value=move || form_name.get()
                                                            on:input=move |ev| form_name.set(event_target_value(&ev))
                                                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                            placeholder="my-provider"
                                                        />
                                                    </div>
                                                }.into_any()
                                            } else {
                                                view! { <div></div> }.into_any()
                                            }}

                                            // Protocol
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.protocol)}</label>
                                                <select
                                                    prop:value=move || form_protocol.get()
                                                    on:change=move |ev| form_protocol.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                >
                                                    <option value="openai">{t!(i18n, settings.providers.protocol_openai)}</option>
                                                    <option value="openai-responses">{t!(i18n, settings.providers.protocol_openai_responses)}</option>
                                                    <option value="anthropic">{t!(i18n, settings.providers.protocol_anthropic)}</option>
                                                    <option value="gemini">{t!(i18n, settings.providers.protocol_gemini)}</option>
                                                    <option value="ollama">{t!(i18n, settings.providers.protocol_ollama)}</option>
                                                    <option value="codex">{t!(i18n, settings.providers.protocol_codex)}</option>
                                                </select>
                                            </div>

                                            // API Key
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.api_key)}</label>
                                                {
                                                    let has_api_key = Signal::derive(move || {
                                                        selected.get()
                                                            .and_then(|s| providers.get().into_iter().find(|p| p.name == s))
                                                            .map(|p| p.has_api_key)
                                                            .unwrap_or(false)
                                                    });
                                                    match preset_info.map(|p| p.api_key_placeholder.to_string()) {
                                                        Some(hint) => view! {
                                                            <ProviderKeyField value=form_api_key has_api_key=has_api_key hint=hint />
                                                        }.into_any(),
                                                        None => view! {
                                                            <ProviderKeyField value=form_api_key has_api_key=has_api_key />
                                                        }.into_any(),
                                                    }
                                                }
                                                {move || if preset_info.map(|p| !p.needs_api_key).unwrap_or(false) {
                                                    view! {
                                                        <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.providers.no_api_key_needed)}</p>
                                                    }.into_any()
                                                } else {
                                                    view! { <span></span> }.into_any()
                                                }}
                                            </div>

                                            // Model
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.model)}</label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_model.get()
                                                    on:input=move |ev| form_model.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                    placeholder="e.g. gpt-4o, claude-sonnet-4-20250514"
                                                />
                                                <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.providers.model_hint)}</p>
                                            </div>

                                            // Base URL
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.base_url)}</label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_base_url.get()
                                                    on:input=move |ev| form_base_url.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                    placeholder=move || {
                                                        preset_info.map(|p| p.base_url.to_string()).unwrap_or_else(|| "https://api.example.com/v1".to_string())
                                                    }
                                                />
                                            </div>

                                            // Enabled
                                            <label class="flex items-center gap-3 cursor-pointer">
                                                <input
                                                    type="checkbox"
                                                    prop:checked=move || form_enabled.get()
                                                    on:change=move |ev| form_enabled.set(event_target_checked(&ev))
                                                    class="w-4 h-4 rounded"
                                                />
                                                <div>
                                                    <span class="text-sm text-text-primary">{t!(i18n, settings.providers.enabled)}</span>
                                                    <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.enabled_desc)}</p>
                                                </div>
                                            </label>
                                        </div>

                                        // Advanced Settings card
                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.providers.advanced_settings)}</h3>
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.timeout)}</label>
                                                <input
                                                    type="number"
                                                    prop:value=move || form_timeout.get()
                                                    on:input=move |ev| { if let Ok(v) = event_target_value(&ev).parse() { form_timeout.set(v); } }
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.max_tokens)}</label>
                                                <input
                                                    type="number"
                                                    prop:value=move || form_max_tokens.get()
                                                    on:input=move |ev| form_max_tokens.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                    placeholder="Optional"
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.temperature)}</label>
                                                <input
                                                    type="number"
                                                    step="0.1"
                                                    prop:value=move || form_temperature.get()
                                                    on:input=move |ev| form_temperature.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                    placeholder="0.0 - 2.0"
                                                />
                                            </div>
                                        </div>

                                        // Actions
                                        <div class="space-y-2">
                                            <div class="flex gap-2">
                                                <button
                                                    on:click=on_test
                                                    prop:disabled=move || testing.get() || saving.get()
                                                    class="flex-1 px-4 py-2.5 bg-info text-white text-sm font-medium rounded-lg hover:bg-primary-hover transition-colors disabled:opacity-50"
                                                >
                                                    {move || if testing.get() { t_string!(i18n, settings.providers.testing).to_string() } else { t_string!(i18n, settings.providers.test_connection).to_string() }}
                                                </button>
                                                <button
                                                    on:click=on_save
                                                    prop:disabled=move || saving.get()
                                                    class="flex-1 px-4 py-2.5 bg-primary hover:bg-primary-hover disabled:opacity-50 text-white text-sm font-medium rounded-lg transition-colors"
                                                >
                                                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                                </button>
                                            </div>

                                            {move || {
                                                let s = selected.get();
                                                let is_existing = s.as_ref().map(|s| !s.starts_with("__")).unwrap_or(false);
                                                let is_preset = s.as_deref().map(|n| find_preset(n).is_some()).unwrap_or(false);
                                                if is_existing {
                                                    view! {
                                                        <div class="flex gap-2">
                                                            <button
                                                                on:click=on_set_default
                                                                prop:disabled=move || saving.get() || !selected.get()
                                                                    .and_then(|s| providers.get().into_iter().find(|p| p.name == s))
                                                                    .map(|p| p.verified).unwrap_or(false)
                                                                class="flex-1 px-4 py-2.5 bg-success-subtle border border-success/20 text-success text-sm font-medium rounded-lg hover:bg-success-subtle/80 disabled:opacity-50"
                                                            >
                                                                {t!(i18n, settings.providers.set_default)}
                                                            </button>
                                                            {if !is_preset {
                                                                view! {
                                                                    {move || if confirming.get() {
                                                                        view! {
                                                                            <ConfirmButton confirming=confirming on_confirm=on_confirm_delete />
                                                                        }.into_any()
                                                                    } else {
                                                                        view! {
                                                                            <button
                                                                                on:click=move |_| confirming.set(true)
                                                                                prop:disabled=move || saving.get()
                                                                                class="px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger text-sm font-medium rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50"
                                                                            >
                                                                                {t!(i18n, settings.providers.delete)}
                                                                            </button>
                                                                        }.into_any()
                                                                    }}
                                                                }.into_any()
                                                            } else {
                                                                view! { <span></span> }.into_any()
                                                            }}
                                                        </div>
                                                        {move || {
                                                            let not_verified = !selected.get()
                                                                .and_then(|s| providers.get().into_iter().find(|p| p.name == s))
                                                                .map(|p| p.verified).unwrap_or(false);
                                                            not_verified.then(|| view! {
                                                                <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.verify_before_default)}</p>
                                                            })
                                                        }}
                                                    }.into_any()
                                                } else {
                                                    view! { <div></div> }.into_any()
                                                }
                                            }}
                                        </div>

                                        // Test result
                                        {move || test_result.get().map(|result| {
                                            if result.success {
                                                view! {
                                                    <div class="p-3 bg-success-subtle border border-success/20 rounded-lg">
                                                        <div class="flex items-center gap-2 text-success text-sm">
                                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/>
                                                            </svg>
                                                            <span class="font-medium">{t!(i18n, settings.providers.connection_successful)}</span>
                                                        </div>
                                                        {result.latency_ms.map(|ms| view! {
                                                            <p class="mt-1 text-xs text-success">{format!("Latency: {ms}ms")}</p>
                                                        })}
                                                    </div>
                                                }.into_any()
                                            } else {
                                                view! {
                                                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg">
                                                        <div class="flex items-center gap-2 text-danger text-sm">
                                                            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                                                            </svg>
                                                            <span class="font-medium">{t!(i18n, settings.providers.connection_failed)}</span>
                                                        </div>
                                                        {result.error.map(|e| view! {
                                                            <p class="mt-1 text-xs text-danger">{e}</p>
                                                        })}
                                                    </div>
                                                }.into_any()
                                            }
                                        })}
                                    </div>
                                }.into_any()
                            }}
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
