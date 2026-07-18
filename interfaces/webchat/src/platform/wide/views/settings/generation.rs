//! Generation Settings View
//!
//! Provides UI for managing generation configuration (output dir, thresholds, routing).

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{GenerationConfig, GenerationConfigApi};
use crate::i18n::*;

#[component]
pub fn GenerationView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State
    let config = RwSignal::new(GenerationConfig {
        default_image_provider: None,
        default_video_provider: None,
        default_audio_provider: None,
        default_speech_provider: None,
        output_dir: String::new(),
        auto_paste_threshold_mb: 5,
        background_task_threshold_seconds: 30,
        smart_routing_enabled: true,
    });
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);

    // Load config on mount
    spawn_local(async move {
        match GenerationConfigApi::get(&state).await {
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

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, settings.generation_config.title)}</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    {t!(i18n, settings.generation_config.description)}
                </p>
            </div>

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">{t!(i18n, common.loading)}</div>
                        </div>
                    }.into_any()
                } else if let Some(err) = error.get() {
                    view! {
                        <div class="p-4 bg-danger-subtle border border-danger/20 rounded text-danger">
                            {err}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="space-y-6">
                            <OutputDirSection config=config />
                            <ThresholdsSection config=config />
                            <SmartRoutingSection config=config />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn OutputDirSection(config: RwSignal<GenerationConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let output_dir = RwSignal::new(config.get().output_dir.clone());
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.output_dir = output_dir.get();
        config.set(cfg.clone());

        spawn_local(async move {
            match GenerationConfigApi::update(&state, cfg).await {
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
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, settings.generation_config.output_dir)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.generation_config.output_dir_desc)}
            </p>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">
                        {t!(i18n, settings.generation_config.dir_path_label)}
                    </label>
                    <input
                        type="text"
                        value=move || output_dir.get()
                        on:input=move |ev| output_dir.set(event_target_value(&ev))
                        placeholder="~/.aleph/generation"
                        class="w-full px-3 py-2 border border-border rounded focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        {t!(i18n, settings.generation_config.dir_path_hint)}
                    </p>
                </div>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                {t!(i18n, settings.generation_config.saved_successfully)}
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>
        </div>
    }
}

#[component]
fn ThresholdsSection(config: RwSignal<GenerationConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let auto_paste_threshold = RwSignal::new(config.get().auto_paste_threshold_mb);
    let background_task_threshold = RwSignal::new(config.get().background_task_threshold_seconds);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.auto_paste_threshold_mb = auto_paste_threshold.get();
        cfg.background_task_threshold_seconds = background_task_threshold.get();
        config.set(cfg.clone());

        spawn_local(async move {
            match GenerationConfigApi::update(&state, cfg).await {
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
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, settings.generation_config.thresholds)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.generation_config.thresholds_desc)}
            </p>

            <div class="space-y-6">
                <div>
                    <div class="flex items-center justify-between mb-2">
                        <label class="block text-sm font-medium text-text-secondary">
                            {t!(i18n, settings.generation_config.auto_paste_label)} ": " {move || auto_paste_threshold.get()} " " {t!(i18n, settings.generation_config.auto_paste_unit)}
                        </label>
                    </div>
                    <input
                        type="range"
                        min="1"
                        max="100"
                        step="1"
                        value=move || auto_paste_threshold.get()
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u32>() {
                                auto_paste_threshold.set(val);
                            }
                        }
                        class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        {t!(i18n, settings.generation_config.auto_paste_hint)}
                    </p>
                </div>

                <div>
                    <div class="flex items-center justify-between mb-2">
                        <label class="block text-sm font-medium text-text-secondary">
                            {t!(i18n, settings.generation_config.bg_threshold_label)} ": " {move || background_task_threshold.get()} " " {t!(i18n, settings.generation_config.bg_threshold_unit)}
                        </label>
                    </div>
                    <input
                        type="range"
                        min="1"
                        max="300"
                        step="5"
                        value=move || background_task_threshold.get()
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u32>() {
                                background_task_threshold.set(val);
                            }
                        }
                        class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                    />
                    <p class="mt-1 text-xs text-text-tertiary">
                        {t!(i18n, settings.generation_config.bg_threshold_hint)}
                    </p>
                </div>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                {t!(i18n, settings.generation_config.saved_successfully)}
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>
        </div>
    }
}

#[component]
fn SmartRoutingSection(config: RwSignal<GenerationConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let smart_routing = RwSignal::new(config.get().smart_routing_enabled);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let save_config_fn = store_value(move || {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.smart_routing_enabled = smart_routing.get();
        config.set(cfg.clone());

        spawn_local(async move {
            match GenerationConfigApi::update(&state, cfg).await {
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
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, settings.generation_config.smart_routing)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.generation_config.smart_routing_desc)}
            </p>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || smart_routing.get()
                        on:change=move |ev| smart_routing.set(event_target_checked(&ev))
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">{t!(i18n, settings.generation_config.enable_smart_routing)}</div>
                        <div class="text-sm text-text-tertiary">
                            {t!(i18n, settings.generation_config.smart_routing_hint)}
                        </div>
                    </div>
                </label>

                {move || save_error.get().map(|e| view! {
                    <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                        {e}
                    </div>
                })}

                {move || {
                    if save_success.get() {
                        Some(view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">
                                {t!(i18n, settings.generation_config.saved_successfully)}
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <button
                    on:click=move |_| save_config_fn.with_value(|f| f())
                    disabled=move || saving.get()
                    class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                >
                    {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>
        </div>
    }
}
