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
//! Rows are selectable: clicking one opens the detail drawer below the tree —
//! the node's full metadata, its result preview, and (for a background child
//! that carries `child_session`) its own transcript, fetched through the same
//! `chat.history` RPC every conversation uses.
//!
//! ## Wiring history (why the subscription lines look the way they do)
//!
//! This view once registered its live handler inside an `Effect` keyed on
//! `is_connected` with no cleanup (a fresh handler per reconnect, all leaked
//! past unmount) and never called `subscribe_topic` at all — and a connection
//! that HAS a topic filter receives only what it subscribed to, so the live
//! half of this view was structurally dead: snapshot on mount, then silence.
//! Now: one `subscribe_events` per mount + `on_cleanup` (the canvas idiom),
//! and a connect-gated `subscribe_topic` whose ledger entry the context
//! replays across reconnects.

mod state;
mod visuals;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, I18nCtx};
use aleph_protocol::subagent_tree::{build_tree, SubagentNode, SubagentTreeEvent, TreeNode};
use state::{apply_event, arrange, summarize, FilterMode, SortMode};
use visuals::{fmt_duration, fmt_tokens, heat_color, lifecycle_color, lifecycle_glyph, sparkline};

/// Topic the gateway relay publishes live tree deltas on — the shared
/// protocol constant, so this filter and the producer cannot drift.
const TREE_TOPIC: &str = aleph_protocol::subagent_tree::TOPIC;

/// One transcript row of a child session, flattened for display.
type TranscriptRow = (String, String);

/// Render one reconstructed tree node (and, recursively, its children). Plain
/// function — built inside the parent's reactive body, so the whole tree
/// re-renders wholesale on each event (fine for a modest background fleet).
fn render_node(node: TreeNode, selected: RwSignal<Option<String>>, i18n: I18nCtx) -> AnyView {
    let n = &node.node;
    let glyph = lifecycle_glyph(n.lifecycle);
    let glyph_color = lifecycle_color(n.lifecycle);
    let heat = heat_color(node.rollup.hotness);
    let task: String = n.task.chars().take(96).collect();
    let model = n.model.clone().unwrap_or_default();
    let tools_line = format!(
        "{} {}",
        n.tool_count,
        t_string!(i18n, subagent_tree.tools_suffix)
    );
    let tokens = n.total_tokens.map(|t| {
        format!(
            "{} {}",
            fmt_tokens(t),
            t_string!(i18n, subagent_tree.tokens_suffix)
        )
    });
    let elapsed = fmt_duration(n.elapsed_ms);
    let activity = describe_activity(n, i18n);
    let preview = n.result_preview.clone().unwrap_or_default();
    let descendants = node.rollup.descendant_count;
    let children = node.children;
    let node_id = n.node_id.clone();
    let id_for_click = node_id.clone();
    let id_for_class = node_id;

    view! {
        <div class="rounded border border-border bg-surface px-3 py-2 cursor-pointer transition-colors hover:border-primary/60"
            class:border-primary=move || selected.get().as_deref() == Some(id_for_class.as_str())
            on:click=move |ev| {
                ev.stop_propagation();
                let id = id_for_click.clone();
                selected.update(|s| {
                    if s.as_deref() == Some(id.as_str()) {
                        *s = None;
                    } else {
                        *s = Some(id);
                    }
                });
            }
        >
            <div class="flex items-center gap-2 text-sm">
                <span class=format!("font-bold {glyph_color}")>{glyph}</span>
                <span
                    class="inline-block w-1.5 h-4 rounded-sm flex-shrink-0"
                    style=format!("background:{heat}")
                    title=t_string!(i18n, subagent_tree.heat_title)
                ></span>
                <span class="text-text-primary font-medium truncate flex-1">{task}</span>
                {(!model.is_empty()).then(|| view! {
                    <span class="text-xs px-1.5 py-0.5 rounded bg-surface-secondary text-text-secondary whitespace-nowrap">
                        {model}
                    </span>
                })}
                <span class="text-xs text-text-secondary whitespace-nowrap">{tools_line}</span>
                {tokens.map(|t| view! {
                    <span class="text-xs text-text-secondary whitespace-nowrap">{t}</span>
                })}
                <span class="text-xs text-text-secondary whitespace-nowrap">{elapsed}</span>
            </div>
            {(!activity.is_empty()).then(|| view! {
                <div class="text-xs text-text-tertiary mt-0.5">{activity}</div>
            })}
            {(!preview.is_empty()).then(|| view! {
                <div class="text-xs text-text-secondary mt-0.5 truncate" title=t_string!(i18n, subagent_tree.preview_title)>
                    "→ "{preview}
                </div>
            })}
            {(descendants > 1).then(|| view! {
                <div class="text-xs text-text-tertiary">
                    {format!("{descendants} {}", t_string!(i18n, subagent_tree.subtree_agents))}
                </div>
            })}
            {(!children.is_empty()).then(|| view! {
                <div class="ml-3 mt-1 space-y-1 border-l border-border pl-2">
                    {children.into_iter().map(|c| render_node(c, selected, i18n)).collect_view()}
                </div>
            })}
        </div>
    }
    .into_any()
}

/// Human line for a node's most recent activity — folds `last_tool` in, which
/// used to cross the wire and never reach a pixel.
fn describe_activity(n: &SubagentNode, i18n: I18nCtx) -> String {
    match (n.last_activity.as_deref(), n.last_tool.as_deref()) {
        (Some("tool_called" | "tool_returned"), Some(tool)) | (None, Some(tool)) => {
            format!("{} {tool}", t_string!(i18n, subagent_tree.last_tool))
        }
        (Some("llm_thinking"), _) => t_string!(i18n, subagent_tree.thinking).to_string(),
        (Some(other), _) => other.to_string(),
        (None, None) => String::new(),
    }
}

/// Background sub-agent tree page — accessible at `/dashboard/subagents`.
#[component]
#[must_use]
pub fn SubagentTree() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Flat node list (keyed by node_id via apply_event upsert). The tree is
    // rebuilt from this on every render via the shared build_tree.
    let nodes = RwSignal::new(Vec::<SubagentNode>::new());
    let sort = RwSignal::new(SortMode::Status);
    let filter = RwSignal::new(FilterMode::All);
    // Selected node (detail drawer) + its lazily-fetched child transcript.
    // The transcript signal holds (node_id, outcome) so a slow response for a
    // previously-selected node cannot label the currently-selected one.
    let selected = RwSignal::new(Option::<String>::None);
    let transcript = RwSignal::new(Option::<(String, Result<Vec<TranscriptRow>, String>)>::None);

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

    // Server-side topic subscription — without it the connection's filter
    // (seeded with BASE_TOPICS at connect) drops every tree delta before it
    // reaches the socket. Ledgered, so the context replays it on reconnect.
    Effect::new(move |_| {
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            let _ = state.subscribe_topic(TREE_TOPIC).await;
        });
    });

    // Live deltas — registered once per mount, dropped on unmount (the canvas
    // idiom; an `Effect`-registered handler stacked one copy per reconnect).
    let sub_id = state.subscribe_events(move |event| {
        if event.topic != TREE_TOPIC {
            return;
        }
        if let Ok(ev) = serde_json::from_value::<SubagentTreeEvent>(event.data.clone()) {
            nodes.update(|list| apply_event(list, ev));
        }
    });
    on_cleanup(move || state.unsubscribe_events(sub_id));

    // Detail drawer: fetch the selected node's child transcript when it has
    // an address. Keyed by node_id so a stale response cannot mislabel.
    Effect::new(move || {
        let Some(id) = selected.get() else {
            transcript.set(None);
            return;
        };
        // Untracked on purpose: tracking `nodes` here would re-fetch the
        // transcript on EVERY live tree event (each Progress tick) while a
        // node is selected. The address is immutable per node, so reading it
        // once per selection is the whole requirement.
        let child = nodes.with_untracked(|list| {
            list.iter()
                .find(|n| n.node_id == id)
                .and_then(|n| n.child_session.clone())
        });
        let Some(child_key) = child else {
            transcript.set(None);
            return;
        };
        spawn_local(async move {
            let params = serde_json::json!({ "session_key": child_key });
            let outcome = match state.rpc_call("chat.history", params).await {
                Ok(val) => Ok(transcript_rows(&val)),
                Err(e) => Err(e.to_string()),
            };
            // Apply only if this node is still the selected one. `try_` +
            // `flatten`: past the `.await` the owning component may already
            // be disposed, and a plain read there panics the whole panel
            // (`disposed_reads` census).
            if selected.try_get_untracked().flatten().as_deref() == Some(id.as_str()) {
                let _ = transcript.try_set(Some((id, outcome)));
            }
        });
    });

    view! {
        <div class="px-6 pb-6 aleph-content-top max-w-4xl mx-auto space-y-4">
            <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, subagent_tree.title)}</h1>

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
                <span class="text-text-tertiary">{t!(i18n, subagent_tree.sort)}</span>
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

                <span class="text-text-tertiary ml-3">{t!(i18n, subagent_tree.filter)}</span>
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
                                {t!(i18n, subagent_tree.empty)}
                            </div>
                        }
                        .into_any();
                    }
                    let arranged = arrange(build_tree(&flat), sort.get(), filter.get());
                    if arranged.is_empty() {
                        return view! {
                            <div class="text-text-tertiary text-sm py-4">{t!(i18n, subagent_tree.no_match)}</div>
                        }
                        .into_any();
                    }
                    arranged
                        .into_iter()
                        .map(|node| render_node(node, selected, i18n))
                        .collect_view()
                        .into_any()
                }}
            </div>

            // Detail drawer for the selected node.
            {move || {
                let id = selected.get()?;
                let node = nodes.with(|list| list.iter().find(|n| n.node_id == id).cloned())?;
                Some(render_detail(&node, transcript, i18n))
            }}
        </div>
    }
}

/// The detail drawer body: full metadata + result preview + child transcript.
fn render_detail(
    node: &SubagentNode,
    transcript: RwSignal<Option<(String, Result<Vec<TranscriptRow>, String>)>>,
    i18n: I18nCtx,
) -> AnyView {
    let task = node.task.clone();
    let node_id = node.node_id.clone();
    let mut meta = format!(
        "{glyph} {lifecycle:?} · {tools} {tools_word} · {elapsed}",
        glyph = lifecycle_glyph(node.lifecycle),
        lifecycle = node.lifecycle,
        tools = node.tool_count,
        tools_word = t_string!(i18n, subagent_tree.tools_suffix),
        elapsed = fmt_duration(node.elapsed_ms),
    );
    if let Some(model) = node.model.as_deref() {
        meta.push_str(&format!(" · {model}"));
    }
    if let Some(tokens) = node.total_tokens {
        meta.push_str(&format!(
            " · {} {}",
            fmt_tokens(tokens),
            t_string!(i18n, subagent_tree.tokens_suffix)
        ));
    }
    let preview = node.result_preview.clone();
    let has_child = node.child_session.is_some();

    view! {
        <div class="rounded border border-primary/50 bg-surface px-4 py-3 space-y-2">
            <div class="text-sm font-semibold text-text-primary">{task}</div>
            <div class="text-xs text-text-secondary font-mono">{meta}</div>
            {preview.map(|p| view! {
                <div class="text-xs text-text-secondary whitespace-pre-wrap">"→ "{p}</div>
            })}
            {move || {
                if !has_child {
                    return view! {
                        <div class="text-xs text-text-tertiary">
                            {t!(i18n, subagent_tree.no_child_session)}
                        </div>
                    }.into_any();
                }
                match transcript.get() {
                    Some((tid, Ok(rows))) if tid == node_id => {
                        if rows.is_empty() {
                            view! { <div class="text-xs text-text-tertiary">{t!(i18n, subagent_tree.transcript_empty)}</div> }.into_any()
                        } else {
                            view! {
                                <div class="max-h-80 overflow-y-auto space-y-2 border-t border-border pt-2">
                                    {rows.into_iter().map(|(role, content)| view! {
                                        <div>
                                            <div class="text-xs font-semibold text-text-tertiary uppercase">{role}</div>
                                            <div class="text-xs text-text-secondary whitespace-pre-wrap">{content}</div>
                                        </div>
                                    }).collect_view()}
                                </div>
                            }.into_any()
                        }
                    }
                    Some((tid, Err(e))) if tid == node_id => {
                        view! { <div class="text-xs text-error">{t!(i18n, subagent_tree.transcript_unavailable)}" "{e}</div> }.into_any()
                    }
                    _ => view! { <div class="text-xs text-text-tertiary">{t!(i18n, subagent_tree.transcript_loading)}</div> }.into_any(),
                }
            }}
        </div>
    }
    .into_any()
}

/// Flatten a `chat.history` response into `(role, content)` rows.
fn transcript_rows(result: &serde_json::Value) -> Vec<TranscriptRow> {
    result
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let role = row
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let content = row
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (role, content)
                })
                .collect()
        })
        .unwrap_or_default()
}
