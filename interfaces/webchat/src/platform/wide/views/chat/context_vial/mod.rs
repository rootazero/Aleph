mod canvas;
mod water;

use super::state::{ChatState, ContextUsage};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use aleph_protocol::context_breakdown::ContextBreakdown;
use leptos::prelude::*;
use leptos::task::spawn_local;
use shared_ui_logic::transcript::{project_context_detail, ContextOccupancy};

#[derive(Clone)]
enum Detail {
    Loading,
    Unavailable,
    Ready(Box<ContextBreakdown>),
}

#[component]
pub fn ContextVial() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let detail = RwSignal::new(None::<Detail>);
    let request = RwSignal::new(0_u64);
    let trigger = NodeRef::<leptos::html::Button>::new();
    let close = move || {
        request.update(|id| *id = id.wrapping_add(1));
        detail.set(None);
    };
    Effect::new(move |_| {
        let _ = (chat.session_key.get(), dash.connection_epoch.get());
        close();
    });
    let load = move || {
        let Some(key) = chat.session_key.get_untracked() else {
            return;
        };
        let epoch = dash.connection_epoch.get_untracked();
        request.update(|id| *id = id.wrapping_add(1));
        let id = request.get_untracked();
        detail.set(Some(Detail::Loading));
        spawn_local(async move {
            let response = dash
                .rpc_call("context.breakdown", serde_json::json!({"session_key": key}))
                .await;
            if request.try_get_untracked() != Some(id)
                || dash.connection_epoch.try_get_untracked() != Some(epoch)
                || chat.session_key.try_get_untracked().flatten().as_deref() != Some(key.as_str())
            {
                return;
            }
            let result = response
                .ok()
                .and_then(|value| serde_json::from_value::<ContextBreakdown>(value).ok())
                .filter(|report| report.session_key == key);
            detail.set(Some(result.map_or(Detail::Unavailable, |report| {
                Detail::Ready(Box::new(report))
            })));
        });
    };
    let status = Memo::new(move |_| match chat.context_usage.get() {
        Some(usage) => match usage.occupancy() {
            ContextOccupancy::Measured {
                used_tokens,
                window_tokens,
                total_tokens,
            } => {
                let pct = (usage.occupancy().fraction().unwrap_or_default() * 100.0).round() as u32;
                match usage {
                    ContextUsage::Estimate(_) => t_string!(
                        i18n,
                        chat.context_usage_estimate_title,
                        pct = pct,
                        used = used_tokens,
                        window = window_tokens
                    )
                    .to_string(),
                    ContextUsage::Previous(_) => t_string!(
                        i18n,
                        chat.context_usage_previous_title,
                        pct = pct,
                        used = used_tokens,
                        window = window_tokens
                    )
                    .to_string(),
                    ContextUsage::Current(_) => match total_tokens {
                        Some(total) => t_string!(
                            i18n,
                            chat.context_usage_title,
                            pct = pct,
                            used = used_tokens,
                            window = window_tokens,
                            total = total
                        )
                        .to_string(),
                        None => t_string!(
                            i18n,
                            chat.context_usage_measured_title,
                            pct = pct,
                            used = used_tokens,
                            window = window_tokens
                        )
                        .to_string(),
                    },
                }
            }
            ContextOccupancy::CapacityOnly { window_tokens } => {
                t_string!(i18n, chat.context_vial_capacity, window = window_tokens).to_string()
            }
            ContextOccupancy::Unknown => t_string!(i18n, chat.context_vial_unknown).to_string(),
        },
        None => t_string!(i18n, chat.context_vial_unknown).to_string(),
    });
    let level = Memo::new(move |_| match chat.context_usage.get() {
        Some(ContextUsage::Current(occupancy) | ContextUsage::Previous(occupancy)) => {
            occupancy.fraction()
        }
        _ => None,
    });
    let close_with_focus = move || {
        close();
        if let Some(button) = trigger.get_untracked() {
            let _ = button.focus();
        }
    };
    view! {
        <div class="panel-context-vial" on:keydown=move |event: web_sys::KeyboardEvent| {
            if detail.get_untracked().is_some() && event.key() == "Escape" {
                event.stop_propagation();
                close_with_focus();
            }
        }>
            <button type="button" node_ref=trigger class="panel-context-vial-trigger"
                aria-label=move || status.get()
                aria-expanded=move || if detail.get().is_some() { "true" } else { "false" }
                title=move || status.get()
                disabled=move || chat.session_key.get().is_none()
                on:click=move |_| if detail.get_untracked().is_some() { close(); } else { load(); }>
                <span class="panel-context-vial-neck" aria-hidden="true"></span>
                <span class="panel-context-vial-glass" aria-hidden="true"
                    data-level=move || if level.get().is_some() { "measured" } else { "unknown" }>
                    <canvas::WaterDisplay level=level session=chat.session_key.into() />
                </span>
            </button>
            <Show when=move || detail.get().is_some()>
                <section class="panel-context-vial-detail" aria-label=move || t_string!(i18n, chat.context_vial_detail).to_string()>
                    <header class="flex items-center justify-between gap-3">
                        <h3>{t!(i18n, chat.context_vial_detail)}</h3>
                        <button type="button" class="panel-context-vial-action" on:click=move |_| close_with_focus()>
                            {t!(i18n, chat.context_vial_close)}
                        </button>
                    </header>
                    <p class="text-text-secondary">{move || status.get()}</p>
                    {move || match detail.get() {
                        Some(Detail::Loading) => view! { <p role="status">{t!(i18n, chat.context_vial_loading)}</p> }.into_any(),
                        Some(Detail::Unavailable) => view! { <p role="status">{t!(i18n, chat.context_vial_unavailable)}</p> }.into_any(),
                        Some(Detail::Ready(report)) => view! { <RequestDetail report=*report /> }.into_any(),
                        None => ().into_any(),
                    }}
                </section>
            </Show>
        </div>
    }
}

#[component]
fn RequestDetail(report: ContextBreakdown) -> impl IntoView {
    let i18n = use_i18n();
    let detail = project_context_detail(&report);
    let turn = detail.turn;
    let system = detail.system_prompt_bytes;
    let schemas = detail.tool_schema_bytes;
    let results = detail.tool_result_tokens;
    view! {
        <div class="panel-context-vial-request">
            <p class="text-text-secondary">{t!(i18n, chat.context_vial_request, turn = turn)}</p>
            <dl>
                <dt>{t!(i18n, chat.context_vial_system)}</dt>
                <dd>{system.map_or_else(|| "—".to_owned(), |size| format!("{size} B"))}</dd>
                <dt>{t!(i18n, chat.context_vial_tools)}</dt>
                <dd>{format!("{schemas} B")}</dd>
                <dt>{t!(i18n, chat.context_vial_results)}</dt>
                <dd>{results.map_or_else(|| "—".to_owned(), |tokens| format!("≈{tokens}"))}</dd>
            </dl>
        </div>
    }
}
