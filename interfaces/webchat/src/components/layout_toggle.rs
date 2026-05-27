//! Compact button that toggles the chat / split workspace layout.
//!
//! Drops alongside the project menu and model picker in the composer so
//! it's reachable without leaving the composer's reach-zone. Reads /
//! writes `WorkspaceState.mode`; persistence is handled by `WorkspaceState`.

use crate::state::layout::{LayoutMode, WorkspaceState};
use leptos::prelude::*;

#[component]
pub fn LayoutToggle() -> impl IntoView {
    // Allow the toggle to render gracefully when WorkspaceState was not
    // provided (e.g. in storybook-style component tests).
    let Some(workspace) = use_context::<WorkspaceState>() else {
        return ().into_any();
    };

    let label = move || match workspace.mode.get() {
        LayoutMode::ChatOnly => "Open workspace pane",
        LayoutMode::Split => "Close workspace pane",
    };
    let icon_class = move || match workspace.mode.get() {
        LayoutMode::ChatOnly => "opacity-60",
        LayoutMode::Split => "text-primary",
    };

    view! {
        <button
            type="button"
            class="aleph-layout-toggle inline-flex items-center justify-center
                   h-7 px-2 rounded-md text-xs gap-1
                   text-text-tertiary hover:text-text-primary
                   hover:bg-surface-sunken transition-colors"
            title=label
            aria-label=label
            on:click=move |_| workspace.toggle_layout()
        >
            <svg xmlns="http://www.w3.org/2000/svg"
                 class=move || format!("w-3.5 h-3.5 {}", icon_class())
                 viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2"/>
                <line x1="15" y1="3" x2="15" y2="21"/>
            </svg>
        </button>
    }
    .into_any()
}
