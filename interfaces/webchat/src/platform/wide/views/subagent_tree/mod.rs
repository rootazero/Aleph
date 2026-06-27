//! Background sub-agent tree view (`/dashboard/subagents`).
//!
//! Live, hierarchical view of background sub-agents. Cold-starts from the
//! `subagent.tree` RPC (flat nodes) and updates incrementally from the
//! `run.subagent_tree` gateway topic. The forest itself is rebuilt with the
//! shared `aleph_protocol::subagent_tree::build_tree` — the SAME Rust code the
//! server links, compiled to WASM here (no Python+TS-style double tree builder).
//!
//! Rich visualization (hermes overlay parity): status glyphs, per-node hotness
//! heatmap, depth sparkline, sort/filter controls, and a rollup summary line.

mod state;
mod visuals;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::context::{DashboardState, GatewayEvent};
use aleph_protocol::subagent_tree::{build_tree, SubagentNode, SubagentTreeEvent, TreeNode};
use state::{apply_event, arrange, summarize, FilterMode, SortMode};
use visuals::{fmt_duration, heat_color, lifecycle_color, lifecycle_glyph, sparkline};

/// Topic the gateway relay publishes live tree deltas on.
const TREE_TOPIC: &str = "run.subagent_tree";

/// Render one reconstructed tree node (and, recursively, its children). Plain
/// function — built inside the parent's reactive body, so the whole tree
/// re-renders wholesale on each event (fine for a modest background fleet).
fn render_node(node: TreeNode) -> AnyView {
    let n = &node.node;
    let glyph = lifecycle_glyph(n.lifecycle);
    let glyph_color = lifecycle_color(n.lifecycle);
    let heat = heat_color(node.rollup.hotness);
    let task: String = n.task.chars().take(96).collect();
    let model = n.model.clone().unwrap_or_default();
    let tools = n.tool_count;
    let elapsed = fmt_duration(n.elapsed_ms);
    let activity = n.last_activity.clone().unwrap_or_default();
    let descendants = node.rollup.descendant_count;
    let children = node.children;

    view! {
        <div class="rounded border border-border bg-surface px-3 py-2">
            <div class="flex items-center gap-2 text-sm">
                <span class=format!("font-bold {glyph_color}")>{glyph}</span>
                <span
                    class="inline-block w-1.5 h-4 rounded-sm flex-shrink-0"
                    style=format!("background:{heat}")
                    title="activity heat (tools/sec)"
                ></span>
                <span class="text-text-primary font-medium truncate flex-1">{task}</span>
                {(!model.is_empty()).then(|| view! {
                    <span class="text-xs px-1.5 py-0.5 rounded bg-surface-secondary text-text-secondary whitespace-nowrap">
                        {model}
                    </span>
                })}
                <span class="text-xs text-text-secondary whitespace-nowrap">{tools}" tools"</span>
                <span class="text-xs text-text-secondary whitespace-nowrap">{elapsed}</span>
            </div>
            {(!activity.is_empty()).then(|| view! {
                <div class="text-xs text-text-tertiary mt-0.5">{activity}</div>
            })}
            {(descendants > 1).then(|| view! {
                <div class="text-xs text-text-tertiary">{format!("subtree: {descendants} agents")}</div>
            })}
            {(!children.is_empty()).then(|| view! {
                <div class="ml-3 mt-1 space-y-1 border-l border-border pl-2">
                    {children.into_iter().map(render_node).collect_view()}
                </div>
            })}
        </div>
    }
    .into_any()
}

/// Background sub-agent tree page — accessible at `/dashboard/subagents`.
#[component]
#[must_use]
pub fn SubagentTree() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Flat node list (keyed by node_id via apply_event upsert). The tree is
    // rebuilt from this on every render via the shared build_tree.
    let nodes = RwSignal::new(Vec::<SubagentNode>::new());
    let sort = RwSignal::new(SortMode::Status);
    let filter = RwSignal::new(FilterMode::All);

    // Cold-start: fetch the current flat snapshot once connected. Merges (not
    // replaces) so a live Spawned that arrived first is preserved.
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            if let Ok(val) = state.rpc_call("subagent.tree", serde_json::json!({})).await {
                if let Some(arr) = val.get("nodes").and_then(|v| v.as_array()) {
                    let fetched: Vec<SubagentNode> = arr
                        .iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect();
                    nodes.update(|list| {
                        for node in fetched {
                            apply_event(list, SubagentTreeEvent::Spawned { node });
                        }
                    });
                }
            }
        });
    });

    // Live deltas: subscribe to the relay's run.subagent_tree topic.
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        state.subscribe_events(move |event: GatewayEvent| {
            if event.topic != TREE_TOPIC {
                return;
            }
            if let Ok(ev) = serde_json::from_value::<SubagentTreeEvent>(event.data.clone()) {
                nodes.update(|list| apply_event(list, ev));
            }
        });
    });

    view! {
        <div class="px-6 pb-6 aleph-content-top max-w-4xl mx-auto space-y-4">
            <h1 class="text-2xl font-bold text-text-primary">"Background Sub-agents"</h1>

            // Rollup summary line + depth sparkline.
            {move || {
                let s = summarize(&nodes.get());
                let line = format!(
                    "d{} · {} agents · {} tools · {} · ⚡{}",
                    s.max_depth,
                    s.agents,
                    s.tools,
                    fmt_duration(s.total_duration_ms),
                    s.active,
                );
                let spark = sparkline(&s.depth_counts);
                view! {
                    <div class="flex items-center gap-3 text-sm text-text-secondary">
                        <span class="font-mono">{line}</span>
                        <span class="font-mono tracking-tight text-text-tertiary">{spark}</span>
                    </div>
                }
            }}

            // Sort + filter controls.
            <div class="flex items-center gap-2 flex-wrap text-xs">
                <span class="text-text-tertiary">"sort:"</span>
                {SortMode::ALL.into_iter().map(|m| view! {
                    <button
                        class="px-2 py-0.5 rounded border border-border bg-surface-secondary text-text-secondary transition-colors"
                        class:bg-primary=move || sort.get() == m
                        class:text-white=move || sort.get() == m
                        on:click=move |_| sort.set(m)
                    >
                        {m.label()}
                    </button>
                }).collect_view()}

                <span class="text-text-tertiary ml-3">"filter:"</span>
                {FilterMode::ALL.into_iter().map(|m| view! {
                    <button
                        class="px-2 py-0.5 rounded border border-border bg-surface-secondary text-text-secondary transition-colors"
                        class:bg-primary=move || filter.get() == m
                        class:text-white=move || filter.get() == m
                        on:click=move |_| filter.set(m)
                    >
                        {m.label()}
                    </button>
                }).collect_view()}
            </div>

            // Tree body.
            <div class="space-y-1">
                {move || {
                    let flat = nodes.get();
                    if flat.is_empty() {
                        return view! {
                            <div class="text-text-secondary text-sm py-8 text-center">
                                "No background sub-agents yet. They appear here the moment one is spawned."
                            </div>
                        }
                        .into_any();
                    }
                    let arranged = arrange(build_tree(&flat), sort.get(), filter.get());
                    if arranged.is_empty() {
                        return view! {
                            <div class="text-text-tertiary text-sm py-4">"No sub-agents match this filter."</div>
                        }
                        .into_any();
                    }
                    arranged.into_iter().map(render_node).collect_view().into_any()
                }}
            </div>
        </div>
    }
}
