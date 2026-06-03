//! Runtimes dashboard view — read-only runtime status + one-click install.

use crate::api::runtimes::{RuntimeInfo, RuntimeStatus, RuntimesApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn RuntimesView() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loading = RwSignal::new(true);
    let error_msg = RwSignal::new(Option::<String>::None);

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
                                children=move |r| view! { <RuntimeCard info=r /> }
                            />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn RuntimeCard(info: RuntimeInfo) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let installing = RwSignal::new(false);
    let name = info.name.clone();

    let (icon, icon_class) = match info.status {
        RuntimeStatus::Ready => ("✓", "text-success"),
        RuntimeStatus::Missing if info.supported_on_current_os => ("✗", "text-text-tertiary"),
        RuntimeStatus::Missing => ("⊘", "text-text-tertiary"),
        RuntimeStatus::Probing => ("…", "text-text-tertiary"),
        RuntimeStatus::Bootstrapping => ("…", "text-info"),
        RuntimeStatus::Stale => ("?", "text-warning"),
    };

    let can_install = matches!(info.status, RuntimeStatus::Missing) && info.supported_on_current_os;

    let install_handler = {
        let name_clone = name.clone();
        move |_| {
            installing.set(true);
            let state = state;
            let n = name_clone.clone();
            spawn_local(async move {
                let _ = RuntimesApi::install(&state, &n).await;
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
                        disabled=move || installing.get()
                        class="px-3 py-1.5 bg-primary text-white rounded text-sm font-medium disabled:opacity-50"
                    >
                        {move || if installing.get() {
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
        </div>
    }
}
