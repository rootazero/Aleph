use crate::components::right_pane::tray::TraySurface;
use crate::components::right_pane::SlotRenderCtx;
use leptos::prelude::*;

pub(super) fn render(_ctx: SlotRenderCtx) -> AnyView {
    view! { <TraySurface /> }.into_any()
}
