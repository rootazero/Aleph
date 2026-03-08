//! Master-detail layout for a single channel platform type.
//!
//! Shows a sidebar listing all instances of a given platform (e.g. "telegram"),
//! with the selected instance's configuration panel on the right.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use serde_json::json;

use crate::components::ui::channel_status::ChannelStatus;
use crate::context::DashboardState;

use super::config_template::ChannelConfigTemplate;
use super::definitions::{ChannelDefinition, ALL_CHANNELS};
use super::DiscordChannelView;

// ---------------------------------------------------------------------------
// InstanceInfo — lightweight summary of a channel instance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct InstanceInfo {
    channel_id: String,
    status: ChannelStatus,
}

// ---------------------------------------------------------------------------
// ChannelPlatformPage
// ---------------------------------------------------------------------------

/// Master-detail page for managing instances of a single platform type.
///
/// The left sidebar lists all instances (with status dots and a "New Instance"
/// button), and the right panel shows the configuration for the selected
/// instance.
#[component]
pub fn ChannelPlatformPage(platform_type: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // ---- State signals ----
    let instances: RwSignal<Vec<InstanceInfo>> = RwSignal::new(Vec::new());
    let selected_id: RwSignal<Option<String>> = RwSignal::new(None);
    let refresh_trigger: RwSignal<u32> = RwSignal::new(0);

    // New-instance dialog state
    let show_new_dialog = RwSignal::new(false);
    let new_id_input = RwSignal::new(String::new());
    let new_error = RwSignal::new(Option::<String>::None);
    let creating = RwSignal::new(false);

    // Look up the static ChannelDefinition for this platform
    let definition: Option<&'static ChannelDefinition> = ALL_CHANNELS
        .iter()
        .find(|def| def.id == platform_type.as_str());

    let definition = match definition {
        Some(d) => d,
        None => {
            return view! {
                <div class="flex-1 p-6 overflow-y-auto bg-surface">
                    <div class="text-text-tertiary">"Unknown channel platform."</div>
                </div>
            }
            .into_any();
        }
    };

    // Store platform_type in a StoredValue so closures can access it (Copy)
    let platform_type_stored = StoredValue::new(platform_type.clone());

    // ---- Fetch instances on mount and when refresh_trigger changes ----
    Effect::new(move |_| {
        let _ = refresh_trigger.get(); // subscribe to changes
        let pt = platform_type_stored.get_value();
        spawn_local(async move {
            match state.rpc_call("channels.list", json!({})).await {
                Ok(val) => {
                    let mut found = Vec::new();
                    if let Some(channels) = val.get("channels").and_then(|c| c.as_array()) {
                        for ch in channels {
                            let ch_type = ch
                                .get("channel_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if ch_type == pt {
                                let ch_id = ch
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let status_str = ch
                                    .get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("disconnected");
                                found.push(InstanceInfo {
                                    channel_id: ch_id,
                                    status: ChannelStatus::from_str(status_str),
                                });
                            }
                        }
                    }
                    instances.set(found);

                    // Auto-select first instance if nothing is selected or
                    // the previously selected instance no longer exists
                    let current = selected_id.get_untracked();
                    let list = instances.get_untracked();
                    let should_reselect = match &current {
                        None => true,
                        Some(id) => !list.iter().any(|i| &i.channel_id == id),
                    };
                    if should_reselect {
                        selected_id.set(list.first().map(|i| i.channel_id.clone()));
                    }
                }
                Err(_) => {
                    // Keep existing list on error
                }
            }
        });
    });

    // ---- Create new instance handler ----
    let on_create = move || {
        let raw = new_id_input.get();
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            new_error.set(Some("Instance ID cannot be empty.".to_string()));
            return;
        }
        creating.set(true);
        new_error.set(None);

        let pt = platform_type_stored.get_value();
        let id = trimmed.clone();
        spawn_local(async move {
            match state
                .rpc_call(
                    "channel.create",
                    json!({ "channel_type": pt, "channel_id": id }),
                )
                .await
            {
                Ok(_) => {
                    selected_id.set(Some(id));
                    show_new_dialog.set(false);
                    new_id_input.set(String::new());
                    refresh_trigger.update(|n| *n += 1);
                }
                Err(e) => {
                    new_error.set(Some(format!("Failed to create: {}", e)));
                }
            }
            creating.set(false);
        });
    };

    // ---- Callback when an instance is deleted from the config template ----
    let on_instance_deleted = Callback::new(move |_: ()| {
        selected_id.set(None);
        refresh_trigger.update(|n| *n += 1);
    });

    // ---- Static view data from definition ----
    let icon_svg = definition.icon_svg;
    let brand_color = definition.brand_color;
    let name = definition.name;
    let description = definition.description;
    let icon_bg = format!("background-color: {}15", brand_color);
    let is_discord = definition.id == "discord";

    view! {
        <div class="flex-1 flex flex-col overflow-hidden bg-surface">
            // ---- Header: back link + platform identity ----
            <div class="p-6 pb-4 border-b border-border">
                <A
                    href="/settings/channels"
                    attr:class="inline-flex items-center gap-1 text-sm text-text-tertiary hover:text-text-primary transition-colors mb-3"
                >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6"/>
                    </svg>
                    "Back to Channels"
                </A>
                <div class="flex items-center gap-3">
                    <div
                        class="w-10 h-10 rounded-lg flex items-center justify-center"
                        style=icon_bg
                    >
                        <svg
                            width="22"
                            height="22"
                            viewBox="0 0 24 24"
                            fill="none"
                            stroke=brand_color
                            stroke-width="2"
                            stroke-linecap="round"
                            stroke-linejoin="round"
                            inner_html=icon_svg
                        />
                    </div>
                    <div>
                        <h1 class="text-2xl font-semibold text-text-primary">{name}</h1>
                        <p class="text-sm text-text-secondary">{description}</p>
                    </div>
                </div>
            </div>

            // ---- Master-detail body ----
            <div class="flex flex-1 overflow-hidden">
                // ---- Left sidebar: instance list ----
                <div class="w-56 border-r border-border overflow-y-auto p-3 space-y-1 flex-shrink-0">
                    // Instance items
                    <For
                        each=move || instances.get()
                        key=|inst| inst.channel_id.clone()
                        children=move |inst: InstanceInfo| {
                            let id = inst.channel_id.clone();
                            let id_for_click = id.clone();
                            let status = inst.status;
                            let is_selected = Signal::derive({
                                let id = id.clone();
                                move || selected_id.get().as_deref() == Some(id.as_str())
                            });
                            view! {
                                <button
                                    on:click=move |_| selected_id.set(Some(id_for_click.clone()))
                                    class=move || {
                                        if is_selected.get() {
                                            "w-full text-left px-3 py-2 rounded-lg bg-primary/10 text-text-primary text-sm flex items-center gap-2 transition-colors"
                                        } else {
                                            "w-full text-left px-3 py-2 rounded-lg hover:bg-surface-raised text-text-secondary text-sm flex items-center gap-2 transition-colors"
                                        }
                                    }
                                >
                                    <span class=format!("w-2 h-2 rounded-full flex-shrink-0 {}", status.dot_class()) />
                                    <span class="truncate">{id}</span>
                                </button>
                            }
                        }
                    />

                    // ---- New Instance button / inline form ----
                    {move || {
                        if show_new_dialog.get() {
                            view! {
                                <div class="mt-2 p-2 border border-border rounded-lg bg-surface-raised space-y-2">
                                    <input
                                        type="text"
                                        placeholder="instance-id"
                                        class="w-full px-2 py-1 text-sm bg-surface border border-border rounded focus:outline-none focus:border-primary text-text-primary"
                                        prop:value=move || new_id_input.get()
                                        on:input=move |ev| new_id_input.set(event_target_value(&ev))
                                        on:keydown=move |ev| {
                                            if ev.key() == "Enter" {
                                                on_create();
                                            }
                                        }
                                    />
                                    {move || new_error.get().map(|e| view! {
                                        <div class="text-xs text-danger">{e}</div>
                                    })}
                                    <div class="flex gap-1">
                                        <button
                                            on:click=move |_| on_create()
                                            disabled=move || creating.get()
                                            class="flex-1 px-2 py-1 text-xs bg-primary text-text-inverse rounded hover:bg-primary-hover disabled:opacity-50 transition-colors"
                                        >
                                            {move || if creating.get() { "Creating..." } else { "Create" }}
                                        </button>
                                        <button
                                            on:click=move |_| {
                                                show_new_dialog.set(false);
                                                new_id_input.set(String::new());
                                                new_error.set(None);
                                            }
                                            class="flex-1 px-2 py-1 text-xs border border-border text-text-secondary rounded hover:bg-surface-sunken transition-colors"
                                        >
                                            "Cancel"
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    on:click=move |_| show_new_dialog.set(true)
                                    class="w-full mt-2 px-3 py-2 text-sm text-text-tertiary hover:text-text-primary hover:bg-surface-raised rounded-lg transition-colors flex items-center gap-2"
                                >
                                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <line x1="12" y1="5" x2="12" y2="19"/>
                                        <line x1="5" y1="12" x2="19" y2="12"/>
                                    </svg>
                                    "New Instance"
                                </button>
                            }.into_any()
                        }
                    }}
                </div>

                // ---- Right panel: config or empty state ----
                <div class="flex-1 overflow-y-auto p-6">
                    <div class="max-w-3xl">
                        {move || {
                            match selected_id.get() {
                                Some(id) => {
                                    if is_discord {
                                        view! { <DiscordChannelView /> }.into_any()
                                    } else {
                                        view! {
                                            <ChannelConfigTemplate
                                                definition=definition
                                                instance_id=id
                                                on_deleted=on_instance_deleted
                                            />
                                        }.into_any()
                                    }
                                }
                                None => {
                                    view! {
                                        <div class="flex flex-col items-center justify-center py-20 text-text-tertiary">
                                            <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1" stroke-linecap="round" stroke-linejoin="round" class="mb-4 opacity-50">
                                                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
                                                <line x1="12" y1="8" x2="12" y2="16"/>
                                                <line x1="8" y1="12" x2="16" y2="12"/>
                                            </svg>
                                            <p class="text-sm">"No instances yet. Create one to get started."</p>
                                        </div>
                                    }.into_any()
                                }
                            }
                        }}
                    </div>
                </div>
            </div>
        </div>
    }
    .into_any()
}
