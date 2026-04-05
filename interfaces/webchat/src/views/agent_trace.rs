//! Agent Trace view — Live / Replay dual-mode trace timeline.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::trace::TraceApi;
use crate::context::{DashboardState, GatewayEvent};
use crate::models::{TraceNode, TraceStatus};
use crate::views::agent_trace_model::{trace_node_from_event, trace_nodes_from_replay, TraceLabels};

use aleph_protocol::AgentTraceEvent;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Trace timeline page — accessible at `/dashboard/trace`.
///
/// Supports two modes:
/// - **Live**: subscribes to gateway `run.agent_trace` events in real time.
/// - **Replay**: loads a persisted trace by task ID.
#[component]
pub fn AgentTrace() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Signals
    let nodes = RwSignal::new(Vec::<TraceNode>::new());
    let mode = RwSignal::new(TraceMode::Live);
    let task_id_input = RwSignal::new(String::new());
    let error_msg = RwSignal::new(Option::<String>::None);
    let is_loading = RwSignal::new(false);

    // --- Live mode: subscribe to agent_trace events --------------------------
    let step_counter = StoredValue::new(std::sync::Arc::new(std::sync::Mutex::new(0u64)));

    // Subscribe to gateway events for live trace
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        let labels = TraceLabels::default();
        let counter = step_counter.get_value();

        state.subscribe_events(move |event: GatewayEvent| {
            if event.topic != "run.agent_trace" || mode.get() != TraceMode::Live {
                return;
            }
            // Try to parse the agent_trace event from data
            if let Ok(trace_event) = serde_json::from_value::<AgentTraceEvent>(
                event
                    .data
                    .get("event")
                    .cloned()
                    .unwrap_or(event.data.clone()),
            ) {
                let step = {
                    let mut c = counter.lock().unwrap_or_else(|e| e.into_inner());
                    let s = *c;
                    *c += 1;
                    s
                };
                if let Some(node) = trace_node_from_event(&trace_event, step, &labels) {
                    nodes.update(|list| list.push(node));
                }
            }
        });
    });

    // --- Replay loader -------------------------------------------------------
    let load_replay = move |_| {
        let tid = task_id_input.get_untracked();
        if tid.trim().is_empty() {
            return;
        }
        is_loading.set(true);
        error_msg.set(None);
        mode.set(TraceMode::Replay);

        spawn_local(async move {
            match TraceApi::get(&state, &tid).await {
                Ok(replay) => {
                    let labels = TraceLabels::default();
                    let entries: Vec<(u64, AgentTraceEvent)> = replay
                        .traces
                        .iter()
                        .map(|e| (e.step, e.event.clone()))
                        .collect();
                    nodes.set(trace_nodes_from_replay(&entries, &labels));
                }
                Err(e) => {
                    error_msg.set(Some(e));
                }
            }
            is_loading.set(false);
        });
    };

    let clear_live = move |_| {
        mode.set(TraceMode::Live);
        nodes.set(Vec::new());
        error_msg.set(None);
        let counter = step_counter.get_value();
        let mut c = counter.lock().unwrap_or_else(|e| e.into_inner());
        *c = 0;
    };

    // --- Render --------------------------------------------------------------
    view! {
        <div class="p-6 max-w-4xl mx-auto space-y-4">
            <h1 class="text-2xl font-bold text-text-primary">"Agent Trace"</h1>

            // Mode toggle + replay loader
            <div class="flex items-center gap-3 flex-wrap">
                <button
                    class="px-3 py-1.5 rounded text-sm font-medium transition-colors"
                    class:bg-primary=move || mode.get() == TraceMode::Live
                    class:text-white=move || mode.get() == TraceMode::Live
                    class:bg-surface-secondary=move || mode.get() != TraceMode::Live
                    on:click=clear_live
                >
                    "Live"
                </button>

                <input
                    type="text"
                    placeholder="Task ID for replay..."
                    class="px-3 py-1.5 rounded border border-border bg-surface text-sm text-text-primary w-64"
                    prop:value=move || task_id_input.get()
                    on:input=move |ev| {
                        task_id_input.set(event_target_value(&ev));
                    }
                />

                <button
                    class="px-3 py-1.5 rounded bg-primary text-white text-sm font-medium disabled:opacity-50"
                    prop:disabled=move || is_loading.get()
                    on:click=load_replay
                >
                    {move || if is_loading.get() { "Loading..." } else { "Replay" }}
                </button>
            </div>

            // Error display
            {move || error_msg.get().map(|msg| view! {
                <div class="p-3 rounded bg-red-500/10 text-red-400 text-sm">{msg}</div>
            })}

            // Timeline
            <div class="space-y-1">
                {move || {
                    let current_nodes = nodes.get();
                    if current_nodes.is_empty() {
                        view! {
                            <p class="text-text-secondary text-sm italic py-8 text-center">
                                {move || match mode.get() {
                                    TraceMode::Live => "Waiting for trace events...",
                                    TraceMode::Replay => "No trace data loaded.",
                                }}
                            </p>
                        }.into_any()
                    } else {
                        view! {
                            <div class="space-y-1">
                                {current_nodes.into_iter().map(|node| {
                                    view! { <TraceNodeRow node=node /> }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceMode {
    Live,
    Replay,
}

#[component]
fn TraceNodeRow(node: TraceNode) -> impl IntoView {
    let status_color = match node.status {
        TraceStatus::InProgress => "text-blue-400",
        TraceStatus::Success => "text-green-400",
        TraceStatus::Failed => "text-red-400",
        TraceStatus::Pending => "text-text-secondary",
    };

    let status_dot = match node.status {
        TraceStatus::InProgress => "bg-blue-400",
        TraceStatus::Success => "bg-green-400",
        TraceStatus::Failed => "bg-red-400",
        TraceStatus::Pending => "bg-text-secondary",
    };

    let duration_text = node
        .duration_ms
        .map(|ms| format!("{}ms", ms))
        .unwrap_or_default();

    view! {
        <div class="flex items-start gap-3 py-1.5 px-3 rounded hover:bg-surface-secondary/50 transition-colors text-sm">
            // Status dot
            <span class=format!("mt-1.5 w-2 h-2 rounded-full shrink-0 {}", status_dot)></span>
            // Content
            <div class="flex-1 min-w-0">
                <span class=format!("font-mono text-xs {}", status_color)>
                    {node.content}
                </span>
            </div>
            // Duration
            {if !duration_text.is_empty() {
                view! {
                    <span class="text-xs text-text-tertiary shrink-0">{duration_text}</span>
                }.into_any()
            } else {
                ().into_any()
            }}
        </div>
    }
}
