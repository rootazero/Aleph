//! Workspace pane toggle — sits at the chat-surface top-right, beside the
//! `NotificationCenter` bell. Dimensions and palette mirror the bell so the
//! pair reads as a single chrome cluster; placement is owned by
//! `views/chat/view.rs` (this component just renders the affordance).

use crate::i18n::{t_string, use_i18n};
use crate::state::layout::{LayoutMode, WorkspaceState};
use crate::views::chat::state::ChatState;
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
    // Used only to name the run a manual collapse belongs to. `use_context`
    // (not `expect_context`) for the same reason as above: a storybook mount
    // without a chat simply mutes nothing.
    let chat = use_context::<ChatState>();

    // Closing the pane BY HAND is an answer about this turn: it mutes the
    // canvas auto-reveal for the run that is speaking. Opening it is not — a
    // stale mute would silence the next run's first canvas.
    let on_click = move |_| {
        if workspace.mode.get_untracked() == LayoutMode::Split {
            let run = chat.and_then(|c| c.active_run_id.get_untracked());
            workspace.collapse_by_user(run);
        } else {
            workspace.set_layout(LayoutMode::Split);
        }
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
            on:click=on_click
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
                    && workspace.unseen_artifacts.get() > 0
            }>
                // Surface the actual unseen-activity count (capped "9+") rather
                // than a bare presence dot — the count is already tracked in
                // `WorkspaceState::unseen_artifacts`; previously only its >0-ness
                // was consumed.
                <span class="absolute -top-1 -right-1 min-w-[14px] h-[14px] px-[3px]
                             rounded-full bg-primary text-white
                             text-[9px] font-semibold leading-[14px] text-center
                             animate-pulse">
                    {move || {
                        let n = workspace.unseen_artifacts.get();
                        if n > 9 { "9+".to_string() } else { n.to_string() }
                    }}
                </span>
            </Show>
        </button>
    }
    .into_any()
}
