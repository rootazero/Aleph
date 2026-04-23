//! Memory Configuration View
//!
//! Provides UI for managing memory/RAG configuration:
//! - Basic settings (enabled, embedding model, vector DB)
//! - AI retrieval settings
//! - Compression settings
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{FusionStrategy, MemoryConfig, MemoryConfigApi, RetrieveWithTraceResponse};
use crate::context::DashboardState;
use crate::i18n::*;

#[component]
pub fn MemoryView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let config = RwSignal::new(Option::<MemoryConfig>::None);
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    // Load config on mount
    Effect::new(move || {
        if state.is_connected.get() {
            spawn_local(async move {
                loading.set(true);
                match MemoryConfigApi::get(&state).await {
                    Ok(cfg) => {
                        config.set(Some(cfg));
                        error.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to load memory config: {}", e)));
                    }
                }
                loading.set(false);
            });
        } else {
            loading.set(false);
        }
    });

    let save = move |_| {
        if let Some(cfg) = config.get() {
            spawn_local(async move {
                saving.set(true);
                match MemoryConfigApi::update(&state, cfg).await {
                    Ok(_) => {
                        error.set(None);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to save: {}", e)));
                    }
                }
                saving.set(false);
            });
        }
    };

    view! {
        <div class="flex-1 p-6 overflow-y-auto">
            <div class="max-w-4xl">
                <h1 class="text-2xl font-bold mb-6">{t!(i18n, settings.memory.title)}</h1>

                {move || {
                    if loading.get() {
                        view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                    } else if let Some(_cfg) = config.get() {
                        view! {
                            <div class="space-y-6">
                                {move || error.get().map(|e| view! {
                                    <div class="p-3 bg-danger-subtle text-danger rounded">
                                        {e}
                                    </div>
                                })}

                                <BasicSettings config=config />
                                <AIRetrievalSettings config=config />
                                <CompressionSettings config=config />
                                <RetrievalPipelineSettings config=config />
                                <FactDecaySettings config=config />
                                <GraphDecaySettings config=config />
                                <DreamingSettings config=config />
                                <ReflectionSettings config=config />
                                <StorageBackupSettings config=config />
                                <RetrievalDebugPanel />

                                <div class="pt-4 border-t border-border">
                                    <button
                                        on:click=save
                                        prop:disabled=move || saving.get()
                                        class="px-6 py-2 bg-info text-white rounded hover:bg-primary-hover disabled:opacity-50"
                                    >
                                        {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div class="text-text-tertiary">{t!(i18n, settings.memory.no_config)}</div> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn BasicSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">{t!(i18n, settings.memory.basic_settings)}</h2>

            <div class="space-y-4">
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.enable_memory)}</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.vector_db)}</label>
                    <div class="w-full px-3 py-2 border border-border rounded bg-surface-sunken text-text-secondary">
                        {move || config.get().map(|c| c.vector_db.clone()).unwrap_or_else(|| "sqlite-vec".to_string())}
                    </div>
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.vector_db_hint)}</p>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.max_context_items)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.max_context_items).unwrap_or(5)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.max_context_items = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.retention_days)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.retention_days).unwrap_or(90)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.retention_days = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.similarity_threshold)}</label>
                    <input
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        prop:value=move || config.get().map(|c| c.similarity_threshold).unwrap_or(0.7)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.similarity_threshold = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>
            </div>
        </div>
    }
}

#[component]
fn AIRetrievalSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">{t!(i18n, settings.memory.ai_retrieval)}</h2>

            <div class="space-y-4">
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.ai_retrieval_enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.ai_retrieval_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.enable_ai_retrieval)}</label>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.timeout_ms)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.ai_retrieval_timeout_ms).unwrap_or(3000)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.ai_retrieval_timeout_ms = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.max_candidates)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.ai_retrieval_max_candidates).unwrap_or(20)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.ai_retrieval_max_candidates = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.fallback_count)}</label>
                    <input
                        type="number"
                        prop:value=move || config.get().map(|c| c.ai_retrieval_fallback_count).unwrap_or(3)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.ai_retrieval_fallback_count = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>
            </div>
        </div>
    }
}

#[component]
fn CompressionSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">{t!(i18n, settings.memory.compression)}</h2>

            <div class="space-y-4">
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.compression_enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.compression_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.enable_compression)}</label>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.idle_timeout_seconds)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.compression_idle_timeout_seconds).unwrap_or(300)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.compression_idle_timeout_seconds = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.turn_threshold)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.compression_turn_threshold).unwrap_or(20)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.compression_turn_threshold = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.compression_interval_seconds)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.compression_interval_seconds).unwrap_or(3600)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.compression_interval_seconds = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.batch_size)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.compression_batch_size).unwrap_or(50)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.compression_batch_size = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.conflict_similarity_threshold)}</label>
                        <input
                            type="number"
                            step="0.01"
                            min="0"
                            max="1"
                            prop:value=move || config.get().map(|c| c.conflict_similarity_threshold).unwrap_or(0.85)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.conflict_similarity_threshold = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.max_facts_in_context)}</label>
                        <input
                            type="number"
                            prop:value=move || config.get().map(|c| c.max_facts_in_context).unwrap_or(5)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.max_facts_in_context = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.raw_memory_fallback_count)}</label>
                    <input
                        type="number"
                        prop:value=move || config.get().map(|c| c.raw_memory_fallback_count).unwrap_or(3)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.raw_memory_fallback_count = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>
            </div>
        </div>
    }
}

#[component]
fn FactDecaySettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.fact_decay)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.memory.fact_decay_desc)}
            </p>

            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.half_life_days)}</label>
                        <input
                            type="number"
                            step="1"
                            min="1"
                            prop:value=move || config.get().map(|c| c.memory_decay.half_life_days).unwrap_or(30.0)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.memory_decay.half_life_days = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.half_life_days_hint)}</p>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.access_boost)}</label>
                        <input
                            type="number"
                            step="0.01"
                            min="0"
                            max="1"
                            prop:value=move || config.get().map(|c| c.memory_decay.access_boost).unwrap_or(0.2)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.memory_decay.access_boost = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.access_boost_hint)}</p>
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.min_strength)}</label>
                    <input
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        prop:value=move || config.get().map(|c| c.memory_decay.min_strength).unwrap_or(0.1)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.memory_decay.min_strength = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.min_strength_hint)}</p>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.protected_types)}</label>
                    <input
                        type="text"
                        prop:value=move || config.get().map(|c| c.memory_decay.protected_types.join(", ")).unwrap_or_default()
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.memory_decay.protected_types = event_target_value(&ev)
                                    .split(',')
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.protected_types_hint)}</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn GraphDecaySettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.graph_decay)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.memory.graph_decay_desc)}
            </p>

            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.node_decay_per_day)}</label>
                        <input
                            type="number"
                            step="0.001"
                            min="0"
                            max="1"
                            prop:value=move || config.get().map(|c| c.graph_decay.node_decay_per_day).unwrap_or(0.02)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.graph_decay.node_decay_per_day = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.edge_decay_per_day)}</label>
                        <input
                            type="number"
                            step="0.001"
                            min="0"
                            max="1"
                            prop:value=move || config.get().map(|c| c.graph_decay.edge_decay_per_day).unwrap_or(0.03)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.graph_decay.edge_decay_per_day = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.min_score)}</label>
                    <input
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        prop:value=move || config.get().map(|c| c.graph_decay.min_score).unwrap_or(0.1)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.graph_decay.min_score = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.min_score_hint)}</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn DreamingSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.dreaming)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.memory.dreaming_desc)}
            </p>

            <div class="space-y-4">
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.dreaming.enabled).unwrap_or(true)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.dreaming.enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.enable_dreaming)}</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.idle_threshold_seconds)}</label>
                    <input
                        type="number"
                        prop:value=move || config.get().map(|c| c.dreaming.idle_threshold_seconds).unwrap_or(900)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.dreaming.idle_threshold_seconds = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.idle_threshold_hint)}</p>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.window_start)}</label>
                        <input
                            type="text"
                            prop:value=move || config.get().map(|c| c.dreaming.window_start_local).unwrap_or_else(|| "02:00".to_string())
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    cfg.dreaming.window_start_local = event_target_value(&ev);
                                    config.set(Some(cfg));
                                }
                            }
                            placeholder="02:00"
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.window_end)}</label>
                        <input
                            type="text"
                            prop:value=move || config.get().map(|c| c.dreaming.window_end_local).unwrap_or_else(|| "05:00".to_string())
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    cfg.dreaming.window_end_local = event_target_value(&ev);
                                    config.set(Some(cfg));
                                }
                            }
                            placeholder="05:00"
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>
                <p class="text-xs text-text-tertiary">{t!(i18n, settings.memory.window_hint)}</p>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.max_duration_seconds)}</label>
                    <input
                        type="number"
                        prop:value=move || config.get().map(|c| c.dreaming.max_duration_seconds).unwrap_or(600)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.dreaming.max_duration_seconds = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.max_duration_hint)}</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn StorageBackupSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.storage_backup)}</h2>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.dedup_similarity_threshold)}</label>
                    <input
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        prop:value=move || config.get().map(|c| c.dedup_similarity_threshold).unwrap_or(0.95)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.dedup_similarity_threshold = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.dedup_similarity_hint)}</p>
                </div>

                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.backup_enabled).unwrap_or(true)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.backup_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.backup_enabled)}</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.max_backup_files)}</label>
                    <input
                        type="number"
                        min="1"
                        prop:value=move || config.get().map(|c| c.backup_max_files).unwrap_or(7)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.backup_max_files = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.max_backup_files_hint)}</p>
                </div>
            </div>
        </div>
    }
}

// ============================================================================
// Section A: Retrieval Pipeline Settings
// ============================================================================

#[component]
fn RetrievalPipelineSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.retrieval_pipeline)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.memory.retrieval_pipeline_desc)}
            </p>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.fusion_strategy)}</label>
                    <select
                        prop:value=move || config.get().map(|c| c.fusion_strategy.as_str().to_string()).unwrap_or_else(|| "rrf".to_string())
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.fusion_strategy = FusionStrategy::from_str_val(&event_target_value(&ev));
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    >
                        <option value="rrf">{t!(i18n, settings.memory.rrf_option)}</option>
                        <option value="weighted">{t!(i18n, settings.memory.weighted_option)}</option>
                    </select>
                </div>

                {move || {
                    let is_rrf = config.get().map(|c| c.fusion_strategy == FusionStrategy::Rrf).unwrap_or(true);
                    if is_rrf {
                        Some(view! {
                            <div>
                                <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.rrf_k)}</label>
                                <input
                                    type="number"
                                    min="1"
                                    prop:value=move || config.get().map(|c| c.rrf_k).unwrap_or(60)
                                    on:input=move |ev| {
                                        if let Some(mut cfg) = config.get() {
                                            if let Ok(val) = event_target_value(&ev).parse() {
                                                cfg.rrf_k = val;
                                                config.set(Some(cfg));
                                            }
                                        }
                                    }
                                    class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                                />
                                <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.rrf_k_hint)}</p>
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.bm25_bonus_weight)}</label>
                    <input
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        prop:value=move || config.get().map(|c| c.bm25_bonus_weight).unwrap_or(0.15)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.bm25_bonus_weight = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.bm25_bonus_weight_hint)}</p>
                </div>

                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.query_expansion_enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.query_expansion_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.query_expansion)}</label>
                </div>
                <p class="text-xs text-text-tertiary">{t!(i18n, settings.memory.query_expansion_hint)}</p>
            </div>
        </div>
    }
}

// ============================================================================
// Section D: Reflection Settings (extends Dreaming section)
// ============================================================================

#[component]
fn ReflectionSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.reflection)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.memory.reflection_desc)}
            </p>

            <div class="space-y-4">
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.reflection.enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.reflection.enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.enable_reflection)}</label>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.min_turns)}</label>
                        <input
                            type="number"
                            min="1"
                            prop:value=move || config.get().map(|c| c.reflection.min_turns).unwrap_or(5)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.reflection.min_turns = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.min_turns_hint)}</p>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.min_user_chars)}</label>
                        <input
                            type="number"
                            min="0"
                            prop:value=move || config.get().map(|c| c.reflection.min_user_chars).unwrap_or(200)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.reflection.min_user_chars = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.min_user_chars_hint)}</p>
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.cooldown_minutes)}</label>
                    <input
                        type="number"
                        min="1"
                        prop:value=move || config.get().map(|c| c.reflection.cooldown_minutes).unwrap_or(30)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.reflection.cooldown_minutes = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.cooldown_hint)}</p>
                </div>

                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.reflection.open_loop_tracking).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.reflection.open_loop_tracking = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.open_loop_tracking)}</label>
                </div>
                <p class="text-xs text-text-tertiary">{t!(i18n, settings.memory.open_loop_tracking_hint)}</p>

                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.reflection.open_loop_inject_prompt).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.reflection.open_loop_inject_prompt = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.open_loop_inject_prompt)}</label>
                </div>
                <p class="text-xs text-text-tertiary">{t!(i18n, settings.memory.open_loop_inject_hint)}</p>
            </div>
        </div>
    }
}

// ============================================================================
// Section E: Retrieval Debug Panel
// ============================================================================

#[component]
fn RetrievalDebugPanel() -> impl IntoView {
    let i18n = use_i18n();
    let expanded = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let searching = RwSignal::new(false);
    let trace_result = RwSignal::new(Option::<RetrieveWithTraceResponse>::None);
    let trace_error = RwSignal::new(Option::<String>::None);

    let do_search = move |_| {
        let q = query.get();
        if q.trim().is_empty() {
            return;
        }
        let state = expect_context::<DashboardState>();
        spawn_local(async move {
            searching.set(true);
            trace_error.set(None);
            trace_result.set(None);
            let params = serde_json::json!({ "query": q });
            match state.rpc_call("memory.retrieve_with_trace", params).await {
                Ok(result) => match serde_json::from_value::<RetrieveWithTraceResponse>(result) {
                    Ok(resp) => {
                        trace_result.set(Some(resp));
                    }
                    Err(e) => {
                        trace_error.set(Some(format!("Parse error: {}", e)));
                    }
                },
                Err(e) => {
                    trace_error.set(Some(format!("RPC error: {}", e)));
                }
            }
            searching.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <button
                on:click=move |_| expanded.set(!expanded.get())
                class="flex items-center w-full text-left"
            >
                <span class="text-lg font-semibold">
                    {move || {
                        let prefix = if expanded.get() { "- " } else { "+ " };
                        format!("{}{}", prefix, t_string!(i18n, settings.memory.retrieval_debug))
                    }}
                </span>
                <span class="ml-2 text-sm text-text-tertiary">{t!(i18n, settings.memory.click_to_expand)}</span>
            </button>

            {move || {
                if !expanded.get() {
                    return view! { <div></div> }.into_any();
                }

                view! {
                    <div class="mt-4 space-y-4">
                        <div class="flex gap-2">
                            <input
                                type="text"
                                prop:value=move || query.get()
                                on:input=move |ev| {
                                    query.set(event_target_value(&ev));
                                }
                                placeholder=move || t_string!(i18n, settings.memory.test_query_placeholder).to_string()
                                class="flex-1 px-3 py-2 border border-border rounded bg-surface-raised"
                            />
                            <button
                                on:click=do_search
                                prop:disabled=move || searching.get()
                                class="px-4 py-2 bg-info text-white rounded hover:bg-primary-hover disabled:opacity-50"
                            >
                                {move || if searching.get() { t_string!(i18n, settings.memory.searching).to_string() } else { t_string!(i18n, settings.memory.search).to_string() }}
                            </button>
                        </div>

                        {move || trace_error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                        })}

                        {move || trace_result.get().map(|resp| {
                            let stages = resp.trace.stages.clone();
                            let results = resp.results.clone();

                            view! {
                                <div class="space-y-4">
                                    // Trace stages table
                                    <div>
                                        <h3 class="text-sm font-semibold mb-2">{t!(i18n, settings.memory.pipeline_stages)}</h3>
                                        <div class="overflow-x-auto">
                                            <table class="w-full text-sm border border-border">
                                                <thead>
                                                    <tr class="bg-surface-sunken">
                                                        <th class="px-3 py-2 text-left border-b border-border">{t!(i18n, settings.memory.stage)}</th>
                                                        <th class="px-3 py-2 text-right border-b border-border">{t!(i18n, settings.memory.duration_ms)}</th>
                                                        <th class="px-3 py-2 text-right border-b border-border">{t!(i18n, settings.memory.input)}</th>
                                                        <th class="px-3 py-2 text-right border-b border-border">{t!(i18n, settings.memory.output)}</th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    {stages.into_iter().map(|stage| {
                                                        view! {
                                                            <tr class="border-b border-border">
                                                                <td class="px-3 py-2">{stage.name}</td>
                                                                <td class="px-3 py-2 text-right">{stage.duration_ms}</td>
                                                                <td class="px-3 py-2 text-right">{stage.input_count}</td>
                                                                <td class="px-3 py-2 text-right">{stage.output_count}</td>
                                                            </tr>
                                                        }
                                                    }).collect::<Vec<_>>()}
                                                </tbody>
                                            </table>
                                        </div>
                                    </div>

                                    // Results
                                    <div>
                                        <h3 class="text-sm font-semibold mb-2">
                                            {format!("{} ({})", t_string!(i18n, settings.memory.results), results.len())}
                                        </h3>
                                        <div class="space-y-2">
                                            {results.into_iter().map(|r| {
                                                view! {
                                                    <div class="p-3 bg-surface-sunken rounded border border-border">
                                                        <div class="flex justify-between items-center mb-1">
                                                            <span class="text-xs text-text-tertiary font-mono">{r.id}</span>
                                                            <span class="text-xs font-medium">{format!("score: {:.4}", r.score)}</span>
                                                        </div>
                                                        <p class="text-sm">{r.content}</p>
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    </div>
                                </div>
                            }
                        })}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
