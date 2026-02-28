use leptos::prelude::*;
use crate::models::{TraceNode, TraceNodeType, TraceStatus};
use crate::context::{DashboardState, GatewayEvent};

/// Generate a unique node ID
fn next_node_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("trace-{}", COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Get current timestamp as ms since epoch
fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// Extract a string field from JSON value
fn json_str(data: &serde_json::Value, key: &str) -> String {
    data.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Format epoch ms timestamp to HH:MM:SS
fn format_timestamp(epoch_ms: f64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(epoch_ms));
    let h = date.get_hours();
    let m = date.get_minutes();
    let s = date.get_seconds();
    format!("{:02}:{:02}:{:02}", h, m, s)
}

#[component]
pub fn AgentTrace() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    // State - start with empty nodes instead of mock data
    let nodes = RwSignal::new(Vec::<TraceNode>::new());
    let is_active = RwSignal::new(true);

    // Track tool start times for duration calculation
    let tool_start_times = StoredValue::new(std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::HashMap::<String, f64>::new(),
    )));

    // Subscribe to agent events when connected
    Effect::new(move || {
        if state.is_connected.get() {
            let state = state.clone();

            // Subscribe to agent events
            // Note: context.rs remaps "stream.*" → "run.*" topics before dispatch
            let _subscription_id = state.subscribe_events(move |event: GatewayEvent| {
                // Only process if active
                if !is_active.get() {
                    return;
                }

                let topic = event.topic.as_str();

                // Handle stream events (remapped from "stream.*" to "run.*" by context.rs)
                let node = match topic {
                    "run.run_accepted" => {
                        let run_id = json_str(&event.data, "run_id");
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Decision,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!("Agent run started ({})", if run_id.is_empty() { "unknown".to_string() } else { run_id }),
                            status: TraceStatus::InProgress,
                            children: vec![],
                        })
                    }
                    "run.reasoning" => {
                        let content = json_str(&event.data, "content");
                        if content.is_empty() {
                            None
                        } else {
                            Some(TraceNode {
                                id: next_node_id(),
                                node_type: TraceNodeType::Thinking,
                                timestamp: now_ms(),
                                duration_ms: None,
                                content,
                                status: TraceStatus::Success,
                                children: vec![],
                            })
                        }
                    }
                    "run.reasoning_block" => {
                        let label = json_str(&event.data, "label");
                        let content = json_str(&event.data, "content");
                        let display = if label.is_empty() { content } else { format!("{}: {}", label, content) };
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Thinking,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: display,
                            status: TraceStatus::Success,
                            children: vec![],
                        })
                    }
                    "run.tool_start" => {
                        let tool_name = json_str(&event.data, "tool_name");
                        let tool_id = json_str(&event.data, "tool_id");
                        let params = event.data.get("params")
                            .map(|p| serde_json::to_string(p).unwrap_or_default())
                            .unwrap_or_default();

                        // Record start time for duration calculation
                        if !tool_id.is_empty() {
                            let times = tool_start_times.get_value();
                            let lock_result = times.lock();
                            if let Ok(mut map) = lock_result {
                                map.insert(tool_id.clone(), now_ms());
                            }
                        }

                        let content = if params.is_empty() || params == "{}" {
                            format!("Calling tool: {}", tool_name)
                        } else {
                            // Truncate long params
                            let truncated = if params.len() > 200 {
                                format!("{}...", &params[..200])
                            } else {
                                params
                            };
                            format!("Calling tool: {} ({})", tool_name, truncated)
                        };

                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::ToolCall,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content,
                            status: TraceStatus::InProgress,
                            children: vec![],
                        })
                    }
                    "run.tool_end" => {
                        let tool_id = json_str(&event.data, "tool_id");
                        let duration_ms = event.data.get("duration_ms")
                            .and_then(|v| v.as_u64());
                        let success = event.data.get("result")
                            .and_then(|r| r.get("success"))
                            .and_then(|s| s.as_bool())
                            .unwrap_or(true);
                        let output = event.data.get("result")
                            .and_then(|r| r.get("output"))
                            .and_then(|o| o.as_str())
                            .unwrap_or("");
                        let error = event.data.get("result")
                            .and_then(|r| r.get("error"))
                            .and_then(|e| e.as_str())
                            .unwrap_or("");

                        // Calculate duration from start time if not provided
                        let final_duration = duration_ms.or_else(|| {
                            if !tool_id.is_empty() {
                                let times = tool_start_times.get_value();
                                let lock_result = times.lock();
                                if let Ok(mut map) = lock_result {
                                    map.remove(&tool_id).map(|start| (now_ms() - start) as u64)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        });

                        let content = if success {
                            let display = if output.len() > 300 {
                                format!("{}...", &output[..300])
                            } else {
                                output.to_string()
                            };
                            if display.is_empty() {
                                "Tool completed successfully".to_string()
                            } else {
                                format!("Result: {}", display)
                            }
                        } else {
                            format!("Tool failed: {}", if error.is_empty() { "unknown error" } else { error })
                        };

                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::ToolResult,
                            timestamp: now_ms(),
                            duration_ms: final_duration,
                            content,
                            status: if success { TraceStatus::Success } else { TraceStatus::Failed },
                            children: vec![],
                        })
                    }
                    "run.response_chunk" => {
                        // Only show final response chunks to avoid flooding
                        let is_final = event.data.get("is_final")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if is_final {
                            let content = json_str(&event.data, "content");
                            if !content.is_empty() {
                                Some(TraceNode {
                                    id: next_node_id(),
                                    node_type: TraceNodeType::Observation,
                                    timestamp: now_ms(),
                                    duration_ms: None,
                                    content: if content.len() > 500 {
                                        format!("{}...", &content[..500])
                                    } else {
                                        content
                                    },
                                    status: TraceStatus::Success,
                                    children: vec![],
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    "run.run_complete" => {
                        let duration = event.data.get("total_duration_ms")
                            .and_then(|v| v.as_u64());
                        let tool_calls = event.data.get("summary")
                            .and_then(|s| s.get("tool_calls"))
                            .and_then(|t| t.as_u64())
                            .unwrap_or(0);
                        let loops = event.data.get("summary")
                            .and_then(|s| s.get("loops"))
                            .and_then(|l| l.as_u64())
                            .unwrap_or(0);

                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Decision,
                            timestamp: now_ms(),
                            duration_ms: duration,
                            content: format!("Run complete ({} tool calls, {} loops)", tool_calls, loops),
                            status: TraceStatus::Success,
                            children: vec![],
                        })
                    }
                    "run.run_error" => {
                        let error = json_str(&event.data, "error");
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Decision,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!("Run failed: {}", if error.is_empty() { "unknown error".to_string() } else { error }),
                            status: TraceStatus::Failed,
                            children: vec![],
                        })
                    }
                    "run.ask_user" => {
                        let question = json_str(&event.data, "question");
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Observation,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!("Asking user: {}", question),
                            status: TraceStatus::InProgress,
                            children: vec![],
                        })
                    }
                    "run.uncertainty_signal" => {
                        let uncertainty = json_str(&event.data, "uncertainty");
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Observation,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!("Uncertainty: {}", uncertainty),
                            status: TraceStatus::InProgress,
                            children: vec![],
                        })
                    }
                    _ => {
                        // Handle agent.* events (dispatched directly as "event" notifications)
                        if topic.starts_with("agent.") || topic.starts_with("run.") {
                            web_sys::console::log_1(&format!("Trace event: {} - {:?}", topic, event.data).into());
                        }
                        None
                    }
                };

                // Append node if created
                if let Some(node) = node {
                    nodes.update(|list| {
                        list.push(node);
                        // Keep at most 200 nodes to prevent memory bloat
                        if list.len() > 200 {
                            list.drain(0..list.len() - 200);
                        }
                    });
                }
            });

            // Subscribe to stream events on the Gateway
            leptos::task::spawn_local(async move {
                if let Err(e) = state.subscribe_topic("stream.*").await {
                    web_sys::console::error_1(&format!("Failed to subscribe to stream events: {}", e).into());
                }
            });
        }
    });

    view! {
        <div class="h-full flex flex-col">
            // Header
            <header class="p-8 border-b border-border bg-surface-raised sticky top-0 z-10">
                <div class="max-w-7xl mx-auto flex items-center justify-between">
                    <div>
                        <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                            <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                            </svg>
                            "Live Agent Trace"
                        </h2>
                        <p class="text-text-secondary">"Real-time observation of Agent's internal reasoning and actions."</p>
                    </div>

                    <div class="flex items-center gap-3">
                        <button
                            on:click=move |_| is_active.update(|v| *v = !*v)
                            class="flex items-center gap-2 px-4 py-2 rounded-lg bg-surface-sunken hover:bg-surface-raised transition-colors border border-border hover:border-border-strong"
                            disabled=move || !state.is_connected.get()
                        >
                            {move || if is_active.get() {
                                view! {
                                    <div class="flex items-center gap-2">
                                        <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                            <rect x="6" y="4" width="4" height="16" />
                                            <rect x="14" y="4" width="4" height="16" />
                                        </svg>
                                        "Pause"
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="flex items-center gap-2">
                                        <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                            <polygon points="5 3 19 12 5 21 5 3" />
                                        </svg>
                                        "Resume"
                                    </div>
                                }.into_any()
                            }}
                        </button>
                        <button
                            on:click=move |_| nodes.set(Vec::new())
                            class="p-2 rounded-lg text-text-secondary hover:text-danger hover:bg-danger-subtle transition-all border border-transparent hover:border-danger/20"
                        >
                            <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M3 6h18" />
                                <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
                                <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
                            </svg>
                        </button>
                    </div>
                </div>
            </header>

            // Connection status warning
            {move || {
                if !state.is_connected.get() {
                    view! {
                        <div class="p-8">
                            <div class="max-w-4xl mx-auto bg-warning-subtle border border-warning/20 rounded-xl p-6 flex items-start gap-4">
                                <svg width="24" height="24" attr:class="w-6 h-6 text-warning flex-shrink-0 mt-0.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                                    <line x1="12" y1="9" x2="12" y2="13" />
                                    <line x1="12" y1="17" x2="12.01" y2="17" />
                                </svg>
                                <div>
                                    <h3 class="text-warning font-semibold mb-1">"Gateway Connection Required"</h3>
                                    <p class="text-sm text-text-secondary">"Please connect to the Aleph Gateway from the System Status page to receive live Agent trace events."</p>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Timeline Content
            <div class="flex-1 overflow-y-auto p-8">
                <div class="max-w-4xl mx-auto">
                    {move || {
                        let node_list = nodes.get();
                        if node_list.is_empty() {
                            view! {
                                <div class="text-center py-16">
                                    <div class="text-text-tertiary mb-2">
                                        <svg width="48" height="48" attr:class="w-12 h-12 mx-auto mb-4 opacity-50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                            <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                                        </svg>
                                    </div>
                                    <p class="text-text-secondary">"No agent trace events yet"</p>
                                    <p class="text-sm text-text-tertiary mt-2">"Events will appear here when the agent starts processing"</p>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="relative border-l-2 border-border ml-4 pl-10 space-y-12 pb-24">
                                    <For
                                        each=move || nodes.get()
                                        key=|node| node.id.clone()
                                        children=move |node| view! {
                                            <TraceNodeItem node=node />
                                        }
                                    />
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}

#[component]
fn TraceNodeItem(node: TraceNode) -> impl IntoView {
    let icon_content = match node.node_type {
        TraceNodeType::Thinking => view! {
            <path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96.44 2.5 2.5 0 0 1-2.96-3.08 3 3 0 0 1-.34-5.58 2.5 2.5 0 0 1 1.32-4.24 2.5 2.5 0 0 1 4.44-2.08z" />
            <path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96.44 2.5 2.5 0 0 0 2.96-3.08 3 3 0 0 0 .34-5.58 2.5 2.5 0 0 0-1.32-4.24 2.5 2.5 0 0 0-4.44-2.08z" />
        }.into_any(),
        TraceNodeType::ToolCall => view! {
            <polyline points="4 17 10 11 4 5" />
            <line x1="12" y1="19" x2="20" y2="19" />
        }.into_any(),
        TraceNodeType::ToolResult => view! {
            <polyline points="20 6 9 17 4 12" />
        }.into_any(),
        _ => view! {
            <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
        }.into_any(),
    };

    let accent_color = match node.node_type {
        TraceNodeType::Thinking => "text-info bg-info-subtle border-info/20",
        TraceNodeType::ToolCall => "text-warning bg-warning-subtle border-warning/20",
        TraceNodeType::ToolResult => "text-success bg-success-subtle border-success/20",
        _ => "text-text-tertiary bg-surface-sunken border-border",
    };

    view! {
        <div class="relative group">
            // Timeline Dot
            <div class=format!("absolute -left-[51px] top-2 w-10 h-10 rounded-full border-2 bg-surface flex items-center justify-center z-10 group-hover:scale-110 transition-transform {}", accent_color)>
                <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {icon_content}
                </svg>
            </div>

            // Card
            <div class="bg-surface-raised border border-border rounded-2xl p-6 group-hover:border-border-strong transition-all">
                <div class="flex items-center justify-between mb-4">
                    <div class="flex items-center gap-3">
                        <span class=format!("text-[10px] font-bold uppercase tracking-widest px-2 py-0.5 rounded border {}", accent_color)>
                            {format!("{:?}", node.node_type)}
                        </span>
                        {node.duration_ms.map(|ms| {
                            let duration_str = if ms < 1000 {
                                format!("{}ms", ms)
                            } else {
                                format!("{:.1}s", ms as f64 / 1000.0)
                            };
                            view! {
                                <span class="text-[10px] text-text-tertiary font-mono">{duration_str}</span>
                            }
                        })}
                    </div>
                    <span class="text-[10px] text-text-tertiary font-mono">{format_timestamp(node.timestamp)}</span>
                </div>

                <div class="text-text-primary leading-relaxed font-sans text-sm">
                    {node.content}
                </div>

                {if !node.children.is_empty() {
                    let children = node.children.clone();
                    view! {
                        <div class="mt-4 pt-4 border-t border-border-subtle space-y-3">
                            <For
                                each=move || children.clone()
                                key=|child| child.id.clone()
                                children=move |child| view! {
                                    <div class="flex items-start gap-3 text-sm text-text-secondary pl-2 border-l border-border">
                                        <div class="w-1.5 h-1.5 rounded-full bg-border mt-1.5"></div>
                                        <div class="flex-1 text-xs">{child.content}</div>
                                    </div>
                                }
                            />
                        </div>
                    }.into_any()
                } else {
                    view! {}.into_any()
                }}
            </div>
        </div>
    }
}