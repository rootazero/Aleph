//
// Session status bar — a slim footer for the chat sidebar showing the
// gateway connection state and the live count of active agent runs.
//
// Data sources (both existing infrastructure, zero new backend):
//   • gateway state ← DashboardState.is_connected / connection_error
//     (same derivation as the dashboard Home view).
//   • active run count ← `activity.stats` RPC (`active_total`), already
//     registered server-side and consumed by Home. Polled every 10s while
//     connected, mirroring the reference 10s status poll.
//
use leptos::prelude::*;

use crate::context::DashboardState;
use crate::i18n::*;

#[component]
pub fn SessionStatusBar() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let active_runs = RwSignal::new(Option::<u64>::None);
    // Bumped on each (re)connect so stale poll loops self-terminate.
    let poll_gen = RwSignal::new(0u32);

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

    // Poll activity.stats while connected: immediate fetch on connect, then
    // every 10s. Disconnect clears the count.
    Effect::new(move || {
        if dash.is_connected.get() {
            let my_gen = poll_gen.get_untracked().wrapping_add(1);
            poll_gen.set(my_gen);
            leptos::task::spawn_local(async move {
                // `poll_gen`/`active_runs` are owned by this component.
                // On tab switch the component is disposed (and these
                // signals with it), but this detached future keeps
                // running — `try_*` lets it notice the disposal and stop
                // instead of panicking on a dead signal.
                while let Some(gen) = poll_gen.try_get_untracked() {
                    if gen != my_gen || !dash.is_connected.get_untracked() {
                        break;
                    }
                    let count = match dash
                        .rpc_call("activity.stats", serde_json::Value::Null)
                        .await
                    {
                        Ok(result) => result
                            .get("active_total")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        Err(_) => 0,
                    };
                    // `try_set` returns `Some` only when the signal was
                    // already disposed (component gone during the await
                    // above) — stop rather than spin for another 10s.
                    if active_runs.try_set(Some(count)).is_some() {
                        break;
                    }
                    gloo_timers::future::TimeoutFuture::new(10_000).await;
                }
            });
        } else {
            active_runs.set(None);
        }
    });

    view! {
        <div class="flex items-center justify-between px-3 py-2 border-t border-border
                    text-[10px] text-text-tertiary uppercase tracking-wider">
            <div class="flex items-center gap-1.5">
                <span class=dot_class />
                <span>{gateway_label}</span>
            </div>
            <div class="flex items-center gap-1 tabular-nums normal-case">
                <span>{move || active_runs.get().map(|n| n.to_string()).unwrap_or_else(|| "–".to_string())}</span>
                <span class="text-text-tertiary/70">{t!(i18n, session.active)}</span>
            </div>
        </div>
    }
}
