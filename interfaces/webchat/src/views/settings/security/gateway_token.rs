//! Gateway token + QR section (Settings → Security).
//!
//! Single-tier Gateway-token model: this section shows the shared Gateway token
//! and a scannable QR / copyable LAN URL so an operator can authorize a remote
//! Panel — paste the token into the device's login wall, or open the URL / scan
//! the QR (both carry `?token=`). Anyone with the token gets the same authority
//! as this machine; rotating it revokes every previously authorized remote at
//! once. Reachable only by an authorized (operator) connection — the login wall
//! gates everything before this renders.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use crate::context::DashboardState;

/// Build the LAN authorization URL `http(s)://<host>/?token=<token>` from the
/// current page origin (the same origin the Panel is served from).
#[cfg(target_arch = "wasm32")]
fn lan_url(token: &str) -> String {
    web_sys::window()
        .map(|w| {
            let loc = w.location();
            let proto = loc.protocol().unwrap_or_else(|_| "http:".to_string());
            let host = loc.host().unwrap_or_default();
            format!("{proto}//{host}/?token={token}")
        })
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn lan_url(token: &str) -> String {
    format!("http://<core-ip>:18790/?token={token}")
}

/// Render a URL into an inline SVG QR code, or `None` for an empty / unencodable
/// URL. Uses the `qrcode` crate's SVG renderer — no network, the token never
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
    let token = RwSignal::new(String::new());
    let revealed = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let reload = RwSignal::new(0u32);

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
                }
                Err(e) => error.set(Some(e)),
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
            <h2 class="text-lg font-semibold mb-2">"Gateway token"</h2>
            <p class="text-sm text-text-secondary mb-4">
                "Remote devices authorize with this shared token — paste it into the device's login \
                 box, or scan the QR / open the link below (both carry the token). Anyone with the \
                 token gets the same access as this machine. Rotate to revoke every authorized device \
                 at once."
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
                    {move || if revealed.get() { "Hide" } else { "Reveal" }}
                </button>
                <button
                    class="text-xs px-3 py-2 rounded border border-border text-danger hover:bg-danger-subtle"
                    on:click=rotate
                >
                    "Rotate"
                </button>
            </div>
            {move || {
                let url = lan_url(&token.get());
                qr_svg(&url).map(|svg| view! {
                    <div class="flex flex-col items-center gap-2">
                        <div class="bg-white p-3 rounded-lg" inner_html=svg></div>
                        <code class="text-xs text-text-tertiary break-all">{url}</code>
                    </div>
                })
            }}
        </div>
    }
}
