//! iOS Providers screen — focused v1.
//!
//! Scope (v1): list configured providers, set-default, toggle-enable, edit API key.
//! Out of scope for v1: add/remove providers, model-picker, subscription login, connectivity checks.

use crate::api::{ProviderConfig, ProviderInfo, ProvidersApi};
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Build a minimal `ProviderConfig` carrying only `enabled` (preserves all
/// other fields at their current server values by using `None` / passing
/// the existing `model` through).
fn config_enabled(info: &ProviderInfo, enabled: bool) -> ProviderConfig {
    ProviderConfig {
        protocol: info.provider_type.clone(),
        enabled,
        model: info.model.clone(),
        api_key: None,
        base_url: info.base_url.clone(),
        color: None,
        timeout_seconds: Some(info.timeout_seconds),
        max_tokens: info.max_tokens,
        temperature: info.temperature,
        top_p: None,
        top_k: None,
    }
}

/// Build a `ProviderConfig` carrying only a new `api_key` (all other fields
/// passed through from the existing provider).
fn config_with_key(info: &ProviderInfo, key: String) -> ProviderConfig {
    ProviderConfig {
        protocol: info.provider_type.clone(),
        enabled: info.enabled,
        model: info.model.clone(),
        api_key: if key.is_empty() { None } else { Some(key) },
        base_url: info.base_url.clone(),
        color: None,
        timeout_seconds: Some(info.timeout_seconds),
        max_tokens: info.max_tokens,
        temperature: info.temperature,
        top_p: None,
        top_k: None,
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
        }
    });

    view! {
        <PhoneShell title="Providers" back="/settings">
            // Error banner
            {move || error.get().map(|e| view! {
                <div style="padding:10px 14px; background:color-mix(in oklch, var(--color-danger) 12%, transparent); border:1px solid color-mix(in oklch, var(--color-danger) 30%, transparent); border-radius:10px; color:var(--color-danger); font-size:14px;">
                    {e}
                </div>
            })}

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
