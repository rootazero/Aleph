// Channels Overview page — responsive card grid showing all 13 messaging channels.
//
// Fetches real-time status via `channels.list` RPC on mount (when connected),
// then renders each channel as a `ChannelCard` with a derived status signal.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use std::collections::HashMap;

use crate::components::ui::channel_status::ChannelStatus;
use crate::components::ui::ChannelCard;
use crate::context::DashboardState;

use super::definitions::ALL_CHANNELS;

/// Grid overview of all supported messaging channels.
///
/// Layout: 1 column on mobile, 2 on `sm`, 3 on `lg`.
/// Each card links to `/settings/channels/{id}` for configuration.
#[component]
pub fn ChannelsOverview() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let statuses = RwSignal::new(HashMap::<String, String>::new());
    let instance_counts = RwSignal::new(HashMap::<String, usize>::new());

    // Fetch channel statuses on mount
    spawn_local(async move {
        match state.rpc_call("channels.list", json!({})).await {
            Ok(val) => {
                if let Some(channels) = val.get("channels").and_then(|c| c.as_array()) {
                    let mut map = HashMap::new();
                    let mut count_map = HashMap::<String, usize>::new();
                    for ch in channels {
                        if let Some(ch_type) = ch.get("channel_type").and_then(|v| v.as_str()) {
                            *count_map.entry(ch_type.to_string()).or_insert(0) += 1;
                            if let Some(status) = ch.get("status").and_then(|v| v.as_str()) {
                                map.insert(ch_type.to_string(), status.to_string());
                            }
                        }
                    }
                    statuses.set(map);
                    instance_counts.set(count_map);
                }
            }
            Err(_) => {
                // Cards will show "Disconnected" by default — no user-facing error needed.
            }
        }
    });

    view! {
        <div class="flex-1 p-6 overflow-y-auto bg-surface">
            <div class="max-w-5xl">
                // Header
                <div class="mb-6">
                    <h1 class="text-2xl font-semibold text-text-primary mb-1">"Channels"</h1>
                    <p class="text-sm text-text-secondary">
                        "Manage your messaging integrations. Click a channel to configure it."
                    </p>
                </div>

                // Responsive card grid
                <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
                    {ALL_CHANNELS.iter().map(|def| {
                        let channel_id = def.id.to_string();
                        let status_signal = Signal::derive(move || {
                            statuses.get()
                                .get(&channel_id)
                                .map(|s| ChannelStatus::from_str(s))
                                .unwrap_or(ChannelStatus::Disconnected)
                        });
                        let channel_id_for_count = def.id.to_string();
                        let count_signal = Signal::derive(move || {
                            instance_counts.get()
                                .get(&channel_id_for_count)
                                .copied()
                                .unwrap_or(0)
                        });
                        view! {
                            <ChannelCard
                                id=def.id
                                name=def.name
                                description=def.description
                                icon_svg=def.icon_svg
                                brand_color=def.brand_color
                                status=status_signal
                                count=count_signal
                            />
                        }
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}
