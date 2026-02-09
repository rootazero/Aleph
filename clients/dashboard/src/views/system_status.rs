use leptos::prelude::*;
use crate::components::ui::*;
use crate::context::DashboardState;

#[component]
pub fn SystemStatus() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    let is_connecting = RwSignal::new(false);

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

    // Determine connection status text
    let gateway_status = RwSignal::new("Disconnected");

    // Update gateway status when connection state changes
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

    view! {
        <div class="p-8 max-w-7xl mx-auto space-y-12">
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-slate-100">
                        <svg width="32" height="32" attr:class="w-8 h-8 text-emerald-500" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                            <rect x="9" y="9" width="6" height="6" />
                            <line x1="9" y1="1" x2="9" y2="4" />
                            <line x1="15" y1="1" x2="15" y2="4" />
                            <line x1="9" y1="20" x2="9" y2="23" />
                            <line x1="15" y1="20" x2="15" y2="23" />
                            <line x1="20" y1="9" x2="23" y2="9" />
                            <line x1="20" y1="15" x2="23" y2="15" />
                            <line x1="1" y1="9" x2="4" y2="9" />
                            <line x1="1" y1="15" x2="4" y2="15" />
                        </svg>
                        "System Health"
                    </h2>
                    <p class="text-slate-400">"Real-time monitoring of Aleph Core and Infrastructure."</p>
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

            // Show connection error if any
            {move || {
                if let Some(error) = state.connection_error.get() {
                    view! {
                        <div class="bg-red-500/10 border border-red-500/20 rounded-xl p-4 text-sm text-red-400">
                            <strong>"Connection Error: "</strong> {error}
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            <div class="grid grid-cols-1 lg:grid-cols-2 gap-8">
                // Core Services
                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1 text-slate-300">"Core Services"</h3>
                    <div class="space-y-4">
                        <ServiceCard
                            name="Gateway Engine"
                            status=gateway_status
                            uptime="12d 4h"
                            latency="14ms"
                        />
                        <ServiceCard
                            name="Agent Runtime"
                            status=gateway_status
                            uptime="12d 4h"
                            latency="2ms"
                        />
                        <ServiceCard
                            name="Memory Vector DB"
                            status=RwSignal::new("Degraded")
                            uptime="5h 12m"
                            latency="145ms"
                        />
                        <ServiceCard
                            name="MCP Tool Server"
                            status=RwSignal::new("Healthy")
                            uptime="4d 18h"
                            latency="45ms"
                        />
                    </div>
                </div>

                // Resource Usage
                <div class="space-y-6">
                    <h3 class="text-xl font-semibold px-1 text-slate-300">"Resource Utilization"</h3>
                    <Card class="p-8 space-y-8">
                        <ResourceMetric label="CPU Clusters" value="24%" sub="16 Cores Active" color="bg-emerald-500" progress=24>
                            <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                            <rect x="9" y="9" width="6" height="6" />
                            <line x1="9" y1="1" x2="9" y2="4" />
                            <line x1="15" y1="1" x2="15" y2="4" />
                        </ResourceMetric>
                        <ResourceMetric label="Neural Memory" value="4.2 GB" sub="Total 16 GB Allocated" color="bg-indigo-500" progress=26>
                             <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                        </ResourceMetric>
                        <ResourceMetric label="Encrypted Storage" value="128 GB" sub="842 GB Remaining" color="bg-purple-500" progress=15>
                             <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                             <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                        </ResourceMetric>
                        <ResourceMetric label="Security Layer" value="Enabled" sub="All Guards Active" color="bg-blue-500" progress=100>
                             <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
                        </ResourceMetric>
                    </Card>
                </div>
            </div>
        </div>
    }
}

#[component]
fn ServiceCard(
    name: &'static str,
    status: RwSignal<&'static str>,
    uptime: &'static str,
    latency: &'static str,
) -> impl IntoView {
    let badge_variant = move || match status.get() {
        "Healthy" => BadgeVariant::Emerald,
        "Degraded" => BadgeVariant::Amber,
        _ => BadgeVariant::Red,
    };

    view! {
        <div class="bg-slate-900/40 border border-slate-800 p-5 rounded-2xl flex items-center justify-between group hover:border-slate-700 transition-all hover:bg-slate-800/20 shadow-sm hover:shadow-indigo-500/5">
            <div class="flex items-center gap-4">
                <div class=move || format!("w-2.5 h-2.5 rounded-full transition-all duration-500 shadow-[0_0_12px] {}", 
                    if status.get() == "Healthy" { "bg-emerald-500 shadow-emerald-500/60" } 
                    else if status.get() == "Degraded" { "bg-amber-500 shadow-amber-500/60" }
                    else { "bg-red-500 shadow-red-500/60" }
                )></div>
                <div>
                    <div class="font-medium text-slate-200 text-sm">{name}</div>
                    <div class="text-[10px] text-slate-500 font-mono uppercase tracking-tight">{uptime} " uptime"</div>
                </div>
            </div>
            <div class="flex items-center gap-6">
                <div class="text-right">
                    <div class="text-[9px] text-slate-500 uppercase font-bold tracking-widest mb-0.5">"Latency"</div>
                    <div class="font-mono text-xs text-slate-300">{latency}</div>
                </div>
                <div class="w-px h-8 bg-slate-800"></div>
                <div class="w-24 text-right">
                    <Badge variant=badge_variant()>
                        {move || status.get()}
                    </Badge>
                </div>
            </div>
        </div>
    }
}

#[component]
fn ResourceMetric(
    label: &'static str,
    value: &'static str,
    sub: &'static str,
    color: &'static str,
    progress: u32,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-6 group">
            <div class=format!("p-2.5 rounded-xl bg-slate-800/50 text-white transition-transform group-hover:scale-110 {}", color)>
                <svg width="20" height="20" attr:class="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {children()}
                </svg>
            </div>
            <div class="flex-1">
                <div class="flex items-center justify-between mb-1.5">
                    <span class="text-xs font-medium text-slate-400 group-hover:text-slate-200 transition-colors">{label}</span>
                    <span class="text-base font-bold font-mono">{value}</span>
                </div>
                <div class="w-full h-1.5 bg-slate-800 rounded-full overflow-hidden">
                    <div class=format!("h-full rounded-full transition-all duration-1000 ease-out {}", color) style=format!("width: {}%", progress)></div>
                </div>
                <div class="mt-1.5 text-[9px] text-slate-500 font-medium uppercase tracking-wider">{sub}</div>
            </div>
        </div>
    }
}