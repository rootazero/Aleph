//! The one renderer for an extension's invocation record.
//!
//! Shared by the MCP servers page and the Plugins page so the rule that makes
//! this column safe has exactly one implementation:
//!
//! * `calls: Some(0)` → **"never used"**. Genuinely unused; a cleanup candidate.
//! * `calls: None` → **`—`**, with the reason on hover. The entry has no
//!   tool-call channel to measure (a hooks-only plugin). Rendering this as `0`
//!   would invite uninstalling something that runs on every turn — which is why
//!   the predicate lives in `UsageSummary::never_used()` on the shared wire type
//!   rather than being re-derived from `calls == 0` at each call site.
//! * `usage: None` (no summary at all) → nothing. The server could not build the
//!   report; an empty cell says "unknown", which is the truth.

use aleph_protocol::extension_usage::UsageSummary;
use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};

/// Compact usage pill: call count and recency, or the not-measurable dash.
#[component]
#[must_use]
pub fn UsageBadge(usage: Option<UsageSummary>) -> impl IntoView {
    let i18n = use_i18n();
    let Some(u) = usage else {
        return view! { <span></span> }.into_any();
    };

    if u.never_used() {
        return view! {
            <span
                class="px-2 py-0.5 rounded-full text-xs font-medium bg-warning-subtle text-warning"
                title=t_string!(i18n, usage.extension.never_used_hint).to_string()
            >
                {t_string!(i18n, usage.extension.never_used).to_string()}
            </span>
        }
        .into_any();
    }

    let Some(calls) = u.display_calls() else {
        let why = u
            .not_measurable_reason
            .clone()
            .unwrap_or_else(|| t_string!(i18n, usage.extension.not_measurable).to_string());
        return view! {
            <span class="px-2 py-0.5 text-xs text-text-tertiary" title=why>
                "—"
            </span>
        }
        .into_any();
    };

    let calls_label = format!("{calls} {}", t_string!(i18n, usage.extension.calls_suffix));
    let recency = u
        .idle_days
        .map(|d| format!(" · {d}{}", t_string!(i18n, usage.extension.days_ago_suffix)));
    let errors = (u.errors > 0).then(|| {
        format!(
            " · {} {}",
            u.errors,
            t_string!(i18n, usage.extension.errors_suffix)
        )
    });

    view! {
        <span class="px-2 py-0.5 text-xs text-text-tertiary">
            {calls_label}
            {recency}
            {errors.map(|e| view! { <span class="text-danger">{e}</span> })}
        </span>
    }
    .into_any()
}
