//
// Session status bar — a slim footer for the chat sidebar showing the
// gateway connection state and the live count of active agent runs.
//
// Data sources (both existing infrastructure, zero new backend):
//   • gateway state ← DashboardState.is_connected / connection_error
//     (same derivation as the dashboard Home view).
//   • active run count ← SessionMap.running_session_count(), the SAME
//     server-authoritative `server_running` set that lights the per-row red
//     dot. Reading one signal makes the footer count and the dots update
//     together on every `RunningSetChanged` event — no divergence, no lag.
//     (The prior implementation polled `activity.stats` on a 10s timer, an
//     independent source that drifted out of sync with the dots.)
//
use leptos::prelude::*;

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::sessions::SessionMap;

#[component]
#[must_use]
pub fn SessionStatusBar() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let sessions = expect_context::<SessionMap>();
    let i18n = use_i18n();

    // Gateway state string + tone, derived reactively (matches Home).
    let gateway_label = move || {
        if dash.is_connected.get() {
            t_string!(i18n, session.healthy).to_string()
        } else if dash.connection_error.get().is_some() {
            t_string!(i18n, session.degraded).to_string()
        } else {
            t_string!(i18n, session.disconnected).to_string()
        }
    };
    let dot_class = move || {
        let base = "w-1.5 h-1.5 rounded-full flex-shrink-0 ";
        let tone = if dash.is_connected.get() {
            "bg-green-500"
        } else if dash.connection_error.get().is_some() {
            "bg-amber-500"
        } else {
            "bg-text-tertiary"
        };
        format!("{base}{tone}")
    };

    // Active run count = size of the server-authoritative running set. Tracked
    // read → re-renders in lockstep with the sidebar dots on every event.
    let active_count = move || sessions.running_session_count().to_string();

    view! {
        <div class="flex items-center justify-between px-3 py-2 border-t border-border
                    text-[10px] text-text-tertiary uppercase tracking-wider">
            <div class="flex items-center gap-1.5">
                <span class=dot_class />
                <span>{gateway_label}</span>
            </div>
            <div class="flex items-center gap-1 tabular-nums normal-case">
                <span>{active_count}</span>
                <span class="text-text-tertiary/70">{t!(i18n, session.active)}</span>
            </div>
        </div>
    }
}
