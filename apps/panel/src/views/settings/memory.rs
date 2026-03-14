//! Memory Configuration View
//!
//! Provides UI for managing memory/RAG configuration:
//! - Basic settings (enabled, embedding model, vector DB)
//! - AI retrieval settings
//! - Compression settings
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{
    MemoryConfigApi, MemoryConfig, FusionStrategy, RerankProviderType,
    RetrieveWithTraceResponse, TestRerankResponse,
};
use crate::context::DashboardState;

#[component]
pub fn MemoryView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

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
                <h1 class="text-2xl font-bold mb-6">"Memory Configuration"</h1>

                {move || {
                    if loading.get() {
                        view! { <div class="text-text-tertiary">"Loading..."</div> }.into_any()
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
                                <RerankSettings config=config />
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
                                        {move || if saving.get() { "Saving..." } else { "Save Changes" }}
                                    </button>
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div class="text-text-tertiary">"No configuration loaded"</div> }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn BasicSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">"Basic Settings"</h2>

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
                    <label class="font-medium">"Enable Memory Module"</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Vector Database"</label>
                    <div class="w-full px-3 py-2 border border-border rounded bg-surface-sunken text-text-secondary">
                        "LanceDB"
                    </div>
                    <p class="text-xs text-text-tertiary mt-1">"LanceDB is the only supported vector database backend"</p>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Max Context Items"</label>
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
                        <label class="block text-sm font-medium mb-1">"Retention Days"</label>
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
                    <label class="block text-sm font-medium mb-1">"Similarity Threshold (0.0-1.0)"</label>
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
fn AIRetrievalSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">"AI-Based Retrieval"</h2>

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
                    <label class="font-medium">"Enable AI-Based Memory Retrieval"</label>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Timeout (ms)"</label>
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
                        <label class="block text-sm font-medium mb-1">"Max Candidates"</label>
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
                    <label class="block text-sm font-medium mb-1">"Fallback Count"</label>
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
fn CompressionSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-4">"Memory Compression"</h2>

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
                    <label class="font-medium">"Enable Memory Compression"</label>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Idle Timeout (seconds)"</label>
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
                        <label class="block text-sm font-medium mb-1">"Turn Threshold"</label>
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
                        <label class="block text-sm font-medium mb-1">"Compression Interval (seconds)"</label>
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
                        <label class="block text-sm font-medium mb-1">"Batch Size"</label>
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
                        <label class="block text-sm font-medium mb-1">"Conflict Similarity Threshold"</label>
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
                        <label class="block text-sm font-medium mb-1">"Max Facts in Context"</label>
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
                    <label class="block text-sm font-medium mb-1">"Raw Memory Fallback Count"</label>
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
fn FactDecaySettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"Fact Decay Policy"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Control how memory facts age and get pruned over time"
            </p>

            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Half-Life (days)"</label>
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
                        <p class="text-xs text-text-tertiary mt-1">"Days until fact strength halves without access"</p>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">"Access Boost"</label>
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
                        <p class="text-xs text-text-tertiary mt-1">"Strength boost when a fact is accessed"</p>
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Min Strength Before Pruning (0.0-1.0)"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Facts below this strength will be pruned"</p>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Protected Fact Types"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Comma-separated types that never decay (e.g. personal)"</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn GraphDecaySettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"Knowledge Graph Decay"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Control how graph nodes and edges decay over time"
            </p>

            <div class="space-y-4">
                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Node Decay Per Day"</label>
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
                        <label class="block text-sm font-medium mb-1">"Edge Decay Per Day"</label>
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
                    <label class="block text-sm font-medium mb-1">"Min Score Before Pruning"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Nodes/edges below this score will be pruned"</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn DreamingSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"DreamDaemon"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Background process that consolidates and compresses memory facts"
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
                    <label class="font-medium">"Enable DreamDaemon"</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Idle Threshold (seconds)"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Seconds of inactivity before dreaming starts"</p>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Window Start (HH:MM)"</label>
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
                        <label class="block text-sm font-medium mb-1">"Window End (HH:MM)"</label>
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
                <p class="text-xs text-text-tertiary">"Local time window when dreaming is allowed to run"</p>

                <div>
                    <label class="block text-sm font-medium mb-1">"Max Duration (seconds)"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Maximum time per dreaming session"</p>
                </div>
            </div>
        </div>
    }
}

#[component]
fn StorageBackupSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"Storage & Backup"</h2>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium mb-1">"Dedup Similarity Threshold (0.0-1.0)"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Memories above this similarity are considered duplicates"</p>
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
                    <label class="font-medium">"Enable Automatic JSONL Backup"</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Max Backup Files"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Maximum number of backup files to retain"</p>
                </div>
            </div>
        </div>
    }
}

// ============================================================================
// Section A: Retrieval Pipeline Settings
// ============================================================================

#[component]
fn RetrievalPipelineSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"Retrieval Pipeline"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure hybrid retrieval fusion and query expansion"
            </p>

            <div class="space-y-4">
                <div>
                    <label class="block text-sm font-medium mb-1">"Fusion Strategy"</label>
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
                        <option value="rrf">"Reciprocal Rank Fusion (RRF)"</option>
                        <option value="weighted">"Weighted Linear Combination"</option>
                    </select>
                </div>

                {move || {
                    let is_rrf = config.get().map(|c| c.fusion_strategy == FusionStrategy::Rrf).unwrap_or(true);
                    if is_rrf {
                        Some(view! {
                            <div>
                                <label class="block text-sm font-medium mb-1">"RRF Constant k"</label>
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
                                <p class="text-xs text-text-tertiary mt-1">"Higher k reduces the impact of rank differences (default: 60)"</p>
                            </div>
                        })
                    } else {
                        None
                    }
                }}

                <div>
                    <label class="block text-sm font-medium mb-1">"BM25 Bonus Weight"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Extra weight for BM25 full-text matches in fusion"</p>
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
                    <label class="font-medium">"Enable Query Expansion"</label>
                </div>
                <p class="text-xs text-text-tertiary">"Automatically expand queries with Chinese synonyms for broader recall"</p>
            </div>
        </div>
    }
}

// ============================================================================
// Section B: Rerank Provider Settings
// ============================================================================

#[component]
fn RerankSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    let test_status = RwSignal::new(Option::<String>::None);
    let test_loading = RwSignal::new(false);

    let test_connection = move |_| {
        if let Some(cfg) = config.get() {
            let state = expect_context::<DashboardState>();
            let rerank = cfg.rerank.clone();
            spawn_local(async move {
                test_loading.set(true);
                test_status.set(None);
                let params = serde_json::to_value(&rerank).unwrap_or_default();
                match state.rpc_call("memory.test_rerank_connection", params).await {
                    Ok(result) => {
                        if let Ok(resp) = serde_json::from_value::<TestRerankResponse>(result) {
                            if resp.success {
                                test_status.set(Some(format!(
                                    "Success! {} results, top score: {:.3}",
                                    resp.results_count, resp.top_score
                                )));
                            } else {
                                test_status.set(Some(format!(
                                    "Failed: {}",
                                    resp.error.unwrap_or_else(|| "Unknown error".to_string())
                                )));
                            }
                        } else {
                            test_status.set(Some("Failed to parse response".to_string()));
                        }
                    }
                    Err(e) => {
                        test_status.set(Some(format!("RPC error: {}", e)));
                    }
                }
                test_loading.set(false);
            });
        }
    };

    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"Cross-Encoder Reranking"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Use a cross-encoder model to rerank retrieval results for better precision"
            </p>

            <div class="space-y-4">
                <div class="flex items-center">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.rerank.enabled).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.rerank.enabled = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="mr-2"
                    />
                    <label class="font-medium">"Enable Cross-Encoder Rerank"</label>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Provider"</label>
                    <select
                        prop:value=move || config.get().map(|c| c.rerank.provider.as_str().to_string()).unwrap_or_else(|| "jina".to_string())
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.rerank.provider = RerankProviderType::from_str_val(&event_target_value(&ev));
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    >
                        <option value="jina">"Jina AI"</option>
                        <option value="siliconflow">"SiliconFlow"</option>
                        <option value="voyage">"Voyage AI"</option>
                        <option value="pinecone">"Pinecone"</option>
                        <option value="vllm">"vLLM"</option>
                    </select>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"API Base URL"</label>
                    <input
                        type="text"
                        prop:value=move || config.get().map(|c| c.rerank.api_base.clone()).unwrap_or_default()
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.rerank.api_base = event_target_value(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        placeholder="Leave empty for provider default"
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"API Key"</label>
                    <input
                        type="password"
                        prop:value=move || config.get().map(|c| c.rerank.api_key.clone()).unwrap_or_default()
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.rerank.api_key = event_target_value(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Model"</label>
                    <input
                        type="text"
                        prop:value=move || config.get().map(|c| c.rerank.model.clone()).unwrap_or_else(|| "BAAI/bge-reranker-v2-m3".to_string())
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.rerank.model = event_target_value(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Timeout (ms)"</label>
                        <input
                            type="number"
                            min="100"
                            prop:value=move || config.get().map(|c| c.rerank.timeout_ms).unwrap_or(5000)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.rerank.timeout_ms = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">"Rerank Weight (0.0-1.0)"</label>
                        <input
                            type="number"
                            step="0.05"
                            min="0"
                            max="1"
                            prop:value=move || config.get().map(|c| c.rerank.rerank_weight).unwrap_or(0.6)
                            on:input=move |ev| {
                                if let Some(mut cfg) = config.get() {
                                    if let Ok(val) = event_target_value(&ev).parse() {
                                        cfg.rerank.rerank_weight = val;
                                        config.set(Some(cfg));
                                    }
                                }
                            }
                            class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                        />
                        <p class="text-xs text-text-tertiary mt-1">"Blend: rerank_weight * rerank + (1-w) * original"</p>
                    </div>
                </div>

                <div class="pt-2">
                    <button
                        on:click=test_connection
                        prop:disabled=move || test_loading.get()
                        class="px-4 py-2 bg-surface-sunken text-text-primary rounded hover:bg-surface-raised border border-border disabled:opacity-50"
                    >
                        {move || if test_loading.get() { "Testing..." } else { "Test Connection" }}
                    </button>
                    {move || test_status.get().map(|msg| {
                        let is_success = msg.starts_with("Success");
                        let class = if is_success {
                            "mt-2 p-2 text-sm bg-success-subtle text-success rounded"
                        } else {
                            "mt-2 p-2 text-sm bg-danger-subtle text-danger rounded"
                        };
                        view! {
                            <div class=class>{msg}</div>
                        }
                    })}
                </div>
            </div>
        </div>
    }
}

// ============================================================================
// Section D: Reflection Settings (extends Dreaming section)
// ============================================================================

#[component]
fn ReflectionSettings(
    config: RwSignal<Option<MemoryConfig>>,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised p-6 rounded-lg border border-border">
            <h2 class="text-lg font-semibold mb-2">"Session-End Reflection"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Automatically extract insights and track open loops at session end"
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
                    <label class="font-medium">"Enable Session-End Reflection"</label>
                </div>

                <div class="grid grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium mb-1">"Min Turns"</label>
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
                        <p class="text-xs text-text-tertiary mt-1">"Minimum conversation turns before triggering"</p>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">"Min User Chars"</label>
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
                        <p class="text-xs text-text-tertiary mt-1">"Minimum total user characters"</p>
                    </div>
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Cooldown (minutes)"</label>
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
                    <p class="text-xs text-text-tertiary mt-1">"Minimum minutes between reflections"</p>
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
                    <label class="font-medium">"Enable Open Loop Tracking"</label>
                </div>
                <p class="text-xs text-text-tertiary">"Track unresolved questions and tasks across sessions"</p>

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
                    <label class="font-medium">"Inject to System Prompt"</label>
                </div>
                <p class="text-xs text-text-tertiary">"Inject open loop reminders into the next session's system prompt"</p>
            </div>
        </div>
    }
}

// ============================================================================
// Section E: Retrieval Debug Panel
// ============================================================================

#[component]
fn RetrievalDebugPanel() -> impl IntoView {
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
                Ok(result) => {
                    match serde_json::from_value::<RetrieveWithTraceResponse>(result) {
                        Ok(resp) => {
                            trace_result.set(Some(resp));
                        }
                        Err(e) => {
                            trace_error.set(Some(format!("Parse error: {}", e)));
                        }
                    }
                }
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
                    {move || if expanded.get() { "- Retrieval Debug Panel" } else { "+ Retrieval Debug Panel" }}
                </span>
                <span class="ml-2 text-sm text-text-tertiary">"(click to expand)"</span>
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
                                placeholder="Enter a test query..."
                                class="flex-1 px-3 py-2 border border-border rounded bg-surface-raised"
                            />
                            <button
                                on:click=do_search
                                prop:disabled=move || searching.get()
                                class="px-4 py-2 bg-info text-white rounded hover:bg-primary-hover disabled:opacity-50"
                            >
                                {move || if searching.get() { "Searching..." } else { "Search" }}
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
                                        <h3 class="text-sm font-semibold mb-2">"Pipeline Stages"</h3>
                                        <div class="overflow-x-auto">
                                            <table class="w-full text-sm border border-border">
                                                <thead>
                                                    <tr class="bg-surface-sunken">
                                                        <th class="px-3 py-2 text-left border-b border-border">"Stage"</th>
                                                        <th class="px-3 py-2 text-right border-b border-border">"Duration (ms)"</th>
                                                        <th class="px-3 py-2 text-right border-b border-border">"Input"</th>
                                                        <th class="px-3 py-2 text-right border-b border-border">"Output"</th>
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
                                            {format!("Results ({})", results.len())}
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
