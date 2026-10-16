use crate::components::right_pane::SlotRenderCtx;
use leptos::prelude::*;

pub(super) fn render(_ctx: SlotRenderCtx) -> AnyView {
    view! { <crate::components::workspace_panel::WorkspaceBodyView /> }.into_any()
}
