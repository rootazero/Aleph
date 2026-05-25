use crate::context::DashboardState;
use crate::i18n::*;
use crate::state::connection::ConnectionPhase;
use leptos::prelude::*;

/// Compact connection-state chip rendered in the dashboard chrome.
///
/// Reads the four orthogonal signals on `DashboardState` and projects them
/// into a single `ConnectionPhase` so dot colour, label, and counter all
/// move together. The phase enum is the source of truth — see
/// [`crate::state::connection`] for the derivation rules and tests.
#[component]
pub fn ConnectionStatus() -> impl IntoView {
    let state = use_context::<DashboardState>().expect("DashboardState not provided");
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
        ConnectionPhase::Reconnecting { attempt, max } => Some(format!("{}/{}", attempt, max)),
        ConnectionPhase::Failed { reason } => Some(reason),
        _ => None,
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-2xl p-4">
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-3">
                    <div class=move || format!("w-2 h-2 rounded-full {}", dot_class())></div>
                    <span class="text-sm font-medium">{status_text}</span>
                </div>

                {move || detail_text().map(|s| {
                    // Split the owned String into a title attribute (clone)
                    // and a child text node (move) — the view! macro takes
                    // ownership for both, so we can't reuse the binding.
                    let title = s.clone();
                    view! {
                        <div class="text-xs text-text-tertiary truncate max-w-[16rem]" title=title>
                            {s}
                        </div>
                    }
                })}
            </div>
        </div>
    }
}
