use leptos::prelude::*;
use leptos_router::components::A;

use super::channel_status::{ChannelStatus, ChannelStatusPill};

/// Card component for the Channels Overview page grid.
///
/// Displays a channel's icon, name, description, and connection status.
/// Links to the channel's configuration page at `/settings/channels/{id}`.
#[component]
pub fn ChannelCard(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    icon_svg: &'static str,
    brand_color: &'static str,
    status: Signal<ChannelStatus>,
    /// Number of bot instances for this platform
    #[prop(optional)]
    count: Option<Signal<usize>>,
) -> impl IntoView {
    let href = format!("/settings/channels/{}", id);

    // Build the icon container background with 15% opacity hex suffix
    let icon_bg = format!("background-color: {}15", brand_color);

    let action_label = "Configure";

    view! {
        <A
            href=href
            attr:class="block p-4 bg-surface-raised border border-border rounded-xl hover:border-primary/40 hover:shadow-sm transition-all group"
        >
            // Top row: icon + status pill
            <div class="flex items-start justify-between mb-3">
                <div
                    class="w-10 h-10 rounded-lg flex items-center justify-center"
                    style=icon_bg
                >
                    <svg
                        width="20"
                        height="20"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke=brand_color
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        inner_html=icon_svg
                    />
                </div>
                <ChannelStatusPill status=status />
            </div>

            // Channel name + count badge
            <div class="flex items-center gap-2 mb-1">
                <h3 class="text-sm font-semibold text-text-primary group-hover:text-primary transition-colors">
                    {name}
                </h3>
                {move || {
                    count.and_then(|c| {
                        let n = c.get();
                        if n > 0 {
                            Some(view! {
                                <span class="px-1.5 py-0.5 text-xs font-medium bg-surface-sunken text-text-tertiary rounded-full">
                                    {n}
                                </span>
                            })
                        } else {
                            None
                        }
                    })
                }}
            </div>

            // Description (2-line clamp)
            <p class="text-xs text-text-tertiary line-clamp-2 mb-3">
                {description}
            </p>

            // Action label
            <span class="text-xs font-medium text-primary">
                {action_label}
                <span class="inline-block ml-1 group-hover:translate-x-0.5 transition-transform">
                    "→"
                </span>
            </span>
        </A>
    }
}
