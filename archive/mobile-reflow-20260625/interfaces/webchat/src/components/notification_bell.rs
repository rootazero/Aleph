//! `NotificationBell` — the bell *trigger* button, split out of
//! `notification_center.rs` (§11 P-②) so `MobileTopBar` can mount it in its
//! right slot. Reads the SAME root-provided `NotificationsState` +
//! `DashboardState` the popover does; toggles `NotificationsState::is_open`.
//! The popover/sheet + dismissed-set logic STAY at the root in
//! `NotificationCenter` — only the button moved, so there is no lifecycle
//! change (R-2).

use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::notifications::{unread_count, NotificationsState};
use leptos::prelude::*;

#[component]
#[must_use]
pub fn NotificationBell() -> impl IntoView {
    let dashboard = use_context::<DashboardState>().expect("DashboardState not provided");
    let notif = use_context::<NotificationsState>().expect("NotificationsState not provided");

    let alerts = dashboard.alerts;
    let pending_approvals = dashboard.pending_approvals;
    let is_open = notif.is_open;
    let dismissed = notif.dismissed;

    // Hide the bell until we've ever connected — otherwise first boot shows a
    // stray icon over the BootCheckGate spinner.
    let bell_visible = Memo::new(move |_| dashboard.has_connected_once.get());

    let badge_count = Memo::new(move |_| {
        let a = alerts.get();
        let d = dismissed.get();
        unread_count(&a, &d) + pending_approvals.get().len()
    });

    view! {
        <Show when=move || bell_visible.get() fallback=|| ()>
            <button
                type="button"
                class="aleph-no-drag relative flex items-center justify-center \
                       h-7 w-7 rounded-full text-text-secondary hover:text-text-primary \
                       hover:bg-surface-raised transition-colors"
                data-tauri-drag-region="false"
                on:click=move |_| is_open.update(|v| *v = !*v)
                aria-label=move || t_string!(use_i18n(), notifications.open_label).to_string()
                title=move || t_string!(use_i18n(), notifications.open_label).to_string()
            >
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="1.8"
                     stroke-linecap="round" stroke-linejoin="round">
                    <path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9" />
                    <path d="M13.73 21a2 2 0 0 1-3.46 0" />
                </svg>
                <Show when=move || { badge_count.get() > 0 } fallback=|| ()>
                    <span class="absolute -top-0.5 -right-0.5 min-w-[16px] h-[16px] \
                                 px-1 rounded-full bg-danger text-white text-[10px] \
                                 font-semibold flex items-center justify-center \
                                 border border-surface">
                        {move || badge_count.get().min(99).to_string()}
                    </span>
                </Show>
            </button>
        </Show>
    }
}
