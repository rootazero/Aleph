//! Run-meta inspector surface — a completed run's cost / token / model detail.
//!
//! Opened by clicking the assistant bubble's cost/token meta line. Reads only
//! signals `events.rs` already projected — `chat.run_costs` (keyed by run_id)
//! and the run's `assistant-{run_id}` bubble `model_info` — so it is a pure
//! render with no new server round-trip (R4). Shows the per-run breakdown the
//! one-line meta cannot: input/output/total token split, cost, and the model +
//! fallback chain.

use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// Per-run cost / token / model breakdown.
#[component]
pub fn RunMetaInspector(run_id: String) -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    move || {
        let cost = chat.run_costs.with(|m| m.get(&run_id).cloned());
        let model = chat.messages.with(|msgs| {
            let target = format!("assistant-{run_id}");
            msgs.iter()
                .rev()
                .find(|m| m.id == target)
                .and_then(|m| m.model_info.clone())
        });

        let row = |label: String, value: String| {
            view! {
                <div class="flex items-baseline gap-2 text-xs">
                    <span class="w-14 shrink-0 text-text-tertiary">{label}</span>
                    <span class="flex-1 min-w-0 text-text-secondary break-all tabular-nums">
                        {value}
                    </span>
                </div>
            }
            .into_any()
        };

        let body = if cost.is_none() && model.is_none() {
            view! {
                <p class="text-xs text-text-tertiary italic">
                    {t!(i18n, common.inspector_run_meta_empty)}
                </p>
            }
            .into_any()
        } else {
            let mut rows = Vec::new();
            if let Some(m) = &model {
                let value = if m.is_fallback {
                    let orig = m.original_model.clone().unwrap_or_default();
                    let fb = t_string!(i18n, common.inspector_fallback).to_string();
                    format!("{} · {}  ({fb} {orig})", m.model, m.provider)
                } else {
                    format!("{} · {}", m.model, m.provider)
                };
                rows.push(row(
                    t_string!(i18n, common.inspector_model).to_string(),
                    value,
                ));
            }
            if let Some(c) = &cost {
                if let Some(money) = c.cost_label() {
                    rows.push(row(t_string!(i18n, common.inspector_cost).to_string(), money));
                }
                rows.push(row(
                    t_string!(i18n, common.inspector_input).to_string(),
                    c.input_tokens.to_string(),
                ));
                rows.push(row(
                    t_string!(i18n, common.inspector_output).to_string(),
                    c.output_tokens.to_string(),
                ));
                rows.push(row(
                    t_string!(i18n, common.inspector_total).to_string(),
                    c.total_tokens.to_string(),
                ));
            }
            view! { <div class="flex flex-col gap-1.5">{rows}</div> }.into_any()
        };

        view! {
            <div class="flex flex-col gap-2">
                <div class="flex items-center gap-2 pb-2 border-b border-border/60">
                    <span class="text-base shrink-0">"💰"</span>
                    <span class="flex-1 text-sm text-text-primary font-medium">
                        {t!(i18n, common.inspector_run_meta_title)}
                    </span>
                </div>
                {body}
            </div>
        }
        .into_any()
    }
}
