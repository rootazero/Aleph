//! Memory Configuration View
//!
//! Provides UI for managing memory/RAG configuration:
//! - Basic settings (enabled, embedding model, vector DB)
//! - AI retrieval settings
//! - Compression settings
//! - Real-time updates via config events

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{MemoryConfigApi, MemoryConfig};
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
                                <FactDecaySettings config=config />
                                <GraphDecaySettings config=config />
                                <DreamingSettings config=config />
                                <StorageBackupSettings config=config />

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
