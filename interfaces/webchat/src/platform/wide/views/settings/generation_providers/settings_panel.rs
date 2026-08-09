//! Generation Settings panel — output dir + thresholds + smart-routing toggle.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{GenerationConfig, GenerationConfigApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
pub(super) fn GenerationSettingsPanel() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

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
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(Option::<String>::None);
    let save_success = RwSignal::new(false);

    let load_error = RwSignal::new(Option::<String>::None);

    let load_config = move || {
        loading.set(true);
        load_error.set(None);
        spawn_local(async move {
            match GenerationConfigApi::get(&state).await {
                Ok(cfg) => {
                    config.set(cfg);
                    load_error.set(None);
                    loading.set(false);
                }
                Err(e) => {
                    load_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                    loading.set(false);
                }
            }
        });
    };

    load_config();

    let output_dir = RwSignal::new(String::new());
    let auto_paste = RwSignal::new(5u32);
    let bg_threshold = RwSignal::new(30u32);
    let smart_routing = RwSignal::new(true);

    // Sync local signals when config loads
    Effect::new(move || {
        if !loading.get() {
            let cfg = config.get();
            output_dir.set(cfg.output_dir);
            auto_paste.set(cfg.auto_paste_threshold_mb);
            bg_threshold.set(cfg.background_task_threshold_seconds);
            smart_routing.set(cfg.smart_routing_enabled);
        }
    });

    let save = move |_| {
        saving.set(true);
        save_error.set(None);
        save_success.set(false);

        let mut cfg = config.get();
        cfg.output_dir = output_dir.get();
        cfg.auto_paste_threshold_mb = auto_paste.get();
        cfg.background_task_threshold_seconds = bg_threshold.get();
        cfg.smart_routing_enabled = smart_routing.get();

        spawn_local(async move {
            match GenerationConfigApi::update(&state, cfg).await {
                Ok(_) => {
                    saving.set(false);
                    save_success.set(true);
                    set_timeout(
                        move || save_success.set(false),
                        std::time::Duration::from_secs(2),
                    );
                }
                Err(e) => {
                    saving.set(false);
                    save_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                }
            }
        });
    };

    view! {
        {move || {
            if loading.get() {
                view! {
                    <div class="text-text-tertiary text-sm">{t!(i18n, settings.generation.loading_settings)}</div>
                }.into_any()
            } else if let Some(e) = load_error.get() {
                view! {
                    <div class="space-y-4">
                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                            {e}
                        </div>
                        <button
                            on:click=move |_| load_config()
                            class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover"
                        >
                            "Retry"
                        </button>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="space-y-4">
                        // Thresholds
                        <div class="bg-surface-raised rounded-lg border border-border p-4 space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                    {t!(i18n, settings.generation.auto_paste_label)} ": " {move || auto_paste.get()} " " {t!(i18n, settings.generation.auto_paste_unit)}
                                </label>
                                <input
                                    type="range" min="1" max="100" step="1"
                                    value=move || auto_paste.get()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse::<u32>() { auto_paste.set(v); }
                                    }
                                    class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                                />
                                <p class="mt-1 text-xs text-text-tertiary">
                                    {t!(i18n, settings.generation.auto_paste_hint)}
                                </p>
                            </div>
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">
                                    {t!(i18n, settings.generation.bg_threshold_label)} ": " {move || bg_threshold.get()} " " {t!(i18n, settings.generation.bg_threshold_unit)}
                                </label>
                                <input
                                    type="range" min="1" max="300" step="5"
                                    value=move || bg_threshold.get()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse::<u32>() { bg_threshold.set(v); }
                                    }
                                    class="w-full h-2 bg-surface-sunken rounded-lg appearance-none cursor-pointer accent-primary"
                                />
                                <p class="mt-1 text-xs text-text-tertiary">
                                    {t!(i18n, settings.generation.bg_threshold_hint)}
                                </p>
                            </div>
                        </div>

                        // Smart Routing
                        <div class="bg-surface-raised rounded-lg border border-border p-4">
                            <label class="flex items-center gap-3 cursor-pointer">
                                <input
                                    type="checkbox"
                                    checked=move || smart_routing.get()
                                    on:change=move |ev| smart_routing.set(event_target_checked(&ev))
                                    class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                                />
                                <div>
                                    <div class="text-sm font-medium text-text-primary">{t!(i18n, settings.generation.smart_routing)}</div>
                                    <div class="text-xs text-text-tertiary">
                                        {t!(i18n, settings.generation.smart_routing_hint)}
                                    </div>
                                </div>
                            </label>
                        </div>

                        // Save feedback
                        {move || save_error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{e}</div>
                        })}
                        {move || save_success.get().then(|| view! {
                            <div class="p-3 bg-success-subtle border border-success/20 rounded text-success text-sm">{t!(i18n, common.saved)}</div>
                        })}

                        // Save button
                        <button
                            on:click=save
                            disabled=move || saving.get()
                            class="px-4 py-2 bg-primary text-white rounded hover:bg-primary-hover disabled:opacity-50"
                        >
                            {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                        </button>
                    </div>
                }.into_any()
            }
        }}
    }
}
