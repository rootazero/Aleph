//! `WorkersView` — live ACP harness session pool.
//!
//! Surfaces every active ACP session (Claude Code, Codex, Gemini, custom)
//! as a card with: harness, cwd, optional session name, liveness, state
//! (idle/busy/error), and per-session actions (cancel / shutdown).
//!
//! Subscribes to the gateway topic `acp.sessions.changed` and re-fetches
//! `acp.sessions.list` on every signal — no polling. A manual refresh button
//! handles the rare case where the broadcast is missed (reconnect window).

use crate::api::acp::{AcpApi, AcpSessionSnapshot};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
#[must_use]
pub fn WorkersView() -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();
    let sessions: RwSignal<Vec<AcpSessionSnapshot>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(true);
    let error: RwSignal<Option<String>> = RwSignal::new(None);

    let refresh = move || {
        spawn_local(async move {
            loading.set(true);
            match AcpApi::list_sessions(&dash).await {
                Ok(list) => {
                    sessions.set(list);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| e.to_string(),
                    )));
                }
            }
            loading.set(false);
        });
    };

    Effect::new(move |_| {
        if dash.is_connected.get() {
            refresh();
        } else {
            sessions.set(Vec::new());
        }
    });

    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic("acp.sessions.changed").await;
        });
    });

    let sub_id = dash.subscribe_events(move |evt| {
        if evt.topic == "acp.sessions.changed" {
            refresh();
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden">
            <div class="flex items-center justify-between px-6 py-4 border-b border-border aleph-content-top">
                <div>
                    <h1 class="text-lg font-semibold text-text-primary">{t!(i18n, teams.workers_title)}</h1>
                    <p class="text-xs text-text-tertiary">
                        {t!(i18n, teams.workers_desc)}
                    </p>
                </div>
                <button
                    class="px-3 py-1.5 text-sm rounded-md border border-border hover:bg-surface-raised disabled:opacity-50"
                    on:click=move |_| refresh()
                    disabled=move || loading.get()
                >
                    {move || if loading.get() {
                        t_string!(i18n, teams.workers_refreshing).to_string()
                    } else {
                        t_string!(i18n, teams.workers_refresh).to_string()
                    }}
                </button>
            </div>

            <div class="flex-1 overflow-y-auto p-6 space-y-3">
                {move || {
                    if let Some(err) = error.get() {
                        let prefix = t_string!(i18n, teams.workers_load_failed).to_string();
                        return view! {
                            <div class="bg-red-50 border border-red-200 text-red-800 text-sm p-3 rounded-md">
                                {format!("{prefix} {err}")}
                            </div>
                        }.into_any();
                    }
                    let list = sessions.get();
                    if list.is_empty() {
                        return view! {
                            <div class="bg-surface-raised border border-border rounded-xl p-8 text-center">
                                <p class="text-text-secondary text-sm">
                                    {t!(i18n, teams.workers_no_sessions_prefix)}
                                    <code class="bg-surface px-1.5 py-0.5 rounded text-xs">"acp_delegate"</code>
                                    {t!(i18n, teams.workers_no_sessions_suffix)}
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
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();
    let busy = RwSignal::new(false);
    let action_error: RwSignal<Option<String>> = RwSignal::new(None);

    let harness = snapshot.harness_id.clone();
    let cwd = snapshot.cwd.clone();
    let session_name = snapshot.session_name.clone();
    let acp_session_id = snapshot.acp_session_id.clone();
    let alive = snapshot.alive;
    let state = snapshot.state;

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
                Err(e) => action_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
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
                Err(e) => action_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
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
    let alive_label = t_string!(i18n, teams.workers_state_alive).to_string();
    let dead_label = t_string!(i18n, teams.workers_state_dead).to_string();
    let liveness_chip = if alive {
        (
            alive_label,
            "bg-green-50 text-green-700 border border-green-200",
        )
    } else {
        (dead_label, "bg-red-50 text-red-700 border border-red-200")
    };

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-4">
            <div class="flex items-start justify-between gap-4">
                <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2 mb-1">
                        <span class="font-medium text-text-primary truncate">{harness}</span>
                        {session_name.map(|name| view! {
                            <span class="text-xs px-2 py-0.5 rounded-md bg-surface border border-border text-text-secondary">
                                {format!("@{name}")}
                            </span>
                        })}
                        <span class=format!("text-xs px-2 py-0.5 rounded-md font-medium {state_chip_class}")>
                            {state}
                        </span>
                        <span class=format!("text-xs px-2 py-0.5 rounded-md font-medium {}", liveness_chip.1)>
                            {liveness_chip.0}
                        </span>
                    </div>
                    <div class="text-xs text-text-tertiary truncate" title=cwd>
                        {cwd.clone()}
                    </div>
                    {acp_session_id.map(|sid| view! {
                        <div class="text-[10px] text-text-tertiary font-mono mt-1 truncate" title=sid>
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
                        {t!(i18n, teams.workers_cancel)}
                    </button>
                    <button
                        class="px-3 py-1 text-xs rounded-md border border-red-200 text-red-700 hover:bg-red-50 disabled:opacity-50"
                        on:click=on_shutdown
                        disabled=move || busy.get()
                    >
                        {t!(i18n, teams.workers_shutdown)}
                    </button>
                </div>
            </div>
            {move || action_error.get().map(|e| view! {
                <div class="text-xs text-red-700 mt-2">{e}</div>
            })}
        </div>
    }
}
