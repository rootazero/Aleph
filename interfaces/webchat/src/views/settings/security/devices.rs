//! Panel device tier management (G2/G3).
//!
//! Operator-only section (rendered inside `SecurityView`, itself behind
//! `ConfigGate`) that lists remote Panel devices and grants/revokes their
//! Config tier via the `devices.*` RPCs. A just-paired remote device appears
//! here at Chat tier; granting Config is the pairing-time tier selection.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use crate::context::DashboardState;

#[derive(Clone)]
struct DeviceRow {
    device_id: String,
    device_name: String,
    tier: String,
    last_seen_at: String,
}

fn parse_devices(value: &serde_json::Value) -> Vec<DeviceRow> {
    value
        .get("devices")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .map(|d| DeviceRow {
                    device_id: d
                        .get("device_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    device_name: d
                        .get("device_name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    tier: d
                        .get("tier")
                        .and_then(|x| x.as_str())
                        .unwrap_or("chat")
                        .to_string(),
                    last_seen_at: d
                        .get("last_seen_at")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[component]
#[must_use]
pub fn PanelDevicesSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let devices = RwSignal::new(Vec::<DeviceRow>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let reload = RwSignal::new(0u32);

    Effect::new(move |_| {
        // Re-run on connect and on every refresh / mutation.
        let _ = reload.get();
        if !state.is_connected.get() {
            loading.set(false);
            return;
        }
        spawn_local(async move {
            loading.set(true);
            match state.rpc_call("devices.list", json!({})).await {
                Ok(v) => {
                    devices.set(parse_devices(&v));
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    });

    let set_level = move |device_id: String, level: &'static str| {
        spawn_local(async move {
            let _ = state
                .rpc_call(
                    "devices.set_level",
                    json!({ "device_id": device_id, "level": level }),
                )
                .await;
            reload.update(|n| *n += 1);
        });
    };
    let revoke = move |device_id: String| {
        spawn_local(async move {
            let _ = state
                .rpc_call("devices.revoke", json!({ "device_id": device_id }))
                .await;
            reload.update(|n| *n += 1);
        });
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <div class="flex items-center justify-between mb-2">
                <h2 class="text-lg font-semibold">"Panel devices"</h2>
                <button
                    class="text-sm text-text-secondary hover:text-text-primary"
                    on:click=move |_| reload.update(|n| *n += 1)
                >
                    "Refresh"
                </button>
            </div>
            <p class="text-sm text-text-secondary mb-4">
                "Remote Panels connect at Chat tier (converse + read). Grant Config to let a device change Aleph's settings. The local app is always Config."
            </p>
            {move || error.get().map(|e| view! {
                <div class="p-2 mb-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
            })}
            {move || {
                if loading.get() {
                    view! { <div class="text-text-tertiary text-sm">"Loading…"</div> }.into_any()
                } else if devices.get().is_empty() {
                    view! {
                        <div class="text-text-tertiary text-sm">
                            "No remote Panel devices have connected yet."
                        </div>
                    }
                    .into_any()
                } else {
                    devices
                        .get()
                        .into_iter()
                        .map(|d| {
                            let is_config = d.tier == "config";
                            let id_grant = d.device_id.clone();
                            let id_chat = d.device_id.clone();
                            let id_revoke = d.device_id.clone();
                            let display_name = if d.device_name.is_empty() {
                                "Unnamed Panel".to_string()
                            } else {
                                d.device_name.clone()
                            };
                            let badge_class = if is_config {
                                "text-xs px-2 py-0.5 rounded bg-info/15 text-info"
                            } else {
                                "text-xs px-2 py-0.5 rounded bg-surface text-text-secondary"
                            };
                            let badge_text = if is_config { "Config" } else { "Chat" };
                            let tier_button = if is_config {
                                view! {
                                    <button
                                        class="text-xs px-2 py-1 rounded border border-border hover:bg-surface"
                                        on:click=move |_| set_level(id_chat.clone(), "chat")
                                    >
                                        "Set Chat"
                                    </button>
                                }
                                .into_any()
                            } else {
                                view! {
                                    <button
                                        class="text-xs px-2 py-1 rounded bg-info text-white hover:bg-primary-hover"
                                        on:click=move |_| set_level(id_grant.clone(), "config")
                                    >
                                        "Grant Config"
                                    </button>
                                }
                                .into_any()
                            };
                            view! {
                                <div class="flex items-center justify-between gap-3 p-3 rounded border border-border">
                                    <div class="min-w-0">
                                        <div class="text-sm font-medium truncate">{display_name}</div>
                                        <div class="text-xs text-text-tertiary truncate">
                                            {d.device_id.clone()}
                                        </div>
                                        <div class="text-xs text-text-tertiary">
                                            "Last seen: " {d.last_seen_at.clone()}
                                        </div>
                                    </div>
                                    <div class="flex items-center gap-2 shrink-0">
                                        <span class=badge_class>{badge_text}</span>
                                        {tier_button}
                                        <button
                                            class="text-xs px-2 py-1 rounded border border-border text-danger hover:bg-danger-subtle"
                                            on:click=move |_| revoke(id_revoke.clone())
                                        >
                                            "Revoke"
                                        </button>
                                    </div>
                                </div>
                            }
                        })
                        .collect_view()
                        .into_any()
                }
            }}
        </div>
    }
}
