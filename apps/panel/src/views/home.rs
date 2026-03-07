use leptos::prelude::*;
use crate::context::DashboardState;
use crate::api::{MemoryApi, MemoryStats, SystemApi, SystemInfo};

#[component]
pub fn Home() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    // State for stats
    let memory_stats = RwSignal::new(None::<Option<MemoryStats>>); // Some(Some(stats)) = loaded, Some(None) = failed, None = not fetched
    let system_info = RwSignal::new(None::<SystemInfo>);
    let active_tasks = RwSignal::new(None::<u64>);
    let gateway_latency_ms = RwSignal::new(None::<u64>);

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

    view! {
        <div class="p-8 max-w-7xl mx-auto space-y-12">
            // Header
            <header>
                <h2 class="text-3xl font-bold tracking-tight mb-2">"System Overview"</h2>
                <p class="text-text-secondary">"Command center for your personal AI instance."</p>
            </header>

            // Connection warning
            {move || {
                if !state.is_connected.get() {
                    view! {
                        <div class="bg-warning-subtle border border-warning/20 rounded-xl p-6 flex items-start gap-4">
                            <svg width="24" height="24" attr:class="w-6 h-6 text-warning flex-shrink-0 mt-0.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                                <line x1="12" y1="9" x2="12" y2="13" />
                                <line x1="12" y1="17" x2="12.01" y2="17" />
                            </svg>
                            <div>
                                <h3 class="text-warning font-semibold mb-1">"Gateway Connection Required"</h3>
                                <p class="text-sm text-text-secondary">"Please connect to the Aleph Gateway from the System Status page to view real-time data."</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Stats Grid
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-5 gap-6 pt-8">
                <StatCard label="Active Tasks" value=Signal::derive(move || {
                    active_tasks.get()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "—".to_string())
                }) icon_color="text-primary" icon_bg="bg-primary-subtle">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </StatCard>
                <StatCard label="CPU Usage" value=Signal::derive(move || {
                    system_info.get()
                        .map(|info| format!("{:.0}%", info.cpu_usage_percent))
                        .unwrap_or_else(|| "—".to_string())
                }) icon_color="text-success" icon_bg="bg-success-subtle">
                    <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                    <rect x="9" y="9" width="6" height="6" />
                    <line x1="9" y1="1" x2="9" y2="4" />
                    <line x1="15" y1="1" x2="15" y2="4" />
                </StatCard>
                <StatCard label="Raw Memories" value=Signal::derive(move || {
                    match memory_stats.get() {
                        Some(Some(ref stats)) => format!("{}", stats.total_memories),
                        Some(None) => "—".to_string(),
                        None => "—".to_string(),
                    }
                }) icon_color="text-info" icon_bg="bg-info-subtle">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </StatCard>
                <StatCard label="Knowledge Base" value=Signal::derive(move || {
                    match memory_stats.get() {
                        Some(Some(ref stats)) => format!("{} facts", stats.total_facts),
                        Some(None) => "—".to_string(),
                        None => "—".to_string(),
                    }
                }) icon_color="text-accent" icon_bg="bg-accent-subtle">
                    <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
                </StatCard>
                <StatCard label="Gateway Latency" value=Signal::derive(move || {
                    gateway_latency_ms.get()
                        .map(|ms| format!("{} ms", ms))
                        .unwrap_or_else(|| "—".to_string())
                }) icon_color="text-warning" icon_bg="bg-warning-subtle">
                    <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                </StatCard>
            </div>

            // Main Section
            <div class="grid grid-cols-1 lg:grid-cols-3 gap-8 pt-12">
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
            <div class="text-text-tertiary group-hover:translate-x-1 transition-transform">"→"</div>
        </button>
    }
}