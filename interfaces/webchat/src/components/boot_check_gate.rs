//! `BootCheckGate` — pre-app-shell overlay that blocks rendering of the main
//! UI until the panel has connected to the Gateway at least once.
//!
//! Aleph's desktop App is a single-core single-window Tauri build, so this
//! gate intentionally omits openhuman's Local/Cloud mode picker (R3 — no
//! choice the user doesn't actually have). What it keeps:
//!   * A "Connecting…" overlay while the first probe is in flight, so the
//!     user doesn't see an empty/broken shell that hasn't loaded data yet.
//!   * A "Cannot reach core" trouble screen with a Retry button when the
//!     probe fails — the existing `connect()` path on app.rs only logs to the
//!     console, leaving the user with no recovery affordance.
//!
//! Once `has_connected_once` latches true, the gate disengages permanently
//! for this session — runtime drops are handled by [`ServiceBlockingGate`].

use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::connection::ConnectionPhase;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Pure visibility predicate for the boot gate. The gate blocks the shell only
/// while we are still booting **and** have not landed on the login wall:
///   * `has_connected_once` — a first successful connect latched; runtime drops
///     are ServiceBlockingGate's job, so never gate again.
///   * `is_connected` — currently connected; nothing to gate.
///   * `needs_token` — reachable-but-walled (a remote, unauthorized Panel). The
///     core IS reachable, so a "cannot reach core" trouble screen would be a
///     lie; the login wall (`TokenWall`, mounted at the shell root) owns the
///     screen. The gate MUST yield here — otherwise its higher z-index buries
///     the token box and the user can never authorize.
#[must_use]
pub(crate) fn boot_gate_visible(
    has_connected_once: bool,
    is_connected: bool,
    needs_token: bool,
) -> bool {
    !has_connected_once && !is_connected && !needs_token
}

/// Overlay-only — the surrounding shell renders unconditionally, and this
/// component is mounted as a sibling whose `<Show>` controls visibility.
/// No children pass-through (that pattern is a React idiom; Leptos parents
/// own their subtree directly).
#[component]
#[must_use]
pub fn BootCheckGate() -> impl IntoView {
    let Some(state) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    let i18n = use_i18n();

    // The gate disengages permanently once we've ever connected. After
    // that, ServiceBlockingGate owns runtime recovery.
    let has_connected_once = state.has_connected_once;
    let failure = state.connection_failure;

    // Derived: should the overlay be visible at all? We hide on three signals:
    //   * Connected at least once (handed off to ServiceBlockingGate)
    //   * Currently connected (caught up to ready state)
    //   * Awaiting a token (reachable-but-walled — TokenWall owns the screen)
    let show_gate = Memo::new(move |_| {
        boot_gate_visible(
            has_connected_once.get(),
            state.is_connected.get(),
            state.needs_token.get(),
        )
    });

    // Inner phase — only consulted when show_gate=true, but recomputed
    // anyway because Memo dependencies must be reactive.
    let phase = Memo::new(move |_| {
        let error = state.connection_error.get();
        ConnectionPhase::derive(
            state.is_connected.get(),
            state.is_reconnecting.get(),
            state.reconnect_count.get(),
            error.as_deref(),
            has_connected_once.get(),
        )
    });

    let is_retrying = RwSignal::new(false);

    view! {
        <Show when=move || show_gate.get() fallback=|| ()>
            <div
                class="fixed inset-0 z-[9000] flex items-center justify-center bg-surface/95 aleph-scrim p-4"
                role="dialog"
                aria-modal="true"
            >
                <div class="w-full max-w-md rounded-2xl border border-border bg-surface-raised p-6 shadow-2xl">
                    {move || match phase.get() {
                        ConnectionPhase::Failed { .. } => {
                            view! {
                                <h2 class="text-xl font-semibold text-text-primary">
                                    {move || match failure.get() {
                                        Some(shared_ui_logic::connection::ConnectionFailure::Timeout { .. }) => {
                                            t_string!(i18n, conn_error.timeout_title).to_string()
                                        }
                                        _ => t_string!(i18n, conn_error.unreachable_title).to_string(),
                                    }}
                                </h2>
                                <p class="mt-2 text-sm text-text-secondary">
                                    {move || match failure.get() {
                                        Some(shared_ui_logic::connection::ConnectionFailure::Timeout { .. }) => {
                                            t_string!(i18n, conn_error.timeout_body).to_string()
                                        }
                                        _ => t_string!(i18n, conn_error.unreachable_body).to_string(),
                                    }}
                                </p>
                                <p class="mt-3 text-xs text-text-tertiary">
                                    {move || t_string!(i18n, boot_gate.trouble_hint).to_string()}
                                </p>
                                <div class="mt-3 rounded-lg border border-danger/20 bg-danger-subtle p-3 text-xs font-mono text-danger break-all">
                                    {move || match failure.get() {
                                        Some(shared_ui_logic::connection::ConnectionFailure::Unreachable { detail })
                                        | Some(shared_ui_logic::connection::ConnectionFailure::Timeout { detail })
                                        | Some(shared_ui_logic::connection::ConnectionFailure::Dropped { detail })
                                        | Some(shared_ui_logic::connection::ConnectionFailure::Unknown { detail }) => detail,
                                        _ => String::new(),
                                    }}
                                </div>
                                <div class="mt-5 flex justify-end">
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            if is_retrying.get_untracked() { return; }
                                            is_retrying.set(true);
                                            spawn_local(async move {
                                                // reconnect() drives is_reconnecting and reconnect_count
                                                // internally and sets has_connected_once on success —
                                                // phase derivation picks it up and the gate disengages.
                                                let _ = state.reconnect().await;
                                                is_retrying.set(false);
                                            });
                                        }
                                        disabled=move || is_retrying.get()
                                        class="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-white hover:bg-primary-hover disabled:opacity-60"
                                    >
                                        {move || if is_retrying.get() {
                                            t_string!(i18n, boot_gate.retrying).to_string()
                                        } else {
                                            t_string!(i18n, boot_gate.retry).to_string()
                                        }}
                                    </button>
                                </div>
                            }.into_any()
                        }
                        _ => view! {
                            <div class="flex flex-col items-center gap-4 py-2">
                                <div class="h-8 w-8 animate-spin rounded-full border-2 border-border border-t-primary" />
                                <h2 class="text-lg font-semibold text-text-primary">
                                    {move || t_string!(i18n, boot_gate.checking_title).to_string()}
                                </h2>
                                <p class="text-sm text-text-secondary text-center">
                                    {move || t_string!(i18n, boot_gate.checking_body).to_string()}
                                </p>
                            </div>
                        }.into_any()
                    }}
                </div>
            </div>
        </Show>
    }.into_any()
}

#[cfg(test)]
mod tests {
    use super::boot_gate_visible;

    #[test]
    fn booting_and_never_connected_gates_the_shell() {
        // Cold start, no connect yet, not walled → show the "Connecting…" gate.
        assert!(boot_gate_visible(false, false, false));
    }

    #[test]
    fn connected_once_never_gates_again() {
        // Latched a first connect → ServiceBlockingGate owns runtime drops.
        assert!(!boot_gate_visible(true, false, false));
    }

    #[test]
    fn currently_connected_never_gates() {
        assert!(!boot_gate_visible(false, true, false));
    }

    #[test]
    fn walled_yields_to_token_wall() {
        // Regression: a remote unauthorized Panel is reachable-but-walled. The
        // boot gate (z-9000) must yield so the login wall (TokenWall, z-100) is
        // visible and clickable; otherwise the token box is buried and the user
        // can never authorize. This is the "server not found covers the token box" bug.
        assert!(!boot_gate_visible(false, false, true));
    }
}
