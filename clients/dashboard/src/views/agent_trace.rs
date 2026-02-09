use leptos::prelude::*;
use crate::models::{TraceNode, TraceNodeType};
use crate::mock_data::generate_mock_trace_nodes;
use crate::context::DashboardState;

#[component]
pub fn AgentTrace() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    // State
    let nodes = RwSignal::new(generate_mock_trace_nodes());
    let is_active = RwSignal::new(true);

    view! {
        <div class="h-full flex flex-col">
            // Header
            <header class="p-8 border-b border-slate-800 bg-slate-900/20 backdrop-blur-md sticky top-0 z-10">
                <div class="max-w-7xl mx-auto flex items-center justify-between">
                    <div>
                        <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-slate-100">
                            <svg width="32" height="32" attr:class="w-8 h-8 text-indigo-500" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                            </svg>
                            "Live Agent Trace"
                        </h2>
                        <p class="text-slate-400">"Real-time observation of Agent's internal reasoning and actions."</p>
                    </div>

                    <div class="flex items-center gap-3">
                        <button
                            on:click=move |_| is_active.update(|v| *v = !*v)
                            class="flex items-center gap-2 px-4 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 transition-colors border border-slate-700 hover:border-slate-600 shadow-sm"
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
                            class="p-2 rounded-lg text-slate-400 hover:text-red-400 hover:bg-red-400/10 transition-all border border-transparent hover:border-red-400/20"
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
                            <div class="max-w-4xl mx-auto bg-amber-500/10 border border-amber-500/20 rounded-xl p-6 flex items-start gap-4">
                                <svg width="24" height="24" attr:class="w-6 h-6 text-amber-500 flex-shrink-0 mt-0.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                                    <line x1="12" y1="9" x2="12" y2="13" />
                                    <line x1="12" y1="17" x2="12.01" y2="17" />
                                </svg>
                                <div>
                                    <h3 class="text-amber-400 font-semibold mb-1">"Gateway Connection Required"</h3>
                                    <p class="text-sm text-amber-300/80">"Please connect to the Aleph Gateway from the System Status page to receive live Agent trace events."</p>
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
                    <div class="relative border-l-2 border-slate-800 ml-4 pl-10 space-y-12 pb-24">
                        <For
                            each=move || nodes.get()
                            key=|node| node.id.clone()
                            children=move |node| view! {
                                <TraceNodeItem node=node />
                            }
                        />
                    </div>
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
        TraceNodeType::Thinking => "text-blue-400 bg-blue-400/10 border-blue-400/20",
        TraceNodeType::ToolCall => "text-amber-400 bg-amber-400/10 border-amber-400/20",
        TraceNodeType::ToolResult => "text-emerald-400 bg-emerald-400/10 border-emerald-400/20",
        _ => "text-slate-400 bg-slate-800 border-slate-700",
    };

    view! {
        <div class="relative group">
            // Timeline Dot
            <div class=format!("absolute -left-[51px] top-2 w-10 h-10 rounded-full border-2 bg-slate-950 flex items-center justify-center z-10 group-hover:scale-110 transition-transform shadow-glass {}", accent_color)>
                <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {icon_content}
                </svg>
            </div>

            // Card
            <div class="bg-slate-900/40 border border-slate-800 rounded-2xl p-6 backdrop-blur-sm group-hover:border-slate-700 transition-all shadow-xl shadow-black/20">
                <div class="flex items-center justify-between mb-4">
                    <div class="flex items-center gap-3">
                        <span class=format!("text-[10px] font-bold uppercase tracking-widest px-2 py-0.5 rounded border {}", accent_color)>
                            {format!("{:?}", node.node_type)}
                        </span>
                        <span class="text-[10px] text-slate-500 font-mono">"0.4s duration"</span>
                    </div>
                    <span class="text-[10px] text-slate-500 font-mono">"14:20:45"</span>
                </div>

                <div class="text-slate-200 leading-relaxed font-sans text-sm">
                    {node.content}
                </div>

                {if !node.children.is_empty() {
                    let children = node.children.clone();
                    view! {
                        <div class="mt-4 pt-4 border-t border-slate-800/50 space-y-3">
                            <For
                                each=move || children.clone()
                                key=|child| child.id.clone()
                                children=move |child| view! {
                                    <div class="flex items-start gap-3 text-sm text-slate-400 pl-2 border-l border-slate-800">
                                        <div class="w-1.5 h-1.5 rounded-full bg-slate-700 mt-1.5"></div>
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