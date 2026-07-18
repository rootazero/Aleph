//! Agent Trace view — Live / Replay dual-mode trace timeline.
//!
//! Replay mode now supports a UI-TARS-parity **scrubber**: play / pause,
//! 1×/2×/4×/8× speeds, and a draggable timeline that slices the loaded
//! node list down to `[..=current_step]` for progressive playback.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::trace::TraceApi;
use crate::context::{DashboardState, GatewayEvent};
use crate::i18n::{t, t_string, use_i18n};
use crate::models::{TraceNode, TraceStatus};
use crate::views::agent_trace_model::{
    trace_node_from_event, trace_nodes_from_replay, TraceLabels,
};

use aleph_protocol::AgentTraceEvent;

/// Playback speed presets exposed by the scrubber controls.
const REPLAY_SPEED_STEPS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];
/// Base inter-step delay at 1× speed (ms). Divided by the selected speed.
const REPLAY_BASE_INTERVAL_MS: u32 = 600;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Trace timeline page — accessible at `/dashboard/trace`.
///
/// Supports two modes:
/// - **Live**: subscribes to gateway `run.agent_trace` events in real time.
/// - **Replay**: loads a persisted trace by task ID.
#[component]
#[must_use]
pub fn AgentTrace() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();

    // Signals
    let nodes = RwSignal::new(Vec::<TraceNode>::new());
    let mode = RwSignal::new(TraceMode::Live);
    let task_id_input = RwSignal::new(String::new());
    let error_msg = RwSignal::new(Option::<String>::None);
    let is_loading = RwSignal::new(false);

    // --- Replay scrubber signals (only meaningful in Replay mode) -----------
    // Inclusive cursor — nodes[..=current_step] are rendered. When 0 and
    // `nodes` is non-empty, exactly one node shows; matches typical
    // playhead semantics.
    let current_step = RwSignal::new(0usize);
    let is_playing = RwSignal::new(false);
    let playback_speed = RwSignal::new(1.0f32);

    // --- Live mode: subscribe to agent_trace events --------------------------
    let step_counter = StoredValue::new(std::sync::Arc::new(std::sync::Mutex::new(0u64)));
    // Track the live handler id so a reconnect re-installs exactly one (mirrors
    // home.rs). Without this, each reconnect stacks another handler and every
    // run.agent_trace event pushes a duplicate row (2x, 3x, ...); the leaked
    // handlers also outlive an unmount.
    let live_sub_id = StoredValue::new(None::<usize>);

    // Subscribe to gateway events for live trace
    Effect::new(move || {
        if state.is_connected.get() {
            let labels = TraceLabels::default();
            let counter = step_counter.get_value();

            let sub_id = state.subscribe_events(move |event: GatewayEvent| {
                if event.topic != "run.agent_trace" || mode.get() != TraceMode::Live {
                    return;
                }
                // Try to parse the agent_trace event from data
                if let Ok(trace_event) = serde_json::from_value::<AgentTraceEvent>(
                    event.data.get("event").cloned().unwrap_or(event.data),
                ) {
                    let step = {
                        let mut c = counter
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let s = *c;
                        *c += 1;
                        s
                    };
                    if let Some(node) = trace_node_from_event(&trace_event, step, &labels) {
                        nodes.update(|list| list.push(node));
                    }
                }
            });
            live_sub_id.set_value(Some(sub_id));
        } else if let Some(id) = live_sub_id.get_value() {
            // Drop the prior subscriber so a future connect re-installs cleanly.
            state.unsubscribe_events(id);
            live_sub_id.set_value(None);
        }
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
        is_playing.set(false);
        current_step.set(0);

        spawn_local(async move {
            match TraceApi::get(&state, &tid).await {
                Ok(replay) => {
                    let labels = TraceLabels::default();
                    let entries: Vec<(u64, AgentTraceEvent)> = replay
                        .traces
                        .iter()
                        .map(|e| (e.step, e.event.clone()))
                        .collect();
                    let loaded = trace_nodes_from_replay(&entries, &labels);
                    let last = loaded.len().saturating_sub(1);
                    nodes.set(loaded);
                    // Seed scrubber at the end (full playback) — user can
                    // drag back to step through, then press Play to resume.
                    current_step.set(last);
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
        is_playing.set(false);
        current_step.set(0);
        let counter = step_counter.get_value();
        let mut c = counter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *c = 0;
    };

    // --- Playback driver — Effect schedules the next tick when playing ------
    // Single `setTimeout` per Effect run, recomputed when speed / cursor /
    // playing state changes. Stops when cursor hits the end.
    Effect::new(move |_| {
        if !is_playing.get() || mode.get() != TraceMode::Replay {
            return;
        }
        let total = nodes.get().len();
        let cur = current_step.get();
        if cur + 1 >= total {
            is_playing.set(false);
            return;
        }
        let speed = playback_speed.get().max(0.5);
        let delay_ms = ((REPLAY_BASE_INTERVAL_MS as f32) / speed).max(60.0) as i32;
        let handle = leptos::leptos_dom::helpers::set_timeout_with_handle(
            move || {
                if is_playing.get_untracked() && mode.get_untracked() == TraceMode::Replay {
                    current_step.update(|s| *s = (*s + 1).min(total.saturating_sub(1)));
                }
            },
            std::time::Duration::from_millis(delay_ms as u64),
        );
        if let Ok(h) = handle {
            on_cleanup(move || h.clear());
        }
    });

    // --- Render --------------------------------------------------------------
    view! {
        <div class="px-6 pb-6 aleph-content-top max-w-4xl mx-auto space-y-4">
            <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, trace.title)}</h1>

            // Mode toggle + replay loader
            <div class="flex items-center gap-3 flex-wrap">
                <button
                    class="px-3 py-1.5 rounded text-sm font-medium transition-colors"
                    class:bg-primary=move || mode.get() == TraceMode::Live
                    class:text-white=move || mode.get() == TraceMode::Live
                    class:bg-surface-secondary=move || mode.get() != TraceMode::Live
                    on:click=clear_live
                >
                    {t!(i18n, trace.live)}
                </button>

                <input
                    type="text"
                    placeholder=move || t_string!(i18n, trace.task_id_placeholder).to_string()
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
                    {move || if is_loading.get() {
                        t_string!(i18n, trace.loading).to_string()
                    } else {
                        t_string!(i18n, trace.replay).to_string()
                    }}
                </button>
            </div>

            // Error display
            {move || error_msg.get().map(|msg| view! {
                <div class="p-3 rounded bg-red-500/10 text-red-400 text-sm">{msg}</div>
            })}

            // Replay scrubber — only shown when in Replay mode AND we have
            // nodes to traverse. Live mode hides it (the timeline auto-
            // advances with incoming events anyway).
            <Show when=move || mode.get() == TraceMode::Replay && !nodes.get().is_empty()>
                <ReplayScrubber
                    nodes=nodes
                    current_step=current_step
                    is_playing=is_playing
                    playback_speed=playback_speed
                />
            </Show>

            // Timeline
            <div class="space-y-1">
                {move || {
                    let current_nodes = nodes.get();
                    if current_nodes.is_empty() {
                        view! {
                            <p class="text-text-secondary text-sm italic py-8 text-center">
                                {move || match mode.get() {
                                    TraceMode::Live => t_string!(i18n, trace.waiting_for_events).to_string(),
                                    TraceMode::Replay => t_string!(i18n, trace.no_data_loaded).to_string(),
                                }}
                            </p>
                        }.into_any()
                    } else {
                        // In Replay mode we slice to current_step + 1; in
                        // Live mode the cursor isn't engaged and we render
                        // every accumulated node.
                        let visible: Vec<TraceNode> = if mode.get() == TraceMode::Replay {
                            let upto = current_step.get();
                            current_nodes
                                .into_iter()
                                .take(upto.saturating_add(1))
                                .collect()
                        } else {
                            current_nodes
                        };
                        view! {
                            <div class="space-y-1">
                                {visible.into_iter().map(|node| {
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

/// Replay scrubber row — play/pause + speed + slider + step counter.
#[component]
fn ReplayScrubber(
    nodes: RwSignal<Vec<TraceNode>>,
    current_step: RwSignal<usize>,
    is_playing: RwSignal<bool>,
    playback_speed: RwSignal<f32>,
) -> impl IntoView {
    let i18n = use_i18n();
    let on_play_pause = move |_: web_sys::MouseEvent| {
        let total = nodes.get_untracked().len();
        let cur = current_step.get_untracked();
        // If we're at the end and the user presses play, rewind to start
        // so playback is meaningful instead of a no-op.
        if !is_playing.get_untracked() && cur + 1 >= total {
            current_step.set(0);
        }
        is_playing.update(|p| *p = !*p);
    };
    let on_step_back = move |_: web_sys::MouseEvent| {
        current_step.update(|s| *s = s.saturating_sub(1));
    };
    let on_step_forward = move |_: web_sys::MouseEvent| {
        let total = nodes.get_untracked().len();
        current_step.update(|s| *s = (*s + 1).min(total.saturating_sub(1)));
    };
    let on_slider = move |ev: web_sys::Event| {
        let value = event_target_value(&ev);
        if let Ok(parsed) = value.parse::<usize>() {
            current_step.set(parsed);
            // Manual scrub pauses auto-advance so the user keeps the
            // selected position.
            is_playing.set(false);
        }
    };
    let on_speed = move |new_speed: f32| {
        move |_: web_sys::MouseEvent| {
            playback_speed.set(new_speed);
        }
    };

    view! {
        <div class="flex items-center gap-3 p-3 rounded-lg border border-border
                    bg-surface-secondary/30 text-sm">
            // Play / Pause
            <button
                class="w-8 h-8 rounded-full bg-primary text-white flex items-center justify-center
                       hover:bg-primary-hover transition-colors shrink-0"
                title=move || if is_playing.get() {
                    t_string!(i18n, trace.pause).to_string()
                } else {
                    t_string!(i18n, trace.play).to_string()
                }
                on:click=on_play_pause
            >
                {move || if is_playing.get() {
                    view! {
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                             viewBox="0 0 20 20" fill="currentColor">
                            <rect x="5" y="4" width="3.5" height="12" rx="1"/>
                            <rect x="11.5" y="4" width="3.5" height="12" rx="1"/>
                        </svg>
                    }.into_any()
                } else {
                    view! {
                        <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                             viewBox="0 0 20 20" fill="currentColor">
                            <path d="M5.5 4.21c0-1.05 1.13-1.7 2.04-1.18l9.27 5.29a1.36 1.36 0 0 1 0 2.36L7.54 16a1.36 1.36 0 0 1-2.04-1.18V4.21Z"/>
                        </svg>
                    }.into_any()
                }}
            </button>

            // Step back / forward
            <button
                class="w-7 h-7 rounded-md bg-surface-raised hover:bg-surface-sunken
                       text-text-secondary hover:text-text-primary transition-colors shrink-0"
                title=move || t_string!(i18n, trace.prev_step).to_string()
                on:click=on_step_back
            >"\u{25C0}"</button>
            <button
                class="w-7 h-7 rounded-md bg-surface-raised hover:bg-surface-sunken
                       text-text-secondary hover:text-text-primary transition-colors shrink-0"
                title=move || t_string!(i18n, trace.next_step).to_string()
                on:click=on_step_forward
            >"\u{25B6}"</button>

            // Slider
            <input
                type="range"
                min="0"
                prop:max=move || nodes.get().len().saturating_sub(1).to_string()
                prop:value=move || current_step.get().to_string()
                class="flex-1 accent-primary"
                on:input=on_slider
            />

            // Step counter
            <span class="font-mono text-xs text-text-tertiary shrink-0 min-w-[5ch] text-right">
                {move || format!("{}/{}",
                    current_step.get() + 1,
                    nodes.get().len().max(1))}
            </span>

            // Speed selector
            <div class="flex items-center gap-1 shrink-0">
                {REPLAY_SPEED_STEPS.iter().map(|&speed| {
                    view! {
                        <button
                            class=move || {
                                let active = (playback_speed.get() - speed).abs() < 0.01;
                                if active {
                                    "px-2 py-1 rounded-md bg-primary text-white text-xs font-mono"
                                } else {
                                    "px-2 py-1 rounded-md bg-surface-raised text-text-secondary
                                     hover:bg-surface-sunken text-xs font-mono transition-colors"
                                }
                            }
                            on:click=on_speed(speed)
                        >
                            {format!("{}\u{00D7}", speed as i32)}
                        </button>
                    }
                }).collect::<Vec<_>>()}
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
        .map(|ms| format!("{ms}ms"))
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
