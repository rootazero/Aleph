//! `ApprovalCard` — the one renderer for a pending operator approval.
//!
//! Rendered on two surfaces, from the same [`PendingApprovalView`]:
//!   * inline in the conversation, under the tool row that is blocked
//!     (`views::chat::messages`) — the surface the operator is actually
//!     looking at when a tool stops for permission;
//!   * in the notification-center popover, which stays the catch-all for
//!     approvals with no visible tool row (channel / background runs).
//!
//! Both resolve through the same `exec.approval.resolve` RPC, so a decision
//! taken on either surface clears the other (the `approval.**` event triggers
//! a refetch of `exec.approvals.pending`, the single source of truth).
//!
//! The countdown ticks: [`PendingApprovalView::expires_at_ms`] is an absolute
//! deadline, so the remaining seconds are recomputed against the shared 1s
//! clock rather than frozen at fetch time.

use crate::api::ExecApprovalApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::notifications::PendingApprovalView;
use crate::state::run_clock::SecondTick;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Resolve `id` with `decision` and drop it from the pending list on success.
/// Optimistic removal keeps the surface responsive; the authoritative refetch
/// driven by the `approval.resolved` event lands right behind it. `reason` is
/// the operator's free-text objection on a deny, relayed verbatim to the model.
fn resolve(dashboard: DashboardState, id: String, decision: &'static str, reason: Option<String>) {
    spawn_local(async move {
        match ExecApprovalApi::resolve(&dashboard, id.clone(), decision, reason).await {
            Ok(()) => dashboard
                .pending_approvals
                .update(|l| l.retain(|x| x.id != id)),
            Err(e) => web_sys::console::warn_1(
                &format!("Failed to resolve approval ({decision}): {e:?}").into(),
            ),
        }
    });
}

#[component]
#[must_use]
pub fn ApprovalCard(approval: PendingApprovalView) -> impl IntoView {
    let i18n = use_i18n();
    let Some(dashboard) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    // Absent in storybook mounts — the countdown then simply holds its
    // fetch-time value instead of ticking.
    let tick = use_context::<SecondTick>();

    let id_once = approval.id.clone();
    let id_session = approval.id.clone();
    let id_always = approval.id.clone();
    let id_deny = approval.id.clone();
    // Which tiers this card may offer is the SERVER's decision (it depends on
    // why the gate fired and who is being asked), carried on the record and
    // enforced when the answer comes back. Rendering a fixed three was the
    // asymmetry this closes: Telegram already read the list, the Panel did not.
    let offers_session = approval
        .allowed_decisions
        .iter()
        .any(|d| d == "allow-session");
    let offers_always = approval
        .allowed_decisions
        .iter()
        .any(|d| d == "allow-always");
    // Stored (not captured by value) so the deny-with-reason submit closure
    // stays `Copy` — the input's Enter handler and the confirm button both
    // need it (same pattern as `AskUserCard`).
    let id_deny_reason = StoredValue::new(approval.id.clone());
    let deny_reason = RwSignal::new(String::new());
    let deny_input_open = RwSignal::new(false);
    let command = approval.command.clone();
    let agent_id = approval.agent_id.clone();
    let reason = approval.reason.clone();
    let expires_at = approval.expires_at_ms;
    let approval_for_secs = approval.clone();

    let remaining = move || match tick {
        Some(t) => approval_for_secs.remaining_secs(t.0.get()),
        None => approval_for_secs.remaining_secs(expires_at),
    };

    // Deny with the typed objection; the API layer drops a blank reason, so an
    // empty field would degrade to a plain deny — the disabled confirm button
    // steers the operator to the plain `Deny` button for that instead.
    let submit_deny_reason = move || {
        let objection = deny_reason.get_untracked().trim().to_string();
        if objection.is_empty() {
            return;
        }
        deny_reason.set(String::new());
        deny_input_open.set(false);
        resolve(
            dashboard,
            id_deny_reason.get_value(),
            "deny",
            Some(objection),
        );
    };

    view! {
        <div class="rounded-lg border border-yellow-500/40 bg-yellow-500/5 px-3 py-2">
            <div class="flex items-center gap-1.5">
                <span class="text-sm leading-none">"🔐"</span>
                <span class="text-sm font-medium text-text-primary">
                    {t!(i18n, notifications.approval_header)}
                </span>
            </div>
            <div class="font-mono text-sm my-1 text-primary break-all">{command}</div>
            // Server-supplied escalation context. Without it the operator is
            // asked to authorise a bare tool name.
            {reason.map(|r| view! {
                <p class="text-xs text-text-secondary leading-snug">{r}</p>
            })}
            <div class="text-xs text-text-tertiary mt-0.5">
                {t!(i18n, notifications.approval_requested_by)} ": " {agent_id}
                " · " {t!(i18n, notifications.approval_expires)} " "
                <span class="tabular-nums">{move || remaining().to_string()}</span> "s"
            </div>
            <div class="flex gap-2 mt-2">
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs font-semibold transition-colors"
                    on:click=move |_| resolve(dashboard, id_once.clone(), "allow-once", None)
                >
                    {t!(i18n, notifications.approval_allow_once)}
                </button>
                <Show when=move || offers_session>
                    {
                        let id_session = id_session.clone();
                        view! {
                            <button
                                type="button"
                                class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors"
                                on:click=move |_| resolve(dashboard, id_session.clone(), "allow-session", None)
                            >
                                {t!(i18n, notifications.approval_allow_session)}
                            </button>
                        }
                    }
                </Show>
                // Only rendered when the server offered it: an operator-tier
                // turn, on a gate that is not the tool's own declared floor.
                // The hint says where to take it back — a permanent grant
                // nobody can find is the part that makes permanence scary.
                <Show when=move || offers_always>
                    {
                        let id_always = id_always.clone();
                        view! {
                            <button
                                type="button"
                                title=move || t_string!(i18n, notifications.approval_allow_always_hint).to_string()
                                class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors"
                                on:click=move |_| resolve(dashboard, id_always.clone(), "allow-always", None)
                            >
                                {t!(i18n, notifications.approval_allow_always)}
                            </button>
                        }
                    }
                </Show>
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary text-xs transition-colors"
                    on:click=move |_| resolve(dashboard, id_deny.clone(), "deny", None)
                >
                    {t!(i18n, notifications.approval_deny)}
                </button>
            </div>
            // "Deny with reason" entry — the reason is relayed verbatim to the
            // model so it re-plans on the operator's actual objection instead
            // of a bare refusal (kimi-cli's approval option 4).
            <div class="mt-1.5">
                {move || if deny_input_open.get() {
                    view! {
                        <div class="flex gap-2">
                            <input
                                type="text"
                                class="flex-1 min-w-0 px-2 py-1.5 rounded bg-surface-sunken border border-border
                                       text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none
                                       focus:border-primary transition-colors"
                                placeholder=move || t_string!(i18n, notifications.approval_deny_reason_placeholder).to_string()
                                prop:value=move || deny_reason.get()
                                on:input=move |ev| deny_reason.set(event_target_value(&ev))
                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                    if ev.key() == "Enter" {
                                        ev.prevent_default();
                                        submit_deny_reason();
                                    } else if ev.key() == "Escape" {
                                        deny_input_open.set(false);
                                    }
                                }
                            />
                            <button
                                type="button"
                                class="px-3 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary
                                       text-xs font-semibold disabled:opacity-35 disabled:cursor-not-allowed transition-colors"
                                disabled=move || deny_reason.get().trim().is_empty()
                                on:click=move |_| submit_deny_reason()
                            >
                                {t!(i18n, notifications.approval_deny)}
                            </button>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <button
                            type="button"
                            class="text-xs text-text-tertiary hover:text-text-secondary transition-colors"
                            on:click=move |_| deny_input_open.set(true)
                        >
                            {t!(i18n, notifications.approval_deny_with_reason)}
                        </button>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
    .into_any()
}
