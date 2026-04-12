//! Runtime Status card: fnm / Node / playwright-cli / Chromium / skills
//! health + "Install All" / "Refresh" buttons.

use crate::api::browser::{BrowserRuntimeApi, ComponentStatus, RuntimeStatusResponse};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn BrowserRuntimeCard() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let status = RwSignal::new(Option::<RuntimeStatusResponse>::None);
    let loading = RwSignal::new(true);
    let installing = RwSignal::new(false);
    let error_msg = RwSignal::new(Option::<String>::None);

    // Initial probe on mount.
    spawn_local(async move {
        match BrowserRuntimeApi::status(&state).await {
            Ok(s) => {
                status.set(Some(s));
                error_msg.set(None);
            }
            Err(e) => error_msg.set(Some(e)),
        }
        loading.set(false);
    });

    let refresh = move |_| {
        loading.set(true);
        spawn_local(async move {
            match BrowserRuntimeApi::refresh(&state).await {
                Ok(s) => {
                    status.set(Some(s));
                    error_msg.set(None);
                }
                Err(e) => error_msg.set(Some(e)),
            }
            loading.set(false);
        });
    };

    let install = move |_| {
        installing.set(true);
        error_msg.set(None);
        spawn_local(async move {
            match BrowserRuntimeApi::install(&state).await {
                Ok(_) => {
                    // Install is async on the backend; probe after triggering so the
                    // card starts showing updated status within a few seconds.
                    match BrowserRuntimeApi::status(&state).await {
                        Ok(s) => status.set(Some(s)),
                        Err(e) => error_msg.set(Some(e)),
                    }
                }
                Err(e) => error_msg.set(Some(e)),
            }
            installing.set(false);
        });
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-1">"Runtime Status"</h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Playwright CLI requires Node.js (via fnm) and a local Chromium. Click \"Install All\" to bootstrap the browser runtime."
            </p>
            {move || {
                if loading.get() {
                    view! {
                        <div class="text-text-tertiary text-sm py-2">"Probing runtime..."</div>
                    }.into_any()
                } else if let Some(s) = status.get() {
                    view! {
                        <ul class="space-y-2 mb-4">
                            <ComponentRow name="fnm" status=s.fnm />
                            <ComponentRow name="Node.js (via fnm)" status=s.node />
                            <ComponentRow name="@playwright/cli" status=s.playwright_cli />
                            <ComponentRow name="Chromium" status=s.chromium />
                            <ComponentRow name="Skills (~/.aleph/skills/)" status=s.skills />
                        </ul>
                    }.into_any()
                } else {
                    view! {
                        <div class="text-danger text-sm py-2">
                            "Gateway unavailable — cannot probe runtime."
                        </div>
                    }.into_any()
                }
            }}
            {move || error_msg.get().map(|msg| view! {
                <div class="mb-4 p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                    {msg}
                </div>
            })}
            <div class="flex gap-2">
                <button
                    on:click=install
                    disabled=move || installing.get()
                    class="px-4 py-2 bg-primary text-white rounded-lg disabled:opacity-50 text-sm font-medium"
                >
                    {move || if installing.get() { "Installing..." } else { "Install All" }}
                </button>
                <button
                    on:click=refresh
                    class="px-4 py-2 border border-border rounded-lg text-text-primary text-sm font-medium"
                >
                    "Refresh"
                </button>
            </div>
        </div>
    }
}

#[component]
fn ComponentRow(name: &'static str, status: ComponentStatus) -> impl IntoView {
    let (icon, text, color_class) = match &status {
        ComponentStatus::Installed { version, .. } => {
            let v = version.as_deref().unwrap_or("");
            let label = if v.is_empty() {
                name.to_string()
            } else {
                format!("{name} {v}")
            };
            ("✓", label, "text-success")
        }
        ComponentStatus::Missing => (
            "✗",
            format!("{name} — not installed"),
            "text-text-tertiary",
        ),
        ComponentStatus::Probing => (
            "…",
            format!("{name} — probing…"),
            "text-text-tertiary",
        ),
        ComponentStatus::Error { message } => (
            "!",
            format!("{name} — error: {message}"),
            "text-danger",
        ),
    };
    view! {
        <li class=format!("flex items-center gap-2 text-sm {color_class}")>
            <span class="w-4 text-center font-mono">{icon}</span>
            <span>{text}</span>
        </li>
    }
}
