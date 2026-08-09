//! Memory Configuration View
//!
//! Provides UI for managing memory/RAG configuration:
//! - Basic settings (enabled, embedding model, vector DB)
//! - AI retrieval settings
//! - Compression settings
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{MemoryConfig, MemoryConfigApi, RetrieveWithTraceResponse};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
#[must_use]
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
                        error.set(Some(crate::components::admin_refusal::settings_load_error(
                            i18n,
                            &e,
                            |e| format!("Failed to load memory config: {e}"),
                        )));
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
                        error.set(Some(
                            crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Failed to save: {e}")
                            }),
                        ));
                    }
                }
                saving.set(false);
            });
        }
    };

    view! {
        <div class="flex-1 px-6 pb-6 overflow-y-auto aleph-content-top">
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
                                <CompressionSettings config=config />
                                <RetrievalPipelineSettings config=config />
                                <SalienceScoringSettings config=config />
                                <FactDecaySettings config=config />
                                <DreamingSettings config=config />
                                <ReflectionSettings config=config />
                                <CuratedEnvelopeSettings config=config />
                                <StorageBackupSettings config=config />
                                <RetrievalDebugPanel />
                                <DreamInsightsPanel />
                                <CorrectionsPanel />

                                <div class="pt-4 border-t border-border">
                                    <button
                                        on:click=save
                                        prop:disabled=move || saving.get()
                                        class="px-6 py-2 bg-info text-white rounded hover:bg-primary-hover disabled:opacity-50"
                                    >
                                        {move || if saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                                    </button>
                                    // Every knob on this page writes `[memory]`, and the
                                    // server classifies that whole section as
                                    // `ReloadImpact::Restart` (`config/reload_impact.rs` —
                                    // `LIVE_SECTIONS` is route/behavior/execution only): the
                                    // subsystems that consume it captured their configuration
                                    // at boot and nothing rebuilds them from a live edit.
                                    // Saying so is not a second source of truth, it is the
                                    // only place the operator can learn that a saved change
                                    // has not happened yet.
                                    <p class="text-xs text-text-tertiary mt-3">
                                        {t!(i18n, settings.memory.restart_required_hint)}
                                    </p>
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
                        {move || config.get().map(|c| c.vector_db).unwrap_or_else(|| "sqlite-vec".to_string())}
                    </div>
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.vector_db_hint)}</p>
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
fn CompressionSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">{t!(i18n, settings.memory.compression)}</h2>
            <p class="text-xs text-text-tertiary mb-4">{t!(i18n, settings.memory.compression_hint)}</p>

            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.turn_threshold)}</label>
                        <input
                            type="number"
                            min="1"
                            prop:value=move || config.get().map(|c| c.compression.turn_threshold).unwrap_or(20)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.compression.turn_threshold = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.compression_interval_seconds)}</label>
                    <input
                        type="number"
                        min="1"
                        prop:value=move || config.get().map(|c| c.compression.background_interval_seconds).unwrap_or(3600)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.compression.background_interval_seconds = val;
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
            </div>
        </div>
    }
}

// ============================================================================
// Section B: Retrieval Salience Scoring (recency / reinforcement / MMR)
// ============================================================================

#[component]
fn SalienceScoringSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.salience)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.memory.salience_desc)}
            </p>

            <div class="space-y-4">
                // Recency decay
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.retrieval_scoring.recency_enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.retrieval_scoring.recency_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.recency_enabled)}</label>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.recency_half_life_days)}</label>
                        <input
                            type="number"
                            step="1"
                            min="1"
                            prop:value=move || config.get().map(|c| c.retrieval_scoring.recency_half_life_days).unwrap_or(90.0)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.retrieval_scoring.recency_half_life_days = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.recency_weight)}</label>
                        <input
                            type="number"
                            step="0.05"
                            min="0"
                            max="1"
                            prop:value=move || config.get().map(|c| c.retrieval_scoring.recency_weight).unwrap_or(0.3)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.retrieval_scoring.recency_weight = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>
                </div>

                // Reinforcement salience
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.retrieval_scoring.reinforcement_enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.retrieval_scoring.reinforcement_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.reinforcement_enabled)}</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.reinforcement_weight)}</label>
                    <input
                        type="number"
                        step="0.05"
                        min="0"
                        max="1"
                        prop:value=move || config.get().map(|c| c.retrieval_scoring.reinforcement_weight).unwrap_or(0.3)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.retrieval_scoring.reinforcement_weight = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>

                // MMR diversity
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.retrieval_scoring.mmr_enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.retrieval_scoring.mmr_enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">{t!(i18n, settings.memory.mmr_enabled)}</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">{t!(i18n, settings.memory.mmr_lambda)}</label>
                    <input
                        type="number"
                        step="0.05"
                        min="0"
                        max="1"
                        prop:value=move || config.get().map(|c| c.retrieval_scoring.mmr_lambda).unwrap_or(0.7)
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.retrieval_scoring.mmr_lambda = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">{t!(i18n, settings.memory.mmr_lambda_hint)}</p>
                </div>
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

                // The two bounds on what the toggles above produce. They live
                // in `[memory.curated]`, not `reflection`, but an operator who
                // switches injection on and cannot see the age ceiling has
                // been shown half the mechanism — indented here rather than
                // filed under their own section for that reason.
                <div class="pl-6 border-l-2 border-border space-y-3">
                    <div>
                        <label class="block text-sm font-medium mb-1">
                            {t!(i18n, settings.memory.open_loop_max_age_days)}
                        </label>
                        <input
                            type="number"
                            min="0"
                            prop:value=move || {
                                config.get().map(|c| c.curated.open_loops_max_age_days).unwrap_or(14)
                            }
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.curated.open_loops_max_age_days = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">
                            {t!(i18n, settings.memory.open_loop_max_age_hint)}
                        </p>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">
                            {t!(i18n, settings.memory.open_loop_char_limit)}
                        </label>
                        <input
                            type="number"
                            min="0"
                            prop:value=move || {
                                config.get().map(|c| c.curated.open_loops_char_limit).unwrap_or(2000)
                            }
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.curated.open_loops_char_limit = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">
                            {t!(i18n, settings.memory.open_loop_char_limit_hint)}
                        </p>
                    </div>
                </div>
            </div>
        </div>
    }
}

// ============================================================================
// Section D2: Curated Envelope Budgets (`[memory.curated]`)
// ============================================================================

/// The three `[memory.curated]` budgets that are **not** about open loops.
///
/// The section's other two keys (`open_loops_char_limit` /
/// `open_loops_max_age_days`) render inside [`ReflectionSettings`] instead,
/// indented under the toggles that decide whether the block they bound is
/// produced at all. Splitting one config section across two places is
/// deliberate: the operator's question is "what does the agent always see",
/// not "which TOML table is this in".
#[component]
fn CuratedEnvelopeSettings(config: RwSignal<Option<MemoryConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, settings.memory.curated_envelope)}</h2>
            <p class="text-sm text-text-tertiary mb-4">
                {t!(i18n, settings.memory.curated_envelope_desc)}
            </p>

            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">
                            {t!(i18n, settings.memory.memory_char_limit)}
                        </label>
                        <input
                            type="number"
                            min="0"
                            prop:value=move || {
                                config.get().map(|c| c.curated.memory_char_limit).unwrap_or(2200)
                            }
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.curated.memory_char_limit = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">
                            {t!(i18n, settings.memory.memory_char_limit_hint)}
                        </p>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">
                            {t!(i18n, settings.memory.user_char_limit)}
                        </label>
                        <input
                            type="number"
                            min="0"
                            prop:value=move || {
                                config.get().map(|c| c.curated.user_char_limit).unwrap_or(1375)
                            }
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.curated.user_char_limit = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">
                            {t!(i18n, settings.memory.user_char_limit_hint)}
                        </p>
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">
                        {t!(i18n, settings.memory.legacy_warn_threshold)}
                    </label>
                    <input
                        type="number"
                        step="0.01"
                        min="0"
                        max="1"
                        prop:value=move || {
                            config.get().map(|c| c.curated.legacy_warn_threshold).unwrap_or(0.95)
                        }
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                if let Ok(val) = event_target_value(&ev).parse() {
                                    cfg.curated.legacy_warn_threshold = val;
                                    config.set(Some(cfg));
                                }
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                    <p class="text-xs text-text-tertiary mt-1">
                        {t!(i18n, settings.memory.legacy_warn_threshold_hint)}
                    </p>
                </div>
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
                        // `e` is a serde parse failure, not a gateway verdict —
                        // wrapped anyway because the rule has no allowlist and
                        // a non-refusal passes through byte-for-byte.
                        trace_error.set(Some(
                            crate::components::admin_refusal::settings_write_error(
                                i18n,
                                &e.to_string(),
                                |e| format!("Parse error: {e}"),
                            ),
                        ));
                    }
                },
                Err(e) => {
                    trace_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("RPC error: {e}")
                        }),
                    ));
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
                            let results = resp.results;

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

// ============================================================================
// Section F: Dream Insights Panel
// ============================================================================

#[component]
fn DreamInsightsPanel() -> impl IntoView {
    use crate::api::memory_config::{DreamInsightsApi, DreamInsightsResponse};
    use crate::platform::wide::views::memory::data::format_ts;
    let i18n = use_i18n();
    let expanded = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let data = RwSignal::new(Option::<DreamInsightsResponse>::None);
    let error = RwSignal::new(Option::<String>::None);
    // Which corpus the run list is scoped to. `None` = whatever the server
    // considers the base agent; the corpus table below sets it to a
    // `{base}__proj-*` namespace to read that project's nightly history.
    let selected_ns = RwSignal::new(Option::<String>::None);

    let load = move || {
        let state = expect_context::<DashboardState>();
        let ns = selected_ns.get_untracked();
        spawn_local(async move {
            loading.set(true);
            error.set(None);
            match DreamInsightsApi::list(&state, ns, Some(30)).await {
                Ok(resp) => data.set(Some(resp)),
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| e.to_string(),
                ))),
            }
            loading.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <button
                on:click=move |_| {
                    let next = !expanded.get();
                    expanded.set(next);
                    if next && data.get().is_none() {
                        load();
                    }
                }
                class="flex items-center w-full text-left"
            >
                <span class="text-lg font-semibold">
                    {move || {
                        let prefix = if expanded.get() { "- " } else { "+ " };
                        format!("{}{}", prefix, t_string!(i18n, settings.memory.dream_insights))
                    }}
                </span>
            </button>

            {move || {
                if !expanded.get() {
                    return view! { <div></div> }.into_any();
                }
                view! {
                    <div class="mt-4 space-y-4">
                        {move || if loading.get() {
                            view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                        } else { view! { <div></div> }.into_any() }}

                        {move || error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                        })}

                        {move || data.get().map(|resp| {
                            let runs = resp.runs.clone();
                            let daily = resp.daily.clone();
                            let synthesis = resp.synthesis.clone();
                            let corpora = resp.namespaces.clone();
                            let active_ns = resp.agent_id.clone();
                            let is_empty = runs.is_empty() && daily.is_empty() && synthesis.is_empty();
                            let latest_run_summary = runs.first().map(|latest| {
                                let label = t_string!(i18n, settings.memory.dream_latest_run).to_string();
                                format!("{}: {} · {}ms · {} synth",
                                    label, latest.pipeline_type, latest.duration_ms, latest.synthesis_count)
                            });
                            // Only interesting once a project corpus exists: with
                            // project scoping off there is exactly one corpus and
                            // this table would be a row that says "you are here".
                            let show_corpora = corpora.len() > 1;
                            view! {
                                {if is_empty {
                                    view! { <div class="text-text-tertiary text-sm">{t!(i18n, settings.memory.dream_no_insights)}</div> }.into_any()
                                } else { view! { <div></div> }.into_any() }}

                                // Corpora. Project namespaces run their own nightly
                                // cycle under their own churn gate; until this table
                                // existed their history was reachable only by the
                                // model, which reads their event log directly.
                                {show_corpora.then(|| {
                                    let rows = corpora.into_iter().map(|c| {
                                        let ns = c.namespace.clone();
                                        let is_active = ns == active_ns;
                                        let conserved = c.last_decision.as_ref()
                                            .and_then(|d| d.gate.as_ref())
                                            .is_some_and(|g| g.kind == "conserve");
                                        let strategy = c.last_decision.as_ref()
                                            .map_or(c.last_pipeline_type.clone(), |d| d.strategy.clone());
                                        // The cycle count carries the locale's plural form, so it
                                        // is a `t!` fragment rather than part of the format
                                        // string — `t_string!` rejects interpolated keys unless
                                        // the `interpolate_display` feature is on.
                                        let runs = c.runs;
                                        let summary_tail = format!(
                                            " · {} · {}{}",
                                            format_ts(c.last_started_at),
                                            strategy,
                                            if conserved { " ⚠" } else { "" },
                                        );
                                        let target = ns.clone();
                                        view! {
                                            <button
                                                on:click=move |_| {
                                                    if !is_active {
                                                        selected_ns.set(Some(target.clone()));
                                                        load();
                                                    }
                                                }
                                                class=move || if is_active {
                                                    "flex justify-between w-full text-left p-2 rounded border border-accent bg-surface-sunken text-sm"
                                                } else {
                                                    "flex justify-between w-full text-left p-2 rounded border border-border text-sm hover:bg-surface-sunken"
                                                }
                                            >
                                                <span class="font-mono">{ns}</span>
                                                <span class=move || if conserved { "text-xs text-warning" } else { "text-xs text-text-tertiary" }>
                                                    {t!(i18n, settings.memory.dream_corpus_cycles, count = move || runs)}
                                                    {summary_tail}
                                                </span>
                                            </button>
                                        }
                                    }).collect::<Vec<_>>();
                                    view! {
                                        <div>
                                            <h3 class="text-sm font-semibold mb-2">{t!(i18n, settings.memory.dream_corpora)}</h3>
                                            <div class="space-y-1">{rows}</div>
                                        </div>
                                    }
                                })}

                                // Recent runs
                                <div>
                                    // The corpus chip appears only when there is
                                    // more than one corpus to confuse it with —
                                    // with project scoping off the default view
                                    // is unchanged.
                                    <h3 class="text-sm font-semibold mb-2">
                                        {t!(i18n, settings.memory.dream_runs)}
                                        {show_corpora.then(|| view! {
                                            <span class="ml-2 font-mono font-normal text-xs text-text-tertiary">{active_ns.clone()}</span>
                                        })}
                                    </h3>
                                    {latest_run_summary.map(|summary| view! {
                                        <div class="mb-2 px-3 py-1.5 bg-info-subtle rounded text-sm font-medium text-info">
                                            {summary}
                                        </div>
                                    })}
                                    <div class="space-y-1">
                                        {runs.into_iter().map(|r| {
                                            let err = r.errors.clone();
                                            // The evolution-gate verdict and the cycle decision were
                                            // both already on the wire; nothing rendered them, so the
                                            // Panel could show *that* a cycle conserved but never why.
                                            let health = r.evolution.as_ref().map(|e| {
                                                let verdict = match e.outcome.as_str() {
                                                    "accept_new_best" => t_string!(i18n, settings.memory.dream_verdict_accepted_best),
                                                    "accept" => t_string!(i18n, settings.memory.dream_verdict_accepted),
                                                    _ => t_string!(i18n, settings.memory.dream_verdict_rejected),
                                                };
                                                let accepted = e.outcome.starts_with("accept");
                                                let text = format!(
                                                    "{} {:.3} → {:.3} ({} {:.3}) — {}",
                                                    t_string!(i18n, settings.memory.dream_health),
                                                    e.baseline, e.candidate,
                                                    t_string!(i18n, settings.memory.dream_best), e.best,
                                                    verdict,
                                                );
                                                (text, accepted, e.merges_rejected)
                                            });
                                            let decision = r.decision.clone();
                                            view! {
                                                <div class="p-2 bg-surface-sunken rounded border border-border text-sm space-y-1">
                                                    <div class="flex justify-between">
                                                        <span>{r.pipeline_type}</span>
                                                        <span class="text-text-tertiary">
                                                            {format!("{}ms · {} synth · {} merged · {} archived",
                                                                r.duration_ms, r.synthesis_count,
                                                                r.notes_consolidated, r.notes_archived)}
                                                        </span>
                                                    </div>
                                                    {health.map(|(text, accepted, merges_rejected)| view! {
                                                        <div class=move || if accepted { "text-xs text-success" } else { "text-xs text-warning" }>
                                                            {text}
                                                            {(merges_rejected > 0).then(|| format!(
                                                                " · {} {}",
                                                                merges_rejected,
                                                                t_string!(i18n, settings.memory.dream_merges_rejected),
                                                            ))}
                                                        </div>
                                                    })}
                                                    {decision.map(|d| {
                                                        let conserved = d.gate.as_ref().is_some_and(|g| g.kind == "conserve");
                                                        let gate_line = d.gate.as_ref().and_then(|g| {
                                                            (g.kind == "conserve").then(|| format!(
                                                                "⚠ {}: {}{}",
                                                                t_string!(i18n, settings.memory.dream_gate_conserve),
                                                                g.reason.clone().unwrap_or_default(),
                                                                if g.cooldown_remaining > 0 {
                                                                    format!(" ({} {})", t_string!(i18n, settings.memory.dream_cooldown), g.cooldown_remaining)
                                                                } else {
                                                                    String::new()
                                                                },
                                                            ))
                                                        });
                                                        let stages = (!d.stages.is_empty()).then(|| format!(
                                                            "{}: {}",
                                                            t_string!(i18n, settings.memory.dream_stages),
                                                            d.stages.join(" → "),
                                                        ));
                                                        let validation_failed = !d.validation_passed;
                                                        view! {
                                                            <div class="text-xs text-text-tertiary">{d.rationale}</div>
                                                            {gate_line.map(|g| view! {
                                                                <div class=move || if conserved { "text-xs text-warning" } else { "text-xs text-text-tertiary" }>{g}</div>
                                                            })}
                                                            {stages.map(|s| view! {
                                                                <div class="text-xs font-mono text-text-tertiary">{s}</div>
                                                            })}
                                                            {validation_failed.then(|| view! {
                                                                <div class="text-xs text-danger">{t!(i18n, settings.memory.dream_validation_failed)}</div>
                                                            })}
                                                        }
                                                    })}
                                                    {err.map(|e| view! { <div class="text-xs text-danger">{e}</div> })}
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>

                                // Daily digests
                                <div>
                                    <h3 class="text-sm font-semibold mb-2">{t!(i18n, settings.memory.dream_daily)}</h3>
                                    <div class="space-y-2">
                                        {daily.into_iter().map(|d| {
                                            view! {
                                                <div class="p-3 bg-surface-sunken rounded border border-border">
                                                    <div class="flex justify-between mb-1">
                                                        <span class="text-xs font-mono text-text-tertiary">{d.date}</span>
                                                        <span class="text-xs">{format!("{} {}", d.source_memory_count, t_string!(i18n, settings.memory.dream_source_count))}</span>
                                                    </div>
                                                    <p class="text-sm">{d.content}</p>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>

                                // Synthesis notes
                                <div>
                                    <h3 class="text-sm font-semibold mb-2">{t!(i18n, settings.memory.dream_synthesis)}</h3>
                                    <div class="space-y-2">
                                        {synthesis.into_iter().map(|s| {
                                            view! {
                                                <div class="p-3 bg-surface-sunken rounded border border-border">
                                                    <div class="flex justify-between">
                                                        <span class="text-sm font-medium">{s.title}</span>
                                                        <span class="text-xs font-mono text-text-tertiary">{s.path}</span>
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
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

// ============================================================================
// Section G: Corrections Panel
// ============================================================================

#[component]
fn CorrectionsPanel() -> impl IntoView {
    use crate::api::memory_config::{CorrectionsApi, CorrectionsResponse};
    let i18n = use_i18n();
    let expanded = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let show_distilled = RwSignal::new(true);
    let data = RwSignal::new(Option::<CorrectionsResponse>::None);
    let error = RwSignal::new(Option::<String>::None);

    let load = move || {
        let state = expect_context::<DashboardState>();
        let include = show_distilled.get();
        spawn_local(async move {
            loading.set(true);
            error.set(None);
            match CorrectionsApi::list(&state, None, include).await {
                Ok(resp) => data.set(Some(resp)),
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| e.to_string(),
                ))),
            }
            loading.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <button
                on:click=move |_| {
                    let next = !expanded.get();
                    expanded.set(next);
                    if next && data.get().is_none() {
                        load();
                    }
                }
                class="flex items-center w-full text-left"
            >
                <span class="text-lg font-semibold">
                    {move || {
                        let prefix = if expanded.get() { "- " } else { "+ " };
                        format!("{}{}", prefix, t_string!(i18n, settings.memory.corrections))
                    }}
                </span>
            </button>

            {move || {
                if !expanded.get() {
                    return view! { <div></div> }.into_any();
                }
                view! {
                    <div class="mt-4 space-y-3">
                        <label class="flex items-center gap-2 text-sm">
                            <input
                                type="checkbox"
                                prop:checked=move || show_distilled.get()
                                on:change=move |ev| {
                                    show_distilled.set(event_target_checked(&ev));
                                    load();
                                }
                            />
                            <span>{t!(i18n, settings.memory.corrections_show_distilled)}</span>
                        </label>

                        {move || if loading.get() {
                            view! { <div class="text-text-tertiary">{t!(i18n, common.loading)}</div> }.into_any()
                        } else { view! { <div></div> }.into_any() }}

                        {move || error.get().map(|e| view! {
                            <div class="p-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                        })}

                        {move || data.get().map(|resp| {
                            let items = resp.corrections.clone();
                            if items.is_empty() {
                                return view! { <div class="text-text-tertiary text-sm">{t!(i18n, settings.memory.corrections_none)}</div> }.into_any();
                            }
                            let pending_count = items.iter().filter(|c| c.status == "pending").count();
                            let distilled_count = items.len() - pending_count;
                            let summary = format!(
                                "{} {} · {} {}",
                                pending_count,
                                t_string!(i18n, settings.memory.corrections_pending),
                                distilled_count,
                                t_string!(i18n, settings.memory.corrections_distilled),
                            );
                            view! {
                                <div class="space-y-2">
                                    <p class="text-xs text-text-secondary font-medium">{summary}</p>
                                    {items.into_iter().map(|c| {
                                        let is_pending = c.status == "pending";
                                        let badge = if is_pending {
                                            t_string!(i18n, settings.memory.corrections_pending).to_string()
                                        } else {
                                            t_string!(i18n, settings.memory.corrections_distilled).to_string()
                                        };
                                        let badge_class = if is_pending {
                                            "text-xs px-2 py-0.5 rounded bg-warning-subtle text-warning"
                                        } else {
                                            "text-xs px-2 py-0.5 rounded bg-success-subtle text-success"
                                        };
                                        let rule = c.suggested_rule.clone();
                                        view! {
                                            <div class="p-3 bg-surface-sunken rounded border border-border">
                                                <div class="flex justify-between items-center mb-1">
                                                    <span class=badge_class>{badge}</span>
                                                    <span class="text-xs text-text-tertiary">{c.severity}</span>
                                                </div>
                                                <p class="text-sm">{c.content}</p>
                                                {rule.map(|r| view! {
                                                    <p class="text-xs text-text-tertiary mt-1">
                                                        {format!("{}: {}", t_string!(i18n, settings.memory.corrections_suggested_rule), r)}
                                                    </p>
                                                })}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        })}
                    </div>
                }.into_any()
            }}
        </div>
    }
}
