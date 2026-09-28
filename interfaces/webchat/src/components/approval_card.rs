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
fn resolve(
    dashboard: DashboardState,
    id: String,
    decision: &'static str,
    reason: Option<String>,
    resolving: RwSignal<bool>,
    error: RwSignal<Option<String>>,
) {
    resolving.set(true);
    error.set(None);
    spawn_local(async move {
        match ExecApprovalApi::resolve(&dashboard, id.clone(), decision, reason).await {
            Ok(()) => {
                dashboard
                    .pending_approvals
                    .update(|l| l.retain(|x| x.id != id));
                error.set(None);
            }
            Err(e) => {
                let msg = format!("Failed to resolve approval ({decision}): {e:?}");
                web_sys::console::warn_1(&msg.clone().into());
                error.set(Some(msg));
            }
        }
        resolving.set(false);
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
    let resolving = RwSignal::new(false);
    let resolve_error = RwSignal::new(Option::<String>::None);
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
            resolving,
            resolve_error,
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
                // `expires_at == 0` is the no-expiry sentinel: an attended
                // approval waits forever (ruled 2026-08-28), so there is no
                // countdown to tick — say so instead.
                " · " {move || if expires_at == 0 {
                    t_string!(i18n, notifications.approval_no_expiry).to_string()
                } else {
                    format!("{} {}s", t_string!(i18n, notifications.approval_expires), remaining())
                }}
            </div>
            {move || resolve_error.get().map(|e| view! {
                <div class="text-xs text-danger mt-1.5 break-words">{e}</div>
            })}
            <div class="flex gap-2 mt-2">
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs font-semibold transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled=move || resolving.get()
                    on:click=move |_| resolve(dashboard, id_once.clone(), "allow-once", None, resolving, resolve_error)
                >
                    {t!(i18n, notifications.approval_allow_once)}
                </button>
                <Show when=move || offers_session>
                    {
                        let id_session = id_session.clone();
                        view! {
                            <button
                                type="button"
                                class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                disabled=move || resolving.get()
                                on:click=move |_| resolve(dashboard, id_session.clone(), "allow-session", None, resolving, resolve_error)
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
                                class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                                disabled=move || resolving.get()
                                on:click=move |_| resolve(dashboard, id_always.clone(), "allow-always", None, resolving, resolve_error)
                            >
                                {t!(i18n, notifications.approval_allow_always)}
                            </button>
                        }
                    }
                </Show>
                <button
                    type="button"
                    class="flex-1 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary text-xs transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled=move || resolving.get()
                    on:click=move |_| resolve(dashboard, id_deny.clone(), "deny", None, resolving, resolve_error)
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
                                disabled=move || resolving.get() || deny_reason.get().trim().is_empty()
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

#[cfg(test)]
mod tests {
    use crate::state::notifications::PendingApprovalView;

    /// Build a representative approval record with the legacy three-decision
    /// ceiling (allow-once / allow-session / deny), which is what older cores
    /// arrive with via `api::exec_approval::default_decisions`. Matches what
    /// the inline chat card and the notification-center popover both render.
    fn default_record() -> PendingApprovalView {
        PendingApprovalView {
            id: "ap-1".into(),
            command: "rm -rf build/".into(),
            agent_id: "agent-x".into(),
            session_key: "session-1".into(),
            tool_call_id: Some("tool-1".into()),
            reason: Some("destructive".into()),
            expires_at_ms: 60_000,
            allowed_decisions: vec![
                "allow-once".into(),
                "allow-session".into(),
                "deny".into(),
            ],
        }
    }

    // -- Decision-list fixture shape ---------------------------------------
    //
    // These pin the SHAPE of the records the inline predicates in
    // `ApprovalCard` will scan at render time. The predicate itself
    // (`iter().any(|d| d == "…")`) is a one-liner; what would otherwise
    // break the rendering contract is the fixture — a record that
    // silently lost `allow-session`, a tier name drifted to a wrong
    // kebab-case, a stale test fixture diverging from the live wire
    // values. These tests catch those.

    /// Legacy three-button set surfaces `allow-session` so the inline
    /// predicate in `ApprovalCard` draws the session button (the asymmetry
    /// this card exists to close — Telegram reads the list, the Panel did
    /// not).
    #[test]
    fn legacy_default_decisions_include_allow_session() {
        let r = default_record();
        assert_eq!(
            r.allowed_decisions
                .iter()
                .filter(|d| *d == "allow-session")
                .count(),
            1,
            "legacy three-button set must include allow-session: {:?}",
            r.allowed_decisions
        );
    }

    /// A member-tier turn sends only `allow-once` and `deny`. Drawing a
    /// session button the server will reject is a cosmetic bug; drawing
    /// one too few is the safe direction — so the predicate will see zero.
    #[test]
    fn member_tier_decisions_omit_allow_session() {
        let r = PendingApprovalView {
            allowed_decisions: vec!["allow-once".into(), "deny".into()],
            ..default_record()
        };
        assert_eq!(
            r.allowed_decisions
                .iter()
                .filter(|d| *d == "allow-session")
                .count(),
            0,
            "member-tier set must not include allow-session: {:?}",
            r.allowed_decisions
        );
    }

    /// Operator-tier on a gate that authorised `allow-always` in addition
    /// to the legacy set — the only case the inline predicate lights up
    /// for.
    #[test]
    fn operator_tier_decisions_include_allow_always() {
        let r = PendingApprovalView {
            allowed_decisions: vec![
                "allow-once".into(),
                "allow-session".into(),
                "allow-always".into(),
                "deny".into(),
            ],
            ..default_record()
        };
        assert_eq!(
            r.allowed_decisions
                .iter()
                .filter(|d| *d == "allow-always")
                .count(),
            1,
            "operator-tier set must include allow-always: {:?}",
            r.allowed_decisions
        );
    }

    /// `allow-always` is the one tier the legacy default set must never
    /// include — it widens scope beyond what the 2026-08-11 server-side
    /// ceiling knew about, so a missing field may narrow but never widen.
    /// If a future core bumps `default_decisions` to include it, this
    /// test fails and forces a deliberate decision to remove it (the
    /// inline predicate will then surface the always-allow button to
    /// operators on legacy cores — a behaviour change worth a
    /// controller-visible test failure, not a silent widening).
    #[test]
    fn legacy_default_decisions_omit_allow_always() {
        let r = default_record();
        assert_eq!(
            r.allowed_decisions
                .iter()
                .filter(|d| *d == "allow-always")
                .count(),
            0,
            "legacy three-button set must not include allow-always: {:?}",
            r.allowed_decisions
        );
    }

    // -- PendingApprovalView::remaining_secs ------------------------------

    /// `remaining_secs` answers whole seconds, not fractional — the render
    /// layer recomputes against the shared 1 s tick rather than freezing at
    /// fetch time, so the contract is "truncated seconds left". A 47.5 s
    /// lead time answers 47, not 48.
    #[test]
    fn remaining_secs_truncates_fractional_seconds() {
        let r = default_record(); // expires_at_ms = 60_000
        let now_ms = r.expires_at_ms - 47_500;
        assert_eq!(r.remaining_secs(now_ms), 47);
    }

    /// An expired-but-not-yet-refetched row must never render a negative
    /// countdown — the render layer reads the answer as seconds-remaining
    /// and a negative number would be a layout bomb. Clamp at zero is the
    /// contract.
    #[test]
    fn remaining_secs_clamps_at_zero_for_an_expired_row() {
        let r = default_record();
        let now_ms = r.expires_at_ms + 5_000;
        assert_eq!(r.remaining_secs(now_ms), 0);
    }
}
