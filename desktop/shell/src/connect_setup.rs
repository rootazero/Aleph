//! First-run connection setup for the panel-only (`--no-default-features`)
//! shell variant.
//!
//! The panel-only shell hosts no local `aleph-server`, so on first launch it
//! has no Gateway to point the webview at. Instead of stranding the user on a
//! white "can't connect" page (spec §8), it reveals the bundled connection
//! page (`splash/connect.html`) where they can:
//!   * type a remote Gateway address (validated by [`connection::ConnectionTarget`]), or
//!   * pick one of the `aleph-server` instances this module discovers on the
//!     LAN via mDNS.
//!
//! Both Tauri commands here (`discover_servers` / `connect_to`) are lite-only
//! (mirrored by the `#![cfg(...)]` below) and are registered in `main.rs`'s
//! lite `generate_handler!` arm; the variant flag the page keys off
//! (`connection::is_lite_shell`) is registered by both variants. The
//! discovery browse is a blocking 3-second operation, so the command wrapper
//! always hops onto a blocking thread — never the Tauri event loop.
//!
//! **Reachability is not decided here.** "Does an Aleph Gateway answer at this
//! address" — and the sentence explaining that it does not — live in
//! [`crate::gateway_probe`], which the full app depends on as well. Keeping
//! that predicate inside this lite-only module is exactly what left the full
//! app's remote leg on a weaker one for four rounds.

#![cfg(not(feature = "embedded-core"))]

use std::net::IpAddr;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};

use crate::connection;
use crate::gateway_probe;

/// The mDNS service type the Gateway broadcasts itself as. Must match the
/// server-side broadcaster (`src/gateway/mdns_broadcaster.rs`).
const SERVICE_TYPE: &str = "_aleph._tcp.local.";

/// How long to listen for mDNS responses before returning the collected set.
const DISCOVER_WINDOW: Duration = Duration::from_secs(3);

/// Format a discovered `(ip, port)` pair as an `http://` origin string that
/// [`connection::ConnectionTarget::parse`] accepts. IPv6 literals are wrapped
/// in brackets (`http://[::1]:18790`) so the URL parses; IPv4 is left bare.
fn format_service_url(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
    }
}

/// Browse the LAN for running `aleph-server` instances over a fixed
/// [`DISCOVER_WINDOW`]. Returns a sorted, de-duplicated list of `http://…`
/// origins. Any mDNS failure (no daemon, no multicast) degrades to an empty
/// list — discovery is a convenience, never a hard dependency.
///
/// Blocking: this parks the caller for up to [`DISCOVER_WINDOW`]. Always
/// invoke it off the Tauri event loop (see [`discover_servers`]).
pub fn discover() -> Vec<String> {
    let Ok(daemon) = ServiceDaemon::new() else {
        return Vec::new();
    };
    let Ok(rx) = daemon.browse(SERVICE_TYPE) else {
        return Vec::new();
    };

    let deadline = Instant::now() + DISCOVER_WINDOW;
    let mut found = Vec::new();
    while let Ok(event) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if let ServiceEvent::ServiceResolved(info) = event {
            for addr in info.get_addresses() {
                // `get_addresses()` yields `ScopedIp` (mdns-sd carries an IPv6
                // scope id alongside the address); the origin string only ever
                // needed the bare address.
                found.push(format_service_url(addr.to_ip_addr(), info.get_port()));
            }
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    // Best-effort cleanup; the daemon also shuts down on drop.
    let _ = daemon.shutdown();

    found.sort();
    found.dedup();
    found
}

/// Navigate the main window to the bundled connection page and bring it
/// forward. This is the panel-only shell's first-run surface and its
/// not-reachable fallback — never a white screen (spec §8). Both `connect.html`
/// and `index.html` are served from the bundled `splash/` frontendDist; the
/// page URL is resolved per platform by [`connection::connect_page_url`]
/// (`tauri://localhost` on macOS/Linux, `http://tauri.localhost` on Windows).
///
/// `message` is why we came here, rendered by the page on load (see
/// [`connection::connect_page_url_with_error`]). Empty means "no failure to
/// explain" — the first-run case, where the page's own copy is the whole
/// story. Every other call site has a reason, and withholding it leaves the
/// user staring at a prefilled address with no hint that it is the problem.
///
/// Named with the `lite_` prefix to stay distinct from the full-app-only
/// `main.rs::show_connection_page` (same page, same message channel).
pub fn show_lite_connect_page(handle: &tauri::AppHandle, message: &str) {
    use tauri::Manager;
    let Some(window) = handle.get_webview_window("main") else {
        tracing::error!("main window missing — cannot show the connection page");
        return;
    };
    match tauri::Url::parse(&connection::connect_page_url_with_error(message)) {
        Ok(url) => {
            if let Err(e) = window.navigate(url) {
                tracing::error!("failed to navigate to the connection page: {e}");
            }
        }
        Err(e) => tracing::error!("invalid connection-page URL: {e}"),
    }
    crate::focus_window(handle);
}

// ---------------------------------------------------------------------------
// Tauri commands — lite-only extensions to the shell's connection surface.
// Registered in main.rs's `#[cfg(not(feature = "embedded-core"))]`
// `generate_handler!`. Like the sibling `connection::*` commands these are
// pure I/O config (discovery + reachability + target persistence), not
// business or UI logic (R2/R4 boundary, spec §5.2).
// ---------------------------------------------------------------------------

/// Browse the LAN for `aleph-server` instances and return their `http://…`
/// origins. Runs the blocking [`discover`] on a blocking thread so the Tauri
/// event loop is never parked for the discovery window.
#[tauri::command]
pub async fn discover_servers() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(discover)
        .await
        .unwrap_or_default()
}

/// Validate, probe, then commit a connection target. Unlike the bare
/// `connection::set_connection_target`, this first checks the target is
/// actually reachable and only persists + re-routes the webview on success;
/// an unreachable address returns `Err` so the connection page can surface it
/// inline and keep the user on an operable page instead of navigating them to
/// a dead origin (spec §8 — no white screen). Used by both the manual address
/// field and the discovery list in `connect.html`.
#[tauri::command]
pub async fn connect_to(app: tauri::AppHandle, raw: String) -> Result<(), String> {
    let target = connection::ConnectionTarget::parse(&raw)?;
    if !gateway_probe::target_reachable(&target).await {
        return Err(gateway_probe::target_unreachable_message(&target));
    }
    // Reachable: persist + re-route through the shared connection path so the
    // external-link allow-list and webview navigation stay in one place.
    connection::set_connection_target(app, raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn ipv4_formats_bare() {
        let url = format_service_url(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)), 18790);
        assert_eq!(url, "http://192.168.1.5:18790");
        // Must round-trip through the real target parser.
        assert_eq!(
            connection::ConnectionTarget::parse(&url)
                .unwrap()
                .to_persisted(),
            "http://192.168.1.5:18790"
        );
    }

    #[test]
    fn ipv6_is_bracketed() {
        let url = format_service_url(IpAddr::V6(Ipv6Addr::LOCALHOST), 9000);
        assert_eq!(url, "http://[::1]:9000");
        assert_eq!(
            connection::ConnectionTarget::parse(&url)
                .unwrap()
                .to_persisted(),
            "http://[::1]:9000"
        );
    }
}
