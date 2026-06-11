//! Compact runtime-readiness banner shown at the top of the Browser page.
//!
//! Keeps the Browser config page focused on configuration while giving
//! visibility into whether the underlying runtime is installed.

use crate::api::runtimes::{RuntimeInfo, RuntimeStatus, RuntimesApi};
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;
use leptos::task::spawn_local;

const BROWSER_RUNTIMES: &[&str] = &["fnm", "node", "playwright-cli"];

#[component]
#[must_use]
pub fn RuntimeSummaryBanner() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loaded = RwSignal::new(false);

    {
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
                        <span>{t!(i18n, browser_banner.ready)}</span>
                    </div>
                }.into_any())
            } else {
                let names = missing.join(", ");
                Some(view! {
                    <div class="p-3 bg-warning-subtle border border-warning/20 rounded-lg text-warning text-sm flex items-center justify-between gap-2">
                        <span>{format!("{}{names}", t_string!(i18n, browser_banner.missing_prefix))}</span>
                        <a href="/dashboard/runtimes"
                           class="text-sm font-medium underline hover:no-underline">
                            {t!(i18n, browser_banner.configure)}
                        </a>
                    </div>
                }.into_any())
            }
        }}
    }
}
