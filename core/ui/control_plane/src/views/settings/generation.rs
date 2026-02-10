//! Generation Settings View
//!
//! Provides UI for managing generation configuration (output dir, thresholds, routing).

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::context::DashboardState;
use crate::api::{GenerationConfig, GenerationConfigApi};

#[component]
pub fn GenerationView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

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
    Effect::new(move || {
        if state.is_connected.get() {
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
        }
    });

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-slate-900">"Generation Settings"</h1>
                <p class="mt-1 text-sm text-slate-600">
                    "Configure media generation settings"
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
        <div class="bg-white rounded-lg border border-slate-200 p-6">
            <h2 class="text-lg font-semibold text-slate-900 mb-4">"Output Directory"</h2>
            <p class="text-sm text-slate-600 mb-4">
                "Directory where generated files (images, videos, audio) will be saved"
            </p>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium text-slate-700 mb-2">
                        "Directory Path"
                    </label>
                    <input
                        type="text"
                        value=move || output_dir.get()
                        on:input=move |ev| output_dir.set(event_target_value(&ev))
                        placeholder="~/Downloads/aleph-gen"
                        class="w-full px-3 py-2 border border-slate-300 rounded focus:outline-none focus:ring-2 focus:ring-indigo-500"
                    />
                    <p class="mt-1 text-xs text-slate-500">
                        "Supports ~ for home directory expansion"
                    </p>
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
fn ThresholdsSection(config: RwSignal<GenerationConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
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
        <div class="bg-white rounded-lg border border-slate-200 p-6">
            <h2 class="text-lg font-semibold text-slate-900 mb-4">"Thresholds"</h2>
            <p class="text-sm text-slate-600 mb-4">
                "Configure automatic behavior thresholds"
            </p>

            <div class="space-y-6">
                <div>
                    <div class="flex items-center justify-between mb-2">
                        <label class="block text-sm font-medium text-slate-700">
                            "Auto-paste threshold: " {move || auto_paste_threshold.get()} " MB"
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
                        class="w-full h-2 bg-slate-200 rounded-lg appearance-none cursor-pointer accent-indigo-600"
                    />
                    <p class="mt-1 text-xs text-slate-500">
                        "Files smaller than this will be auto-pasted to clipboard"
                    </p>
                </div>

                <div>
                    <div class="flex items-center justify-between mb-2">
                        <label class="block text-sm font-medium text-slate-700">
                            "Background task threshold: " {move || background_task_threshold.get()} " seconds"
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
                        class="w-full h-2 bg-slate-200 rounded-lg appearance-none cursor-pointer accent-indigo-600"
                    />
                    <p class="mt-1 text-xs text-slate-500">
                        "Tasks longer than this will run in background"
                    </p>
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
fn SmartRoutingSection(config: RwSignal<GenerationConfig>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
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
        <div class="bg-white rounded-lg border border-slate-200 p-6">
            <h2 class="text-lg font-semibold text-slate-900 mb-4">"Smart Routing"</h2>
            <p class="text-sm text-slate-600 mb-4">
                "Automatically select the best provider based on generation type and capabilities"
            </p>

            <div class="space-y-4">
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked=move || smart_routing.get()
                        on:change=move |ev| smart_routing.set(event_target_checked(&ev))
                        class="w-4 h-4 text-indigo-600 focus:ring-indigo-500 rounded"
                    />
                    <div>
                        <div class="font-medium text-slate-900">"Enable Smart Routing"</div>
                        <div class="text-sm text-slate-500">
                            "Automatically route requests to the most suitable provider"
                        </div>
                    </div>
                </label>

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
