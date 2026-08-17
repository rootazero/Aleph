//! iOS Providers screen.
//!
//! Scope: add a provider from the preset catalogue, list configured providers,
//! set-default, toggle-enable, edit API key.
//! Out of scope: remove providers, model-picker, subscription login,
//! connectivity checks.
//!
//! # Why "add" is not optional here
//!
//! The phone build ships as a panel against a remote core, so this screen is
//! the whole provider surface for anyone who owns no desktop. Without an add
//! path a fresh install rendered "no providers configured" and nothing else —
//! a dead end on the one screen whose job is to end it. The desktop settings
//! page grew a searchable catalogue disclosure
//! ([`crate::components::preset_picker`]); this is the same idea in the iOS
//! cell idiom, and it is deliberately *not* that component: `PresetPicker`
//! draws Tailwind cards and is built around ↑/↓/Enter, none of which exists on
//! a touch device.
//!
//! What the two screens do share is the parts that would drift if copied — the
//! matcher ([`filter_catalog`], so `sonnet` finds Anthropic here too) and the
//! two predicates that relate a catalogue row to a configured one
//! ([`is_configured`], [`configured_key`]). `platform` enforces that phone code
//! cannot reach into `wide`, so those live beside the wire types.

use aleph_protocol::providers::search::filter_catalog;

use crate::api::{
    configured_key, is_configured, AuthKind, CatalogEntry, CatalogView, ProviderConfigJson,
    ProviderInfo, ProvidersApi,
};
use crate::components::picker_nav::publish_more_below;
use crate::context::DashboardState;
use crate::i18n::{t, t_string};
use crate::platform::phone::shell::PhoneShell;
use leptos::html::Div;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The catalogue rows this screen offers for `query`, best match first.
///
/// Two exclusions, and only one of them has the same reason as the desktop
/// screen's:
///
/// * `protocol == "moa"` — a virtual multiplexer over other providers'
///   credentials with no config section of its own. Adding it would write
///   nonsense. Same exclusion, same reason, as `wide`'s `pickable`.
/// * `auth_kind == OAuth` — the desktop hides these here because they have an
///   always-visible subscription-login section of their own. This screen has no
///   such section and runs no OAuth flow at all, so the reason is different and
///   stronger: the only form it could offer is an API-key form, and these rows
///   take no API key. A row that can only produce a broken provider is worse
///   than an absent one.
///
/// Configured rows are deliberately **kept** and marked. Hiding a provider
/// because you already set it up teaches the reader that Aleph does not support
/// it; picking one opens its existing row instead of creating a duplicate.
///
/// An empty query returns every offerable row, in the server's curated order —
/// the same contract the desktop picker's `offer` closure owes. Browsing must
/// not require knowing a vendor's name first.
#[must_use]
fn offerable(catalog: &[CatalogEntry], query: &str) -> Vec<CatalogEntry> {
    filter_catalog(catalog, query)
        .into_iter()
        .filter(|e| e.protocol != "moa" && e.auth_kind != AuthKind::OAuth)
        .collect()
}

/// True when this row needs no credential — a local endpoint (Ollama, LM
/// Studio, …) reached over the LAN.
///
/// Off the row rather than off a `needs_api_key` list in this crate: the same
/// fact the desktop editor reads, so the two screens cannot come to disagree
/// about which vendors are keyless.
#[must_use]
fn keyless(entry: &CatalogEntry) -> bool {
    entry.endpoint == "local"
}

/// The config an add writes for `entry`.
///
/// Mirrors what the desktop setup form submits for a `__preset__` selection:
/// the preset's own protocol and base URL, one model rung, enabled, and the
/// 300s default timeout. `color` is not sent — the server owns the preset's
/// palette, and a client that supplied one would be a second author for it.
#[must_use]
fn preset_config(entry: &CatalogEntry, model: &str, api_key: &str) -> ProviderConfigJson {
    ProviderConfigJson {
        protocol: Some(entry.protocol.clone()),
        enabled: true,
        models: vec![model.trim().to_string()],
        // Empty means "keep looking in the environment", which is exactly what
        // an operator with `OPENAI_API_KEY` already exported wants.
        api_key: (!api_key.is_empty()).then(|| api_key.to_string()),
        base_url: (!entry.base_url.is_empty()).then(|| entry.base_url.clone()),
        color: None,
        timeout_seconds: Some(300),
        max_tokens: None,
        context_window: None,
        temperature: None,
        top_p: None,
        top_k: None,
    }
}

/// The fields both writers below pass through untouched.
///
/// `providers.update` replaces the stored config wholesale, so every write from
/// this screen has to re-send everything it is not changing. The ladder is the
/// field that used to be lost: this screen sent a single `model`, so toggling a
/// provider's switch on a phone truncated its failover ladder to the first rung.
fn passthrough(info: &ProviderInfo) -> ProviderConfigJson {
    ProviderConfigJson {
        protocol: info.provider_type.clone(),
        enabled: info.enabled,
        models: info.models.clone(),
        api_key: None,
        base_url: info.base_url.clone(),
        color: None,
        timeout_seconds: Some(info.timeout_seconds),
        max_tokens: info.max_tokens,
        context_window: info.context_window,
        temperature: info.temperature,
        top_p: None,
        top_k: None,
    }
}

/// Build a minimal config carrying only a changed `enabled`.
fn config_enabled(info: &ProviderInfo, enabled: bool) -> ProviderConfigJson {
    ProviderConfigJson {
        enabled,
        ..passthrough(info)
    }
}

/// Build a config carrying only a new `api_key`.
fn config_with_key(info: &ProviderInfo, key: String) -> ProviderConfigJson {
    ProviderConfigJson {
        api_key: if key.is_empty() { None } else { Some(key) },
        ..passthrough(info)
    }
}

/// iOS-native providers list screen (phone form-factor only).
///
/// Three per-provider actions — set as default, toggle enabled, update API key.
/// Shows a tap-to-expand edit row per provider for inline editing.
#[component]
#[must_use]
pub fn PhoneProviders() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let state = expect_context::<DashboardState>();

    let providers = RwSignal::new(Vec::<ProviderInfo>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Which provider's edit region is open (by name).
    let expanded = RwSignal::new(Option::<String>::None);

    // Per-provider key input values: keyed by provider name.
    // We use a single RwSignal<String> per session slot; since users edit one
    // provider at a time we keep just the currently-expanded key input.
    let key_input = RwSignal::new(String::new());
    let key_saving = RwSignal::new(false);
    let default_saving = RwSignal::new(false);

    // Every preset the core ships. `All`, not `Configured`: the point of the
    // disclosure below is offering providers nobody has set up yet.
    let catalog = RwSignal::new(Vec::<CatalogEntry>::new());
    // Add-a-provider disclosure. `add_target` is the preset being set up;
    // `None` while browsing.
    let add_open = RwSignal::new(false);
    let add_search = RwSignal::new(String::new());
    let add_target = RwSignal::new(Option::<CatalogEntry>::None);
    let add_model = RwSignal::new(String::new());
    let add_key = RwSignal::new(String::new());
    let add_saving = RwSignal::new(false);
    let add_list_ref = NodeRef::<Div>::new();
    let add_more_below = RwSignal::new(false);

    let reload = {
        move || {
            spawn_local(async move {
                match ProvidersApi::list(&state).await {
                    Ok(list) => {
                        providers.set(list);
                        error.set(None);
                    }
                    Err(e) => error.set(Some(
                        crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                            format!("加载失败: {e}")
                        }),
                    )),
                }
                loading.set(false);
            });
        }
    };

    // Initial load — connect-gated so a cold-boot deep-link / route-restore
    // doesn't fire before the WS handshake (which returns "Not connected" and
    // strands a permanent error banner). Re-runs when is_connected flips true.
    Effect::new(move || {
        if state.is_connected.get() {
            reload();
            spawn_local(async move {
                // A catalogue this screen could not fetch costs the add path
                // and nothing else, so it does not touch the error banner the
                // provider list owns — the disclosure reports its own emptiness.
                if let Ok(items) = ProvidersApi::catalog(&state, CatalogView::All).await {
                    catalog.set(items);
                }
            });
        }
    });

    // Whether the offered list still has rows under its bottom edge. Measured
    // on the frame after the list changes: Leptos effects are queued off the
    // render pass, so a synchronous `scrollHeight` here would describe the
    // list as it was before this keystroke narrowed it.
    let remeasure_add = move || publish_more_below(add_list_ref, add_more_below);
    Effect::new(move |_| {
        add_open.track();
        add_search.track();
        add_target.track();
        catalog.track();
        request_animation_frame(remeasure_add);
    });

    // Submit an add. Validation mirrors the desktop form: the wire rejects an
    // empty ladder, so a model id is required even when the preset ships one
    // (`requires_explicit_model` rows seed nothing).
    let submit_add = move |entry: CatalogEntry| {
        let model = add_model.get_untracked();
        if model.trim().is_empty() {
            error.set(Some(
                t_string!(i18n, settings.providers.model_required).to_string(),
            ));
            return;
        }
        let config = preset_config(&entry, &model, &add_key.get_untracked());
        let name = entry.id.clone();
        let reload = reload;
        add_saving.set(true);
        error.set(None);
        spawn_local(async move {
            match ProvidersApi::create(&state, name.clone(), config).await {
                Ok(()) => {
                    add_open.set(false);
                    add_target.set(None);
                    add_search.set(String::new());
                    add_key.set(String::new());
                    add_model.set(String::new());
                    // Land on the row that was just created, expanded: the next
                    // thing anyone does is set it as default or check the key.
                    expanded.set(Some(name));
                    reload();
                }
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("添加失败: {e}")
                    }),
                )),
            }
            add_saving.set(false);
        });
    };

    view! {
        <PhoneShell title="Providers" back="/settings">
            // Error banner
            {move || error.get().map(|e| view! {
                <div style="padding:10px 14px; background:color-mix(in oklch, var(--color-danger) 12%, transparent); border:1px solid color-mix(in oklch, var(--color-danger) 30%, transparent); border-radius:10px; color:var(--color-danger); font-size:14px;">
                    {e}
                </div>
            })}

            // Add a provider — the disclosure, and inside it either the
            // searchable catalogue or the setup form for a chosen preset.
            //
            // One reactive read decides which, rather than a `<Show>` guard
            // plus a body that re-reads the same signal: those are two
            // reactive scopes, and the body can run once against a value the
            // guard has already invalidated.
            <div class="list">
                <div
                    class="cell"
                    style="cursor:pointer;"
                    on:click=move |_| {
                        let next = !add_open.get_untracked();
                        add_open.set(next);
                        if !next {
                            add_target.set(None);
                            add_search.set(String::new());
                        }
                    }
                >
                    <div class="cell-body">
                        <div class="cell-title" style="color:var(--color-primary);">
                            {t!(i18n, settings.picker.add)}
                        </div>
                        <div style="font-size:12px; color:var(--color-text-tertiary); margin-top:2px;">
                            {t!(i18n, settings.picker.phone_add_hint)}
                        </div>
                    </div>
                    <svg
                        class="cell-chevron"
                        width="18" height="18" viewBox="0 0 24 24" fill="none"
                        stroke="currentColor" stroke-width="1.8"
                        stroke-linecap="round" stroke-linejoin="round"
                        style=move || if add_open.get() {
                            "transform:rotate(90deg); transition:transform .18s;"
                        } else {
                            "transform:rotate(0deg); transition:transform .18s;"
                        }
                    >
                        <polyline points="9 6 15 12 9 18"></polyline>
                    </svg>
                </div>

                {move || {
                    if !add_open.get() {
                        return ().into_any();
                    }
                    let Some(entry) = add_target.get() else {
                        // Browsing.
                        return view! {
                            <div style="padding:10px 16px; border-top:1px solid var(--color-border);">
                                <input
                                    type="text"
                                    placeholder=move || t_string!(i18n, settings.picker.search).to_string()
                                    prop:value=move || add_search.get()
                                    on:input=move |ev| add_search.set(event_target_value(&ev))
                                    style="width:100%; padding:8px 10px; background:var(--color-surface-sunken); border:1px solid var(--color-border); border-radius:8px; font-size:14px; color:var(--color-text-primary); outline:none; box-sizing:border-box;"
                                />
                            </div>
                            <div
                                node_ref=add_list_ref
                                on:scroll=move |_| remeasure_add()
                                class:aleph-scroll-more=move || add_more_below.get()
                                style="max-height:46vh; overflow-y:auto;"
                            >
                                {move || {
                                    let rows = offerable(&catalog.get(), &add_search.get());
                                    if rows.is_empty() {
                                        return view! {
                                            <div style="text-align:center; color:var(--color-text-tertiary); font-size:14px; padding:20px 0;">
                                                {t!(i18n, settings.picker.empty)}
                                            </div>
                                        }.into_any();
                                    }
                                    rows.into_iter().map(|e| {
                                        let already = is_configured(&e);
                                        let subtitle = if e.default_model.is_empty() {
                                            e.notes.clone().unwrap_or_else(|| e.base_url.clone())
                                        } else {
                                            e.default_model.clone()
                                        };
                                        let display = e.display_name.clone();
                                        let color = e.color.clone();
                                        let held = StoredValue::new(e);
                                        view! {
                                            <div class="cell" style="cursor:pointer;"
                                                on:click=move |_| {
                                                    let entry = held.get_value();
                                                    // Already set up: open its
                                                    // existing row rather than
                                                    // `create` a duplicate the
                                                    // server would refuse. The
                                                    // key is what
                                                    // `providers.list` reports,
                                                    // which an alias makes
                                                    // different from the
                                                    // catalogue id.
                                                    if let Some(key) =
                                                        configured_key(&entry, &providers.get_untracked())
                                                    {
                                                        add_open.set(false);
                                                        add_search.set(String::new());
                                                        key_input.set(String::new());
                                                        expanded.set(Some(key));
                                                    } else {
                                                        add_model.set(entry.default_model.clone());
                                                        add_key.set(String::new());
                                                        add_target.set(Some(entry));
                                                    }
                                                }
                                            >
                                                <span
                                                    style=format!("width:10px; height:10px; border-radius:5px; margin-right:10px; flex:none; background:{color};")
                                                ></span>
                                                <div class="cell-body">
                                                    <div class="cell-title">{display}</div>
                                                    <div style="font-size:12px; color:var(--color-text-tertiary); margin-top:2px;">
                                                        {subtitle}
                                                    </div>
                                                </div>
                                                {already.then(|| view! {
                                                    <span class="cell-value">
                                                        {t!(i18n, settings.picker.configured)}
                                                    </span>
                                                })}
                                            </div>
                                        }
                                    }).collect_view().into_any()
                                }}
                            </div>
                        }.into_any();
                    };

                    // Setting up a chosen preset.
                    let needs_key = !keyless(&entry);
                    let title = entry.display_name.clone();
                    let held = StoredValue::new(entry);
                    view! {
                        <div style="padding:12px 16px; display:flex; flex-direction:column; gap:10px; border-top:1px solid var(--color-border);">
                            <div style="font-size:15px; font-weight:600; color:var(--color-text-primary);">
                                {format!("{} {}", t_string!(i18n, settings.providers.setup_prefix), title)}
                            </div>

                            <div style="font-size:13px; color:var(--color-text-secondary); font-weight:500;">
                                {t!(i18n, settings.providers.models_label)}
                            </div>
                            <input
                                type="text"
                                placeholder=move || t_string!(i18n, settings.providers.models_add_placeholder).to_string()
                                prop:value=move || add_model.get()
                                on:input=move |ev| add_model.set(event_target_value(&ev))
                                style="width:100%; padding:8px 10px; background:var(--color-surface-sunken); border:1px solid var(--color-border); border-radius:8px; font-size:14px; color:var(--color-text-primary); outline:none; box-sizing:border-box;"
                            />

                            <div style="font-size:13px; color:var(--color-text-secondary); font-weight:500;">
                                {t!(i18n, settings.providers.api_key)}
                            </div>
                            {if needs_key {
                                view! {
                                    <input
                                        type="password"
                                        placeholder=move || t_string!(i18n, settings.providers.api_key_placeholder).to_string()
                                        prop:value=move || add_key.get()
                                        on:input=move |ev| add_key.set(event_target_value(&ev))
                                        style="width:100%; padding:8px 10px; background:var(--color-surface-sunken); border:1px solid var(--color-border); border-radius:8px; font-size:14px; color:var(--color-text-primary); outline:none; box-sizing:border-box;"
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <div style="font-size:13px; color:var(--color-text-tertiary);">
                                        {t!(i18n, settings.providers.no_api_key_needed)}
                                    </div>
                                }.into_any()
                            }}

                            <div style="display:flex; gap:10px; justify-content:flex-end; margin-top:2px;">
                                <button
                                    on:click=move |_| add_target.set(None)
                                    style="padding:7px 16px; background:transparent; color:var(--color-text-secondary); border:1px solid var(--color-border); border-radius:8px; font-size:14px; cursor:pointer;"
                                >
                                    {t!(i18n, settings.providers.cancel)}
                                </button>
                                <button
                                    prop:disabled=move || add_saving.get()
                                    on:click=move |_| submit_add(held.get_value())
                                    style="padding:7px 18px; background:var(--color-primary); color:#fff; border:0; border-radius:8px; font-size:14px; font-weight:500; cursor:pointer;"
                                >
                                    {move || if add_saving.get() {
                                        t_string!(i18n, settings.providers.adding).to_string()
                                    } else {
                                        t_string!(i18n, settings.picker.create).to_string()
                                    }}
                                </button>
                            </div>
                        </div>
                    }.into_any()
                }}
            </div>

            // Loading state
            {move || loading.get().then(|| view! {
                <div style="text-align:center; color:var(--color-text-tertiary); font-size:14px; padding:24px 0;">
                    "加载中…"
                </div>
            })}

            // Providers list
            {move || {
                let list = providers.get();
                if list.is_empty() && !loading.get() {
                    return view! {
                        <div style="text-align:center; color:var(--color-text-tertiary); font-size:14px; padding:24px 0;">
                            "暂无配置的 Provider"
                        </div>
                    }.into_any();
                }

                view! {
                    <div class="list">
                        {list.into_iter().map(|info| {
                            let name = info.name.clone();
                            let name_for_expand = name.clone();
                            let name_for_default = name.clone();
                            let name_for_enable = name.clone();
                            let name_for_key_save = name.clone();

                            let is_expanded = {
                                let name = name.clone();
                                move || expanded.get().as_deref() == Some(name.as_str())
                            };

                            // Badge text shown on the collapsed row.
                            let badge = {
                                let info = info.clone();
                                move || {
                                    if info.is_default {
                                        "默认".to_string()
                                    } else if info.enabled {
                                        "已启用".to_string()
                                    } else {
                                        "已禁用".to_string()
                                    }
                                }
                            };

                            // When expanding a row, populate the key input with an empty
                            // string (never pre-fill secrets — mirrors desktop behaviour).
                            let on_expand = move |_| {
                                let currently_open = expanded.get();
                                if currently_open.as_deref() == Some(name_for_expand.as_str()) {
                                    expanded.set(None);
                                } else {
                                    key_input.set(String::new());
                                    expanded.set(Some(name_for_expand.clone()));
                                }
                            };

                            let info_for_default = StoredValue::new(info.clone());
                            let info_for_enable = StoredValue::new(info.clone());
                            let info_for_key = StoredValue::new(info.clone());
                            let state_for_default = state;
                            let state_for_enable = state;
                            let state_for_key = state;
                            let reload_for_default = StoredValue::new(reload);
                            let reload_for_enable = StoredValue::new(reload);
                            let reload_for_key = StoredValue::new(reload);
                            let name_for_default = StoredValue::new(name_for_default);
                            let name_for_enable = StoredValue::new(name_for_enable);
                            let name_for_key_save = StoredValue::new(name_for_key_save);

                            view! {
                                // Collapsed summary row — tap to expand/collapse.
                                <div class="cell" on:click=on_expand style="cursor:pointer;">
                                    <div class="cell-body">
                                        <div class="cell-title">{name.clone()}</div>
                                        <div style="font-size:12px; color:var(--color-text-tertiary); margin-top:2px;">
                                            {info.model.clone()}
                                        </div>
                                    </div>
                                    <span class="cell-value">{badge}</span>
                                    // Chevron rotates to indicate open/closed.
                                    <svg
                                        class="cell-chevron"
                                        width="18" height="18"
                                        viewBox="0 0 24 24"
                                        fill="none"
                                        stroke="currentColor"
                                        stroke-width="1.8"
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        style={
                                            let is_expanded_style = is_expanded.clone();
                                            move || if is_expanded_style() {
                                                "transform:rotate(90deg); transition:transform .18s;"
                                            } else {
                                                "transform:rotate(0deg); transition:transform .18s;"
                                            }
                                        }
                                    >
                                        <polyline points="9 6 15 12 9 18"></polyline>
                                    </svg>
                                </div>

                                // Expanded edit region (inline, same .list group).
                                <Show when=is_expanded>
                                    // "Set as default" action row
                                    <div
                                        class="cell"
                                        style="cursor:pointer; background:color-mix(in oklch, var(--color-primary) 6%, transparent);"
                                        on:click=move |_| {
                                            let name = name_for_default.get_value();
                                            let state = state_for_default;
                                            let reload = reload_for_default.get_value();
                                            default_saving.set(true);
                                            spawn_local(async move {
                                                match ProvidersApi::set_default(&state, name).await {
                                                    Ok(()) => {
                                                        error.set(None);
                                                        reload();
                                                    }
                                                    Err(e) => error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                        i18n,
                                                        &e,
                                                        |e| format!("设置默认失败: {e}"),
                                                    ))),
                                                }
                                                default_saving.set(false);
                                            });
                                        }
                                    >
                                        <div class="cell-body">
                                            <div class="cell-title" style="color:var(--color-primary);">
                                                {move || if default_saving.get() { "设置中…" } else { "设为默认" }}
                                            </div>
                                        </div>
                                        {move || info_for_default.get_value().is_default.then(|| view! {
                                            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--color-primary)" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                                                <polyline points="20 6 9 17 4 12"></polyline>
                                            </svg>
                                        })}
                                    </div>

                                    // "Enabled" toggle row
                                    <div class="cell" style="cursor:default;">
                                        <div class="cell-body">
                                            <div class="cell-title">"启用"</div>
                                        </div>
                                        <button
                                            class="ios-switch"
                                            attr:aria-pressed=move || info_for_enable.get_value().enabled.to_string()
                                            on:click=move |ev| {
                                                ev.stop_propagation();
                                                let info = info_for_enable.get_value();
                                                let new_enabled = !info.enabled;
                                                let name = name_for_enable.get_value();
                                                let state = state_for_enable;
                                                let reload = reload_for_enable.get_value();
                                                spawn_local(async move {
                                                    let cfg = config_enabled(&info, new_enabled);
                                                    match ProvidersApi::update(&state, name, cfg).await {
                                                        Ok(()) => {
                                                            error.set(None);
                                                            reload();
                                                        }
                                                        Err(e) => error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                            i18n,
                                                            &e,
                                                            |e| format!("更新失败: {e}"),
                                                        ))),
                                                    }
                                                });
                                            }
                                        >
                                            <span class="ios-knob"></span>
                                        </button>
                                    </div>

                                    // "API Key" inline input row
                                    <div style="padding:10px 16px; display:flex; flex-direction:column; gap:8px; border-top:1px solid var(--color-border);">
                                        <div style="font-size:13px; color:var(--color-text-secondary); font-weight:500;">
                                            "API Key"
                                        </div>
                                        <input
                                            type="password"
                                            placeholder=move || if info_for_key.get_value().has_api_key { "••••••••（已设置，输入新值覆盖）" } else { "输入 API Key" }
                                            prop:value=move || key_input.get()
                                            on:input=move |ev| key_input.set(event_target_value(&ev))
                                            style="width:100%; padding:8px 10px; background:var(--color-surface-sunken); border:1px solid var(--color-border); border-radius:8px; font-size:14px; color:var(--color-text-primary); outline:none; box-sizing:border-box;"
                                        />
                                        <button
                                            prop:disabled=move || key_saving.get() || key_input.get().is_empty()
                                            on:click=move |_| {
                                                let key = key_input.get();
                                                if key.is_empty() { return; }
                                                let info = info_for_key.get_value();
                                                let name = name_for_key_save.get_value();
                                                let state = state_for_key;
                                                let reload = reload_for_key.get_value();
                                                key_saving.set(true);
                                                spawn_local(async move {
                                                    let cfg = config_with_key(&info, key);
                                                    match ProvidersApi::update(&state, name, cfg).await {
                                                        Ok(()) => {
                                                            key_input.set(String::new());
                                                            error.set(None);
                                                            reload();
                                                        }
                                                        Err(e) => error.set(Some(crate::components::admin_refusal::settings_write_error(
                                                            i18n,
                                                            &e,
                                                            |e| format!("保存失败: {e}"),
                                                        ))),
                                                    }
                                                    key_saving.set(false);
                                                });
                                            }
                                            style="align-self:flex-end; padding:7px 18px; background:var(--color-primary); color:#fff; border:0; border-radius:8px; font-size:14px; font-weight:500; cursor:pointer; opacity: 1;"
                                        >
                                            {move || if key_saving.get() { "保存中…" } else { "保存" }}
                                        </button>
                                    </div>
                                </Show>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }}
        </PhoneShell>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalogue row as `providers.catalog` would report it. Built through
    /// serde rather than a struct literal so a field added later with a serde
    /// default does not have to be repeated here.
    fn entry(id: &str, protocol: &str, auth: AuthKind) -> CatalogEntry {
        let mut e: CatalogEntry = serde_json::from_value(serde_json::json!({
            "id": id,
            "display_name": id,
            "default_model": format!("{id}-default"),
            "base_url": format!("https://{id}.example/v1"),
            "protocol": protocol,
            "color": "#808080",
            "has_api_key": false,
            "verified": false,
            "enabled": true,
            "is_default": false,
        }))
        .expect("the ten required catalogue fields are all present");
        e.auth_kind = auth;
        e
    }

    fn key_row(id: &str) -> CatalogEntry {
        entry(id, "openai", AuthKind::ApiKey)
    }

    fn ids(rows: &[CatalogEntry]) -> Vec<String> {
        rows.iter().map(|e| e.id.clone()).collect()
    }

    #[test]
    fn an_empty_query_offers_every_offerable_row_in_catalogue_order() {
        // The contract the desktop picker's `offer` closure owes, restated for
        // this screen: browsing must not require typing a vendor's name.
        let cat = [key_row("groq"), key_row("mistral"), key_row("together")];
        assert_eq!(
            ids(&offerable(&cat, "")),
            vec!["groq".to_string(), "mistral".into(), "together".into()]
        );
    }

    #[test]
    fn the_moa_multiplexer_is_not_offerable() {
        let cat = [key_row("groq"), entry("moa", "moa", AuthKind::ApiKey)];
        assert_eq!(ids(&offerable(&cat, "")), vec!["groq".to_string()]);
    }

    /// This screen runs no OAuth flow, so the only form it could offer a
    /// subscription row is an API-key form — and those rows take no key.
    #[test]
    fn subscription_login_rows_are_not_offerable() {
        let cat = [key_row("groq"), entry("chatgpt", "codex", AuthKind::OAuth)];
        assert_eq!(ids(&offerable(&cat, "")), vec!["groq".to_string()]);
    }

    #[test]
    fn a_configured_row_stays_offerable_so_search_can_still_find_it() {
        let mut configured = key_row("groq");
        configured.models = vec!["llama-3".to_string()];
        assert!(is_configured(&configured));
        assert_eq!(
            ids(&offerable(&[configured], "groq")),
            vec!["groq".to_string()]
        );
    }

    #[test]
    fn a_local_endpoint_needs_no_credential() {
        let mut local = key_row("ollama");
        local.endpoint = "local".to_string();
        assert!(keyless(&local));
        assert!(!keyless(&key_row("groq")));
    }

    #[test]
    fn an_added_preset_carries_the_rows_protocol_and_base_url() {
        let cfg = preset_config(&key_row("groq"), "llama-3", "sk-abc");
        assert_eq!(cfg.protocol.as_deref(), Some("openai"));
        assert_eq!(cfg.base_url.as_deref(), Some("https://groq.example/v1"));
        assert_eq!(cfg.models, vec!["llama-3".to_string()]);
        assert_eq!(cfg.api_key.as_deref(), Some("sk-abc"));
        assert!(cfg.enabled);
        assert_eq!(cfg.timeout_seconds, Some(300));
    }

    /// An empty key means "keep looking in the environment", which is what an
    /// operator who already exported `OPENAI_API_KEY` wants — not an empty
    /// string stored as the credential.
    #[test]
    fn an_empty_key_is_omitted_rather_than_stored_blank() {
        let cfg = preset_config(&key_row("groq"), "llama-3", "");
        assert_eq!(cfg.api_key, None);
    }

    /// The wire rejects an empty ladder, and a model id pasted on a phone
    /// keyboard arrives with whitespace more often than not.
    #[test]
    fn the_model_rung_is_trimmed() {
        let cfg = preset_config(&key_row("groq"), "  llama-3 ", "");
        assert_eq!(cfg.models, vec!["llama-3".to_string()]);
    }

    #[test]
    fn a_preset_without_a_base_url_sends_none_rather_than_an_empty_string() {
        let mut row = key_row("groq");
        row.base_url = String::new();
        assert_eq!(preset_config(&row, "llama-3", "").base_url, None);
    }

    /// An alias-configured preset must resolve to the key `providers.list`
    /// reports, or picking it would `create` a duplicate the server refuses.
    #[test]
    fn an_alias_configured_preset_resolves_to_its_configured_key() {
        let mut moonshot = key_row("moonshot");
        moonshot.aliases = vec!["kimi".to_string()];
        let known: Vec<ProviderInfo> =
            vec![
                serde_json::from_value(serde_json::json!({ "name": "kimi" }))
                    .expect("every field but `name` has a serde default"),
            ];
        assert_eq!(
            configured_key(&moonshot, &known).as_deref(),
            Some("kimi"),
            "picking this row must open the existing editor, not create a second provider"
        );
        assert_eq!(configured_key(&key_row("groq"), &known), None);
    }
}
