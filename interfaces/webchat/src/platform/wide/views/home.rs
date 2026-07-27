use crate::api::{MemoryApi, MemoryStats, SystemApi, SystemInfo};
use crate::components::ui::{Badge, BadgeVariant, Button, ButtonVariant, Card};
use crate::context::{DashboardState, GatewayEvent};
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;
use std::collections::VecDeque;
use wasm_bindgen::JsCast;

/// Maximum number of activity entries kept in the dashboard ring buffer.
/// Older entries roll off the bottom; UI never re-fetches history.
const ACTIVITY_BUFFER_CAP: usize = 30;

/// A single rendered row in the Recent Activity feed.
#[derive(Clone, Debug)]
struct ActivityEntry {
    topic: String,
    summary: String,
    severity: ActivitySeverity,
    ts_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivitySeverity {
    Info,
    Success,
    Warning,
    Danger,
}

/// Classify a topic into a dashboard-visible category. Returns `None`
/// for high-volume streaming chatter we never want on the dashboard
/// (per-chunk agent reasoning, tool start/update bursts, typing pings).
fn classify_topic(topic: &str) -> Option<ActivitySeverity> {
    match topic {
        "agent.response.chunk"
        | "agent.reasoning"
        | "agent.reasoning.block"
        | "agent.tool.start"
        | "agent.tool.update"
        // Live ASR deltas fire many times per second and would both spam the
        // feed and leak the user's spoken words into it.
        | "voice.transcribe.delta"
        | "channel.typing" => None,

        "agent.run.error" | "channel.error" | "approval.expired" => Some(ActivitySeverity::Danger),

        "approval.requested" | "pairing.requested" => Some(ActivitySeverity::Warning),

        "agent.run.complete" | "approval.resolved" | "pairing.completed" => {
            Some(ActivitySeverity::Success)
        }

        _ => Some(ActivitySeverity::Info),
    }
}

/// Produce a short, human-friendly summary line from a topic + payload.
fn summarize_event(topic: &str, data: &serde_json::Value) -> String {
    let pick = |keys: &[&str]| -> Option<String> {
        for k in keys {
            if let Some(s) = data.get(k).and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
        None
    };

    match topic {
        "session.lifecycle.changed" | "session.updated" => pick(&["session_key", "session_id"])
            .map(|s| format!("session {s}"))
            .unwrap_or_else(|| topic.to_string()),
        "channel.message" => pick(&["channel", "from", "user"])
            .map(|s| format!("message from {s}"))
            .unwrap_or_else(|| "channel message".to_string()),
        "channel.status" => pick(&["channel", "status"])
            .map(|s| format!("channel {s}"))
            .unwrap_or_else(|| "channel status".to_string()),
        "config.changed" => pick(&["section"])
            .map(|s| format!("config: {s}"))
            .unwrap_or_else(|| "config changed".to_string()),
        "agent.run.complete" => pick(&["run_id", "session_key"])
            .map(|s| format!("run {s} complete"))
            .unwrap_or_else(|| "run complete".to_string()),
        "agent.run.error" => pick(&["error", "message"])
            .map(|s| format!("run error: {s}"))
            .unwrap_or_else(|| "run error".to_string()),
        "approval.requested" => pick(&["tool", "scope"])
            .map(|s| format!("approval requested: {s}"))
            .unwrap_or_else(|| "approval requested".to_string()),
        "approval.resolved" => pick(&["decision", "tool"])
            .map(|s| format!("approval resolved: {s}"))
            .unwrap_or_else(|| "approval resolved".to_string()),
        "pairing.requested" => "device pairing requested".to_string(),
        "pairing.completed" => "device pairing completed".to_string(),
        "presence.joined" => pick(&["conn_id"])
            .map(|s| format!("client joined ({})", short_id(&s)))
            .unwrap_or_else(|| "client joined".to_string()),
        "presence.left" => pick(&["conn_id"])
            .map(|s| format!("client left ({})", short_id(&s)))
            .unwrap_or_else(|| "client left".to_string()),
        "acp.sessions.changed" => "ACP sessions updated".to_string(),
        _ => pick(&["message", "name", "title", "summary"]).unwrap_or_else(|| topic.to_string()),
    }
}

/// Truncate an identifier to its first 8 characters. UTF-8 safe — counts
/// characters, never byte offsets, so a multi-byte `conn_id` can't panic.
fn short_id(s: &str) -> String {
    s.chars().take(8).collect()
}

/// Format a wall-clock millisecond timestamp into a `HH:MM:SS` label
/// based on the browser's local timezone.
fn format_clock(ts_ms: f64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms));
    let hh = date.get_hours();
    let mm = date.get_minutes();
    let ss = date.get_seconds();
    format!("{hh:02}:{mm:02}:{ss:02}")
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1_073_741_824.0;
    const MB: f64 = 1_048_576.0;
    const KB: f64 = 1_024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else {
        format!("{:.0} KB", b / KB)
    }
}

/// "N entries" summary for the Memory Vault stat card.
///
/// Deliberately excludes `total_graph_nodes`: graph nodes ARE the notes (one
/// row per note — `get_graph_data` selects straight from `notes_index`), so
/// adding it to `total_facts` would double-count every note. It also used to
/// fold a real `None` ("could not count") into a fake `0`, which is the exact
/// dishonesty the dedicated Graph Nodes card avoids by rendering "—".
fn memory_vault_summary(stats: &MemoryStats) -> String {
    format!("{} entries", stats.total_facts + stats.total_memories)
}

#[component]
#[must_use]
pub fn Home() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State for stats
    let memory_stats = RwSignal::new(None::<Option<MemoryStats>>); // Some(Some(stats)) = loaded, Some(None) = failed, None = not fetched
    let system_info = RwSignal::new(None::<SystemInfo>);
    let active_tasks = RwSignal::new(None::<u64>);
    let gateway_latency_ms = RwSignal::new(None::<u64>);
    let is_connecting = RwSignal::new(false);

    // Recent activity ring buffer (newest first). Bounded at
    // `ACTIVITY_BUFFER_CAP` — older entries roll off.
    let activity_buffer = RwSignal::new(VecDeque::<ActivityEntry>::with_capacity(
        ACTIVITY_BUFFER_CAP,
    ));
    let activity_sub_id = StoredValue::new(None::<usize>);

    // Wire the gateway event stream into the activity buffer. Fires
    // exactly once per (re)connection so we don't double-subscribe.
    Effect::new(move || {
        if state.is_connected.get() {
            // Local event-handler registration (always safe to redo —
            // each handler index is independent).
            let sub_id = state.subscribe_events(move |event: GatewayEvent| {
                let Some(severity) = classify_topic(&event.topic) else {
                    return;
                };
                let entry = ActivityEntry {
                    topic: event.topic.clone(),
                    summary: summarize_event(&event.topic, &event.data),
                    severity,
                    ts_ms: js_sys::Date::now(),
                };
                activity_buffer.update(|buf| {
                    buf.push_front(entry);
                    while buf.len() > ACTIVITY_BUFFER_CAP {
                        buf.pop_back();
                    }
                });
            });
            activity_sub_id.set_value(Some(sub_id));

            // Server-side topic subscription. The bus already filters
            // by pattern, so `**` is the simplest single round-trip;
            // streaming chatter is dropped by `classify_topic`.
            let state_for_topic = state;
            leptos::task::spawn_local(async move {
                if let Err(e) = state_for_topic.subscribe_topic("**").await {
                    web_sys::console::warn_1(
                        &format!("Activity feed: subscribe failed: {e}").into(),
                    );
                }
            });
        } else {
            // Drop any prior subscriber so a future connect re-installs cleanly.
            if let Some(id) = activity_sub_id.get_value() {
                state.unsubscribe_events(id);
                activity_sub_id.set_value(None);
            }
            activity_buffer.update(std::collections::VecDeque::clear);
        }
    });

    // Fetch stats when connected
    Effect::new(move || {
        if state.is_connected.get() {
            let state_clone = state;
            leptos::task::spawn_local(async move {
                // Fetch memory stats
                match MemoryApi::stats(&state_clone, "main").await {
                    Ok(stats) => memory_stats.set(Some(Some(stats))),
                    Err(_) => memory_stats.set(Some(None)),
                }

                // Fetch system info (includes CPU usage)
                if let Ok(info) = SystemApi::info(&state_clone).await {
                    system_info.set(Some(info));
                }

                // Measure gateway latency via health ping
                let start = js_sys::Date::now();
                if state_clone
                    .rpc_call("health", serde_json::Value::Null)
                    .await
                    .is_ok()
                {
                    let elapsed = (js_sys::Date::now() - start) as u64;
                    gateway_latency_ms.set(Some(elapsed));
                }

                // Fetch active task count (agent runs + coordination tasks)
                match state_clone
                    .rpc_call("activity.stats", serde_json::Value::Null)
                    .await
                {
                    Ok(result) => {
                        let count = result
                            .get("active_total")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        active_tasks.set(Some(count));
                    }
                    Err(_) => active_tasks.set(Some(0)),
                }
            });
        } else {
            memory_stats.set(None);
            system_info.set(None);
            active_tasks.set(None);
            gateway_latency_ms.set(None);
        }
    });

    // Gateway status
    let gateway_status = RwSignal::new("Disconnected");
    Effect::new(move || {
        let status = if state.is_connected.get() {
            "Healthy"
        } else if state.connection_error.get().is_some() {
            "Degraded"
        } else {
            "Disconnected"
        };
        gateway_status.set(status);
    });

    // Connection handlers
    let handle_connect = move |_| {
        let state = state;
        leptos::task::spawn_local(async move {
            is_connecting.set(true);
            match state.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Successfully connected to gateway".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to connect: {e}").into());
                }
            }
            is_connecting.set(false);
        });
    };

    let handle_disconnect = move |_| {
        let state = state;
        leptos::task::spawn_local(async move {
            match state.disconnect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Successfully disconnected from gateway".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to disconnect: {e}").into());
                }
            }
        });
    };

    let handle_reconnect = move |_| {
        let state = state;
        leptos::task::spawn_local(async move {
            match state.reconnect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Successfully reconnected to gateway".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to reconnect: {e}").into());
                }
            }
        });
    };

    let handle_restart_gateway = move |_| {
        let state = state;
        leptos::task::spawn_local(async move {
            web_sys::console::log_1(&"Restarting gateway...".into());
            match state
                .rpc_call("daemon.shutdown", serde_json::Value::Null)
                .await
            {
                Ok(_) => {
                    web_sys::console::log_1(
                        &"Shutdown command sent, triggering reconnect...".into(),
                    );
                    leptos::task::spawn_local(async move {
                        let _ = state.reconnect().await;
                    });
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to restart gateway: {e}").into());
                }
            }
        });
    };

    let handle_clear_buffer = move |_ev: web_sys::MouseEvent| {
        let state = state;
        leptos::task::spawn_local(async move {
            web_sys::console::log_1(&"Clearing chat buffer...".into());
            match state.rpc_call("chat.clear", serde_json::Value::Null).await {
                Ok(_) => {
                    web_sys::console::log_1(&"Chat buffer cleared successfully".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to clear buffer: {e}").into());
                }
            }
        });
    };

    let handle_export_memory = move |_ev: web_sys::MouseEvent| {
        let state = state;
        leptos::task::spawn_local(async move {
            web_sys::console::log_1(&"Exporting memory...".into());
            match MemoryApi::list_facts(&state, "main", 1000, 0).await {
                Ok((facts, _total)) => {
                    let export_data = serde_json::json!({
                        "export_type": "memory_facts",
                        "exported_at": js_sys::Date::new_0().to_iso_string().as_string(),
                        "total_facts": facts.len(),
                        "facts": facts,
                    });

                    // Convert to JSON string
                    let json_str = match serde_json::to_string_pretty(&export_data) {
                        Ok(s) => s,
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("Failed to serialize memory: {e}").into(),
                            );
                            return;
                        }
                    };

                    // Create a blob and download link
                    let window = match web_sys::window() {
                        Some(w) => w,
                        None => return,
                    };
                    let document = match window.document() {
                        Some(d) => d,
                        None => return,
                    };
                    let blob = match web_sys::Blob::new_with_str_sequence(&js_sys::Array::of1(
                        &json_str.into(),
                    )) {
                        Ok(b) => b,
                        Err(_) => return,
                    };
                    let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap_or_default();
                    let link = match document.create_element("a") {
                        Ok(el) => match el.dyn_into::<web_sys::HtmlAnchorElement>() {
                            Ok(anchor) => anchor,
                            Err(_) => return,
                        },
                        Err(_) => return,
                    };

                    let timestamp = js_sys::Date::new_0()
                        .to_iso_string()
                        .as_string()
                        .unwrap_or_default()
                        .replace(":", "-");
                    link.set_href(&url);
                    link.set_download(&format!("aleph-memory-export-{timestamp}.json"));
                    let _ = document.body().map(|body| body.append_child(&link));
                    link.click();
                    let _ = document.body().map(|body| body.remove_child(&link));
                    let _ = web_sys::Url::revoke_object_url(&url);

                    web_sys::console::log_1(&"Memory exported successfully".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to export memory: {e}").into());
                }
            }
        });
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-7xl mx-auto space-y-12">
            // Header with connection controls
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2">{move || t_string!(i18n, dashboard.title).to_string()}</h2>
                    <p class="text-text-secondary">{move || t_string!(i18n, dashboard.description).to_string()}</p>
                </div>

                <div class="flex gap-3">
                    {move || if state.is_connected.get() {
                        view! {
                            <Button
                                on:click=handle_disconnect
                                variant=ButtonVariant::Secondary
                            >
                                {move || t_string!(i18n, dashboard.actions.disconnect).to_string()}
                            </Button>
                        }.into_any()
                    } else if state.is_reconnecting.get() {
                        view! {
                            <Button
                                variant=ButtonVariant::Primary
                                disabled=Signal::derive(|| true)
                            >
                                {move || format!("{} ({})", t_string!(i18n, dashboard.actions.reconnecting), state.reconnect_count.get() + 1)}
                            </Button>
                        }.into_any()
                    } else if state.connection_error.get().is_some() {
                        view! {
                            <>
                                <Button
                                    on:click=handle_reconnect
                                    variant=ButtonVariant::Secondary
                                >
                                    {move || t_string!(i18n, dashboard.actions.retry).to_string()}
                                </Button>
                                <Button
                                    on:click=handle_connect
                                    variant=ButtonVariant::Primary
                                    class=if is_connecting.get() { "opacity-80 pointer-events-none" } else { "" }.to_string()
                                >
                                    {move || if is_connecting.get() { t_string!(i18n, dashboard.actions.connecting).to_string() } else { t_string!(i18n, dashboard.actions.connect).to_string() }}
                                </Button>
                            </>
                        }.into_any()
                    } else {
                        view! {
                            <Button
                                on:click=handle_connect
                                variant=ButtonVariant::Primary
                                class=if is_connecting.get() { "opacity-80 pointer-events-none" } else { "" }.to_string()
                            >
                                {move || if is_connecting.get() { t_string!(i18n, dashboard.actions.connecting).to_string() } else { t_string!(i18n, dashboard.actions.connect).to_string() }}
                            </Button>
                        }.into_any()
                    }}
                </div>
            </header>

            // Connection error
            {move || {
                if let Some(error) = state.connection_error.get() {
                    view! {
                        <div class="bg-danger-subtle border border-danger/20 rounded-xl p-4 text-sm text-danger">
                            <strong>{move || t_string!(i18n, dashboard.connection_error).to_string()}</strong> {error}
                        </div>
                    }.into_any()
                } else if !state.is_connected.get() && state.connection_error.get().is_none() && !state.is_reconnecting.get() {
                    view! {
                        <div class="bg-warning-subtle border border-warning/20 rounded-xl p-6 flex items-start gap-4">
                            <svg width="24" height="24" attr:class="w-6 h-6 text-warning flex-shrink-0 mt-0.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                                <line x1="12" y1="9" x2="12" y2="13" />
                                <line x1="12" y1="17" x2="12.01" y2="17" />
                            </svg>
                            <div>
                                <h3 class="text-warning font-semibold mb-1">{move || t_string!(i18n, dashboard.gateway_required).to_string()}</h3>
                                <p class="text-sm text-text-secondary">{move || t_string!(i18n, dashboard.gateway_required_desc).to_string()}</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Stats Grid
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6 pt-8">
                <StatCard label=Signal::derive(move || t_string!(i18n, dashboard.stats.active_tasks).to_string()) value=Signal::derive(move || {
                    active_tasks.get()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "\u{2014}".to_string())
                }) icon_color="text-primary" icon_bg="bg-primary-subtle">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </StatCard>
                <StatCard label=Signal::derive(move || t_string!(i18n, dashboard.stats.cpu_usage).to_string()) value=Signal::derive(move || {
                    system_info.get()
                        .map(|info| format!("{:.0}%", info.cpu_usage_percent))
                        .unwrap_or_else(|| "\u{2014}".to_string())
                }) icon_color="text-success" icon_bg="bg-success-subtle">
                    <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                    <rect x="9" y="9" width="6" height="6" />
                    <line x1="9" y1="1" x2="9" y2="4" />
                    <line x1="15" y1="1" x2="15" y2="4" />
                </StatCard>
                <StatCard label=Signal::derive(move || t_string!(i18n, dashboard.stats.memory_vault).to_string()) value=Signal::derive(move || {
                    match memory_stats.get() {
                        Some(Some(ref stats)) => memory_vault_summary(stats),
                        Some(None) => "\u{2014}".to_string(),
                        None => "\u{2014}".to_string(),
                    }
                }) icon_color="text-info" icon_bg="bg-info-subtle">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </StatCard>
                <StatCard label=Signal::derive(move || t_string!(i18n, dashboard.stats.gateway_latency).to_string()) value=Signal::derive(move || {
                    gateway_latency_ms.get()
                        .map(|ms| format!("{ms} ms"))
                        .unwrap_or_else(|| "\u{2014}".to_string())
                }) icon_color="text-warning" icon_bg="bg-warning-subtle">
                    <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                </StatCard>
            </div>

            // System Health + Recent Activity
            <div class="grid grid-cols-1 lg:grid-cols-2 gap-8 pt-4">
                // Left: Core Services + System Info
                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1 text-text-secondary">{move || t_string!(i18n, dashboard.sections.core_services).to_string()}</h3>
                    <div class="space-y-4">
                        <ServiceCard
                            name="Gateway Engine"
                            status=gateway_status
                        />

                        // System info card
                        {move || {
                            if let Some(info) = system_info.get() {
                                view! {
                                    <Card class="p-5 space-y-3">
                                        <div class="flex items-center gap-3 mb-2">
                                            <div class="p-2 rounded-lg bg-surface-sunken">
                                                <svg width="16" height="16" attr:class="w-4 h-4 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                    <circle cx="12" cy="12" r="3" />
                                                    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
                                                </svg>
                                            </div>
                                            <span class="font-medium text-text-primary text-sm">{move || t_string!(i18n, dashboard.system_info.title).to_string()}</span>
                                        </div>
                                        <div class="grid grid-cols-3 gap-4">
                                            <div>
                                                <div class="text-[9px] text-text-tertiary uppercase font-bold tracking-widest mb-1">{move || t_string!(i18n, dashboard.system_info.version).to_string()}</div>
                                                <div class="font-mono text-xs text-text-secondary">{info.version.clone()}</div>
                                            </div>
                                            <div>
                                                <div class="text-[9px] text-text-tertiary uppercase font-bold tracking-widest mb-1">{move || t_string!(i18n, dashboard.system_info.platform).to_string()}</div>
                                                <div class="font-mono text-xs text-text-secondary">{info.platform.clone()}</div>
                                            </div>
                                            <div>
                                                <div class="text-[9px] text-text-tertiary uppercase font-bold tracking-widest mb-1">{move || t_string!(i18n, dashboard.system_info.uptime).to_string()}</div>
                                                <div class="font-mono text-xs text-text-secondary">{format_uptime(info.uptime_secs)}</div>
                                            </div>
                                        </div>
                                    </Card>
                                }.into_any()
                            } else {
                                view! {
                                    <Card class="p-5">
                                        <div class="flex items-center gap-3">
                                            <div class="p-2 rounded-lg bg-surface-sunken">
                                                <svg width="16" height="16" attr:class="w-4 h-4 text-text-tertiary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                    <circle cx="12" cy="12" r="3" />
                                                    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
                                                </svg>
                                            </div>
                                            <span class="text-sm text-text-tertiary">{move || t_string!(i18n, dashboard.connect_to_view).to_string()}</span>
                                        </div>
                                    </Card>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>

                // Right: Resource Utilization
                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1 text-text-secondary">{move || t_string!(i18n, dashboard.sections.resource_utilization).to_string()}</h3>
                    {move || {
                        if let Some(info) = system_info.get() {
                            let cpu_value = format!("{:.0}%", info.cpu_usage_percent);
                            let cpu_sub = format!("{} Cores", info.cpu_count);
                            let cpu_progress = info.cpu_usage_percent as u32;

                            let mem_value = format_bytes(info.memory_used_bytes);
                            let mem_sub = format!("of {} Total", format_bytes(info.memory_total_bytes));
                            let mem_progress = if info.memory_total_bytes > 0 {
                                ((info.memory_used_bytes as f64 / info.memory_total_bytes as f64) * 100.0) as u32
                            } else {
                                0
                            };

                            let disk_value = format_bytes(info.disk_used_bytes);
                            let disk_free_bytes = info.disk_total_bytes.saturating_sub(info.disk_used_bytes);
                            let disk_sub = format!("{} Free", format_bytes(disk_free_bytes));
                            let disk_progress = if info.disk_total_bytes > 0 {
                                ((info.disk_used_bytes as f64 / info.disk_total_bytes as f64) * 100.0) as u32
                            } else {
                                0
                            };

                            view! {
                                <Card class="p-8 space-y-8">
                                    <ResourceMetric label=t_string!(i18n, dashboard.resource.cpu) value=cpu_value sub=cpu_sub color="bg-success" progress=cpu_progress>
                                        <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                                        <rect x="9" y="9" width="6" height="6" />
                                        <line x1="9" y1="1" x2="9" y2="4" />
                                        <line x1="15" y1="1" x2="15" y2="4" />
                                    </ResourceMetric>
                                    <ResourceMetric label=t_string!(i18n, dashboard.resource.memory) value=mem_value sub=mem_sub color="bg-primary" progress=mem_progress>
                                         <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                                    </ResourceMetric>
                                    <ResourceMetric label=t_string!(i18n, dashboard.resource.storage) value=disk_value sub=disk_sub color="bg-primary" progress=disk_progress>
                                         <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                                         <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                                    </ResourceMetric>
                                </Card>
                            }.into_any()
                        } else {
                            let connect_to_view = t_string!(i18n, dashboard.resource.connect_to_view).to_string();
                            view! {
                                <Card class="p-8 space-y-8">
                                    <ResourceMetric label=t_string!(i18n, dashboard.resource.cpu) value="--".to_string() sub=connect_to_view.clone() color="bg-surface-sunken" progress=0>
                                        <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                                        <rect x="9" y="9" width="6" height="6" />
                                        <line x1="9" y1="1" x2="9" y2="4" />
                                        <line x1="15" y1="1" x2="15" y2="4" />
                                    </ResourceMetric>
                                    <ResourceMetric label=t_string!(i18n, dashboard.resource.memory) value="--".to_string() sub=connect_to_view.clone() color="bg-surface-sunken" progress=0>
                                         <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                                    </ResourceMetric>
                                    <ResourceMetric label=t_string!(i18n, dashboard.resource.storage) value="--".to_string() sub=connect_to_view.clone() color="bg-surface-sunken" progress=0>
                                         <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                                         <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                                    </ResourceMetric>
                                </Card>
                            }.into_any()
                        }
                    }}
                </div>
            </div>

            // Scheduled job sparkline — pulse of recent cron health
            <div class="pt-4">
                <h3 class="text-xl font-semibold px-1 mb-4">"Scheduled Activity"</h3>
                <crate::views::dashboard_cron::CronSparklines />
            </div>

            // Recent Activity + Quick Actions
            <div class="grid grid-cols-1 lg:grid-cols-3 gap-8 pt-4">
                <div class="lg:col-span-2 space-y-6">
                    <h3 class="text-xl font-semibold px-1">{move || t_string!(i18n, dashboard.sections.recent_activity).to_string()}</h3>
                    <div class="bg-surface-raised border border-border rounded-2xl overflow-hidden">
                        <div class="p-4 border-b border-border bg-surface-sunken">
                            <div class="flex items-center justify-between">
                                <span class="text-sm font-medium text-text-secondary">{move || t_string!(i18n, dashboard.activity.event_log).to_string()}</span>
                                <button class="text-xs text-primary hover:text-primary-hover">{move || t_string!(i18n, dashboard.activity.view_all).to_string()}</button>
                            </div>
                        </div>
                        <div class="max-h-96 overflow-y-auto">
                            {move || {
                                if !state.is_connected.get() {
                                    view! {
                                        <div class="p-8 text-center text-text-tertiary">
                                            <p>{move || t_string!(i18n, dashboard.connect_to_view_activity).to_string()}</p>
                                        </div>
                                    }.into_any()
                                } else {
                                    let buf = activity_buffer.get();
                                    if buf.is_empty() {
                                        view! {
                                            <div class="p-8 text-center text-text-tertiary">
                                                <p>{move || t_string!(i18n, dashboard.activity.no_recent).to_string()}</p>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <ul class="divide-y divide-border">
                                                {buf.into_iter().map(|entry| {
                                                    let dot_class = match entry.severity {
                                                        ActivitySeverity::Success => "bg-success",
                                                        ActivitySeverity::Warning => "bg-warning",
                                                        ActivitySeverity::Danger => "bg-danger",
                                                        ActivitySeverity::Info => "bg-primary/60",
                                                    };
                                                    let summary_title = entry.summary.clone();
                                                    view! {
                                                        <li class="flex items-center gap-3 px-4 py-2.5 hover:bg-surface-sunken transition-colors">
                                                            <span class=format!("w-2 h-2 rounded-full flex-shrink-0 {}", dot_class)></span>
                                                            <span class="flex-1 min-w-0 text-sm text-text-primary truncate" title=summary_title>{entry.summary}</span>
                                                            <span class="text-[10px] font-mono text-text-tertiary uppercase tracking-wider flex-shrink-0">{entry.topic}</span>
                                                            <span class="text-xs font-mono text-text-tertiary flex-shrink-0">{format_clock(entry.ts_ms)}</span>
                                                        </li>
                                                    }
                                                }).collect_view()}
                                            </ul>
                                        }.into_any()
                                    }
                                }
                            }}
                        </div>
                    </div>
                </div>

                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1">{move || t_string!(i18n, dashboard.sections.quick_actions).to_string()}</h3>
                    <div class="grid gap-3">
                        <QuickAction
                            label=Signal::derive(move || t_string!(i18n, dashboard.actions.restart_gateway).to_string())
                            on_click=Box::new(handle_restart_gateway)
                        >
                            <path d="M23 4v6h-6" />
                            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
                        </QuickAction>
                        <QuickAction
                            label=Signal::derive(move || t_string!(i18n, dashboard.actions.clear_buffer).to_string())
                            on_click=Box::new(handle_clear_buffer)
                        >
                            <path d="M3 6h18" />
                            <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
                            <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
                        </QuickAction>
                        <QuickAction
                            label=Signal::derive(move || t_string!(i18n, dashboard.actions.export_memory).to_string())
                            on_click=Box::new(handle_export_memory)
                        >
                            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                            <polyline points="7 10 12 15 17 10" />
                            <line x1="12" y1="15" x2="12" y2="3" />
                        </QuickAction>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn StatCard(
    label: Signal<String>,
    value: Signal<String>,
    icon_color: &'static str,
    icon_bg: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised border border-border p-6 rounded-2xl hover:border-border-strong hover:shadow-sm transition-all duration-200 group">
            <div class="flex items-start justify-between mb-4">
                <div class=format!("p-2.5 rounded-xl {} {}", icon_bg, icon_color)>
                    <svg width="24" height="24" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        {children()}
                    </svg>
                </div>
            </div>
            <div class="text-sm font-medium text-text-secondary mb-1 group-hover:text-text-primary transition-colors">{move || label.get()}</div>
            <div class="text-2xl font-bold tracking-tight">{move || value.get()}</div>
        </div>
    }
}

#[component]
fn QuickAction(
    label: Signal<String>,
    #[prop(optional)] on_click: Option<Box<dyn Fn(web_sys::MouseEvent) + 'static>>,
    children: Children,
) -> impl IntoView {
    view! {
        <button
            on:click=move |ev| {
                if let Some(ref handler) = on_click {
                    handler(ev);
                }
            }
            class="flex items-center justify-between p-4 rounded-xl bg-surface-raised border border-border hover:bg-surface-sunken hover:border-primary/30 transition-all group text-left w-full"
        >
            <div class="flex items-center gap-3">
                <svg width="20" height="20" attr:class="w-5 h-5 text-text-tertiary group-hover:text-primary transition-colors" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {children()}
                </svg>
                <span class="text-sm font-medium text-text-secondary group-hover:text-text-primary transition-colors">{move || label.get()}</span>
            </div>
            <div class="text-text-tertiary group-hover:translate-x-1 transition-transform">{"\u{2192}"}</div>
        </button>
    }
}

#[component]
fn ServiceCard(name: &'static str, status: RwSignal<&'static str>) -> impl IntoView {
    let badge_variant = move || match status.get() {
        "Healthy" => BadgeVariant::Emerald,
        "Degraded" => BadgeVariant::Amber,
        _ => BadgeVariant::Red,
    };

    view! {
        <div class="bg-surface-raised border border-border p-5 rounded-2xl flex items-center justify-between group hover:border-border-strong transition-all">
            <div class="flex items-center gap-4">
                <div class=move || format!("w-2.5 h-2.5 rounded-full transition-all duration-500 {}",
                    if status.get() == "Healthy" { "bg-success" }
                    else if status.get() == "Degraded" { "bg-warning" }
                    else { "bg-danger" }
                )></div>
                <div>
                    <div class="font-medium text-text-primary text-sm">{name}</div>
                </div>
            </div>
            <div class="w-24 text-right">
                {move || view! {
                    <Badge variant=badge_variant()>
                        {status.get()}
                    </Badge>
                }}
            </div>
        </div>
    }
}

#[component]
fn ResourceMetric(
    label: &'static str,
    value: String,
    sub: String,
    color: &'static str,
    progress: u32,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-6 group">
            <div class=format!("p-2.5 rounded-xl bg-surface-sunken text-white transition-transform group-hover:scale-110 {}", color)>
                <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {children()}
                </svg>
            </div>
            <div class="flex-1">
                <div class="flex items-center justify-between mb-1.5">
                    <span class="text-xs font-medium text-text-secondary group-hover:text-text-primary transition-colors">{label}</span>
                    <span class="text-base font-bold font-mono">{value}</span>
                </div>
                <div class="w-full h-1.5 bg-border rounded-full overflow-hidden">
                    <div class=format!("h-full rounded-full transition-all duration-1000 ease-out {}", color) style=format!("width: {}%", progress)></div>
                </div>
                <div class="mt-1.5 text-[9px] text-text-tertiary font-medium uppercase tracking-wider">{sub}</div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_vault_summary_excludes_graph_nodes() {
        // total_graph_nodes counts the same rows as total_facts (one row per
        // note); if it were re-added to the sum this assertion would fail
        // (would compute "25 entries" instead of "15 entries").
        let stats = MemoryStats {
            total_facts: 10,
            total_memories: 5,
            valid_facts: 10,
            total_graph_nodes: Some(10),
            total_graph_edges: Some(3),
            scope: "agent".to_string(),
        };
        assert_eq!(memory_vault_summary(&stats), "15 entries");
    }

    #[test]
    fn memory_vault_summary_handles_unknown_graph_count() {
        // A None graph count (store-wide scope) must not panic or need
        // unwrapping — the summary never touches that field at all.
        let stats = MemoryStats {
            total_facts: 3,
            total_memories: 7,
            valid_facts: 3,
            total_graph_nodes: None,
            total_graph_edges: None,
            scope: "global".to_string(),
        };
        assert_eq!(memory_vault_summary(&stats), "10 entries");
    }
}
