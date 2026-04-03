use crate::api::TraceApi;
use crate::context::{DashboardState, GatewayEvent};
use crate::i18n::*;
use crate::models::{TraceNode, TraceNodeType, TraceStatus};
use crate::views::agent_trace_model::{
    trace_node_from_agent_trace_event, trace_nodes_from_replay, TraceLabels,
};
use aleph_protocol::{
    present_agent_trace_event_with_labels_and_preset, AgentTraceEvent, AgentTraceTaskSummary,
    AgentTraceToolCallEnd, AgentTraceToolCallStart, AgentTraceToolResult, AgentTraceTurnMetrics,
    AgentTraceTurnOutcome, AgentTracePresentationPreset, ToolResult,
};
use leptos::prelude::*;
use leptos_i18n::I18nContext;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TraceMode {
    Live,
    Replay,
}

fn format_replay_task_label(task: &AgentTraceTaskSummary) -> String {
    let status = task.status.replace('_', " ");
    let prompt = if task.prompt_preview.is_empty() {
        "untitled".to_string()
    } else {
        task.prompt_preview.clone()
    };

    format!(
        "{} | {} | {} traces | {}",
        task.agent_id, status, task.trace_count, prompt
    )
}

#[derive(Clone, Copy)]
enum TraceLabelKey {
    CallingTool,
    ToolCompleted,
    ToolResult,
    ToolFailed,
    TurnStarted,
    StateEntered,
    TurnCompleted,
    SessionCompleted,
    UnknownError,
}

fn build_trace_labels(mut label_for: impl FnMut(TraceLabelKey) -> String) -> TraceLabels {
    TraceLabels {
        calling_tool: label_for(TraceLabelKey::CallingTool),
        tool_completed: label_for(TraceLabelKey::ToolCompleted),
        tool_result: label_for(TraceLabelKey::ToolResult),
        tool_failed: label_for(TraceLabelKey::ToolFailed),
        turn_started: label_for(TraceLabelKey::TurnStarted),
        state_entered: label_for(TraceLabelKey::StateEntered),
        turn_completed: label_for(TraceLabelKey::TurnCompleted),
        session_completed: label_for(TraceLabelKey::SessionCompleted),
        unknown_error: label_for(TraceLabelKey::UnknownError),
    }
}

fn localized_trace_labels(i18n: I18nContext<Locale>) -> TraceLabels {
    build_trace_labels(|key| match key {
        TraceLabelKey::CallingTool => t_string!(i18n, trace.calling_tool).to_string(),
        TraceLabelKey::ToolCompleted => t_string!(i18n, trace.tool_completed).to_string(),
        TraceLabelKey::ToolResult => t_string!(i18n, trace.tool_result).to_string(),
        TraceLabelKey::ToolFailed => t_string!(i18n, trace.tool_failed).to_string(),
        TraceLabelKey::TurnStarted => t_string!(i18n, trace.turn_started).to_string(),
        TraceLabelKey::StateEntered => t_string!(i18n, trace.state_entered).to_string(),
        TraceLabelKey::TurnCompleted => t_string!(i18n, trace.turn_completed).to_string(),
        TraceLabelKey::SessionCompleted => t_string!(i18n, trace.session_completed).to_string(),
        TraceLabelKey::UnknownError => t_string!(i18n, trace.unknown_error).to_string(),
    })
}

fn legacy_tool_result(result: &ToolResult) -> AgentTraceToolResult {
    if result.success {
        let output = result
            .output
            .as_ref()
            .map(|output| Value::String(output.clone()))
            .or_else(|| result.metadata.clone())
            .unwrap_or(Value::Null);

        AgentTraceToolResult::Success { output }
    } else {
        AgentTraceToolResult::Error {
            error: result.error.clone().unwrap_or_default(),
            retryable: false,
        }
    }
}

fn legacy_tool_start_content(
    tool_id: &str,
    tool_name: &str,
    params: &Value,
    labels: &TraceLabels,
) -> String {
    present_agent_trace_event_with_labels_and_preset(
        &AgentTraceEvent::ToolCallStarted {
            iteration: 0,
            call: AgentTraceToolCallStart {
                tool_id: tool_id.to_string(),
                tool_name: tool_name.to_string(),
                input: params.clone(),
            },
        },
        &labels.as_presentation_labels(),
        AgentTracePresentationPreset::PanelTrace,
    )
    .map(|presentation| presentation.content)
    .unwrap_or_default()
}

fn legacy_tool_end_summary(
    tool_id: &str,
    duration_ms: u64,
    result: &ToolResult,
    labels: &TraceLabels,
) -> (String, TraceStatus) {
    let presentation = present_agent_trace_event_with_labels_and_preset(
        &AgentTraceEvent::ToolCallCompleted {
            iteration: 0,
            call: AgentTraceToolCallEnd {
                tool_id: tool_id.to_string(),
                tool_name: String::new(),
                input: Value::Null,
                duration_ms,
            },
            result: legacy_tool_result(result),
        },
        &labels.as_presentation_labels(),
        AgentTracePresentationPreset::PanelTrace,
    );

    match presentation {
        Some(presentation) => (
            presentation.content,
            match presentation.status {
                aleph_protocol::AgentTracePresentationStatus::InProgress => TraceStatus::InProgress,
                aleph_protocol::AgentTracePresentationStatus::Success => TraceStatus::Success,
                aleph_protocol::AgentTracePresentationStatus::Failed => TraceStatus::Failed,
            },
        ),
        None => (String::new(), TraceStatus::Pending),
    }
}

#[component]
pub fn AgentTrace() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let nodes = RwSignal::new(Vec::<TraceNode>::new());
    let is_active = RwSignal::new(true);
    let mode = RwSignal::new(TraceMode::Live);
    let replay_tasks = RwSignal::new(Vec::<AgentTraceTaskSummary>::new());
    let selected_task_id = RwSignal::new(Option::<String>::None);
    let replay_error = RwSignal::new(Option::<String>::None);
    let replay_loading = RwSignal::new(false);
    let replay_reload = RwSignal::new(0u64);

    let tool_start_times = StoredValue::new(Arc::new(Mutex::new(std::collections::HashMap::<
        String,
        f64,
    >::new())));
    let trace_runs = StoredValue::new(Arc::new(Mutex::new(HashSet::<String>::new())));

    Effect::new(move || {
        if state.is_connected.get() {
            let state = state;

            let _subscription_id = state.subscribe_events(move |event: GatewayEvent| {
                if !is_active.get() || mode.get() != TraceMode::Live {
                    return;
                }

                let topic = event.topic.as_str();
                let run_id = json_str(&event.data, "run_id");
                let trace_enabled = if run_id.is_empty() {
                    false
                } else {
                    let runs = trace_runs.get_value();
                    let contains = match runs.lock() {
                        Ok(set) => set.contains(&run_id),
                        Err(_) => false,
                    };
                    contains
                };

                let trace_labels = localized_trace_labels(i18n);

                let node = match topic {
                    "run.run_accepted" => {
                        let run_id = json_str(&event.data, "run_id");
                        let unknown = t_string!(i18n, trace.unknown).to_string();
                        let run_started = t_string!(i18n, trace.run_started).to_string();
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Decision,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!(
                                "{} ({})",
                                run_started,
                                if run_id.is_empty() { unknown } else { run_id }
                            ),
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
                    "run.agent_trace" => {
                        if !run_id.is_empty() {
                            let runs = trace_runs.get_value();
                            let _inserted = match runs.lock() {
                                Ok(mut set) => {
                                    set.insert(run_id.clone());
                                    true
                                }
                                Err(_) => false,
                            };
                        }

                        event
                            .data
                            .get("event")
                            .cloned()
                            .and_then(|value| serde_json::from_value::<AgentTraceEvent>(value).ok())
                            .and_then(|trace_event| {
                                trace_node_from_agent_trace_event(
                                    &trace_event,
                                    now_ms(),
                                    &trace_labels,
                                )
                            })
                    }
                    "run.reasoning_block" => {
                        if trace_enabled {
                            None
                        } else {
                            let label = json_str(&event.data, "label");
                            let content = json_str(&event.data, "content");
                            let display = if label.is_empty() {
                                content
                            } else {
                                format!("{}: {}", label, content)
                            };
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
                    }
                    "run.tool_start" => {
                        if trace_enabled {
                            None
                        } else {
                            let tool_name = json_str(&event.data, "tool_name");
                            let tool_id = json_str(&event.data, "tool_id");
                            let params = event
                                .data
                                .get("params")
                                .map(|p| serde_json::to_string(p).unwrap_or_default())
                                .unwrap_or_default();

                            // Record start time for duration calculation
                            if !tool_id.is_empty() {
                                let times = tool_start_times.get_value();
                                let inserted = match times.lock() {
                                    Ok(mut map) => {
                                        map.insert(tool_id.clone(), now_ms());
                                        true
                                    }
                                    Err(_) => false,
                                };
                                let _ = inserted;
                            }

                            let calling_tool = t_string!(i18n, trace.calling_tool).to_string();
                            let content = if params.is_empty() || params == "{}" {
                                format!("{} {}", calling_tool, tool_name)
                            } else {
                                // Truncate long params
                                let truncated = if params.len() > 200 {
                                    format!("{}...", &params[..200])
                                } else {
                                    params
                                };
                                format!("{} {} ({})", calling_tool, tool_name, truncated)
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
                    }
                    "run.tool_end" => {
                        if trace_enabled {
                            None
                        } else {
                            let tool_id = json_str(&event.data, "tool_id");
                            let duration_ms =
                                event.data.get("duration_ms").and_then(|v| v.as_u64());
                            let success = event
                                .data
                                .get("result")
                                .and_then(|r| r.get("success"))
                                .and_then(|s| s.as_bool())
                                .unwrap_or(true);
                            let output = event
                                .data
                                .get("result")
                                .and_then(|r| r.get("output"))
                                .and_then(|o| o.as_str())
                                .unwrap_or("");
                            let error = event
                                .data
                                .get("result")
                                .and_then(|r| r.get("error"))
                                .and_then(|e| e.as_str())
                                .unwrap_or("");

                            // Calculate duration from start time if not provided
                            let final_duration = duration_ms.or_else(|| {
                                if !tool_id.is_empty() {
                                    let times = tool_start_times.get_value();
                                    let removed = match times.lock() {
                                        Ok(mut map) => map
                                            .remove(&tool_id)
                                            .map(|start| (now_ms() - start) as u64),
                                        Err(_) => None,
                                    };
                                    removed
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
                                    t_string!(i18n, trace.tool_completed).to_string()
                                } else {
                                    format!("{} {}", t_string!(i18n, trace.tool_result), display)
                                }
                            } else {
                                let unknown_error =
                                    t_string!(i18n, trace.unknown_error).to_string();
                                format!(
                                    "{} {}",
                                    t_string!(i18n, trace.tool_failed),
                                    if error.is_empty() {
                                        &unknown_error
                                    } else {
                                        error
                                    }
                                )
                            };

                            Some(TraceNode {
                                id: next_node_id(),
                                node_type: TraceNodeType::ToolResult,
                                timestamp: now_ms(),
                                duration_ms: final_duration,
                                content,
                                status: if success {
                                    TraceStatus::Success
                                } else {
                                    TraceStatus::Failed
                                },
                                children: vec![],
                            })
                        }
                    }
                    "run.response_chunk" => {
                        if trace_enabled {
                            None
                        } else {
                            // Only show final response chunks to avoid flooding
                            let is_final = event
                                .data
                                .get("is_final")
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
                    }
                    "run.run_complete" => {
                        if trace_enabled {
                            None
                        } else {
                            let duration =
                                event.data.get("total_duration_ms").and_then(|v| v.as_u64());
                            let tool_calls = event
                                .data
                                .get("summary")
                                .and_then(|s| s.get("tool_calls"))
                                .and_then(|t| t.as_u64())
                                .unwrap_or(0);
                            let loops = event
                                .data
                                .get("summary")
                                .and_then(|s| s.get("loops"))
                                .and_then(|l| l.as_u64())
                                .unwrap_or(0);

                            let run_complete = t_string!(i18n, trace.run_complete).to_string();
                            Some(TraceNode {
                                id: next_node_id(),
                                node_type: TraceNodeType::Decision,
                                timestamp: now_ms(),
                                duration_ms: duration,
                                content: format!(
                                    "{} ({} tool calls, {} loops)",
                                    run_complete, tool_calls, loops
                                ),
                                status: TraceStatus::Success,
                                children: vec![],
                            })
                        }
                    }
                    "run.run_error" => {
                        let error = json_str(&event.data, "error");
                        let unknown_error = t_string!(i18n, trace.unknown_error).to_string();
                        let run_failed = t_string!(i18n, trace.run_failed).to_string();
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Decision,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!(
                                "{} {}",
                                run_failed,
                                if error.is_empty() {
                                    unknown_error
                                } else {
                                    error
                                }
                            ),
                            status: TraceStatus::Failed,
                            children: vec![],
                        })
                    }
                    "run.ask_user" => {
                        let question = json_str(&event.data, "question");
                        let asking_user = t_string!(i18n, trace.asking_user).to_string();
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Observation,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!("{} {}", asking_user, question),
                            status: TraceStatus::InProgress,
                            children: vec![],
                        })
                    }
                    "run.uncertainty_signal" => {
                        let uncertainty = json_str(&event.data, "uncertainty");
                        let uncertainty_label = t_string!(i18n, trace.uncertainty).to_string();
                        Some(TraceNode {
                            id: next_node_id(),
                            node_type: TraceNodeType::Observation,
                            timestamp: now_ms(),
                            duration_ms: None,
                            content: format!("{} {}", uncertainty_label, uncertainty),
                            status: TraceStatus::InProgress,
                            children: vec![],
                        })
                    }
                    _ => {
                        // Handle agent.* events (dispatched directly as "event" notifications)
                        if topic.starts_with("agent.") || topic.starts_with("run.") {
                            web_sys::console::log_1(
                                &format!("Trace event: {} - {:?}", topic, event.data).into(),
                            );
                        }
                        None
                    }
                };

                // Append node if created
                if let Some(node) = node {
                    nodes.update(|list| {
                        list.push(node);
                        if list.len() > 200 {
                            list.drain(0..list.len() - 200);
                        }
                    });
                }
            });

            // Subscribe to stream events on the Gateway
            leptos::task::spawn_local(async move {
                if let Err(e) = state.subscribe_topic("stream.*").await {
                    web_sys::console::error_1(
                        &format!("Failed to subscribe to stream events: {}", e).into(),
                    );
                }
            });
        }
    });

    Effect::new(move || {
        let connected = state.is_connected.get();
        let current_mode = mode.get();
        let _reload = replay_reload.get();

        if !connected || current_mode != TraceMode::Replay {
            return;
        }

        let state = state;
        leptos::task::spawn_local(async move {
            replay_loading.set(true);
            replay_error.set(None);

            match TraceApi::list(&state, Some(20)).await {
                Ok(tasks) => {
                    let next_selected = selected_task_id
                        .get_untracked()
                        .and_then(|current| {
                            tasks
                                .iter()
                                .find(|task| task.task_id == current)
                                .map(|task| task.task_id.clone())
                        })
                        .or_else(|| tasks.first().map(|task| task.task_id.clone()));

                    replay_tasks.set(tasks);
                    selected_task_id.set(next_selected);
                }
                Err(error) => {
                    replay_tasks.set(Vec::new());
                    selected_task_id.set(None);
                    replay_error.set(Some(error));
                }
            }

            replay_loading.set(false);
        });
    });

    Effect::new(move || {
        let connected = state.is_connected.get();
        let current_mode = mode.get();
        let _reload = replay_reload.get();
        let selected = selected_task_id.get();

        if !connected || current_mode != TraceMode::Replay {
            return;
        }

        let Some(task_id) = selected else {
            nodes.set(Vec::new());
            return;
        };

        let labels = localized_trace_labels(i18n);
        let state = state;

        leptos::task::spawn_local(async move {
            replay_loading.set(true);
            replay_error.set(None);

            match TraceApi::get(&state, &task_id).await {
                Ok(replay) => {
                    if mode.get_untracked() == TraceMode::Replay
                        && selected_task_id.get_untracked().as_deref() == Some(task_id.as_str())
                    {
                        nodes.set(trace_nodes_from_replay(&replay.traces, &labels));
                    }
                }
                Err(error) => {
                    if mode.get_untracked() == TraceMode::Replay
                        && selected_task_id.get_untracked().as_deref() == Some(task_id.as_str())
                    {
                        nodes.set(Vec::new());
                        replay_error.set(Some(error));
                    }
                }
            }

            replay_loading.set(false);
        });
    });

    view! {
        <div class="h-full flex flex-col">
            <header class="p-8 border-b border-border bg-surface-raised sticky top-0 z-10">
                <div class="max-w-7xl mx-auto flex items-center justify-between">
                    <div>
                        <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                            <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                            </svg>
                            {t!(i18n, trace.title)}
                        </h2>
                        <p class="text-text-secondary">{t!(i18n, trace.description)}</p>
                    </div>

                    <div class="flex items-center gap-3">
                        <div class="flex items-center rounded-lg border border-border overflow-hidden">
                            <button
                                on:click=move |_| {
                                    mode.set(TraceMode::Live);
                                    replay_error.set(None);
                                    nodes.set(Vec::new());
                                }
                                class=move || {
                                    if mode.get() == TraceMode::Live {
                                        "px-4 py-2 text-sm bg-primary text-white"
                                    } else {
                                        "px-4 py-2 text-sm bg-surface-sunken text-text-secondary hover:text-text-primary"
                                    }
                                }
                            >
                                {t!(i18n, trace.live_mode)}
                            </button>
                            <button
                                on:click=move |_| {
                                    mode.set(TraceMode::Replay);
                                    replay_reload.update(|value| *value += 1);
                                }
                                class=move || {
                                    if mode.get() == TraceMode::Replay {
                                        "px-4 py-2 text-sm bg-primary text-white"
                                    } else {
                                        "px-4 py-2 text-sm bg-surface-sunken text-text-secondary hover:text-text-primary"
                                    }
                                }
                            >
                                {t!(i18n, trace.replay_mode)}
                            </button>
                        </div>
                        {move || {
                            if mode.get() == TraceMode::Live {
                                view! {
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
                                                    {t!(i18n, trace.pause)}
                                                </div>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="flex items-center gap-2">
                                                    <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                        <polygon points="5 3 19 12 5 21 5 3" />
                                                    </svg>
                                                    {t!(i18n, trace.resume)}
                                                </div>
                                            }.into_any()
                                        }}
                                    </button>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="flex items-center gap-2">
                                        <label class="text-sm text-text-secondary">{t!(i18n, trace.replay_select_label)}</label>
                                        <select
                                            class="min-w-[28rem] px-3 py-2 rounded-lg bg-surface-sunken border border-border text-sm text-text-primary"
                                            prop:value=move || selected_task_id.get().unwrap_or_default()
                                            on:change=move |ev| {
                                                let value = event_target_value(&ev);
                                                if value.trim().is_empty() {
                                                    selected_task_id.set(None);
                                                } else {
                                                    selected_task_id.set(Some(value));
                                                }
                                            }
                                        >
                                            <option value="">{t!(i18n, trace.replay_empty)}</option>
                                            {move || replay_tasks.get().into_iter().map(|task| {
                                                let label = format_replay_task_label(&task);
                                                view! {
                                                    <option value={task.task_id.clone()}>{label}</option>
                                                }
                                            }).collect_view()}
                                        </select>
                                        <button
                                            on:click=move |_| replay_reload.update(|value| *value += 1)
                                            class="px-4 py-2 rounded-lg bg-surface-sunken hover:bg-surface-raised transition-colors border border-border hover:border-border-strong text-sm"
                                        >
                                            {t!(i18n, trace.replay_refresh)}
                                        </button>
                                    </div>
                                }.into_any()
                            }
                        }}
                        <button
                            on:click=move |_| {
                                nodes.set(Vec::new());
                                let times = tool_start_times.get_value();
                                if let Ok(mut map) = times.lock() {
                                    map.clear();
                                };
                            }
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
                                    <h3 class="text-warning font-semibold mb-1">{t!(i18n, dashboard.gateway_required)}</h3>
                                    <p class="text-sm text-text-secondary">{t!(i18n, trace.gateway_required_desc)}</p>
                                </div>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            <div class="flex-1 overflow-y-auto p-8">
                <div class="max-w-4xl mx-auto">
                    {move || {
                        let node_list = nodes.get();
                        if let Some(error) = replay_error.get() {
                            view! {
                                <div class="text-center py-16">
                                    <p class="text-danger">{error}</p>
                                </div>
                            }.into_any()
                        } else if replay_loading.get() && mode.get() == TraceMode::Replay {
                            view! {
                                <div class="text-center py-16">
                                    <p class="text-text-secondary">{t!(i18n, trace.replay_loading)}</p>
                                </div>
                            }.into_any()
                        } else if node_list.is_empty() {
                            view! {
                                <div class="text-center py-16">
                                    <div class="text-text-tertiary mb-2">
                                        <svg width="48" height="48" attr:class="w-12 h-12 mx-auto mb-4 opacity-50" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                            <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                                        </svg>
                                    </div>
                                    {move || {
                                        if mode.get() == TraceMode::Replay {
                                            view! {
                                                <p class="text-text-secondary">{t!(i18n, trace.replay_empty)}</p>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div>
                                                    <p class="text-text-secondary">{t!(i18n, trace.no_events)}</p>
                                                    <p class="text-sm text-text-tertiary mt-2">{t!(i18n, trace.events_hint)}</p>
                                                </div>
                                            }.into_any()
                                        }
                                    }}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_trace_labels_maps_fields_in_order() {
        let labels = build_trace_labels(|key| match key {
            TraceLabelKey::CallingTool => "calling".to_string(),
            TraceLabelKey::ToolCompleted => "completed".to_string(),
            TraceLabelKey::ToolResult => "result".to_string(),
            TraceLabelKey::ToolFailed => "failed".to_string(),
            TraceLabelKey::TurnStarted => "turn-start".to_string(),
            TraceLabelKey::StateEntered => "state".to_string(),
            TraceLabelKey::TurnCompleted => "turn-complete".to_string(),
            TraceLabelKey::SessionCompleted => "session-complete".to_string(),
            TraceLabelKey::UnknownError => "unknown".to_string(),
        });

        assert_eq!(labels.calling_tool, "calling");
        assert_eq!(labels.tool_completed, "completed");
        assert_eq!(labels.tool_result, "result");
        assert_eq!(labels.tool_failed, "failed");
        assert_eq!(labels.turn_started, "turn-start");
        assert_eq!(labels.state_entered, "state");
        assert_eq!(labels.turn_completed, "turn-complete");
        assert_eq!(labels.session_completed, "session-complete");
        assert_eq!(labels.unknown_error, "unknown");
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
                    let _: () = view! {};
                    ().into_any()
                }}
            </div>
        </div>
    }
}
