//! Pairing + paired devices + shared token (Settings → Security).
//!
//! Three surfaces, in the order an operator should reach for them:
//!
//! 1. **Pairing code / QR** (the answer): a short-lived, single-use bootstrap
//!    ticket (`?bt=…`) the remote Panel exchanges for its own device token. No
//!    permanent credential ever appears in a URL, a QR, or browser history.
//! 2. **Paired devices**: the resulting inventory, with a live `connected` flag
//!    and per-device revoke. Revoking now ends that device's sessions
//!    immediately, so this is an authority surface, not a log.
//! 3. **Shared token**: recovery / manual entry only. It never expires and is
//!    also the secret vault's master key.
//!
//! The pairing URL comes from the **server** (`gateway.ticket.create` → `urls`),
//! not from `window.location`. Building it client-side produced
//! `http://127.0.0.1:<port>/?bt=…` whenever the operator generated the QR from
//! the local desktop App — a QR that cannot work in the most common case (pair
//! my phone with the core running on this machine).
//!
//! Reachable only by an authorized (operator) connection — the login wall gates
//! everything before this renders.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use crate::components::ui::ConfirmButton;
use crate::context::{local_device_id, DashboardState};
use crate::i18n::{t, t_string, use_i18n};

/// One paired remote Panel device, as returned by `gateway.devices.list`.
#[derive(Clone)]
struct PairedDevice {
    device_id: String,
    device_name: String,
    last_seen_at: Option<i64>,
    connected: bool,
    /// Which principal this device speaks as — a resolved display name when
    /// the directory has one, otherwise the raw `u-` id.
    ///
    /// The list is where an operator revokes a device, so it is also where
    /// they have to be able to tell whose it is. Without this column five
    /// members' phones render as five rows named "iPhone".
    owner: Option<String>,
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

/// This page's `scheme://host[:port]`, or `None` off-browser.
#[cfg(target_arch = "wasm32")]
fn page_origin() -> Option<String> {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .filter(|o| !o.is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
fn page_origin() -> Option<String> {
    None
}

/// Which URLs to offer for this pairing ticket.
///
/// Server-resolved addresses win — only the core knows what it is bound to. The
/// fallback is the Panel's own origin, which is right in exactly one case the
/// server cannot see: a reverse-proxy deployment where the gateway binds
/// loopback and TLS terminates outside, so `gateway.ticket.create` honestly
/// reports "no LAN address" while this browser reached it through a public
/// name. A **loopback** origin is never used — that is the broken
/// `http://127.0.0.1:<port>/?bt=…` QR this whole path exists to stop producing.
fn choose_pairing_urls(
    server_urls: Vec<String>,
    origin: Option<&str>,
    ticket: &str,
) -> Vec<String> {
    if !server_urls.is_empty() {
        return server_urls;
    }
    origin
        .filter(|o| !is_loopback_origin(o))
        .map(|o| vec![format!("{}/?bt={ticket}", o.trim_end_matches('/'))])
        .unwrap_or_default()
}

/// Whether a `scheme://host[:port]` origin points at this machine.
fn is_loopback_origin(origin: &str) -> bool {
    let authority = origin
        .split_once("://")
        .map_or(origin, |(_, rest)| rest)
        .trim_end_matches('/');
    let host = match authority.strip_prefix('[') {
        // IPv6 literal: `[::1]:3033`
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        None => authority.split(':').next().unwrap_or(authority),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Format an epoch-millisecond stamp in local time, or `None` when out of range.
fn format_stamp(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|dt| {
        dt.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
}

#[component]
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn GatewayTokenSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Shared token state.
    let token = RwSignal::new(String::new());
    let revealed = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let reload = RwSignal::new(0u32);
    let confirming_rotate = RwSignal::new(false);

    // Bootstrap pairing ticket state.
    let pairing_ticket = RwSignal::new(String::new());
    let pairing_urls = RwSignal::new(Vec::<String>::new());
    let pairing_expires_at = RwSignal::new(Option::<i64>::None);
    let pairing_error = RwSignal::new(Option::<String>::None);
    // Which principal a generated ticket binds to. Empty = unbound, which
    // means the redeeming device becomes the OWNER — the right default for
    // pairing your own phone and the wrong one for inviting a colleague. This
    // page had no way to express the difference and always sent `{}`.
    let pairing_user_id = RwSignal::new(String::new());
    let dir = expect_context::<crate::state::user_directory::UserDirectoryState>();
    dir.ensure_loaded(state);

    // Paired-device inventory state.
    let devices = RwSignal::new(Vec::<PairedDevice>::new());
    let devices_error = RwSignal::new(Option::<String>::None);
    let devices_reload = RwSignal::new(0u32);
    // Which row (if any) is armed for confirmation. Only ever set for the
    // device the operator is sitting at — revoking that one signs them out.
    let confirming_self_revoke = RwSignal::new(false);
    let this_device = StoredValue::new(local_device_id());

    // Revoke a device by id, surfacing failures. Swallowing the RPC error made a
    // refused revoke look exactly like a successful one: the row simply came
    // back on the refresh with no explanation.
    let revoke_now = move |id: String| {
        spawn_local(async move {
            match state
                .rpc_call("gateway.devices.revoke", json!({ "device_id": id }))
                .await
            {
                Ok(_) => devices_error.set(None),
                Err(e) => devices_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
            devices_reload.update(|n| *n += 1);
        });
    };

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
                                connected: d
                                    .get("connected")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false),
                                owner: d
                                    .get("display_name")
                                    .and_then(|x| x.as_str())
                                    .or_else(|| d.get("user_id").and_then(|x| x.as_str()))
                                    .map(str::to_string),
                            })
                        })
                        .collect::<Vec<_>>();
                    devices.set(parsed);
                    devices_error.set(None);
                }
                Err(e) => devices_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
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
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    });

    let rotate_now = move || {
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
                    // Rotation revokes every paired device, so both the displayed
                    // pairing code and the roster are stale.
                    pairing_ticket.set(String::new());
                    pairing_urls.set(Vec::new());
                    pairing_expires_at.set(None);
                    devices_reload.update(|n| *n += 1);
                }
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
        });
    };

    let generate_pairing_link = move |_| {
        spawn_local(async move {
            // The binding is the difference between "pair my own phone" and
            // "invite a colleague". An UNBOUND ticket defaults the redeeming
            // device to the OWNER — `pair --user` shouts about that in capitals
            // and this page used to send `json!({})` and say nothing.
            let mut params = json!({});
            let bind_to = pairing_user_id.get_untracked();
            if !bind_to.is_empty() {
                params["user_id"] = json!(bind_to);
            }
            match state.rpc_call("gateway.ticket.create", params).await {
                Ok(v) => {
                    pairing_ticket.set(
                        v.get("ticket")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                    pairing_expires_at.set(v.get("expires_at").and_then(|t| t.as_i64()));
                    // Server-resolved reachable URLs, else this browser's own
                    // (non-loopback) origin. Empty means there is genuinely
                    // nothing to hand out — say so rather than invent a link.
                    let server_urls: Vec<String> = v
                        .get("urls")
                        .and_then(|u| u.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|u| u.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    // `try_get_untracked`: this view can be disposed while the
                    // RPC is in flight, and reading a disposed signal panics the
                    // whole panel (see `acp_harnesses` for the reproduction).
                    let Some(ticket) = pairing_ticket.try_get_untracked() else {
                        return;
                    };
                    pairing_urls.set(choose_pairing_urls(
                        server_urls,
                        page_origin().as_deref(),
                        &ticket,
                    ));
                    pairing_error.set(None);
                }
                Err(e) => pairing_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
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

            // --- Pairing code / QR ---
            <div class="mb-6">
                <h3 class="text-sm font-semibold mb-2">{t!(i18n, common.gateway_pair_title)}</h3>
                <p class="text-xs text-text-secondary mb-3">
                    {t!(i18n, common.gateway_pair_desc)}
                </p>
                {move || pairing_error.get().map(|e| view! {
                    <div class="p-2 mb-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                })}
                // Who this ticket is for. Rendered only when a second
                // principal exists — on a single-user install the control has
                // exactly one option and would be pure noise. Same gating
                // shape as the channel-pairing picker, for the same reason.
                {move || {
                    let people = dir.selectable();
                    (people.len() > 1).then(|| view! {
                        <div class="flex items-center gap-2 mb-3">
                            <label class="text-xs text-text-secondary whitespace-nowrap">
                                {t!(i18n, common.gateway_pairing_for)}
                            </label>
                            <select
                                class="flex-1 px-2 py-1.5 text-xs bg-surface border border-border rounded focus:outline-none focus:border-primary text-text-primary"
                                prop:value=move || pairing_user_id.get()
                                on:change=move |ev| pairing_user_id.set(event_target_value(&ev))
                            >
                                <option value="">{t!(i18n, common.gateway_pairing_for_owner)}</option>
                                {people.into_iter().map(|(uid, name)| view! {
                                    <option value=uid.clone()>{name}</option>
                                }).collect_view()}
                            </select>
                        </div>
                    })
                }}
                <button
                    class="text-xs px-3 py-2 rounded border border-border hover:bg-surface mb-3"
                    on:click=generate_pairing_link
                >
                    {t!(i18n, common.gateway_pair_generate)}
                </button>
                {move || {
                    let ticket = pairing_ticket.get();
                    if ticket.is_empty() {
                        return None;
                    }
                    let urls = pairing_urls.get();
                    let primary = urls.first().cloned();
                    let extra: Vec<String> = urls.into_iter().skip(1).collect();
                    let expires = pairing_expires_at.get()
                        .and_then(format_stamp)
                        .map(|t| format!("{} {t}", t_string!(i18n, common.gateway_pair_expires)))
                        .unwrap_or_default();
                    let hint = if primary.is_some() {
                        t_string!(i18n, common.gateway_pair_scan).to_string()
                    } else {
                        t_string!(i18n, common.gateway_pair_local_only).to_string()
                    };
                    Some(view! {
                        <div class="flex flex-col items-center gap-2">
                            {primary.as_deref().and_then(qr_svg).map(|svg| view! {
                                <div class="bg-white p-3 rounded-lg" inner_html=svg></div>
                            })}
                            <p class="text-xs text-text-secondary text-center">{hint}</p>
                            {primary.map(|u| view! {
                                <code class="text-xs text-text-tertiary break-all">{u}</code>
                            })}
                            <code class="text-sm font-mono break-all select-all">{ticket}</code>
                            <p class="text-xs text-text-secondary">{expires}</p>
                            {(!extra.is_empty()).then(|| view! {
                                <details class="w-full text-xs text-text-tertiary">
                                    <summary class="cursor-pointer">
                                        {t!(i18n, common.gateway_pair_other_addresses)}
                                    </summary>
                                    <div class="mt-1 flex flex-col gap-1">
                                        {extra.into_iter().map(|u| view! {
                                            <code class="break-all">{u}</code>
                                        }).collect_view()}
                                    </div>
                                </details>
                            })}
                        </div>
                    })
                }}
            </div>

            <hr class="border-border mb-4" />

            // --- Paired devices ---
            <div class="mb-6">
                <h3 class="text-sm font-semibold mb-2">{t!(i18n, common.gateway_devices_title)}</h3>
                <p class="text-xs text-text-secondary mb-3">
                    {t!(i18n, common.gateway_devices_desc)}
                </p>
                {move || devices_error.get().map(|e| view! {
                    <div class="p-2 mb-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                })}
                {move || {
                    let list = devices.get();
                    if list.is_empty() {
                        return view! {
                            <p class="text-xs text-text-tertiary">
                                {t!(i18n, common.gateway_devices_empty)}
                            </p>
                        }.into_any();
                    }
                    let mine = this_device.get_value();
                    view! {
                        <div>
                            {list.into_iter().map(|d| {
                                let is_self = mine.as_deref() == Some(d.device_id.as_str());
                                let id = d.device_id.clone();
                                let confirm_id = d.device_id.clone();
                                let status = if d.connected {
                                    t_string!(i18n, common.gateway_devices_online).to_string()
                                } else {
                                    d.last_seen_at
                                        .and_then(format_stamp)
                                        .map(|t| format!("{} {t}", t_string!(i18n, common.gateway_devices_last_seen)))
                                        .unwrap_or_else(|| t_string!(i18n, common.gateway_devices_never_seen).to_string())
                                };
                                view! {
                                    <div class="mb-2 p-2 rounded bg-surface-sunken border border-border">
                                        <div class="flex items-center gap-2">
                                            <div class="flex-1 min-w-0">
                                                <div class="text-sm font-medium truncate flex items-center gap-2">
                                                    {d.device_name}
                                                    {is_self.then(|| view! {
                                                        <span class="text-[10px] px-1.5 py-0.5 rounded bg-primary/15 text-primary shrink-0">
                                                            {t!(i18n, common.gateway_devices_this_device)}
                                                        </span>
                                                    })}
                                                </div>
                                                <div class="text-xs text-text-tertiary flex items-center gap-1.5">
                                                    {d.connected.then(|| view! {
                                                        <span class="w-1.5 h-1.5 rounded-full bg-success shrink-0"></span>
                                                    })}
                                                    {status}
                                                    // Whose device this is. The row carries a
                                                    // Revoke button, so it has to say who it
                                                    // would be revoking.
                                                    {d.owner.map(|o| view! {
                                                        <span class="truncate">"· "{o}</span>
                                                    })}
                                                </div>
                                            </div>
                                            {move || if is_self && confirming_self_revoke.get() {
                                                let confirm_id = confirm_id.clone();
                                                view! {
                                                    <ConfirmButton
                                                        confirming=confirming_self_revoke
                                                        on_confirm=move || revoke_now(confirm_id.clone())
                                                        label=Signal::derive(move || t_string!(i18n, common.gateway_devices_revoke_confirm).to_string())
                                                        size_class="px-3 py-1.5 text-xs"
                                                    />
                                                }.into_any()
                                            } else {
                                                let id = id.clone();
                                                view! {
                                                    <button
                                                        class="text-xs px-3 py-1.5 rounded border border-border text-danger hover:bg-danger-subtle shrink-0"
                                                        on:click=move |_| {
                                                            // Revoking the device you are sitting at
                                                            // now ends this session immediately.
                                                            if is_self {
                                                                confirming_self_revoke.set(true);
                                                            } else {
                                                                revoke_now(id.clone());
                                                            }
                                                        }
                                                    >
                                                        {t!(i18n, common.gateway_devices_revoke)}
                                                    </button>
                                                }.into_any()
                                            }}
                                        </div>
                                        {move || (is_self && confirming_self_revoke.get()).then(|| view! {
                                            <p class="mt-2 text-xs text-danger">
                                                {t!(i18n, common.gateway_devices_revoke_self_confirm)}
                                            </p>
                                        })}
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>

            <hr class="border-border mb-4" />

            // --- Shared token (recovery) ---
            <div>
                <h3 class="text-sm font-semibold mb-2">
                    {t!(i18n, common.gateway_token_legacy_title)}
                </h3>
                <p class="text-xs text-text-secondary mb-3">
                    {t!(i18n, common.gateway_token_legacy_desc)}
                </p>
                {move || error.get().map(|e| view! {
                    <div class="p-2 mb-3 bg-danger-subtle text-danger rounded text-sm">{e}</div>
                })}
                <div class="flex items-center gap-2 mb-2">
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
                    {move || if confirming_rotate.get() {
                        view! {
                            <ConfirmButton
                                confirming=confirming_rotate
                                on_confirm=rotate_now
                                label=Signal::derive(move || t_string!(i18n, common.gateway_token_rotate_confirm_action).to_string())
                                size_class="px-3 py-2 text-xs"
                            />
                        }.into_any()
                    } else {
                        view! {
                            <button
                                class="text-xs px-3 py-2 rounded border border-border text-danger hover:bg-danger-subtle shrink-0"
                                on:click=move |_| confirming_rotate.set(true)
                            >
                                {t!(i18n, common.gateway_token_rotate)}
                            </button>
                        }.into_any()
                    }}
                </div>
                {move || confirming_rotate.get().then(|| view! {
                    <p class="mb-4 text-xs text-danger">
                        {t!(i18n, common.gateway_token_rotate_confirm)}
                    </p>
                })}
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_pairing_urls, is_loopback_origin};

    #[test]
    fn server_urls_win_over_the_page_origin() {
        // Only the core knows what it is bound to; the browser's view is a
        // fallback, never an override.
        assert_eq!(
            choose_pairing_urls(
                vec!["http://192.168.1.20:3033/?bt=t".into()],
                Some("https://core.example.com"),
                "t"
            ),
            vec!["http://192.168.1.20:3033/?bt=t"]
        );
    }

    #[test]
    fn reverse_proxy_deployment_falls_back_to_the_page_origin() {
        // Gateway bound to loopback behind a proxy: the server honestly reports
        // no LAN address, but this browser reached it by public name.
        assert_eq!(
            choose_pairing_urls(vec![], Some("https://core.example.com"), "t"),
            vec!["https://core.example.com/?bt=t"]
        );
        // Trailing slash must not double up.
        assert_eq!(
            choose_pairing_urls(vec![], Some("https://core.example.com/"), "t"),
            vec!["https://core.example.com/?bt=t"]
        );
    }

    #[test]
    fn a_loopback_origin_is_never_offered() {
        // The original bug: generating the QR from the local desktop App handed
        // out a link only that machine could open.
        for origin in [
            "http://127.0.0.1:3033",
            "http://localhost:3033",
            "http://[::1]:3033",
            "http://127.0.0.1",
        ] {
            assert!(is_loopback_origin(origin), "{origin} must read as loopback");
            assert!(
                choose_pairing_urls(vec![], Some(origin), "t").is_empty(),
                "{origin} must yield no pairing URL"
            );
        }
    }

    #[test]
    fn public_origins_are_not_loopback() {
        for origin in [
            "https://core.example.com",
            "http://192.168.1.20:3033",
            "http://[fd00::1]:3033",
        ] {
            assert!(!is_loopback_origin(origin), "{origin} must be offerable");
        }
    }

    #[test]
    fn nothing_reachable_yields_nothing() {
        assert!(choose_pairing_urls(vec![], None, "t").is_empty());
    }
}
