use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::connection::ConnectionPhase;
use leptos::prelude::*;
use leptos_router::components::A;

/// Compact connection-state chip rendered in the dashboard chrome.
///
/// Reads the four orthogonal signals on `DashboardState` and projects them
/// into a single `ConnectionPhase` so dot colour, label, and counter all
/// move together. The phase enum is the source of truth — see
/// [`crate::state::connection`] for the derivation rules and tests.
///
/// The second line names the *core this Panel is talking to* — `Local` for a
/// loopback origin (the full app's embedded core) or the remote `host:port`.
/// It is read once on mount from the page origin: the origin that served this
/// Panel *is* the core it talks to (full app = loopback, lite shell = remote,
/// browser = whatever was typed). A target switch reloads the Panel, so a
/// fresh mount re-resolves.
#[component]
#[must_use]
pub fn ConnectionStatus() -> impl IntoView {
    let Some(state) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    let i18n = use_i18n();

    // Project the four signals into a single phase each render.
    let phase = Memo::new(move |_| {
        let error = state.connection_error.get();
        ConnectionPhase::derive(
            state.is_connected.get(),
            state.is_reconnecting.get(),
            state.reconnect_count.get(),
            error.as_deref(),
            state.has_connected_once.get(),
        )
    });

    // Which core is live (Local / remote host). Resolved once on mount; a
    // target switch reloads the whole Panel, so a fresh mount re-resolves.
    let target = RwSignal::new(resolve_target_label());

    let dot_class = move || match phase.get() {
        ConnectionPhase::Connected => "bg-success",
        ConnectionPhase::Reconnecting { .. } | ConnectionPhase::Connecting => {
            "bg-warning animate-pulse"
        }
        ConnectionPhase::Failed { .. } => "bg-danger",
        ConnectionPhase::Initial => "bg-text-tertiary",
    };

    let status_text = move || match phase.get() {
        ConnectionPhase::Connected => t_string!(i18n, common.connected).to_string(),
        ConnectionPhase::Connecting => t_string!(i18n, common.connecting).to_string(),
        ConnectionPhase::Reconnecting { .. } => t_string!(i18n, common.reconnecting).to_string(),
        ConnectionPhase::Failed { .. } => t_string!(i18n, common.connection_failed).to_string(),
        ConnectionPhase::Initial => t_string!(i18n, common.disconnected).to_string(),
    };

    // Sub-line below the label — varies by phase. Reconnecting shows the
    // attempt counter; Failed surfaces the underlying error verbatim (it
    // already comes from WebSocket/network layer in English, no i18n).
    let detail_text = move || match phase.get() {
        ConnectionPhase::Reconnecting { attempt, max } => Some(format!("{attempt}/{max}")),
        ConnectionPhase::Failed { failure } => Some(match failure {
            shared_ui_logic::connection::ConnectionFailure::AuthRequired => String::new(),
            shared_ui_logic::connection::ConnectionFailure::Unreachable { detail }
            | shared_ui_logic::connection::ConnectionFailure::Timeout { detail }
            | shared_ui_logic::connection::ConnectionFailure::Rejected { detail }
            | shared_ui_logic::connection::ConnectionFailure::Dropped { detail }
            | shared_ui_logic::connection::ConnectionFailure::Unknown { detail } => detail,
        }),
        _ => None,
    };

    view! {
        // Clicking the chip jumps to Settings → Service & Cluster, where the
        // connection target can be switched (shell-core separation).
        <A href="/settings/network"
            attr:class="block bg-surface-raised border border-border rounded-xl px-3 py-2.5 hover:border-primary/50 transition-colors">
            <div class="flex items-center justify-between gap-2">
                <div class="flex items-center gap-2 min-w-0">
                    <div class=move || format!("w-2 h-2 rounded-full shrink-0 {}", dot_class())></div>
                    <span class="text-sm font-medium truncate">{status_text}</span>
                </div>

                {move || detail_text().map(|s| {
                    // Split the owned String into a title attribute (clone)
                    // and a child text node (move) — the view! macro takes
                    // ownership for both, so we can't reuse the binding.
                    let title = s.clone();
                    view! {
                        <div class="text-xs text-text-tertiary truncate max-w-[10rem]" title=title>
                            {s}
                        </div>
                    }
                })}
            </div>

            // Target line — which core this Panel is connected to.
            {move || {
                let label = target.get();
                (!label.is_empty()).then(|| {
                    let title = label.clone();
                    view! {
                        <div class="flex items-center gap-1.5 mt-1.5 text-xs text-text-tertiary"
                            title=title>
                            // server / signal glyph
                            <svg class="w-3 h-3 shrink-0" viewBox="0 0 24 24" fill="none"
                                stroke="currentColor" stroke-width="2" stroke-linecap="round"
                                stroke-linejoin="round">
                                <rect x="2" y="2" width="20" height="8" rx="2" ry="2" />
                                <rect x="2" y="14" width="20" height="8" rx="2" ry="2" />
                                <line x1="6" y1="6" x2="6.01" y2="6" />
                                <line x1="6" y1="18" x2="6.01" y2="18" />
                            </svg>
                            <span class="truncate">{label}</span>
                        </div>
                    }
                })
            }}
        </A>
    }.into_any()
}

/// Resolve the human-readable label for the live core. The origin that served
/// this Panel IS the core it talks to — loopback (the full app's embedded core)
/// collapses to `Local`, a remote origin (lite shell / browser) shows its host.
fn resolve_target_label() -> String {
    let host = web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default();
    if host.is_empty() || is_loopback_host(&host) {
        "Local".to_string()
    } else {
        host
    }
}

/// Whether a `host[:port]` names the local loopback.
fn is_loopback_host(host: &str) -> bool {
    let bare = if let Some(rest) = host.strip_prefix('[') {
        // Bracketed IPv6 `[addr]:port` — the address is everything up to `]`
        // (it contains its own colons, so a plain `:` split would mangle it).
        rest.split(']').next().unwrap_or(rest)
    } else {
        // `host[:port]` — drop the optional port.
        host.split(':').next().unwrap_or(host)
    };
    bare == "127.0.0.1" || bare.eq_ignore_ascii_case("localhost") || bare == "::1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_host("127.0.0.1:18790"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("[::1]:18790"));
        assert!(!is_loopback_host("192.168.1.5:18790"));
        assert!(!is_loopback_host("gw.example.com"));
    }
}
