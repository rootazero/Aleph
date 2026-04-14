//! Compact runtime-readiness banner shown at the top of the Browser page.
//!
//! Keeps the Browser config page focused on configuration while giving
//! visibility into whether the underlying runtime is installed.

use crate::api::runtimes::{RuntimeInfo, RuntimeStatus, RuntimesApi};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

const BROWSER_RUNTIMES: &[&str] = &["fnm", "node", "playwright-cli"];

#[component]
pub fn RuntimeSummaryBanner() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loaded = RwSignal::new(false);

    {
        let state = state.clone();
        spawn_local(async move {
            if let Ok(r) = RuntimesApi::list(&state).await {
                runtimes.set(r.runtimes);
            }
            loaded.set(true);
        });
    }

    view! {
        {move || {
            if !loaded.get() {
                return None;
            }
            let list = runtimes.get();
            let missing: Vec<String> = list
                .iter()
                .filter(|r| {
                    BROWSER_RUNTIMES.contains(&r.name.as_str())
                        && r.status != RuntimeStatus::Ready
                        && r.supported_on_current_os
                })
                .map(|r| r.name.clone())
                .collect();
            if missing.is_empty() {
                Some(view! {
                    <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm flex items-center gap-2">
                        <span>"✓"</span>
                        <span>"Browser runtime ready"</span>
                    </div>
                }.into_any())
            } else {
                let names = missing.join(", ");
                Some(view! {
                    <div class="p-3 bg-warning-subtle border border-warning/20 rounded-lg text-warning text-sm flex items-center justify-between gap-2">
                        <span>{format!("⚠ Browser runtime missing: {names}")}</span>
                        <a href="/settings/runtime"
                           class="text-sm font-medium underline hover:no-underline">
                            "Configure →"
                        </a>
                    </div>
                }.into_any())
            }
        }}
    }
}
