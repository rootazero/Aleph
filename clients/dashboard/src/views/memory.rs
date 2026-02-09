use leptos::prelude::*;
use crate::components::ui::*;

#[component]
pub fn Memory() -> impl IntoView {
    view! {
        <div class="p-8 max-w-7xl mx-auto space-y-8">
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-slate-100">
                        <svg width="32" height="32" attr:class="w-8 h-8 text-purple-500" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <ellipse cx="12" cy="5" rx="9" ry="3" />
                            <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                            <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                        </svg>
                        "Memory Vault"
                    </h2>
                    <p class="text-slate-400">"Browse and manage Agent's long-term memory and facts."</p>
                </div>
                
                <div class="flex items-center gap-3">
                    <div class="relative group">
                        <svg width="16" height="16" attr:class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-slate-500 group-focus-within:text-indigo-400 transition-colors" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <circle cx="11" cy="11" r="8" />
                            <line x1="21" y1="21" x2="16.65" y2="16.65" />
                        </svg>
                        <input 
                            type="text" 
                            placeholder="Search facts..."
                            class="pl-10 pr-4 py-2 bg-slate-900/40 border border-slate-800 rounded-xl focus:outline-none focus:border-indigo-500/50 focus:ring-4 focus:ring-indigo-500/10 w-64 transition-all text-sm text-slate-200 placeholder:text-slate-600 shadow-sm"
                        />
                    </div>
                    <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm class="p-2 h-auto rounded-xl".to_string()>
                        <svg width="20" height="20" attr:class="w-5 h-5 text-slate-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3" />
                        </svg>
                    </Button>
                </div>
            </header>

            // Memory Stats
            <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
                 <Card class="bg-indigo-500/5 border-indigo-500/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-indigo-400 uppercase tracking-widest mb-1.5">"Total Facts"</span>
                    <span class="text-3xl font-bold font-mono">"1,248"</span>
                 </Card>
                 <Card class="bg-emerald-500/5 border-emerald-500/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-emerald-400 uppercase tracking-widest mb-1.5">"Vector Size"</span>
                    <span class="text-3xl font-bold font-mono">"42.8 MB"</span>
                 </Card>
                 <Card class="bg-purple-500/5 border-purple-500/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-purple-400 uppercase tracking-widest mb-1.5">"Active Sources"</span>
                    <span class="text-3xl font-bold font-mono">"12"</span>
                 </Card>
            </div>

            // Facts List
            <Card class="overflow-hidden".to_string()>
                <table class="w-full text-left border-collapse">
                    <thead>
                        <tr class="bg-slate-800/20 text-[10px] font-bold text-slate-500 uppercase tracking-widest">
                            <th class="p-4 pl-8">"Fact Content"</th>
                            <th class="p-4">"Source"</th>
                            <th class="p-4">"Date"</th>
                            <th class="p-4 pr-8 text-right">"Actions"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-slate-800/50">
                        <MemoryRow content="User prefers TypeScript over JavaScript for all web projects." source="Chat Session #42" date="2026-02-08" />
                        <MemoryRow content="The product launch is scheduled for early March 2026." source="EMail Analysis" date="2026-02-07" />
                        <MemoryRow content="Aleph architecture uses a decentralized gateway pattern." source="Core Docs" date="2026-02-06" />
                        <MemoryRow content="Favorite color palette is deep slate with indigo accents." source="System Prefs" date="2026-02-05" />
                        <MemoryRow content="Key stakeholder for the Aether project is Dr. Aris." source="Meeting Notes" date="2026-02-04" />
                    </tbody>
                </table>
            </Card>
        </div>
    }
}

#[component]
fn MemoryRow(
    content: &'static str,
    source: &'static str,
    date: &'static str,
) -> impl IntoView {
    view! {
        <tr class="group hover:bg-slate-800/20 transition-colors">
            <td class="p-4 pl-8">
                <div class="text-sm font-medium text-slate-200 line-clamp-1 group-hover:line-clamp-none transition-all">{content}</div>
            </td>
            <td class="p-4">
                <Badge variant=BadgeVariant::Slate>
                    {source}
                </Badge>
            </td>
            <td class="p-4">
                <div class="flex items-center gap-2 text-xs text-slate-500 font-mono">
                    <svg width="12" height="12" attr:class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
                        <line x1="16" y1="2" x2="16" y2="6" />
                        <line x1="8" y1="2" x2="8" y2="6" />
                        <line x1="3" y1="10" x2="21" y2="10" />
                    </svg>
                    {date}
                </div>
            </td>
            <td class="p-4 pr-8 text-right">
                <div class="flex items-center justify-end gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                    <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm class="p-1.5 h-auto".to_string()>
                        <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
                            <polyline points="15 3 21 3 21 9" />
                            <line x1="10" y1="14" x2="21" y2="3" />
                        </svg>
                    </Button>
                    <Button variant=ButtonVariant::Destructive size=ButtonSize::Sm class="p-1.5 h-auto".to_string()>
                        <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="3 6 5 6 21 6" />
                            <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                        </svg>
                    </Button>
                </div>
            </td>
        </tr>
    }
}