//! Settings → Runtime: step-indicator list + Install / Refresh actions + install log.

use crate::api::runtimes::{RuntimeInfo, RuntimeStatus, RuntimesApi};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn RuntimeView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loading = RwSignal::new(true);
    let error_msg = RwSignal::new(Option::<String>::None);
    let log_lines = RwSignal::new(Vec::<String>::new());
    let busy = RwSignal::new(false);

    // Initial load on mount.
    {
        let state = state;
        spawn_local(async move {
            match RuntimesApi::list(&state).await {
                Ok(r) => {
                    runtimes.set(r.runtimes);
                    error_msg.set(None);
                }
                Err(e) => error_msg.set(Some(format!("Failed to load runtimes: {e}"))),
            }
            loading.set(false);
        });
    }

    let do_refresh = {
        let state = state;
        move |_ev: web_sys::MouseEvent| {
            let state = state;
            busy.set(true);
            spawn_local(async move {
                match RuntimesApi::refresh(&state).await {
                    Ok(r) => {
                        runtimes.set(r.runtimes);
                        log_lines.update(|v| v.push("▸ refreshed".into()));
                        error_msg.set(None);
                    }
                    Err(e) => error_msg.set(Some(format!("Refresh failed: {e}"))),
                }
                busy.set(false);
            });
        }
    };

    let do_install_missing = {
        let state = state;
        move |_ev: web_sys::MouseEvent| {
            let state = state;
            let targets: Vec<String> = runtimes
                .get()
                .iter()
                .filter(|r| r.status == RuntimeStatus::Missing && r.supported_on_current_os)
                .map(|r| r.name.clone())
                .collect();
            if targets.is_empty() {
                log_lines.update(|v| v.push("▸ nothing to install — all ready".into()));
                return;
            }
            busy.set(true);
            spawn_local(async move {
                for cap in &targets {
                    log_lines.update(|v| v.push(format!("▸ [{cap}] starting…")));
                    match RuntimesApi::install(&state, cap).await {
                        Ok(_) => {
                            log_lines.update(|v| v.push(format!("▸ [{cap}] accepted (see events)")))
                        }
                        Err(e) => log_lines.update(|v| v.push(format!("▸ [{cap}] FAILED: {e}"))),
                    }
                }
                match RuntimesApi::refresh(&state).await {
                    Ok(r) => {
                        runtimes.set(r.runtimes);
                        error_msg.set(None);
                    }
                    Err(e) => error_msg.set(Some(format!("Post-install refresh failed: {e}"))),
                }
                busy.set(false);
            });
        }
    };

    view! {
        <div class="p-6 space-y-6">
            <div>
                <h1 class="text-2xl font-bold text-text-primary">"Runtime"</h1>
                <p class="mt-1 text-sm text-text-tertiary">
                    "Bootstrap runtime dependencies that power browser automation, \
                     Python execution, and skills."
                </p>
            </div>

            {move || error_msg.get().map(|e| {
                let is_unavailable = e.contains("Send failed") || e.contains("Failed to load");
                if is_unavailable {
                    view! {
                        <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm">
                            "Gateway unavailable. Runtimes will load when connected."
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">
                            {e}
                        </div>
                    }.into_any()
                }
            })}

            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">"Loading..."</div>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="bg-surface-raised rounded-lg border border-border p-6 space-y-4">
                            <div class="space-y-2">
                                <For
                                    each=move || runtimes.get()
                                    key=|r| r.name.clone()
                                    children=move |r| view! { <RuntimeRow info=r /> }
                                />
                            </div>
                            <div class="flex items-center gap-2 pt-4 border-t border-border">
                                <button
                                    on:click=do_install_missing.clone()
                                    disabled=move || busy.get()
                                    class="px-3 py-2 bg-primary text-white rounded-lg text-sm \
                                           font-medium disabled:opacity-50 hover:bg-primary/90 \
                                           transition-colors"
                                >
                                    "Install missing"
                                </button>
                                <button
                                    on:click=do_refresh.clone()
                                    disabled=move || busy.get()
                                    class="px-3 py-2 bg-surface border border-border \
                                           text-text-primary rounded-lg text-sm font-medium \
                                           disabled:opacity-50 hover:bg-surface-hover \
                                           transition-colors"
                                >
                                    "Refresh"
                                </button>
                            </div>
                        </div>
                    }.into_any()
                }
            }}

            <div class="bg-surface-raised rounded-lg border border-border p-4">
                <h2 class="text-sm font-semibold text-text-primary mb-2">"Install log"</h2>
                <pre class="text-xs text-text-secondary whitespace-pre-wrap max-h-48 \
                            overflow-y-auto font-mono">
                    {move || {
                        let v = log_lines.get();
                        if v.is_empty() { "[idle]".to_string() } else { v.join("\n") }
                    }}
                </pre>
            </div>
        </div>
    }
}

#[component]
fn RuntimeRow(info: RuntimeInfo) -> impl IntoView {
    let (dot, dot_class) = match info.status {
        RuntimeStatus::Ready => ("●", "text-success"),
        RuntimeStatus::Probing | RuntimeStatus::Bootstrapping => ("◐", "text-info"),
        RuntimeStatus::Stale => ("◐", "text-warning"),
        RuntimeStatus::Missing => ("○", "text-text-tertiary"),
    };
    let version = info.version.clone().unwrap_or_else(|| "—".into());
    let path = info.bin_path.clone().unwrap_or_default();
    let name = info.name.clone();
    let hint = info.llm_hint.clone();

    view! {
        <div class="flex items-center justify-between py-2">
            <div class="flex items-center gap-3 flex-1 min-w-0">
                <span class=format!("text-lg font-mono {dot_class}")>{dot}</span>
                <div class="flex-1 min-w-0">
                    <div class="font-medium text-text-primary">{name}</div>
                    {(!path.is_empty()).then(|| view! {
                        <div class="text-xs text-text-tertiary font-mono truncate">{path}</div>
                    })}
                    {hint.map(|h| view! {
                        <div class="text-xs text-text-tertiary">{h}</div>
                    })}
                </div>
            </div>
            <div class="text-sm text-text-secondary shrink-0 ml-4">{version}</div>
        </div>
    }
}
