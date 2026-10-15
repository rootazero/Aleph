//! `TelecomApprovalCard` — operator card for a pending telecom A/B approval.
//!
//! Review-before-decide is structural here: the approve/deny buttons stay
//! disabled until `telecom.approval.review` has returned the complete body,
//! and the resolve call echoes exactly that reviewed body (never the list
//! preview) so the server can bind the decision to what the operator saw.
//!
//! Countdown: `expires_at_ms` is an absolute server deadline, ticked against
//! the shared 1s clock like `ApprovalCard`. Once expired every button is
//! disabled; the next refetch removes the row.

use crate::api::TelecomApprovalApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::run_clock::SecondTick;
use crate::state::telecom_approvals::{
    TelecomApprovalChannel, TelecomApprovalReview, TelecomApprovalStage, TelecomApprovalView,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
#[must_use]
pub fn TelecomApprovalCard(approval: TelecomApprovalView) -> impl IntoView {
    let i18n = use_i18n();
    let Some(dashboard) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    // Absent in storybook mounts — the countdown then holds still.
    let tick = use_context::<SecondTick>();

    let id = StoredValue::new(approval.id.clone());
    let stage = approval.stage;
    let channel = approval.channel;
    let preview = approval.preview.clone();
    let truncated = approval.truncated;
    let view_for_time = StoredValue::new(approval);

    let review = RwSignal::new(Option::<TelecomApprovalReview>::None);
    let loading = RwSignal::new(false);
    let resolving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    let now = move || match tick {
        Some(t) => t.0.get(),
        None => crate::views::chat::timeline::now_millis(),
    };
    let expired = move || view_for_time.with_value(|v| v.is_expired(now()));
    let remaining = move || view_for_time.with_value(|v| v.remaining_secs(now()));

    let load_review = move || {
        loading.set(true);
        error.set(None);
        spawn_local(async move {
            let target = id.get_value();
            match TelecomApprovalApi::review(&dashboard, &target).await {
                Ok(r) => review.set(Some(r)),
                Err(e) => {
                    review.set(None);
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to load full content: {e}"),
                    )));
                }
            }
            loading.set(false);
        });
    };

    let decide = move |approve: bool| {
        // Only a loaded review can be resolved, and it carries the complete
        // body; the list preview is not reachable from here.
        let Some(r) = review.get_untracked() else {
            return;
        };
        resolving.set(true);
        error.set(None);
        spawn_local(async move {
            match TelecomApprovalApi::resolve(&dashboard, &r, approve).await {
                Ok(()) => {
                    dashboard
                        .pending_telecom_approvals
                        .update(|l| l.retain(|x| x.id != r.view.id));
                }
                Err(e) => {
                    let msg =
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to resolve telecom approval: {e}")
                        });
                    web_sys::console::warn_1(&msg.clone().into());
                    error.set(Some(msg));
                }
            }
            resolving.set(false);
        });
    };

    let decision_disabled =
        move || review.with(Option::is_none) || resolving.get() || loading.get() || expired();

    let stage_label = move || match stage {
        TelecomApprovalStage::PrivateRead => {
            t_string!(i18n, notifications.telecom_stage_private_read).to_string()
        }
        TelecomApprovalStage::Disclosure => {
            t_string!(i18n, notifications.telecom_stage_disclosure).to_string()
        }
    };
    let channel_label = move || match channel {
        TelecomApprovalChannel::Sms => {
            t_string!(i18n, notifications.telecom_channel_sms).to_string()
        }
        TelecomApprovalChannel::Voice => {
            t_string!(i18n, notifications.telecom_channel_voice).to_string()
        }
    };

    view! {
        <div class="rounded-lg border border-yellow-500/40 bg-yellow-500/5 px-3 py-2">
            <div class="flex items-center gap-1.5">
                <span class="text-sm leading-none">"📞"</span>
                <span class="text-sm font-medium text-text-primary">
                    {t!(i18n, notifications.telecom_header)}
                </span>
                <span class="text-xs px-1.5 py-0.5 rounded bg-surface-sunken text-text-secondary">
                    {stage_label}
                </span>
                <span class="text-xs text-text-tertiary">{channel_label}</span>
            </div>
            // List preview until the complete body is loaded; afterwards the
            // reviewed body replaces it, so what is shown is what is resolved.
            {move || match review.get() {
                Some(r) => view! {
                    <div class="text-xs text-text-secondary my-1 break-all">
                        <div><strong>{t!(i18n, notifications.telecom_binding)}" "</strong>{r.view.binding_ref}</div>
                        <div><strong>{t!(i18n, notifications.telecom_generation)}" "</strong>{r.view.connection_generation}</div>
                        <div><strong>{t!(i18n, notifications.telecom_caller_digest)}" "</strong>{r.view.caller_digest}</div>
                        <div><strong>{t!(i18n, notifications.telecom_turn)}" "</strong>{r.view.turn_id}</div>
                        <div><strong>{t!(i18n, notifications.telecom_session_epoch)}" "</strong>{r.view.session_epoch}</div>
                    </div>
                    <pre class="font-mono text-sm my-1 text-primary whitespace-pre-wrap break-all max-h-60 overflow-y-auto rounded bg-surface-sunken px-2 py-1">
                        {r.body}
                    </pre>
                }
                .into_any(),
                None => {
                    let preview = preview.clone();
                    view! {
                        <div class="font-mono text-sm my-1 text-primary break-all">
                            {preview}
                            {truncated.then(|| view! {
                                <span class="text-xs text-text-tertiary">
                                    " " {t!(i18n, notifications.telecom_preview_truncated)}
                                </span>
                            })}
                        </div>
                    }
                    .into_any()
                }
            }}
            <div class="text-xs text-text-tertiary mt-0.5">
                {move || if expired() {
                    t_string!(i18n, notifications.telecom_expired).to_string()
                } else {
                    format!("{} {}s", t_string!(i18n, notifications.approval_expires), remaining())
                }}
            </div>
            {move || error.get().map(|e| view! {
                <div class="text-xs text-danger mt-1.5 break-words">{e}</div>
            })}
            <Show when=move || review.with(Option::is_none)>
                <div class="text-xs text-text-secondary mt-1">
                    {t!(i18n, notifications.telecom_review_required)}
                </div>
            </Show>
            <div class="flex gap-2 mt-2">
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled=move || loading.get() || resolving.get() || expired()
                    on:click=move |_| load_review()
                >
                    {move || if loading.get() {
                        t_string!(i18n, notifications.telecom_reviewing).to_string()
                    } else {
                        t_string!(i18n, notifications.telecom_review).to_string()
                    }}
                </button>
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs font-semibold transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled=decision_disabled
                    on:click=move |_| decide(true)
                >
                    {t!(i18n, notifications.telecom_approve)}
                </button>
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary text-xs transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled=decision_disabled
                    on:click=move |_| decide(false)
                >
                    {t!(i18n, notifications.telecom_deny)}
                </button>
            </div>
        </div>
    }
    .into_any()
}
