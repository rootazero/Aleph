//! Phone More entry (`/more`): the 5th-tab landing — a full-screen sections
//! menu for the management modes that aren't primary phone tabs
//! (Dashboard / Teams / Extensions). Each row navigates into that mode; that
//! mode's own phone screen is a separate spec, so until then the target renders
//! the existing desktop layout. Mirrors the `PhoneSettings` landing structure.
//! I/O-only (R4): rows only navigate.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::platform::phone::shell::PhoneShell;

#[component]
#[must_use]
pub fn PhoneMore() -> impl IntoView {
    let navigate = use_navigate();
    // `use_navigate` returns a Clone-able Fn; each handler gets its own clone.
    let go = move |path: &'static str| {
        let navigate = navigate.clone();
        move |_| navigate(path, NavigateOptions::default())
    };

    view! {
        <PhoneShell title="More">
            <div class="list">
                <div class="cell" on:click=go("/dashboard")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <rect x="3" y="3" width="7" height="7"></rect>
                            <rect x="14" y="3" width="7" height="7"></rect>
                            <rect x="14" y="14" width="7" height="7"></rect>
                            <rect x="3" y="14" width="7" height="7"></rect>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Dashboard"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/teams")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"></path>
                            <circle cx="9" cy="7" r="4"></circle>
                            <path d="M23 21v-2a4 4 0 0 0-3-3.87"></path>
                            <path d="M16 3.13a4 4 0 0 1 0 7.75"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Teams"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
                <div class="cell" on:click=go("/extensions")>
                    <span class="cell-leading">
                        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M20.5 11H19V7a2 2 0 0 0-2-2h-4V3.5a2.5 2.5 0 0 0-5 0V5H4a2 2 0 0 0-2 2v3.8h1.5a2.2 2.2 0 1 1 0 4.4H2V19a2 2 0 0 0 2 2h3.8v-1.5a2.2 2.2 0 1 1 4.4 0V21H17a2 2 0 0 0 2-2v-4h1.5a2.5 2.5 0 0 0 0-5z"></path>
                        </svg>
                    </span>
                    <div class="cell-body"><div class="cell-title">"Extensions"</div></div>
                    <svg class="cell-chevron" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
                </div>
            </div>
        </PhoneShell>
    }
}
