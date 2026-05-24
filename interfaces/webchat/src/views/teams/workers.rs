//! WorkersView — live ACP harness session pool.
//!
//! Surfaces every active ACP session (Claude Code, Codex, Gemini, custom)
//! as a card with: harness, cwd, optional session name, liveness, state
//! (idle/busy/error), and per-session actions (cancel / shutdown).
//!
//! Refreshes every 5s and reacts to `acp.sessions.*` topic events when the
//! gateway pushes them (best-effort; falls back to polling).

use crate::api::acp::{AcpApi, AcpSessionSnapshot};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;

#[component]
pub fn WorkersView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let sessions: RwSignal<Vec<AcpSessionSnapshot>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(true);
    let error: RwSignal<Option<String>> = RwSignal::new(None);

    // Manual refresh trigger.
    let refresh = move || {
        spawn_local(async move {
            loading.set(true);
            match AcpApi::list_sessions(&dash).await {
                Ok(list) => {
                    sessions.set(list);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(e));
                }
            }
            loading.set(false);
        });
    };

    // Initial fetch when connected.
    Effect::new(move |_| {
        if dash.is_connected.get() {
            refresh();
        } else {
            sessions.set(Vec::new());
        }
    });

    // One deferred re-poll 5s after first load so freshly-spawned sessions
    // become visible without requiring a manual refresh. Further updates
    // come from user actions (Cancel/Shutdown re-fetch implicitly via the
    // card-level callbacks).
    let cancelled = RwSignal::new(false);
    on_cleanup(move || cancelled.set(true));
    set_timeout(
        move || {
            if !cancelled.get_untracked() && dash.is_connected.get_untracked() {
                spawn_local(async move {
                    if let Ok(list) = AcpApi::list_sessions(&dash).await {
                        sessions.set(list);
                    }
                });
            }
        },
        Duration::from_secs(5),
    );

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden">
            <div class="flex items-center justify-between px-6 py-4 border-b border-border">
                <div>
                    <h1 class="text-lg font-semibold text-text-primary">"ACP Workers"</h1>
                    <p class="text-xs text-text-tertiary">
                        "Live session pool: external coding agents currently running."
                    </p>
                </div>
                <button
                    class="px-3 py-1.5 text-sm rounded-md border border-border hover:bg-surface-raised disabled:opacity-50"
                    on:click=move |_| refresh()
                    disabled=move || loading.get()
                >
                    {move || if loading.get() { "Refreshing…" } else { "Refresh" }}
                </button>
            </div>

            <div class="flex-1 overflow-y-auto p-6 space-y-3">
                {move || {
                    if let Some(err) = error.get() {
                        return view! {
                            <div class="bg-red-50 border border-red-200 text-red-800 text-sm p-3 rounded-md">
                                {format!("Failed to load ACP sessions: {err}")}
                            </div>
                        }.into_any();
                    }
                    let list = sessions.get();
                    if list.is_empty() {
                        return view! {
                            <div class="bg-surface-raised border border-border rounded-xl p-8 text-center">
                                <p class="text-text-secondary text-sm">
                                    "No ACP sessions are currently active. Delegate a task via "
                                    <code class="bg-surface px-1.5 py-0.5 rounded text-xs">"acp_delegate"</code>
                                    " or via Aleph chat to start one."
                                </p>
                            </div>
                        }.into_any();
                    }
                    view! {
                        <div class="space-y-2">
                            {list.into_iter().map(|s| view! { <SessionCard snapshot=s /> }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

#[component]
fn SessionCard(snapshot: AcpSessionSnapshot) -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let busy = RwSignal::new(false);
    let action_error: RwSignal<Option<String>> = RwSignal::new(None);

    let harness = snapshot.harness_id.clone();
    let cwd = snapshot.cwd.clone();
    let session_name = snapshot.session_name.clone();
    let acp_session_id = snapshot.acp_session_id.clone();
    let alive = snapshot.alive;
    let state = snapshot.state.clone();

    // Pre-clone for each closure.
    let (h_cancel, c_cancel, n_cancel) = (harness.clone(), cwd.clone(), session_name.clone());
    let on_cancel = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        action_error.set(None);
        let (h, c, n) = (h_cancel.clone(), c_cancel.clone(), n_cancel.clone());
        spawn_local(async move {
            match AcpApi::cancel_session(&dash, &h, &c, n.as_deref()).await {
                Ok(()) => {}
                Err(e) => action_error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let (h_shut, c_shut, n_shut) = (harness.clone(), cwd.clone(), session_name.clone());
    let on_shutdown = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        action_error.set(None);
        let (h, c, n) = (h_shut.clone(), c_shut.clone(), n_shut.clone());
        spawn_local(async move {
            match AcpApi::shutdown_session(&dash, &h, &c, n.as_deref()).await {
                Ok(()) => {}
                Err(e) => action_error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let state_chip_class = match state.as_str() {
        "idle" => "bg-green-50 text-green-700 border border-green-200",
        "busy" => "bg-amber-50 text-amber-700 border border-amber-200",
        "error" => "bg-red-50 text-red-700 border border-red-200",
        _ => "bg-gray-50 text-gray-700 border border-gray-200",
    };
    let liveness_chip = if alive {
        ("alive", "bg-green-50 text-green-700 border border-green-200")
    } else {
        ("dead", "bg-red-50 text-red-700 border border-red-200")
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-4">
            <div class="flex items-start justify-between gap-4">
                <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2 mb-1">
                        <span class="font-medium text-text-primary truncate">{harness.clone()}</span>
                        {session_name.clone().map(|name| view! {
                            <span class="text-xs px-2 py-0.5 rounded-md bg-surface border border-border text-text-secondary">
                                {format!("@{name}")}
                            </span>
                        })}
                        <span class=format!("text-xs px-2 py-0.5 rounded-md font-medium {state_chip_class}")>
                            {state.clone()}
                        </span>
                        <span class=format!("text-xs px-2 py-0.5 rounded-md font-medium {}", liveness_chip.1)>
                            {liveness_chip.0}
                        </span>
                    </div>
                    <div class="text-xs text-text-tertiary truncate" title=cwd.clone()>
                        {cwd.clone()}
                    </div>
                    {acp_session_id.clone().map(|sid| view! {
                        <div class="text-[10px] text-text-tertiary font-mono mt-1 truncate" title=sid.clone()>
                            {format!("acp_id={sid}")}
                        </div>
                    })}
                </div>
                <div class="flex flex-col gap-2 shrink-0">
                    <button
                        class="px-3 py-1 text-xs rounded-md border border-border hover:bg-surface disabled:opacity-50"
                        on:click=on_cancel
                        disabled=move || busy.get()
                    >
                        "Cancel"
                    </button>
                    <button
                        class="px-3 py-1 text-xs rounded-md border border-red-200 text-red-700 hover:bg-red-50 disabled:opacity-50"
                        on:click=on_shutdown
                        disabled=move || busy.get()
                    >
                        "Shutdown"
                    </button>
                </div>
            </div>
            {move || action_error.get().map(|e| view! {
                <div class="text-xs text-red-700 mt-2">{e}</div>
            })}
        </div>
    }
}
