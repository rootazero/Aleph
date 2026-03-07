use leptos::prelude::*;
use crate::components::ui::*;
use crate::context::DashboardState;
use crate::api::{MemoryApi, MemoryStats, SystemApi, SystemInfo};

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
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

#[component]
pub fn Home() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    // State for stats
    let memory_stats = RwSignal::new(None::<Option<MemoryStats>>); // Some(Some(stats)) = loaded, Some(None) = failed, None = not fetched
    let system_info = RwSignal::new(None::<SystemInfo>);
    let active_tasks = RwSignal::new(None::<u64>);
    let gateway_latency_ms = RwSignal::new(None::<u64>);
    let is_connecting = RwSignal::new(false);

    // Fetch stats when connected
    Effect::new(move || {
        if state.is_connected.get() {
            let state_clone = state.clone();
            leptos::task::spawn_local(async move {
                // Fetch memory stats
                match MemoryApi::stats(&state_clone).await {
                    Ok(stats) => memory_stats.set(Some(Some(stats))),
                    Err(_) => memory_stats.set(Some(None)),
                }

                // Fetch system info (includes CPU usage)
                if let Ok(info) = SystemApi::info(&state_clone).await {
                    system_info.set(Some(info));
                }

                // Measure gateway latency via health ping
                let start = js_sys::Date::now();
                if state_clone.rpc_call("health", serde_json::Value::Null).await.is_ok() {
                    let elapsed = (js_sys::Date::now() - start) as u64;
                    gateway_latency_ms.set(Some(elapsed));
                }

                // Fetch active task count
                match state_clone.rpc_call("services.list", serde_json::Value::Null).await {
                    Ok(result) => {
                        let count = result.get("services")
                            .and_then(|s| s.as_array())
                            .map(|arr| arr.iter()
                                .filter(|s| s.get("status").and_then(|v| v.as_str()) == Some("running"))
                                .count() as u64)
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
        let state = state.clone();
        leptos::task::spawn_local(async move {
            is_connecting.set(true);
            match state.connect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Successfully connected to gateway".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to connect: {}", e).into());
                }
            }
            is_connecting.set(false);
        });
    };

    let handle_disconnect = move |_| {
        let state = state.clone();
        leptos::task::spawn_local(async move {
            match state.disconnect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Successfully disconnected from gateway".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to disconnect: {}", e).into());
                }
            }
        });
    };

    let handle_reconnect = move |_| {
        let state = state.clone();
        leptos::task::spawn_local(async move {
            match state.reconnect().await {
                Ok(()) => {
                    web_sys::console::log_1(&"Successfully reconnected to gateway".into());
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to reconnect: {}", e).into());
                }
            }
        });
    };

    view! {
        <div class="p-8 max-w-7xl mx-auto space-y-12">
            // Header with connection controls
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2">"System Overview"</h2>
                    <p class="text-text-secondary">"Command center for your personal AI instance."</p>
                </div>

                <div class="flex gap-3">
                    {move || if state.is_connected.get() {
                        view! {
                            <Button
                                on:click=handle_disconnect
                                variant=ButtonVariant::Secondary
                            >
                                "Disconnect"
                            </Button>
                        }.into_any()
                    } else if state.is_reconnecting.get() {
                        view! {
                            <Button
                                variant=ButtonVariant::Primary
                                disabled=Signal::derive(|| true)
                            >
                                {move || format!("Reconnecting... ({})", state.reconnect_count.get() + 1)}
                            </Button>
                        }.into_any()
                    } else if state.connection_error.get().is_some() {
                        view! {
                            <>
                                <Button
                                    on:click=handle_reconnect
                                    variant=ButtonVariant::Secondary
                                >
                                    "Retry Connection"
                                </Button>
                                <Button
                                    on:click=handle_connect
                                    variant=ButtonVariant::Primary
                                    class=if is_connecting.get() { "opacity-80 pointer-events-none" } else { "" }.to_string()
                                >
                                    {move || if is_connecting.get() { "Connecting..." } else { "Connect to Gateway" }}
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
                                {move || if is_connecting.get() { "Connecting..." } else { "Connect to Gateway" }}
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
                            <strong>"Connection Error: "</strong> {error}
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
                                <h3 class="text-warning font-semibold mb-1">"Gateway Connection Required"</h3>
                                <p class="text-sm text-text-secondary">"Please connect to the Aleph Gateway to view real-time data."</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Stats Grid
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6 pt-8">
                <StatCard label="Active Tasks" value=Signal::derive(move || {
                    active_tasks.get()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "\u{2014}".to_string())
                }) icon_color="text-primary" icon_bg="bg-primary-subtle">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </StatCard>
                <StatCard label="CPU Usage" value=Signal::derive(move || {
                    system_info.get()
                        .map(|info| format!("{:.0}%", info.cpu_usage_percent))
                        .unwrap_or_else(|| "\u{2014}".to_string())
                }) icon_color="text-success" icon_bg="bg-success-subtle">
                    <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                    <rect x="9" y="9" width="6" height="6" />
                    <line x1="9" y1="1" x2="9" y2="4" />
                    <line x1="15" y1="1" x2="15" y2="4" />
                </StatCard>
                <StatCard label="Memory Vault" value=Signal::derive(move || {
                    match memory_stats.get() {
                        Some(Some(ref stats)) => format!("{} entries", stats.total_facts + stats.total_memories + stats.total_graph_nodes),
                        Some(None) => "\u{2014}".to_string(),
                        None => "\u{2014}".to_string(),
                    }
                }) icon_color="text-info" icon_bg="bg-info-subtle">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </StatCard>
                <StatCard label="Gateway Latency" value=Signal::derive(move || {
                    gateway_latency_ms.get()
                        .map(|ms| format!("{} ms", ms))
                        .unwrap_or_else(|| "\u{2014}".to_string())
                }) icon_color="text-warning" icon_bg="bg-warning-subtle">
                    <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                </StatCard>
            </div>

            // System Health + Recent Activity
            <div class="grid grid-cols-1 lg:grid-cols-2 gap-8 pt-4">
                // Left: Core Services + System Info
                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1 text-text-secondary">"Core Services"</h3>
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
                                            <span class="font-medium text-text-primary text-sm">"System Info"</span>
                                        </div>
                                        <div class="grid grid-cols-3 gap-4">
                                            <div>
                                                <div class="text-[9px] text-text-tertiary uppercase font-bold tracking-widest mb-1">"Version"</div>
                                                <div class="font-mono text-xs text-text-secondary">{info.version.clone()}</div>
                                            </div>
                                            <div>
                                                <div class="text-[9px] text-text-tertiary uppercase font-bold tracking-widest mb-1">"Platform"</div>
                                                <div class="font-mono text-xs text-text-secondary">{info.platform.clone()}</div>
                                            </div>
                                            <div>
                                                <div class="text-[9px] text-text-tertiary uppercase font-bold tracking-widest mb-1">"Uptime"</div>
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
                                            <span class="text-sm text-text-tertiary">"Connect to view system info"</span>
                                        </div>
                                    </Card>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>

                // Right: Resource Utilization
                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1 text-text-secondary">"Resource Utilization"</h3>
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
                                    <ResourceMetric label="CPU" value=cpu_value sub=cpu_sub color="bg-success" progress=cpu_progress>
                                        <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                                        <rect x="9" y="9" width="6" height="6" />
                                        <line x1="9" y1="1" x2="9" y2="4" />
                                        <line x1="15" y1="1" x2="15" y2="4" />
                                    </ResourceMetric>
                                    <ResourceMetric label="Memory" value=mem_value sub=mem_sub color="bg-primary" progress=mem_progress>
                                         <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                                    </ResourceMetric>
                                    <ResourceMetric label="Storage" value=disk_value sub=disk_sub color="bg-primary" progress=disk_progress>
                                         <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                                         <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                                    </ResourceMetric>
                                </Card>
                            }.into_any()
                        } else {
                            view! {
                                <Card class="p-8 space-y-8">
                                    <ResourceMetric label="CPU" value="--".to_string() sub="Connect to view".to_string() color="bg-surface-sunken" progress=0>
                                        <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                                        <rect x="9" y="9" width="6" height="6" />
                                        <line x1="9" y1="1" x2="9" y2="4" />
                                        <line x1="15" y1="1" x2="15" y2="4" />
                                    </ResourceMetric>
                                    <ResourceMetric label="Memory" value="--".to_string() sub="Connect to view".to_string() color="bg-surface-sunken" progress=0>
                                         <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                                    </ResourceMetric>
                                    <ResourceMetric label="Storage" value="--".to_string() sub="Connect to view".to_string() color="bg-surface-sunken" progress=0>
                                         <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                                         <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                                    </ResourceMetric>
                                </Card>
                            }.into_any()
                        }
                    }}
                </div>
            </div>

            // Recent Activity + Quick Actions
            <div class="grid grid-cols-1 lg:grid-cols-3 gap-8 pt-4">
                <div class="lg:col-span-2 space-y-6">
                    <h3 class="text-xl font-semibold px-1">"Recent Activity"</h3>
                    <div class="bg-surface-raised border border-border rounded-2xl overflow-hidden">
                        <div class="p-4 border-b border-border bg-surface-sunken">
                            <div class="flex items-center justify-between">
                                <span class="text-sm font-medium text-text-secondary">"Event Log"</span>
                                <button class="text-xs text-primary hover:text-primary-hover">"View All"</button>
                            </div>
                        </div>
                        <div class="p-8 text-center text-text-tertiary">
                            {move || {
                                if !state.is_connected.get() {
                                    view! { <p>"Connect to Gateway to view activity"</p> }.into_any()
                                } else {
                                    view! { <p>"No recent activity"</p> }.into_any()
                                }
                            }}
                        </div>
                    </div>
                </div>

                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1">"Quick Actions"</h3>
                    <div class="grid gap-3">
                        <QuickAction label="Restart Gateway">
                            <path d="M23 4v6h-6" />
                            <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
                        </QuickAction>
                        <QuickAction label="Clear Buffer">
                            <path d="M3 6h18" />
                            <path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
                            <path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
                        </QuickAction>
                        <QuickAction label="Export Memory">
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
    label: &'static str,
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
            <div class="text-sm font-medium text-text-secondary mb-1 group-hover:text-text-primary transition-colors">{label}</div>
            <div class="text-2xl font-bold tracking-tight">{move || value.get()}</div>
        </div>
    }
}

#[component]
fn QuickAction(
    label: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <button class="flex items-center justify-between p-4 rounded-xl bg-surface-raised border border-border hover:bg-surface-sunken hover:border-primary/30 transition-all group text-left w-full">
            <div class="flex items-center gap-3">
                <svg width="20" height="20" attr:class="w-5 h-5 text-text-tertiary group-hover:text-primary transition-colors" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {children()}
                </svg>
                <span class="text-sm font-medium text-text-secondary group-hover:text-text-primary transition-colors">{label}</span>
            </div>
            <div class="text-text-tertiary group-hover:translate-x-1 transition-transform">{"\u{2192}"}</div>
        </button>
    }
}

#[component]
fn ServiceCard(
    name: &'static str,
    status: RwSignal<&'static str>,
) -> impl IntoView {
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
