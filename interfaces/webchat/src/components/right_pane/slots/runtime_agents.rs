use crate::components::right_pane::SlotRenderCtx;
use leptos::prelude::*;

pub(super) fn render(_ctx: SlotRenderCtx) -> AnyView {
    view! { <crate::components::sidebar::agent_panel::AgentPanel /> }.into_any()
}
