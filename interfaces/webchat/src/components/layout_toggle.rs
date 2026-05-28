//! Workspace pane toggle — sits at the chat-surface top-right, beside the
//! NotificationCenter bell. Dimensions and palette mirror the bell so the
//! pair reads as a single chrome cluster; placement is owned by
//! `views/chat/view.rs` (this component just renders the affordance).

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
        LayoutMode::ChatOnly => "",
        LayoutMode::Split => "text-primary",
    };

    view! {
        <button
            type="button"
            class="aleph-layout-toggle aleph-no-drag flex items-center justify-center
                   h-7 w-7 rounded-full
                   text-text-secondary hover:text-text-primary
                   hover:bg-surface-raised transition-colors"
            data-tauri-drag-region="false"
            title=label
            aria-label=label
            on:click=move |_| workspace.toggle_layout()
        >
            <svg xmlns="http://www.w3.org/2000/svg"
                 width="16" height="16"
                 class=icon_class
                 viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2"/>
                <line x1="15" y1="3" x2="15" y2="21"/>
            </svg>
        </button>
    }
    .into_any()
}
