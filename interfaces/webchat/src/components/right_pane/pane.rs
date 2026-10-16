//! App-shell host for the flex right pane.

use crate::components::workspace_panel::WorkspacePanel;
use leptos::prelude::*;

/// Render the existing workspace body in the shell's third flex column.
///
/// Keeping the body in [`WorkspacePanel`] preserves its state, resize
/// behavior, persistence and keep-alive semantics while the host moves out of
/// `ChatView`. Later stages can replace this forwarding body without changing
/// the shell contract.
#[component]
#[must_use]
pub fn RightPane() -> impl IntoView {
    view! { <WorkspacePanel legacy=false /> }
}
