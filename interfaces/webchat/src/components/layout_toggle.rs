//! Workspace pane toggle — sits at the chat-surface top-right, beside the
//! NotificationCenter bell. Dimensions and palette mirror the bell so the
//! pair reads as a single chrome cluster; placement is owned by
//! `views/chat/view.rs` (this component just renders the affordance).

use crate::i18n::*;
use crate::state::layout::{LayoutMode, WorkspaceState};
use leptos::prelude::*;

#[component]
#[must_use]
pub fn LayoutToggle() -> impl IntoView {
    let i18n = use_i18n();

    // Allow the toggle to render gracefully when WorkspaceState was not
    // provided (e.g. in storybook-style component tests).
    let Some(workspace) = use_context::<WorkspaceState>() else {
        return ().into_any();
    };

    let label = move || match workspace.mode.get() {
        LayoutMode::ChatOnly => t_string!(i18n, layout_toggle.open_pane).to_string(),
        LayoutMode::Split => t_string!(i18n, layout_toggle.close_pane).to_string(),
    };
    let icon_class = move || match workspace.mode.get() {
        LayoutMode::ChatOnly => "",
        LayoutMode::Split => "text-primary",
    };

    view! {
        <button
            type="button"
            class="aleph-layout-toggle aleph-no-drag relative flex items-center justify-center
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
            <Show when=move || {
                workspace.mode.get() == LayoutMode::ChatOnly
                    && workspace.unseen_activity.get() > 0
            }>
                <span class="absolute top-0.5 right-0.5 h-2 w-2 rounded-full
                             bg-primary animate-pulse" />
            </Show>
        </button>
    }
    .into_any()
}
