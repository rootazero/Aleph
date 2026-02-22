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

    let config = create_rw_signal(Option::<MemoryConfig>::None);
    let loading = create_rw_signal(true);
    let saving = create_rw_signal(false);
    let error = create_rw_signal(Option::<String>::None);

    // Load config on mount
    create_effect(move |_| {
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
                    } else if let Some(cfg) = config.get() {
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
                    <label class="block text-sm font-medium mb-1">"Embedding Model"</label>
                    <input
                        type="text"
                        prop:value=move || config.get().map(|c| c.embedding_model).unwrap_or_default()
                        on:input=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.embedding_model = event_target_value(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium mb-1">"Vector Database"</label>
                    <select
                        prop:value=move || config.get().map(|c| c.vector_db).unwrap_or_default()
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.vector_db = event_target_value(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-full px-3 py-2 border border-border rounded bg-surface-raised"
                    >
                        <option value="sqlite-vec">"sqlite-vec"</option>
                        <option value="lancedb">"lancedb"</option>
                    </select>
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
