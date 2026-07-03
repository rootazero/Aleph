//! Gateway token + QR section (Settings → Security).
//!
//! This section offers two authorization surfaces:
//!
//! 1. **Pairing link / QR** (recommended): a short-lived, single-use bootstrap
//!    ticket (`?bt=…`) that a remote Panel exchanges for a per-device token.
//!    This keeps the long-lived shared Gateway token out of URLs and browser
//!    history.
//! 2. **Legacy shared token**: the original `aleph-…` token. Displayed for
//!    recovery and advanced use only — do not share this in a QR or URL.
//!
//! Reachable only by an authorized (operator) connection — the login wall gates
//! everything before this renders.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

/// One paired remote Panel device, as returned by `gateway.devices.list`.
#[derive(Clone)]
struct PairedDevice {
    device_id: String,
    device_name: String,
    last_seen_at: Option<i64>,
}

/// Build a LAN URL carrying a bootstrap ticket from the current page origin.
#[cfg(target_arch = "wasm32")]
fn pairing_url(ticket: &str) -> String {
    web_sys::window()
        .map(|w| {
            let loc = w.location();
            let proto = loc.protocol().unwrap_or_else(|_| "http:".to_string());
            let host = loc.host().unwrap_or_default();
            format!("{proto}//{host}/?bt={ticket}")
        })
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn pairing_url(_ticket: &str) -> String {
    String::new()
}

/// Render a URL into an inline SVG QR code, or `None` for an empty / unencodable
/// URL. Uses the `qrcode` crate's SVG renderer — no network, the ticket never
/// leaves the machine.
fn qr_svg(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    use qrcode::render::svg;
    use qrcode::QrCode;
    let code = QrCode::new(url.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .build(),
    )
}

#[component]
#[must_use]
pub fn GatewayTokenSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Legacy shared token state.
    let token = RwSignal::new(String::new());
    let revealed = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let reload = RwSignal::new(0u32);

    // Bootstrap pairing ticket state.
    let pairing_ticket = RwSignal::new(String::new());
    let pairing_expires_at = RwSignal::new(Option::<i64>::None);
    let pairing_error = RwSignal::new(Option::<String>::None);

    // Paired-device inventory state.
    let devices = RwSignal::new(Vec::<PairedDevice>::new());
    let devices_error = RwSignal::new(Option::<String>::None);
    let devices_reload = RwSignal::new(0u32);

    Effect::new(move |_| {
        // Re-run on connect, after a rotate (via `reload`), and after a revoke.
        let _ = reload.get();
        let _ = devices_reload.get();
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            match state.rpc_call("gateway.devices.list", json!({})).await {
                Ok(v) => {
                    let list = v
                        .get("devices")
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default();
                    let parsed = list
                        .into_iter()
                        .filter_map(|d| {
                            Some(PairedDevice {
                                device_id: d.get("device_id")?.as_str()?.to_string(),
                                device_name: d
                                    .get("device_name")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("Unknown")
                                    .to_string(),
                                last_seen_at: d.get("last_seen_at").and_then(|x| x.as_i64()),
                            })
                        })
                        .collect::<Vec<_>>();
                    devices.set(parsed);
                    devices_error.set(None);
                }
                Err(e) => devices_error.set(Some(e)),
            }
        });
    });

    Effect::new(move |_| {
        // Re-run on connect and after a rotate.
        let _ = reload.get();
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            match state.rpc_call("gateway.token.current", json!({})).await {
                Ok(v) => {
                    token.set(
                        v.get("token")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let rotate = move |_| {
        spawn_local(async move {
            match state.rpc_call("gateway.token.rotate", json!({})).await {
                Ok(v) => {
                    token.set(
                        v.get("token")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                    error.set(None);
                    // A rotation invalidates previously issued device tokens, so
                    // clear any displayed pairing link.
                    pairing_ticket.set(String::new());
                    pairing_expires_at.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let generate_pairing_link = move |_| {
        spawn_local(async move {
            match state.rpc_call("gateway.ticket.create", json!({})).await {
                Ok(v) => {
                    let ticket = v
                        .get("ticket")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let expires_at = v.get("expires_at").and_then(|t| t.as_i64());
                    pairing_ticket.set(ticket);
                    pairing_expires_at.set(expires_at);
                    pairing_error.set(None);
                }
                Err(e) => pairing_error.set(Some(e)),
            }
        });
    };

    let masked = move || {
        let t = token.get();
        if revealed.get() || t.is_empty() {
            t
        } else {
            "•".repeat(t.len().min(24))
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold mb-2">{t!(i18n, common.gateway_token_title)}</h2>
            <p class="text-sm text-text-secondary mb-4">
                {t!(i18n, common.gateway_token_desc)}
            </p>

            // --- Pairing link / QR ---
            <div class="mb-6">
                <h3 class="text-sm font-semibold mb-2">{"Pair new device"}</h3>
                <p class="text-xs text-text-secondary mb-3">
                    {"Generate a short-lived, single-use link or QR code. The remote device exchanges it for its own long-lived token — the shared token never appears in a URL."}
                </p>
                {move || pairing_error.get().map(|e| view! {
                    <div class="p-2 mb-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                })}
                <button
                    class="text-xs px-3 py-2 rounded border border-border hover:bg-surface mb-3"
                    on:click=generate_pairing_link
                >
                    {"Generate pairing link / QR"}
                </button>
                {move || {
                    let url = pairing_url(&pairing_ticket.get());
                    if url.is_empty() {
                        return None;
                    }
                    let expires = pairing_expires_at.get()
                        .map(|ts| format!("Expires at {}", chrono::DateTime::from_timestamp_millis(ts).map(|d| d.to_rfc3339()).unwrap_or_else(|| ts.to_string())))
                        .unwrap_or_default();
                    Some(view! {
                        <div class="flex flex-col items-center gap-2">
                            <div class="bg-white p-3 rounded-lg" inner_html=qr_svg(&url).unwrap_or_default()></div>
                            <code class="text-xs text-text-tertiary break-all">{url}</code>
                            <p class="text-xs text-text-secondary">{expires}</p>
                        </div>
                    })
                }}
            </div>

            <hr class="border-border mb-4" />

            // --- Paired devices ---
            <div class="mb-6">
                <h3 class="text-sm font-semibold mb-2">{"Paired devices"}</h3>
                <p class="text-xs text-text-secondary mb-3">
                    {"Remote devices that paired via a pairing link. Revoke one to lock it out on its next reconnect. Rotating the shared token below revokes them all at once."}
                </p>
                {move || devices_error.get().map(|e| view! {
                    <div class="p-2 mb-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                })}
                {move || {
                    let list = devices.get();
                    if list.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary">{"No paired devices."}</p>
                        }.into_any();
                    }
                    view! {
                        <div>
                            {list.into_iter().map(|d| {
                                let id = d.device_id.clone();
                                let revoke = move |_| {
                                    let id = id.clone();
                                    spawn_local(async move {
                                        let _ = state
                                            .rpc_call("gateway.devices.revoke", json!({ "device_id": id }))
                                            .await;
                                        devices_reload.update(|n| *n += 1);
                                    });
                                };
                                let last_seen = d.last_seen_at
                                    .and_then(chrono::DateTime::from_timestamp_millis)
                                    .map(|dt| format!("Last seen {}", dt.to_rfc3339()))
                                    .unwrap_or_else(|| "Never connected".to_string());
                                view! {
                                    <div class="flex items-center gap-2 mb-2 p-2 rounded bg-surface-sunken border border-border">
                                        <div class="flex-1 min-w-0">
                                            <div class="text-sm font-medium truncate">{d.device_name}</div>
                                            <div class="text-xs text-text-tertiary">{last_seen}</div>
                                        </div>
                                        <button
                                            class="text-xs px-3 py-1.5 rounded border border-border text-danger hover:bg-danger-subtle"
                                            on:click=revoke
                                        >
                                            {"Revoke"}
                                        </button>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>

            <hr class="border-border mb-4" />

            // --- Legacy shared token (advanced) ---
            <div>
                <h3 class="text-sm font-semibold mb-2">{"Legacy shared token"}</h3>
                <p class="text-xs text-text-secondary mb-3">
                    {"For recovery or manual entry only. Do not share this in a URL or QR code — use the pairing link above instead."}
                </p>
                {move || error.get().map(|e| view! {
                    <div class="p-2 mb-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                })}
                <div class="flex items-center gap-2 mb-4">
                    <code class="flex-1 px-3 py-2 rounded bg-surface-sunken border border-border text-sm font-mono truncate">
                        {masked}
                    </code>
                    <button
                        class="text-xs px-3 py-2 rounded border border-border hover:bg-surface"
                        on:click=move |_| revealed.update(|r| *r = !*r)
                    >
                        {move || if revealed.get() {
                            t_string!(i18n, common.gateway_token_hide).to_string()
                        } else {
                            t_string!(i18n, common.gateway_token_reveal).to_string()
                        }}
                    </button>
                    <button
                        class="text-xs px-3 py-2 rounded border border-border text-danger hover:bg-danger-subtle"
                        on:click=rotate
                    >
                        {t!(i18n, common.gateway_token_rotate)}
                    </button>
                </div>
            </div>
        </div>
    }
}
