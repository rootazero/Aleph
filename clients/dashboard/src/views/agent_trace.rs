//! Agent Trace view
//!
//! Real-time visualization of Agent's thinking process with streaming updates.

use leptos::*;
use leptos::html::Div;
use wasm_bindgen::JsCast;
use crate::models::{TraceNode, TraceStatus};
use crate::mock_data::{generate_mock_trace_nodes, generate_next_trace_node};

#[component]
pub fn AgentTrace() -> impl IntoView {
    // Trace nodes signal (data bus for future integration)
    let (trace_nodes, set_trace_nodes) = create_signal(generate_mock_trace_nodes());

    // Auto-scroll reference
    let scroll_container_ref = create_node_ref::<Div>();

    // Counter for generating new nodes
    let (node_counter, set_node_counter) = create_signal(3usize);

    // Streaming simulation: add new trace node every 800ms
    let _interval_handle = set_interval(
        move || {
            let current_count = node_counter.get();
            let new_node = generate_next_trace_node(current_count);

            set_trace_nodes.update(|nodes| {
                nodes.push(new_node);
            });

            set_node_counter.set(current_count + 1);

            // Auto-scroll to bottom
            if let Some(container) = scroll_container_ref.get() {
                let element = container.unchecked_ref::<web_sys::HtmlElement>();
                element.set_scroll_top(element.scroll_height());
            }
        },
        std::time::Duration::from_millis(800),
    );

    // Clear trace
    let on_clear = move |_| {
        set_trace_nodes.set(Vec::new());
        set_node_counter.set(0);
    };

    // Pause/Resume streaming
    let (is_paused, set_is_paused) = create_signal(false);

    view! {
        <div class="space-y-6">
            <div class="card">
                <div class="flex items-center justify-between mb-4">
                    <h2 class="card-header mb-0">"Agent Trace (Live)"</h2>
                    <div class="flex space-x-2">
                        <button
                            class="px-3 py-1 bg-blue-600 hover:bg-blue-700 rounded text-sm font-medium"
                            on:click=move |_| set_is_paused.update(|p| *p = !*p)
                        >
                            {move || if is_paused.get() { "▶️ Resume" } else { "⏸️ Pause" }}
                        </button>
                        <button
                            class="px-3 py-1 bg-red-600 hover:bg-red-700 rounded text-sm font-medium"
                            on:click=on_clear
                        >
                            "🗑️ Clear"
                        </button>
                    </div>
                </div>

                <div class="bg-blue-900/20 border border-blue-500 rounded p-3 text-sm text-blue-300 mb-4">
                    <strong>"Live Simulation: "</strong> "New trace nodes are being added every 800ms to simulate real-time Agent thinking process."
                </div>

                // Trace timeline container with auto-scroll
                <div
                    node_ref=scroll_container_ref
                    class="bg-gray-900 rounded-lg p-4 max-h-[600px] overflow-y-auto space-y-3"
                >
                    <For
                        each=move || trace_nodes.get()
                        key=|node| node.id.clone()
                        children=move |node: TraceNode| {
                            view! {
                                <TraceNodeComponent node=node />
                            }
                        }
                    />

                    {move || {
                        if trace_nodes.get().is_empty() {
                            view! {
                                <div class="text-center text-gray-500 py-8">
                                    "No trace data yet. Streaming will start automatically..."
                                </div>
                            }.into_view()
                        } else {
                            view! { <div></div> }.into_view()
                        }
                    }}
                </div>

                // Statistics
                <div class="mt-4 grid grid-cols-3 gap-4 text-sm">
                    <div class="bg-gray-700 rounded p-3">
                        <div class="text-gray-400">"Total Nodes"</div>
                        <div class="text-2xl font-bold text-white">
                            {move || trace_nodes.get().len()}
                        </div>
                    </div>
                    <div class="bg-gray-700 rounded p-3">
                        <div class="text-gray-400">"In Progress"</div>
                        <div class="text-2xl font-bold text-amber-400">
                            {move || trace_nodes.get().iter().filter(|n| n.status == TraceStatus::InProgress).count()}
                        </div>
                    </div>
                    <div class="bg-gray-700 rounded p-3">
                        <div class="text-gray-400">"Completed"</div>
                        <div class="text-2xl font-bold text-green-400">
                            {move || trace_nodes.get().iter().filter(|n| n.status == TraceStatus::Success).count()}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}

/// Individual trace node component
#[component]
fn TraceNodeComponent(node: TraceNode) -> impl IntoView {
    let type_class = node.type_class();
    let status_class = node.status_class();
    let icon = node.type_icon();

    // Format timestamp
    let timestamp = format_timestamp(node.timestamp);

    // Format duration
    let duration_text = node.duration_ms
        .map(|d| format!("{}ms", d))
        .unwrap_or_else(|| "...".to_string());

    view! {
        <div class=format!("border-l-4 {} {} rounded-r-lg p-4 transition-all duration-300", type_class, status_class)>
            <div class="flex items-start justify-between">
                <div class="flex items-start space-x-3 flex-1">
                    <span class="text-2xl">{icon}</span>
                    <div class="flex-1">
                        <div class="flex items-center space-x-2 mb-1">
                            <span class="text-xs font-mono text-gray-400">{timestamp}</span>
                            <span class="text-xs px-2 py-0.5 bg-gray-700 rounded">{format!("{:?}", node.node_type)}</span>
                            {move || {
                                if node.status == TraceStatus::InProgress {
                                    view! {
                                        <span class="text-xs text-amber-400 animate-pulse">"● In Progress"</span>
                                    }.into_view()
                                } else {
                                    view! { <span></span> }.into_view()
                                }
                            }}
                        </div>
                        <div class="text-white">{node.content.clone()}</div>
                    </div>
                </div>
                <div class="text-xs text-gray-400 ml-4">
                    {duration_text}
                </div>
            </div>
        </div>
    }
}

/// Format timestamp to HH:MM:SS
fn format_timestamp(timestamp: f64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(timestamp));
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}
