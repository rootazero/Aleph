//! Section 1 — Service connection: read-only reflection of the Aleph core this Panel is currently connected to (local / remote).
//!
//! The connection form is determined at build time, not toggled in the panel: the full App always connects to the embedded loopback core;
//! the pure-shell Panel always connects to remote; the browser depends on the address bar. All three are determined by `location.host` (authoritative,
//! always fresh) to decide local/remote — no shell injection marker, no IPC dependency (R4: Interface is pure I/O).

use crate::i18n::{t, use_i18n};
use leptos::prelude::*;

/// The `host:port` of the core this Panel is served by (and talks to) — the
/// authoritative, always-fresh answer to "which core am I connected to".
/// Empty string if unavailable.
fn current_host() -> String {
    web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default()
}

/// Strip the port from a `host[:port]`, handling IPv6 literals (`[::1]:port`).
/// Pure.
fn host_only(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.split(':').next().unwrap_or(host)
}

/// Whether a `host[:port]` names a loopback core. Pure.
fn is_loopback_host(host: &str) -> bool {
    let h = host_only(host);
    h.eq_ignore_ascii_case("localhost") || h == "::1" || h.starts_with("127.")
}

#[component]
pub fn ConnectionSection() -> impl IntoView {
    let i18n = use_i18n();

    // Computed once: the origin never changes for the life of a page. A full app
    // is always served from its loopback embedded core, a lite shell from its
    // remote core, a browser from whatever the user typed — so the origin alone
    // is an honest local/remote signal. No signals, no IPC, no shell marker.
    let host = current_host();
    let host_present = !host.is_empty();
    let remote = host_present && !is_loopback_host(&host);

    view! {
        <section class="space-y-4">
            <div>
                <h2 class="text-lg font-semibold text-text-primary mb-1">
                    {t!(i18n, settings.network.section_title)}
                </h2>
                <p class="text-sm text-text-secondary">
                    {t!(i18n, settings.network.description)}
                </p>
            </div>

            <Show when=move || host_present>
                <div class="bg-surface-raised rounded-lg border border-border p-6">
                    <div class="flex items-center gap-2 text-sm">
                        <span class="text-text-secondary">
                            {t!(i18n, settings.network.connected_label)}
                        </span>
                        <span class="font-mono text-text-primary">{host.clone()}</span>
                        <span class=move || {
                            if remote {
                                "px-2 py-0.5 rounded-full text-xs bg-warning/15 text-warning"
                            } else {
                                "px-2 py-0.5 rounded-full text-xs bg-success/15 text-success"
                            }
                        }>
                            {if remote {
                                view! { {t!(i18n, settings.network.badge_remote)} }.into_any()
                            } else {
                                view! { {t!(i18n, settings.network.badge_local)} }.into_any()
                            }}
                        </span>
                    </div>
                </div>
            </Show>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::{host_only, is_loopback_host};

    #[test]
    fn host_only_strips_port_and_handles_ipv6() {
        assert_eq!(host_only("127.0.0.1:18790"), "127.0.0.1");
        assert_eq!(host_only("box.lan"), "box.lan");
        assert_eq!(host_only("[::1]:18790"), "::1");
        assert_eq!(host_only("[fe80::1]"), "fe80::1");
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1:18790"));
        assert!(is_loopback_host("127.5.6.7"));
        assert!(is_loopback_host("localhost:18790"));
        assert!(is_loopback_host("LocalHost"));
        assert!(is_loopback_host("[::1]:18790"));
        assert!(!is_loopback_host("172.245.43.211:18790"));
        assert!(!is_loopback_host("core.example:18790"));
    }
}
