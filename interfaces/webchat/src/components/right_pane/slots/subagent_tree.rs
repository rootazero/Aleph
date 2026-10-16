use crate::components::right_pane::SlotRenderCtx;
use leptos::prelude::*;

pub(super) fn render(_ctx: SlotRenderCtx) -> AnyView {
    view! { <crate::views::subagent_tree::SubagentTree /> }.into_any()
}
