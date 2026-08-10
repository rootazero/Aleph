use super::desktop_autostart::DesktopAutostartSection;
use crate::api::{ConfigApi, GeneralConfig, GeneralConfigApi};
use crate::context::{DashboardState, GATEWAY_NOT_READY};
use crate::i18n::{t, t_string, use_i18n, Locale};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
#[must_use]
pub fn GeneralView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // `None` until the server answers. It used to start as
    // `Some(GeneralConfig { .. })` full of `None`s, which made the "no config"
    // arm of the view below structurally unreachable and turned every failed
    // load into a page that rendered fabricated defaults as if they were the
    // stored settings. Only a completed read may put values on this page —
    // the same rule `components::admin_refusal` states for refusals, applied
    // to the read that never happened.
    let (config, set_config) = signal(Option::<GeneralConfig>::None);
    let (loading, set_loading) = signal(true);
    let (saving, set_saving) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // Store save_config in a StoredValue to avoid closure capture issues
    let save_config_fn = StoredValue::new(move || {
        if !state.is_connected.get() {
            return;
        }
        if let Some(cfg) = config.get() {
            set_saving.set(true);
            set_error.set(None);

            spawn_local(async move {
                match GeneralConfigApi::update(&state, cfg).await {
                    Ok(_) => {
                        set_saving.set(false);
                    }
                    Err(e) => {
                        set_error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Failed to save config: {e}")
                            }),
                        ));
                        set_saving.set(false);
                    }
                }
            });
        }
    });

    // Load config on mount — use spawn_local directly for robustness
    // (Effect::new can have scope issues in reactive closures)
    spawn_local(async move {
        match GeneralConfigApi::get(&state).await {
            Ok(cfg) => {
                // Restore locale from backend language preference
                if let Some(ref lang) = cfg.language {
                    match lang.as_str() {
                        "en" => i18n.set_locale(Locale::en),
                        "zh" => i18n.set_locale(Locale::zh),
                        _ => {}
                    }
                }
                set_config.set(Some(cfg));
                set_loading.set(false);
            }
            Err(e) => {
                set_error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load config: {e}"),
                )));
                set_loading.set(false);
            }
        }
    });

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-4xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    {t!(i18n, settings.general.title)}
                </h1>
                <p class="text-text-secondary">
                    {t!(i18n, settings.general.description)}
                </p>
            </div>

            // Hoisted out of the config branch below. A failed load now leaves
            // `config` at `None`, so an error rendered *inside* that branch
            // would be invisible in exactly the case it exists for.
            {move || {
                error.get().map(|e| {
                    // Classified by the marker the transport actually emits,
                    // not by substring. This arm used to read
                    // `e.contains("Failed to load")` — the framing that
                    // `settings_load_error` puts on EVERY non-refusal error —
                    // so a store error, a malformed response and a timeout all
                    // rendered as "the gateway is unavailable", a claim about
                    // the server that none of them supports.
                    if e.contains(GATEWAY_NOT_READY) {
                        view! {
                            <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm flex items-center gap-2 mb-4">
                                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <circle cx="12" cy="12" r="10"/>
                                    <line x1="12" y1="16" x2="12" y2="12"/>
                                    <line x1="12" y1="8" x2="12.01" y2="8"/>
                                </svg>
                                {t!(i18n, common.gateway_unavailable)}
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm mb-4">
                                {e}
                            </div>
                        }.into_any()
                    }
                })
            }}

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-secondary">{t!(i18n, common.loading)}</div>
                        </div>
                    }.into_any()
                } else if let Some(cfg) = config.get() {
                    view! {
                        <div class="space-y-6">
                            <LanguageSection
                                language=cfg.language
                                on_change=move |lang| {
                                    if let Some(mut c) = config.get() {
                                        c.language = lang;
                                        set_config.set(Some(c));
                                        save_config_fn.with_value(|f| f());
                                    }
                                }
                            />

                            <ConfigReloadSection />

                            <DesktopAutostartSection />

                            {move || {
                                if saving.get() {
                                    Some(view! {
                                        <div class="p-3 bg-primary-subtle border border-primary/20 rounded-lg text-primary text-sm">
                                            {t!(i18n, common.saving)}
                                        </div>
                                    })
                                } else {
                                    None
                                }
                            }}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="text-text-secondary">{t!(i18n, settings.general.config_reload.no_config)}</div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn LanguageSection(
    language: Option<String>,
    on_change: impl Fn(Option<String>) + 'static + Copy,
) -> impl IntoView {
    let i18n = use_i18n();
    let (selected, set_selected) = signal(language.unwrap_or_else(|| "system".to_string()));

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-4">{t!(i18n, settings.general.language.title)}</h2>

            <div>
                <label class="block text-sm font-medium text-text-secondary mb-2">
                    {t!(i18n, settings.general.language.label)}
                </label>
                <select
                    prop:value=move || selected.get()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        set_selected.set(value.clone());

                        // Apply locale switch immediately
                        match value.as_str() {
                            "en" => i18n.set_locale(Locale::en),
                            "zh" => i18n.set_locale(Locale::zh),
                            _ => {
                                // "system" — detect from browser
                                let browser_lang = web_sys::window()
                                    .and_then(|w| w.navigator().language())
                                    .unwrap_or_default();
                                if browser_lang.starts_with("zh") {
                                    i18n.set_locale(Locale::zh);
                                } else {
                                    i18n.set_locale(Locale::en);
                                }
                            }
                        }

                        let lang = if value == "system" { None } else { Some(value) };
                        on_change(lang);
                    }
                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                >
                    <option value="system">{t!(i18n, settings.general.language.system)}</option>
                    <option value="en">{t!(i18n, settings.general.language.en)}</option>
                    <option value="zh">{t!(i18n, settings.general.language.zh)}</option>
                </select>
            </div>
        </div>
    }
}

#[component]
fn ConfigReloadSection() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let (reloading, set_reloading) = signal(false);
    let (result_msg, set_result_msg) = signal(Option::<(bool, String)>::None);

    let on_reload = move |_| {
        if !state.is_connected.get() {
            return;
        }
        set_reloading.set(true);
        set_result_msg.set(None);

        spawn_local(async move {
            match ConfigApi::reload(&state).await {
                Ok(result) => {
                    let msg = if result.ok {
                        format!("Reloaded: {}", result.reloaded.join(", "))
                    } else {
                        let failed_names: Vec<String> = result
                            .failed
                            .iter()
                            .filter_map(|f| {
                                f.get("subsystem")
                                    .and_then(|s| s.as_str())
                                    .map(String::from)
                            })
                            .collect();
                        format!("Partial reload. Failed: {}", failed_names.join(", "))
                    };
                    set_result_msg.set(Some((result.ok, msg)));
                }
                Err(e) => {
                    set_result_msg.set(Some((
                        false,
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Reload failed: {e}")
                        }),
                    )));
                }
            }
            set_reloading.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-2">{t!(i18n, settings.general.config_reload.title)}</h2>
            <p class="text-sm text-text-secondary mb-4">
                {t!(i18n, settings.general.config_reload.description)}
            </p>

            <button
                on:click=on_reload
                disabled=move || reloading.get() || !state.is_connected.get()
                class="px-4 py-2 bg-primary text-white rounded-lg hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed text-sm font-medium"
            >
                {move || if reloading.get() {
                    t_string!(i18n, settings.general.config_reload.reloading).to_string()
                } else {
                    t_string!(i18n, settings.general.config_reload.button).to_string()
                }}
            </button>

            {move || result_msg.get().map(|(ok, msg)| {
                let class = if ok {
                    "mt-3 p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm"
                } else {
                    "mt-3 p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm"
                };
                view! { <div class=class>{msg}</div> }
            })}
        </div>
    }
}
