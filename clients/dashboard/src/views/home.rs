use leptos::prelude::*;

#[component]
pub fn Home() -> impl IntoView {
    view! {
        <div class="p-8 max-w-7xl mx-auto space-y-12">
            // Header
            <header>
                <h2 class="text-3xl font-bold tracking-tight mb-2">"System Overview"</h2>
                <p class="text-slate-400">"Command center for your personal AI instance."</p>
            </header>

            // Stats Grid
            <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-6 pt-8">
                <StatCard label="Active Tasks" value="3" color="text-indigo-400">
                    <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
                </StatCard>
                <StatCard label="CPU Usage" value="12%" color="text-emerald-400">
                    <rect x="4" y="4" width="16" height="16" rx="2" ry="2" />
                    <rect x="9" y="9" width="6" height="6" />
                    <line x1="9" y1="1" x2="9" y2="4" />
                    <line x1="15" y1="1" x2="15" y2="4" />
                </StatCard>
                <StatCard label="Knowledge Base" value="1,248 facts" color="text-purple-400">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                </StatCard>
                <StatCard label="Gateway Latency" value="14ms" color="text-amber-400">
                    <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
                </StatCard>
            </div>

            // Main Section
            <div class="grid grid-cols-1 lg:grid-cols-3 gap-8 pt-12">
                <div class="lg:col-span-2 space-y-6">
                    <h3 class="text-xl font-semibold px-1">"Recent Activity"</h3>
                    <div class="bg-slate-900/40 border border-slate-800 rounded-2xl overflow-hidden backdrop-blur-sm shadow-glass">
                        <div class="p-4 border-b border-slate-800 bg-slate-800/20">
                            <div class="flex items-center justify-between">
                                <span class="text-sm font-medium text-slate-300">"Event Log"</span>
                                <button class="text-xs text-indigo-400 hover:text-indigo-300">"View All"</button>
                            </div>
                        </div>
                        <div class="p-2">
                             <ActivityItem time="2m ago" action="Tool Execution" target="shell_exec" status="Success" />
                             <ActivityItem time="15m ago" action="Memory Scan" target="local_files" status="Running" />
                             <ActivityItem time="1h ago" action="Agent Run" target="market_research" status="Success" />
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
    value: &'static str,
    color: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="bg-slate-900/40 border border-slate-800 p-6 rounded-2xl backdrop-blur-sm hover:border-slate-700 transition-colors group shadow-sm">
            <div class="flex items-start justify-between mb-4">
                <div class=format!("p-2 rounded-lg bg-slate-800/50 {}", color)>
                    <svg width="24" height="24" attr:class="w-6 h-6" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        {children()}
                    </svg>
                </div>
            </div>
            <div class="text-sm font-medium text-slate-400 mb-1 group-hover:text-slate-300 transition-colors">{label}</div>
            <div class="text-2xl font-bold tracking-tight">{value}</div>
        </div>
    }
}

#[component]
fn ActivityItem(
    time: &'static str,
    action: &'static str,
    target: &'static str,
    status: &'static str,
) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between p-3 hover:bg-slate-800/30 rounded-xl transition-colors">
            <div class="flex items-center gap-4">
                <div class="w-2 h-2 rounded-full bg-indigo-500 shadow-neon-indigo"></div>
                <div>
                    <div class="text-sm font-medium text-slate-200">{action}</div>
                    <div class="text-xs text-slate-500 font-mono">{target}</div>
                </div>
            </div>
            <div class="text-right">
                <div class="text-[10px] text-slate-500 font-mono mb-1">{time}</div>
                <div class=format!("text-[10px] px-1.5 py-0.5 rounded border uppercase tracking-wider font-bold {}",
                    if status == "Success" { "border-green-500/20 text-green-500 bg-green-500/5" }
                    else { "border-amber-500/20 text-amber-500 bg-amber-500/5" }
                )>
                    {status}
                </div>
            </div>
        </div>
    }
}

#[component]
fn QuickAction(
    label: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <button class="flex items-center justify-between p-4 rounded-xl bg-slate-900/40 border border-slate-800 hover:bg-slate-800/50 hover:border-indigo-500/30 hover:shadow-neon-indigo transition-all group text-left w-full">
            <div class="flex items-center gap-3">
                <svg width="20" height="20" attr:class="w-5 h-5 text-slate-400 group-hover:text-indigo-400 transition-colors" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    {children()}
                </svg>
                <span class="text-sm font-medium text-slate-300 group-hover:text-white transition-colors">{label}</span>
            </div>
            <div class="text-slate-600 group-hover:translate-x-1 transition-transform">"→"</div>
        </button>
    }
}