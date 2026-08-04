//! Phone Alerts screen (`/more/alerts`).
//!
//! The desktop surface for alerts is the `NotificationCenter` bell, and it is
//! desktop-only for a concrete reason: its bell is `z-50` and its popover
//! `z-55`, both under every phone screen's `z-70` overlay, so on a phone it was
//! rendered but never clickable. Removing it from the phone tree cost the user
//! nothing — and left them with no way at all to see an alert. This screen is
//! that way.
//!
//! Reads the same two signals the bell reads (`DashboardState::alerts` and
//! `::pending_approvals`) through the same two pure helpers
//! (`visible_alerts` / `unread_count`) and renders approvals with the same
//! `ApprovalCard`. No new RPC, no new event variant, no second notion of what
//! counts as unread — a second derivation is how the badge and the list start
//! disagreeing about how many things are waiting.
//!
//! Route: `/more/alerts`, so `PanelMode::from_path`'s existing
//! `starts_with("/more")` arm keeps the ••• tab highlighted with no new mode.
//! Entry points are the More menu row and the tab-bar badge, both in
//! [`super::more`] and [`super::shell`].
//!
//! I/O-only (R4): the only mutation is the local dismiss set.

use leptos::prelude::*;

use crate::components::approval_card::ApprovalCard;
use crate::components::sidebar::AlertLevel;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::platform::phone::shell::PhoneShell;
use crate::state::notifications::{visible_alerts, NotificationsState, PendingApprovalView};

/// Dot colour per severity. Same mapping as the desktop popover.
const fn level_class(level: AlertLevel) -> &'static str {
    match level {
        AlertLevel::Critical => "bg-danger",
        AlertLevel::Warning => "bg-yellow-500",
        AlertLevel::Info => "bg-primary",
        AlertLevel::None => "bg-text-tertiary",
    }
}

#[component]
#[must_use]
pub fn PhoneAlerts() -> impl IntoView {
    let i18n = use_i18n();
    // Both contexts are provided at the app root. `use_context` (not `expect_`)
    // so a mount outside that tree renders empty instead of panicking the panel
    // — the same guard the desktop bell takes.
    let (Some(dashboard), Some(notif)) = (
        use_context::<DashboardState>(),
        use_context::<NotificationsState>(),
    ) else {
        return ().into_any();
    };

    let alerts = dashboard.alerts;
    let pending_approvals = dashboard.pending_approvals;
    let dismissed = notif.dismissed;

    let list = Memo::new(move |_| visible_alerts(&alerts.get(), &dismissed.get()));

    view! {
        <PhoneShell title="Alerts" back="/more" back_label="More">
            {move || {
                let approvals = pending_approvals.get();
                if approvals.is_empty() {
                    return ().into_any();
                }
                view! {
                    // Approvals first: an operator prompt is blocking work,
                    // an alert is describing it.
                    <div class="list-header">"Approvals"</div>
                    <div class="list">
                        {approvals.into_iter().map(|a: PendingApprovalView| view! {
                            <div class="cell" style="display:block;">
                                <ApprovalCard approval=a />
                            </div>
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }}

            {move || {
                let items = list.get();
                if items.is_empty() {
                    // Empty state has to distinguish itself from the approvals
                    // block above, which may well be non-empty.
                    if pending_approvals.get().is_empty() {
                        return view! {
                            <div class="px-4 py-10 text-center text-sm text-text-tertiary">
                                {t_string!(i18n, notifications.empty).to_string()}
                            </div>
                        }.into_any();
                    }
                    return ().into_any();
                }
                view! {
                    <div class="list-header">"Alerts"</div>
                    <div class="list">
                        {items.into_iter().map(|alert| {
                            let key_for_dismiss = alert.key.clone();
                            let dot = level_class(alert.level);
                            let message = alert.message.clone();
                            let count = alert.count;
                            view! {
                                <div class="cell" style="align-items:flex-start;">
                                    <span
                                        class=format!("mt-2 h-2 w-2 rounded-full flex-shrink-0 {dot}")
                                        aria-hidden="true"
                                    />
                                    <div class="cell-body">
                                        <div class="cell-title">
                                            {alert.key.clone()}
                                            {count.map(|n| view! {
                                                <span class="cell-value">{format!(" ({n})")}</span>
                                            })}
                                        </div>
                                        {message.map(|m| view! {
                                            <div class="cell-sub">{m}</div>
                                        })}
                                    </div>
                                    <button
                                        type="button"
                                        // 44 px is the iOS minimum touch target;
                                        // the desktop popover's `×` is a 20 px
                                        // hover affordance and is not usable here.
                                        class="cell-chevron"
                                        style="min-width:44px; min-height:44px; display:flex; align-items:center; justify-content:center; background:none; border:0; cursor:pointer; font-size:20px; line-height:1;"
                                        aria-label="Dismiss"
                                        on:click=move |_| {
                                            let k = key_for_dismiss.clone();
                                            dismissed.update(|set| { set.insert(k); });
                                        }
                                    >
                                        "×"
                                    </button>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }}

            {move || {
                let items = list.get();
                if items.len() < 2 {
                    return ().into_any();
                }
                view! {
                    <button
                        type="button"
                        class="w-full py-3 text-sm text-primary"
                        style="background:none; border:0; cursor:pointer;"
                        // Dismisses alerts only, and stays put. Navigating back
                        // on success would have hidden any pending approvals
                        // still rendered above — this button does not speak for
                        // them, and they are the half that blocks work.
                        on:click=move |_| {
                            let keys: Vec<String> =
                                list.get_untracked().into_iter().map(|a| a.key).collect();
                            dismissed.update(|set| {
                                for k in keys {
                                    set.insert(k);
                                }
                            });
                        }
                    >
                        {t_string!(i18n, notifications.dismiss_all).to_string()}
                    </button>
                }.into_any()
            }}
        </PhoneShell>
    }
    .into_any()
}
