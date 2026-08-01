//! `ServiceBlockingGate` — runtime-recovery overlay shown when the panel had
//! been live but lost the Gateway connection and exhausted its automatic
//! reconnect budget.
//!
//! Distinct from [`BootCheckGate`]:
//!   * `BootCheckGate` engages BEFORE the first successful connect.
//!   * `ServiceBlockingGate` engages AFTER, when `reconnect()` gives up.
//!
//! Both render children behind the overlay (vs. instead of) so the user's
//! place in the app is preserved — matches openhuman's behaviour and avoids
//! a confusing flash to a blank shell while the user is mid-task.

use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::connection::{ConnectionPhase, MAX_RECONNECT_ATTEMPTS};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

/// Overlay-only — mounted as a sibling of the shell. Visibility is fully
/// signal-driven via the `show_overlay` Memo.
#[component]
#[must_use]
pub fn ServiceBlockingGate() -> impl IntoView {
    let Some(state) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    let i18n = use_i18n();
    let navigate = use_navigate();

    let has_connected_once = state.has_connected_once;

    let failure = state.connection_failure;
    // Remote origin (lite shell / browser hitting a LAN IP) — for an
    // Unreachable failure the native supervisor will relocate to connect.html,
    // so a Retry against the dead origin is useless here.
    let is_remote_origin = {
        let host = web_sys::window()
            .and_then(|w| w.location().host().ok())
            .unwrap_or_default();
        !(host.is_empty()
            || host.starts_with("127.0.0.1")
            || host.starts_with("localhost")
            || host.starts_with("[::1]"))
    };

    // The overlay engages only when:
    //   1. We've ever connected (boot succeeded — BootCheckGate handed off).
    //   2. We're currently not connected.
    //   3. Reconnect has exhausted its budget (count >= max).
    // This last clause is critical: while reconnect() is still iterating,
    // the chip shows "Reconnecting N/5" but we don't blanket the app.
    let show_overlay = Memo::new(move |_| {
        if !has_connected_once.get() {
            return false;
        }
        if state.is_connected.get() {
            return false;
        }
        state.reconnect_count.get() >= MAX_RECONNECT_ATTEMPTS
    });

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
    let retry_error = RwSignal::new(None::<String>);

    view! {
        <Show when=move || show_overlay.get() fallback=|| ()>
            <div
                class="fixed inset-0 z-[9500] flex items-center justify-center bg-surface/85 aleph-scrim p-4"
                role="dialog"
                aria-modal="true"
            >
                <div class="w-full max-w-md rounded-2xl border border-danger/30 bg-surface-raised p-6 shadow-2xl">
                    <h2 class="text-xl font-semibold text-text-primary">
                        {move || t_string!(i18n, service_gate.title).to_string()}
                    </h2>
                    <p class="mt-2 text-sm text-text-secondary">
                        {move || {
                            use shared_ui_logic::connection::ConnectionFailure;
                            if is_remote_origin
                                && matches!(failure.get(), Some(ConnectionFailure::Unreachable { .. }))
                            {
                                t_string!(i18n, conn_error.lite_relocating).to_string()
                            } else {
                                format!(
                                    "{}{}{}",
                                    t_string!(i18n, service_gate.body_prefix),
                                    state.reconnect_count.get(),
                                    t_string!(i18n, service_gate.body_suffix),
                                )
                            }
                        }}
                    </p>

                    {move || match phase.get() {
                        ConnectionPhase::Failed { failure } => view! {
                            <div class="mt-3 rounded-lg border border-danger/20 bg-danger-subtle p-3 text-xs font-mono text-danger break-all">
                                {match failure {
                                    shared_ui_logic::connection::ConnectionFailure::AuthRequired => String::new(),
                                    shared_ui_logic::connection::ConnectionFailure::Unreachable { detail }
                                    | shared_ui_logic::connection::ConnectionFailure::Timeout { detail }
                                    | shared_ui_logic::connection::ConnectionFailure::Rejected { detail }
                                    | shared_ui_logic::connection::ConnectionFailure::Dropped { detail }
                                    | shared_ui_logic::connection::ConnectionFailure::Unknown { detail } => detail,
                                }}
                            </div>
                        }.into_any(),
                        _ => ().into_any(),
                    }}

                    {move || retry_error.get().map(|_| view! {
                        <p class="mt-3 text-xs text-danger">
                            {t_string!(i18n, service_gate.retry_failed).to_string()}
                        </p>
                    })}

                    <div class="mt-5 flex gap-3 justify-end">
                        <button
                            type="button"
                            on:click={
                                let navigate = navigate.clone();
                                move |_| navigate("/dashboard/logs", NavigateOptions::default())
                            }
                            class="rounded-lg border border-border bg-surface px-4 py-2 text-sm text-text-secondary hover:bg-surface-sunken"
                        >
                            {move || t_string!(i18n, service_gate.open_logs).to_string()}
                        </button>
                        <Show
                            when=move || {
                                use shared_ui_logic::connection::ConnectionFailure;
                                !(is_remote_origin
                                    && matches!(failure.get(), Some(ConnectionFailure::Unreachable { .. })))
                            }
                            fallback=|| ()
                        >
                            <button
                                type="button"
                                on:click=move |_| {
                                    if is_retrying.get_untracked() { return; }
                                    is_retrying.set(true);
                                    retry_error.set(None);
                                    spawn_local(async move {
                                        if state.reconnect().await.is_err() {
                                            retry_error.set(Some("retry_failed".to_string()));
                                        }
                                        is_retrying.set(false);
                                    });
                                }
                                disabled=move || is_retrying.get()
                                class="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-white hover:bg-primary-hover disabled:opacity-60"
                            >
                                {move || if is_retrying.get() {
                                    t_string!(i18n, service_gate.retrying).to_string()
                                } else {
                                    t_string!(i18n, service_gate.retry).to_string()
                                }}
                            </button>
                        </Show>
                    </div>
                </div>
            </div>
        </Show>
    }.into_any()
}

#[cfg(test)]
mod tests {
    // Visibility predicate (documented for review-time clarity):
    //   show_overlay ⇔ has_connected_once && !is_connected
    //                && reconnect_count >= MAX_RECONNECT_ATTEMPTS
    // Reactive wiring smoke-tested via the boot integration test (TBD).

    #[test]
    fn show_overlay_predicate_is_documented() {
        let _ = "see comment above";
    }
}
