//! Phone Memory menu landing (`/memory`): mirrors the desktop `MemorySidebar`
//! as a full-screen list — an inline-expandable agent selector plus two drill
//! rows (Graph → /memory/graph, List → /memory/list). Search lives in the List
//! screen, Fold in the Graph screen; the menu stays a clean hub. Pure I/O (R4):
//! reads/writes `MemoryState` only; `mem.agents` is populated by the PhoneMemory
//! router's agent-bootstrap Effect.

use crate::i18n::t;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;

#[component]
#[must_use]
pub fn PhoneMemoryMenu() -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let mem = expect_context::<MemoryState>();
    let navigate = use_navigate();
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };
    // Inline agent-picker expansion (replaces the desktop popover).
    let agent_open = RwSignal::new(false);

    // Current agent label: "emoji name" | "name" | id.
    let current_label = move || {
        let id = mem.agent_id.get();
        mem.agents
            .get()
            .iter()
            .find(|a| a.id == id)
            .map(|a| {
                a.name
                    .as_deref()
                    .map(|n| match a.emoji.as_deref() {
                        Some(e) => format!("{e} {n}"),
                        None => n.to_string(),
                    })
                    .unwrap_or_else(|| a.id.clone())
            })
            .unwrap_or(id)
    };

    view! {
        <PhoneShell title="Memory">
        // ── Agent group ──
        <div>
            <div class="list-header">"Agent"</div>
            <div class="list">
                <div class="cell" on:click=move |_| agent_open.update(|v| *v = !*v)>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="8" r="4"></circle><path d="M5 21a7 7 0 0 1 14 0"></path></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">{current_label}</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg>
                </div>
                // Dynamic agent list lives INSIDE the .list div (a plain DOM
                // element), so mixing it with the static cell above is safe.
                {move || agent_open.get().then(|| {
                    let cur = mem.agent_id.get();
                    mem.agents.get().into_iter().map(|a| {
                        let id_for_click = a.id.clone();
                        let is_selected = a.id == cur;
                        let label = a.name.as_deref()
                            .map(|n| match a.emoji.as_deref() {
                                Some(e) => format!("{e} {n}"),
                                None => n.to_string(),
                            })
                            .unwrap_or_else(|| a.id.clone());
                        view! {
                            <div class="cell" on:click=move |_| { agent_open.set(false); mem.agent_id.set(id_for_click.clone()); }>
                                <div class="cell-body">
                                    <div class="cell-title" style=if is_selected { "color:var(--color-primary);" } else { "" }>{label}</div>
                                </div>
                                {is_selected.then(|| view! {
                                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" style="color:var(--color-primary);"><polyline points="20 6 9 17 4 12"></polyline></svg>
                                })}
                            </div>
                        }
                    }).collect_view()
                })}
            </div>
        </div>

        // ── Views group ──
        <div style="margin-top:20px;">
            <div class="list-header">{t!(i18n, memory.phone_view)}</div>
            <div class="list">
                <div class="cell" on:click=go("/memory/graph")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="5" cy="6" r="2"></circle><circle cx="19" cy="6" r="2"></circle><circle cx="12" cy="18" r="2"></circle><line x1="6.6" y1="7.4" x2="10.6" y2="16.4"></line><line x1="17.4" y1="7.4" x2="13.4" y2="16.4"></line><line x1="7" y1="6" x2="17" y2="6"></line></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">{t!(i18n, memory.phone_graph)}</div><div class="cell-sub">{t!(i18n, memory.phone_graph_sub)}</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/memory/list")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="8" y1="6" x2="21" y2="6"></line><line x1="8" y1="12" x2="21" y2="12"></line><line x1="8" y1="18" x2="21" y2="18"></line><line x1="3" y1="6" x2="3.01" y2="6"></line><line x1="3" y1="12" x2="3.01" y2="12"></line><line x1="3" y1="18" x2="3.01" y2="18"></line></svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">{t!(i18n, memory.phone_list)}</div><div class="cell-sub">{t!(i18n, memory.phone_list_sub)}</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
            </div>
        </div>
        </PhoneShell>
    }
}
