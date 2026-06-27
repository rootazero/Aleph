//! Phone Teams menu landing (`/teams`): a full-screen sections menu whose rows
//! mirror the desktop `TeamsSidebar` (team selector + Overview / Kanban / Plan /
//! Replay / Workers). Each row drills into a full-screen leaf. Mirrors the
//! `PhoneDashboardMenu` structure. I/O-only (R4): rows only navigate; the team
//! selector reuses the desktop component reading `TeamsTabState` from context.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::shell::PhoneShell;
use crate::views::teams::components::team_selector::TeamSelector;

#[component]
#[must_use]
pub fn PhoneTeamsMenu() -> impl IntoView {
    let navigate = use_navigate();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <PhoneShell title="Teams">
            <div>
                <div class="px-4 py-3">
                    <TeamSelector/>
                </div>
                <div class="list">
                    <div class="cell" on:click=go("/teams/overview")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <line x1="8" y1="6" x2="21" y2="6"></line>
                                <line x1="8" y1="12" x2="21" y2="12"></line>
                                <line x1="8" y1="18" x2="21" y2="18"></line>
                                <line x1="3" y1="6" x2="3.01" y2="6"></line>
                                <line x1="3" y1="12" x2="3.01" y2="12"></line>
                                <line x1="3" y1="18" x2="3.01" y2="18"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Overview"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/kanban")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                                <line x1="9" y1="3" x2="9" y2="21"></line>
                                <line x1="15" y1="3" x2="15" y2="21"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Kanban"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/plan")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <circle cx="18" cy="5" r="3"></circle>
                                <circle cx="6" cy="12" r="3"></circle>
                                <circle cx="18" cy="19" r="3"></circle>
                                <line x1="8.59" y1="13.51" x2="15.42" y2="17.49"></line>
                                <line x1="15.41" y1="6.51" x2="8.59" y2="10.49"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Plan"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/replay")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"></path>
                                <path d="M3 3v5h5"></path>
                                <path d="M12 7v5l4 2"></path>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Replay"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                    <div class="cell" on:click=go("/teams/workers")>
                        <span class="cell-leading">
                            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                                <rect x="4" y="4" width="16" height="16" rx="2"></rect>
                                <rect x="9" y="9" width="6" height="6"></rect>
                                <line x1="9" y1="2" x2="9" y2="4"></line>
                                <line x1="15" y1="2" x2="15" y2="4"></line>
                                <line x1="9" y1="20" x2="9" y2="22"></line>
                                <line x1="15" y1="20" x2="15" y2="22"></line>
                                <line x1="20" y1="9" x2="22" y2="9"></line>
                                <line x1="20" y1="14" x2="22" y2="14"></line>
                                <line x1="2" y1="9" x2="4" y2="9"></line>
                                <line x1="2" y1="14" x2="4" y2="14"></line>
                            </svg>
                        </span>
                        <div class="cell-body"><div class="cell-title">"Workers"</div></div>
                        <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                    </div>
                </div>
            </div>
        </PhoneShell>
    }
}
