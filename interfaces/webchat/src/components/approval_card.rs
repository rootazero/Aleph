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
use crate::i18n::{t, use_i18n};
use crate::state::notifications::PendingApprovalView;
use crate::state::run_clock::SecondTick;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Resolve `id` with `decision` and drop it from the pending list on success.
/// Optimistic removal keeps the surface responsive; the authoritative refetch
/// driven by the `approval.resolved` event lands right behind it.
fn resolve(dashboard: DashboardState, id: String, decision: &'static str) {
    spawn_local(async move {
        match ExecApprovalApi::resolve(&dashboard, id.clone(), decision).await {
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
    let id_deny = approval.id.clone();
    let command = approval.command.clone();
    let agent_id = approval.agent_id.clone();
    let reason = approval.reason.clone();
    let expires_at = approval.expires_at_ms;
    let approval_for_secs = approval.clone();

    let remaining = move || match tick {
        Some(t) => approval_for_secs.remaining_secs(t.0.get()),
        None => approval_for_secs.remaining_secs(expires_at),
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
                    on:click=move |_| resolve(dashboard, id_once.clone(), "allow-once")
                >
                    {t!(i18n, notifications.approval_allow_once)}
                </button>
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors"
                    on:click=move |_| resolve(dashboard, id_session.clone(), "allow-session")
                >
                    {t!(i18n, notifications.approval_allow_session)}
                </button>
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary text-xs transition-colors"
                    on:click=move |_| resolve(dashboard, id_deny.clone(), "deny")
                >
                    {t!(i18n, notifications.approval_deny)}
                </button>
            </div>
        </div>
    }
    .into_any()
}
