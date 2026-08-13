//! Right-pane detail editor for a selected provider.
//!
//! Three modes driven by `selected`:
//! - `__new__` → blank custom form
//! - `__preset__<id>` → prefill from the catalogue row (protocol, base URL,
//!   default model, colour, signup link)
//! - any other → existing provider edit (full form, including the OAuth
//!   section for rows the server marks `auth_kind: oauth`)
//!
//! # The model field is a ladder, not a string
//!
//! `providers.update` takes `models: [..]` and replaces the stored list
//! wholesale. This page used to send a single `model`, so saving anything here
//! — including a bare "enabled" toggle — truncated a provider's failover ladder
//! to its first rung. The editor is therefore ordered and says so: `models[0]`
//! is what a turn uses when it names no model, the rest are the failover rungs.
//!
//! The editor itself lives in [`super::model_ladder`]; this file owns the form
//! around it, and the [`RefreshState`] both it and the ladder write to.

use super::model_ladder::{ModelLadder, RefreshState};
use crate::api::{
    AuthKind, CatalogEntry, OAuthStatus, ProviderConfigJson, ProviderInfo, ProvidersApi, TestResult,
};
use crate::components::provider_key_field::ProviderKeyField;
use crate::components::ui::ConfirmButton;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Find the catalogue row a selection key edits.
///
/// Matches aliases as well as the id, because a provider configured under an
/// alias (`kimi` for `moonshot`) attaches to the canonical catalogue row.
fn row_for<'a>(catalog: &'a [CatalogEntry], key: &str) -> Option<&'a CatalogEntry> {
    catalog
        .iter()
        .find(|e| e.id == key || e.aliases.iter().any(|a| a == key))
}

/// The provider id a selection refers to, with the `__preset__` marker peeled
/// off. `None` for the sentinels that name no provider (`__new__`).
fn provider_key(selected: Option<&str>) -> Option<String> {
    let sel = selected?;
    let key = sel.strip_prefix("__preset__").unwrap_or(sel);
    (!key.starts_with("__")).then(|| key.to_string())
}

#[component]
pub(super) fn ProviderDetailPanel(
    providers: RwSignal<Vec<ProviderInfo>>,
    catalog: RwSignal<Vec<CatalogEntry>>,
    selected: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Form state
    let form_name = RwSignal::new(String::new());
    let form_protocol = RwSignal::new(String::from("openai"));
    // The ordered ladder, not a single id — see the module doc.
    let form_model = RwSignal::new(Vec::<String>::new());
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
    // Owned here, not inside `ModelLadder`: the post-save sweep started by
    // `on_save` has to report through the same badge the ladder's own button
    // writes. When this lived in the child, the save path had nowhere to put
    // its answer and threw it away — while a comment claimed the opposite.
    let refresh = RwSignal::new(RefreshState::Idle);

    // Equality-gated so the hydration Effect below re-runs exactly once when
    // the catalogue arrives — a plain `catalog.get()` would also re-run after
    // every "Fetch models" refetch and clobber whatever the operator was
    // typing.
    let catalog_ready = Memo::new(move |_| !catalog.get().is_empty());

    let is_new = move || {
        let sel = selected.get();
        sel.as_deref() == Some("__new__")
            || sel
                .as_ref()
                .map(|s| s.starts_with("__preset__"))
                .unwrap_or(false)
    };

    // Load form when selection changes (or when the catalogue first lands).
    Effect::new(move || {
        let _ = catalog_ready.get();
        test_result.set(None);
        error.set(None);

        if let Some(sel) = selected.get() {
            if sel == "__new__" {
                form_name.set(String::new());
                form_protocol.set("openai".to_string());
                form_model.set(Vec::new());
                form_api_key.set(String::new());
                form_base_url.set(String::new());
                form_enabled.set(true);
                form_timeout.set(300);
                form_max_tokens.set(String::new());
                form_temperature.set(String::new());
            } else if let Some(preset_id) = sel.strip_prefix("__preset__") {
                let cat = catalog.get_untracked();
                if let Some(entry) = row_for(&cat, preset_id) {
                    form_name.set(entry.id.clone());
                    form_protocol.set(entry.protocol.clone());
                    // Seed with the preset's own default. A preset that ships
                    // none (`requires_explicit_model`) seeds nothing rather
                    // than an empty-string rung the server would reject.
                    form_model.set(if entry.default_model.is_empty() {
                        Vec::new()
                    } else {
                        vec![entry.default_model.clone()]
                    });
                    form_api_key.set(String::new());
                    form_base_url.set(entry.base_url.clone());
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
                    form_model.set(provider.models.clone());
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

    // Check OAuth status when a subscription-login provider is selected. Which
    // providers those are is `auth_kind` on the catalogue row — the Panel no
    // longer keeps its own list.
    Effect::new(move || {
        let _ = catalog_ready.get();
        let sel = selected.get();
        if let Some(key) = provider_key(sel.as_deref()) {
            let cat = catalog.get_untracked();
            if let Some(entry) = row_for(&cat, &key) {
                if entry.auth_kind == AuthKind::OAuth {
                    let id = entry.id.clone();
                    oauth_loading.set(true);
                    let state = expect_context::<DashboardState>();
                    spawn_local(async move {
                        match ProvidersApi::oauth_status(&state, id).await {
                            Ok(status) => oauth_status.set(Some(status)),
                            Err(_) => oauth_status.set(Some(OAuthStatus {
                                connected: false,
                                provider: None,
                                expires_in_seconds: None,
                                error: None,
                            })),
                        }
                        oauth_loading.set(false);
                    });
                    return;
                }
            }
        }
        oauth_status.set(None);
    });

    // Build config from form
    let build_config = move || -> ProviderConfigJson {
        ProviderConfigJson {
            protocol: Some(form_protocol.get()),
            enabled: form_enabled.get(),
            models: form_model.get(),
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
            context_window: None,
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
            error.set(Some(
                t_string!(i18n, settings.providers.name_required).to_string(),
            ));
            return;
        }
        if form_model.get().is_empty() {
            error.set(Some(
                t_string!(i18n, settings.providers.model_required).to_string(),
            ));
            return;
        }

        saving.set(true);
        error.set(None);
        let config = build_config();
        // Whether a post-save discovery sweep could possibly return anything:
        // the server only visits providers that are enabled AND have a
        // resolvable key, and only ones that publish a listing at all. A row we
        // have never seen is a custom provider being created — those are
        // OpenAI-compatible by construction and the server does probe them.
        let cat = catalog.get();
        let discoverable = row_for(&cat, &name).is_none_or(|e| e.discoverable);
        let has_credential = config.api_key.is_some()
            || providers
                .get()
                .iter()
                .any(|p| p.name == name && p.has_api_key);
        let sweep_worthwhile = discoverable && has_credential && config.enabled;

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
                    selected.set(Some(name.clone()));
                    // Fire-and-forget for *latency*, not for the answer: a
                    // vendor that is down must not make "save my API key" slow,
                    // so the sweep runs in its own task and the save has already
                    // reported success by the time it answers — but the verdict
                    // still lands on the ladder's refresh badge, through the
                    // same `settle` the manual button uses.
                    //
                    // The previous shape tested `.is_ok()`, which is true even
                    // when every row is a failure: per-provider failures are
                    // rows and not RPC errors, by design. So linking a vendor
                    // that was unreachable said nothing at all.
                    if sweep_worthwhile {
                        refresh.set(RefreshState::Running);
                        spawn_local(async move {
                            match ProvidersApi::models_refresh(&state, Some(name.clone())).await {
                                Ok(result) => {
                                    refresh.set(RefreshState::settle(&result, &name));
                                    if let Ok(items) =
                                        ProvidersApi::catalog(&state, crate::api::CatalogView::All)
                                            .await
                                    {
                                        catalog.set(items);
                                    }
                                }
                                // A transport failure on the sweep is not a
                                // failure of the save, which has already
                                // succeeded — so it does not become the form's
                                // error banner. Dropping back to `Idle` says
                                // "no answer" rather than inventing one.
                                Err(_) => refresh.set(RefreshState::Idle),
                            }
                        });
                    }
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
                let cat = catalog.get();
                let key = provider_key(Some(sel.as_str()));
                let entry = key.as_deref().and_then(|k| row_for(&cat, k)).cloned();
                let title = if sel == "__new__" {
                    t_string!(i18n, settings.providers.custom_provider).to_string()
                } else if preset_name.is_some() {
                    let label = entry.as_ref().map_or_else(
                        || preset_name.clone().unwrap_or_default(),
                        |e| e.display_name.clone(),
                    );
                    format!("{} {}", t_string!(i18n, settings.providers.setup_prefix), label)
                } else {
                    sel.clone()
                };

                let is_oauth = entry.as_ref().is_some_and(|e| e.auth_kind == AuthKind::OAuth);
                // A local endpoint (Ollama, LM Studio, …) needs no credential;
                // neither does a subscription login. Both facts come off the
                // row rather than from a `needs_api_key` column this crate used
                // to maintain by hand.
                let keyless = is_oauth || entry.as_ref().is_some_and(|e| e.endpoint == "local");
                let signup_url = entry.as_ref().and_then(|e| e.signup_url.clone());
                let icon_color = entry.as_ref().map(|e| e.color.clone());
                let base_url_hint = entry.as_ref().map(|e| e.base_url.clone());
                let description = entry.as_ref().and_then(|e| e.notes.clone());
                let oauth_provider_id = entry.as_ref().map(|e| e.id.clone());
                let title_char = entry.as_ref().map_or_else(
                    || sel.chars().next().unwrap_or('?'),
                    |e| e.display_name.chars().next().unwrap_or('?'),
                );

                view! {
                    <div class="flex flex-col h-full">
                        // Header
                        <div class="px-6 py-4 border-b border-border">
                            <div class="flex items-center gap-3">
                                {match icon_color {
                                    Some(color) => view! {
                                        <div
                                            class="w-10 h-10 rounded-xl flex items-center justify-center text-white font-bold"
                                            style=format!("background-color: {color}")
                                        >
                                            {title_char.to_uppercase().to_string()}
                                        </div>
                                    }.into_any(),
                                    None => view! {
                                        <div class="w-10 h-10 rounded-xl bg-surface-sunken flex items-center justify-center text-text-tertiary font-bold">
                                            "?"
                                        </div>
                                    }.into_any(),
                                }}
                                <div class="flex-1">
                                    <h2 class="text-lg font-semibold text-text-primary capitalize">{title}</h2>
                                    {match description {
                                        Some(d) => view! { <p class="text-xs text-text-tertiary">{d}</p> }.into_any(),
                                        None => view! { <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.custom_provider_desc)}</p> }.into_any(),
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

                            // OAuth login section (subscription providers)
                            {if is_oauth {
                                let login_id = oauth_provider_id.clone().unwrap_or_default();
                                let logout_id = login_id.clone();
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
                                                    let provider_name = logout_id.clone();
                                                    view! {
                                                        <button
                                                            on:click=move |_| {
                                                                let provider_name = provider_name.clone();
                                                                let state = expect_context::<DashboardState>();
                                                                spawn_local(async move {
                                                                    match ProvidersApi::oauth_logout(&state, provider_name).await {
                                                                        Ok(()) => {
                                                                            oauth_status.set(Some(OAuthStatus {
                                                                                connected: false,
                                                                                provider: None,
                                                                                expires_in_seconds: None,
                                                                                error: None,
                                                                            }));
                                                                            // Refresh providers list
                                                                            if let Ok(list) = ProvidersApi::list(&state).await {
                                                                                providers.set(list);
                                                                            }
                                                                        }
                                                                        Err(e) => {
                                                                            error.set(Some(crate::components::admin_refusal::settings_write_error(
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
                                                    let provider_name = login_id.clone();
                                                    view! {
                                                        <button
                                                            on:click=move |_| {
                                                                let provider_name = provider_name.clone();
                                                                oauth_loading.set(true);
                                                                let state = expect_context::<DashboardState>();
                                                                spawn_local(async move {
                                                                    match ProvidersApi::oauth_login(&state, provider_name).await {
                                                                        Ok(status) => {
                                                                            // The status echoes the canonical
                                                                            // provider the login speaks for, which
                                                                            // is not always the id we asked about.
                                                                            let canonical = status.provider.clone();
                                                                            oauth_status.set(Some(status));
                                                                            if let Ok(list) = ProvidersApi::list(&state).await {
                                                                                let actual = canonical
                                                                                    .filter(|c| list.iter().any(|p| &p.name == c))
                                                                                    .or_else(|| list.iter()
                                                                                        .find(|p| p.name == "chatgpt" || p.name == "codex")
                                                                                        .map(|p| p.name.clone()));
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

                                        // Model ladder
                                        <ModelLadder
                                            provider_id=key.clone()
                                            models=form_model
                                            catalog=catalog
                                            error=error
                                            refresh=refresh
                                        />

                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.providers.configuration)}</h3>
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
                                let sel_for_name = sel.clone();
                                view! {
                                    <div class="space-y-6">
                                        // Configuration form card
                                        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
                                            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">{t!(i18n, settings.providers.configuration)}</h3>

                                            // Name (editable only for new custom)
                                            {move || if sel_for_name == "__new__" {
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
                                                    view! {
                                                        <ProviderKeyField value=form_api_key has_api_key=has_api_key />
                                                    }
                                                }
                                                {if keyless {
                                                    view! {
                                                        <p class="mt-1 text-xs text-text-tertiary">{t!(i18n, settings.providers.no_api_key_needed)}</p>
                                                    }.into_any()
                                                } else {
                                                    match signup_url.clone() {
                                                        Some(url) => view! {
                                                            <a
                                                                href=url
                                                                target="_blank"
                                                                rel="noopener noreferrer"
                                                                class="mt-1 inline-block text-xs text-primary hover:underline"
                                                            >
                                                                {t!(i18n, settings.providers.get_a_key)}
                                                            </a>
                                                        }.into_any(),
                                                        None => view! { <span></span> }.into_any(),
                                                    }
                                                }}
                                            </div>

                                            // Base URL
                                            <div>
                                                <label class="block text-sm text-text-secondary mb-1">{t!(i18n, settings.providers.base_url)}</label>
                                                <input
                                                    type="text"
                                                    prop:value=move || form_base_url.get()
                                                    on:input=move |ev| form_base_url.set(event_target_value(&ev))
                                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                                    placeholder=base_url_hint.clone().unwrap_or_else(|| "https://api.example.com/v1".to_string())
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

                                        // Model ladder
                                        <ModelLadder
                                            provider_id=key.clone()
                                            models=form_model
                                            catalog=catalog
                                            error=error
                                            refresh=refresh
                                        />

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
                                                // Delete removes the `[providers.<id>]` section, so
                                                // it is offered for anything that has one. It used
                                                // to be hidden for presets, on the theory that a
                                                // preset row cannot be deleted — but the row is not
                                                // what is deleted, the configuration is, and that is
                                                // the only way to clear a preset's key from this
                                                // page. The preset itself comes back unconfigured.
                                                let deletable = s.as_ref().is_some_and(|s| {
                                                    providers.get().iter().any(|p| &p.name == s)
                                                });
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
                                                            {if deletable {
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
