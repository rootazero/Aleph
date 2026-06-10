//! Execution-history table rendered inside the job editor.

use super::helpers::{format_duration, format_timestamp};
use crate::api::cron::JobRunInfo;
use crate::i18n::*;
use leptos::prelude::*;

#[component]
pub(super) fn RunHistory(runs: RwSignal<Vec<JobRunInfo>>) -> impl IntoView {
    let i18n = use_i18n();

    view! {
        <div class="border border-border rounded-lg">
            <div class="px-4 py-3 border-b border-border flex items-center justify-between gap-3">
                <h3 class="text-sm font-semibold text-text-primary">{t!(i18n, cron.history_title)}</h3>
                // Sparkline of the last 12 run outcomes (newest on the right).
                // Pure CSS divs — no chart library (R3 core-lightweight).
                {move || {
                    let run_list = runs.get();
                    if run_list.is_empty() {
                        view! { <span></span> }.into_any()
                    } else {
                        let take = 12usize;
                        // RunHistory is sorted newest-first by the backend;
                        // reverse a windowed copy so the newest sits on the right.
                        let mut window: Vec<_> = run_list.iter().take(take).cloned().collect();
                        window.reverse();
                        view! {
                            <div class="flex items-center gap-0.5"
                                 title="Recent runs — newest on right (green=ok, red=failed, yellow=timeout, gray=other)">
                                {window.into_iter().map(|run| {
                                    // Backend persists RunStatus Debug-lowercased:
                                    // ok / error / skipped / timeout (config.rs RunStatus).
                                    let cls = match run.status.as_str() {
                                        "ok"      => "w-1.5 h-4 rounded-sm bg-success",
                                        "error"   => "w-1.5 h-4 rounded-sm bg-danger",
                                        "timeout" => "w-1.5 h-4 rounded-sm bg-warning",
                                        _         => "w-1.5 h-4 rounded-sm bg-text-tertiary opacity-40",
                                    };
                                    view! { <div class=cls></div> }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            {move || {
                let run_list = runs.get();
                if run_list.is_empty() {
                    view! {
                        <div class="p-6 text-center text-sm text-text-tertiary">
                            {t!(i18n, cron.history_no_records)}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b border-border text-text-secondary">
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_status)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_time)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_duration)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_delivery)}</th>
                                    <th class="px-4 py-2 text-left font-medium">{t!(i18n, cron.col_error)}</th>
                                </tr>
                            </thead>
                            <tbody>
                                {run_list.into_iter().map(|run| {
                                    let (icon, color) = match run.status.as_str() {
                                        "ok" => ("\u{2713}", "text-success"),
                                        "error" => ("\u{2717}", "text-danger"),
                                        "timeout" => ("\u{23F1}", "text-warning"),
                                        "skipped" => ("\u{2014}", "text-text-tertiary"),
                                        _ => ("?", "text-text-tertiary"),
                                    };
                                    let time_str = format_timestamp(run.started_at);
                                    let duration_str = format_duration(run.duration_ms);

                                    // Delivery status with icon
                                    let delivery_str = run.delivery_status.clone().unwrap_or_default();
                                    // Backend persists DeliveryStatus Debug-lowercased:
                                    // delivered / notdelivered / alreadysentbyagent / notrequested.
                                    let delivery_icon = match delivery_str.as_str() {
                                        "delivered" => "\u{2713}",
                                        "alreadysentbyagent" => "\u{2261}",
                                        "notdelivered" => "\u{2717}",
                                        _ => "",
                                    };

                                    // Combine error_reason prefix with error
                                    let error_str = match (&run.error_reason, &run.error) {
                                        (Some(reason), Some(err)) => {
                                            let reason_str = reason.get("message")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| reason.to_string());
                                            format!("[{}] {}", reason_str, err)
                                        }
                                        (Some(reason), None) => {
                                            reason.get("message")
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| reason.to_string())
                                        }
                                        (None, Some(err)) => err.clone(),
                                        (None, None) => String::new(),
                                    };

                                    view! {
                                        <tr class="border-b border-border last:border-b-0 hover:bg-surface-sunken/50">
                                            <td class=format!("px-4 py-2 {}", color)>
                                                {icon}
                                            </td>
                                            <td class="px-4 py-2 text-text-primary">
                                                {time_str}
                                            </td>
                                            <td class="px-4 py-2 text-text-secondary">
                                                {duration_str}
                                            </td>
                                            <td class="px-4 py-2 text-text-secondary">
                                                {delivery_icon}" "{delivery_str}
                                            </td>
                                            <td class="px-4 py-2 text-text-tertiary truncate max-w-xs">
                                                {error_str}
                                            </td>
                                        </tr>
                                    }
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    }.into_any()
                }
            }}
        </div>
    }
}
