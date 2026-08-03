//! Runtimes dashboard view — read-only runtime status + one-click install.

use crate::api::runtimes::{
    InstallProgress, RuntimeInfo, RuntimeStatus, RuntimesApi, RUNTIME_INSTALL_PROGRESS_TOPIC,
};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

#[component]
#[must_use]
pub fn RuntimesView() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loading = RwSignal::new(true);
    let error_msg = RwSignal::new(Option::<String>::None);
    // Live install progress, keyed by capability name. Fed by the
    // `runtimes.install.progress` event stream below.
    let progress = RwSignal::new(HashMap::<String, InstallProgress>::new());

    {
        spawn_local(async move {
            match RuntimesApi::list(&state).await {
                Ok(r) => {
                    runtimes.set(r.runtimes);
                    error_msg.set(None);
                }
                Err(e) => error_msg.set(Some(e)),
            }
            loading.set(false);
        });
    }

    // Subscribe to live install progress and reflect it on the cards. The
    // backend pushes `started`/`done`/`failed` events; on `done` we
    // refresh the ledger so the card flips to its new (Ready/Stale) status.
    {
        let handler_id = state.subscribe_events(move |ev| {
            if ev.topic != RUNTIME_INSTALL_PROGRESS_TOPIC {
                return;
            }
            let Ok(p) = serde_json::from_value::<InstallProgress>(ev.data) else {
                return;
            };
            match p.status.as_str() {
                "done" => {
                    progress.update(|m| {
                        m.remove(&p.step);
                    });
                    spawn_local(async move {
                        if let Ok(r) = RuntimesApi::list(&state).await {
                            runtimes.set(r.runtimes);
                        }
                    });
                }
                _ => {
                    progress.update(|m| {
                        m.insert(p.step.clone(), p);
                    });
                }
            }
        });

        spawn_local(async move {
            let _ = state.subscribe_topic(RUNTIME_INSTALL_PROGRESS_TOPIC).await;
        });

        on_cleanup(move || {
            state.unsubscribe_events(handler_id);
            spawn_local(async move {
                let _ = state
                    .unsubscribe_topic(RUNTIME_INSTALL_PROGRESS_TOPIC)
                    .await;
            });
        });
    }

    let refresh = {
        move |_| {
            loading.set(true);
            let state = state;
            spawn_local(async move {
                match RuntimesApi::refresh(&state).await {
                    Ok(r) => {
                        runtimes.set(r.runtimes);
                        error_msg.set(None);
                    }
                    Err(e) => error_msg.set(Some(e)),
                }
                loading.set(false);
            });
        }
    };

    view! {
        <div class="px-6 pb-6 aleph-content-top space-y-4">
            <div class="flex items-center justify-between">
                <div>
                    <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, runtimes.title)}</h1>
                    <p class="text-sm text-text-tertiary mt-1">
                        {t!(i18n, runtimes.description)}
                    </p>
                </div>
                <button
                    on:click=refresh
                    class="px-4 py-2 border border-border rounded-lg text-text-primary text-sm font-medium"
                >
                    {t!(i18n, runtimes.refresh)}
                </button>
            </div>

            {move || error_msg.get().map(|msg| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                    {msg}
                </div>
            })}

            {move || {
                if loading.get() {
                    view! { <div class="text-text-tertiary text-sm py-8">{t!(i18n, common.loading)}</div> }.into_any()
                } else {
                    view! {
                        <div class="space-y-3">
                            <For
                                each=move || runtimes.get()
                                key=|r| r.name.clone()
                                children=move |r| view! { <RuntimeCard info=r runtimes=runtimes error_msg=error_msg progress=progress /> }
                            />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn RuntimeCard(
    info: RuntimeInfo,
    runtimes: RwSignal<Vec<RuntimeInfo>>,
    error_msg: RwSignal<Option<String>>,
    progress: RwSignal<HashMap<String, InstallProgress>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let installing = RwSignal::new(false);
    let name = info.name.clone();

    // This card's live install progress, if any.
    let my_progress = {
        let name = info.name.clone();
        Signal::derive(move || progress.get().get(&name).cloned())
    };
    // An install is in flight while a non-terminal progress event is the latest
    // one for this capability (or the click handler is mid-flight).
    let is_active = Signal::derive(move || {
        installing.get()
            || my_progress
                .get()
                .is_some_and(|p| p.status == "started")
    });

    let (icon, icon_class) = match info.status {
        RuntimeStatus::Ready => ("✓", "text-success"),
        RuntimeStatus::Missing if info.supported_on_current_os => ("✗", "text-text-tertiary"),
        RuntimeStatus::Missing => ("⊘", "text-text-tertiary"),
        RuntimeStatus::Bootstrapping => ("…", "text-info"),
        RuntimeStatus::Stale => ("?", "text-warning"),
    };

    let can_install = matches!(info.status, RuntimeStatus::Missing) && info.supported_on_current_os;

    let install_handler = {
        let name_clone = name;
        move |_| {
            installing.set(true);
            error_msg.set(None);
            let state = state;
            let n = name_clone.clone();
            spawn_local(async move {
                // The install runs in the background on the server; this call
                // returns once it's accepted. Refresh the ledger so the card
                // reflects the new status (Bootstrapping → Ready / Stale).
                match RuntimesApi::install(&state, &n).await {
                    Ok(_) => match RuntimesApi::list(&state).await {
                        Ok(r) => runtimes.set(r.runtimes),
                        Err(e) => error_msg.set(Some(e)),
                    },
                    Err(e) => error_msg.set(Some(e)),
                }
                installing.set(false);
            });
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-4">
            <div class="flex items-start justify-between gap-4">
                <div class="flex items-start gap-3 flex-1 min-w-0">
                    <span class=format!("w-5 text-center font-mono text-lg {icon_class}")>{icon}</span>
                    <div class="flex-1 min-w-0">
                        <div class="flex items-baseline gap-2">
                            <span class="font-medium text-text-primary">{info.name.clone()}</span>
                            {info.version.clone().map(|v| view! {
                                <span class="text-xs text-text-tertiary">{v}</span>
                            })}
                        </div>
                        {info.bin_path.clone().map(|p| view! {
                            <div class="text-xs text-text-tertiary font-mono truncate mt-1">{p}</div>
                        })}
                        {info.llm_hint.clone().map(|h| view! {
                            <div class="text-xs text-text-tertiary mt-1">{h}</div>
                        })}
                        {(!info.deps.is_empty()).then(|| view! {
                            <div class="text-xs text-text-tertiary mt-1">
                                {t!(i18n, runtimes.deps_prefix)} " " {info.deps.join(", ")}
                            </div>
                        })}
                    </div>
                </div>
                {can_install.then(|| view! {
                    <button
                        on:click=install_handler
                        disabled=move || is_active.get()
                        class="px-3 py-1.5 bg-primary text-white rounded text-sm font-medium disabled:opacity-50"
                    >
                        {move || if is_active.get() {
                            t_string!(i18n, runtimes.installing).to_string()
                        } else {
                            t_string!(i18n, runtimes.install).to_string()
                        }}
                    </button>
                })}
                {(!info.supported_on_current_os
                    && matches!(info.status, RuntimeStatus::Missing)).then(|| view! {
                    <span class="text-xs text-text-tertiary italic">{t!(i18n, runtimes.install_manually)}</span>
                })}
            </div>
            // Live install progress strip: an indeterminate bar while
            // running, or the real error/stderr on failure.
            {move || my_progress.get().map(|p| {
                if p.status == "failed" {
                    let detail = p.error.clone().or(p.stderr).unwrap_or_default();
                    view! {
                        <div class="mt-3 text-xs text-danger break-words">{detail}</div>
                    }.into_any()
                } else {
                    view! {
                        <div class="mt-3 space-y-1">
                            <div class="h-1 w-full bg-surface-sunken rounded overflow-hidden">
                                <div class="h-full w-1/3 bg-primary rounded animate-pulse"></div>
                            </div>
                            <div class="text-xs text-text-tertiary truncate">
                                {t!(i18n, runtimes.installing)}
                            </div>
                        </div>
                    }.into_any()
                }
            })}
        </div>
    }
}
