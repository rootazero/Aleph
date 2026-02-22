use leptos::prelude::*;
use crate::models::{TraceNode, TraceNodeType};
use crate::context::{DashboardState, GatewayEvent};

#[component]
pub fn AgentTrace() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    // State - start with empty nodes instead of mock data
    let nodes = RwSignal::new(Vec::<TraceNode>::new());
    let is_active = RwSignal::new(true);

    // Subscribe to agent events when connected
    Effect::new(move || {
        if state.is_connected.get() {
            let state = state.clone();
            let nodes = nodes.clone();

            // Subscribe to agent events
            let _subscription_id = state.subscribe_events(move |event: GatewayEvent| {
                // Only process if active
                if !is_active.get() {
                    return;
                }

                // Handle different event types
                match event.topic.as_str() {
                    "agent.started" => {
                        web_sys::console::log_1(&format!("Agent started: {:?}", event.data).into());
                        // Add trace node for agent start
                    }
                    "agent.completed" => {
                        web_sys::console::log_1(&format!("Agent completed: {:?}", event.data).into());
                        // Add trace node for agent completion
                    }
                    "stream.chunk" => {
                        web_sys::console::log_1(&format!("Stream chunk: {:?}", event.data).into());
                        // Add trace node for stream chunk
                    }
                    "stream.tool_start" => {
                        web_sys::console::log_1(&format!("Tool start: {:?}", event.data).into());
                        // Add trace node for tool start
                    }
                    "stream.tool_end" => {
                        web_sys::console::log_1(&format!("Tool end: {:?}", event.data).into());
                        // Add trace node for tool end
                    }
                    _ => {
                        web_sys::console::log_1(&format!("Unknown event: {}", event.topic).into());
                    }
                }
            });

            // Subscribe to agent.* events on the Gateway
            leptos::task::spawn_local(async move {
                if let Err(e) = state.subscribe_topic("agent.*").await {
                    web_sys::console::error_1(&format!("Failed to subscribe to agent events: {}", e).into());
                }
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
                        <span class="text-[10px] text-text-tertiary font-mono">"0.4s duration"</span>
                    </div>
                    <span class="text-[10px] text-text-tertiary font-mono">"14:20:45"</span>
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