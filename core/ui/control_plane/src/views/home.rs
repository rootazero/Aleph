use leptos::prelude::*;
use crate::context::DashboardState;
use crate::api::{MemoryApi, SystemApi};

#[component]
pub fn Home() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    // State for stats
    let memory_stats = RwSignal::new(None::<(u64, u64)>); // (count, size)
    let system_info = RwSignal::new(None::<String>); // version

    // Fetch stats when connected
    Effect::new(move || {
        if state.is_connected.get() {
            let state_clone = state.clone();
            leptos::task::spawn_local(async move {
                // Fetch memory stats
                if let Ok(stats) = MemoryApi::stats(&state_clone).await {
                    memory_stats.set(Some((stats.total_facts, stats.total_size)));
                }

                // Fetch system info
                if let Ok(info) = SystemApi::info(&state_clone).await {
                    system_info.set(Some(info.version));
                }

                // Note: The following stats are not yet available via Gateway RPC:
                // - Active Tasks: No task.list or task.stats RPC method exists
                // - CPU Usage: No system.metrics or system.resources RPC method exists
                // - Gateway Latency: Could be calculated from RPC round-trip time
                // These will show "—" until the corresponding RPC methods are implemented
            });
        } else {
            memory_stats.set(None);
            system_info.set(None);
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
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6 pt-8">
                <StatCard label="Active Tasks" value=Signal::derive(move || "—".to_string()) color="text-primary">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </StatCard>
                <StatCard label="CPU Usage" value=Signal::derive(move || "—".to_string()) color="text-success">
                    <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                    <rect x="9" y="9" width="6" height="6" />
                    <line x1="9" y1="1" x2="9" y2="4" />
                    <line x1="15" y1="1" x2="15" y2="4" />
                </StatCard>
                <StatCard label="Knowledge Base" value=Signal::derive(move || {
                    memory_stats.get()
                        .map(|(count, _)| format!("{} facts", count))
                        .unwrap_or_else(|| "Loading...".to_string())
                }) color="text-primary">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                </StatCard>
                <StatCard label="Gateway Latency" value=Signal::derive(move || "—".to_string()) color="text-warning">
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
    color: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="bg-surface-raised border border-border p-6 rounded-2xl hover:border-border-strong transition-colors group">
            <div class="flex items-start justify-between mb-4">
                <div class=format!("p-2 rounded-lg bg-surface-sunken {}", color)>
                    <svg width="24" height="24" attr:class="w-6 h-6" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
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