//! `NotificationCenter` — aggregate alert surface.
//!
//! A bell button anchored to the top-right of the window, with a popover
//! listing every active alert from [`crate::context::DashboardState::alerts`].
//! Reads the existing `alerts.**` data stream — no new RPC, no new gateway
//! event variant. UI-private state lives in
//! [`crate::state::notifications::NotificationsState`].
//!
//! Anchoring: fixed-positioned so it survives any view's reflow. z-index
//! sits BELOW the boot/service gates (which take over the whole screen) so
//! a runtime disconnect still blanks the bell.

use crate::components::approval_card::ApprovalCard;
use crate::components::sidebar::AlertLevel;
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::notifications::{
    unread_count, visible_alerts, NotificationsState, PendingApprovalView,
};
use leptos::ev::keydown;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn NotificationCenter() -> impl IntoView {
    let Some(dashboard) = use_context::<DashboardState>() else {
        return ().into_any();
    };
    let Some(notif) = use_context::<NotificationsState>() else {
        return ().into_any();
    };

    let alerts = dashboard.alerts;
    let pending_approvals = dashboard.pending_approvals;
    let is_open = notif.is_open;
    let dismissed = notif.dismissed;

    // Hide the bell entirely until we've ever connected — otherwise the
    // first-boot user sees a stray icon over the BootCheckGate spinner.
    let bell_visible = Memo::new(move |_| dashboard.has_connected_once.get());

    // Reactive badge count. Pure derivation — no side effects.
    // Includes both system alerts and pending operator approvals so the
    // operator sees a single "things to look at" pulse.
    let badge_count = Memo::new(move |_| {
        let a = alerts.get();
        let d = dismissed.get();
        unread_count(&a, &d) + pending_approvals.get().len()
    });

    // Visible alerts list (sorted). Re-evaluates on either signal change.
    let list = Memo::new(move |_| {
        let a = alerts.get();
        let d = dismissed.get();
        visible_alerts(&a, &d)
    });

    // ESC closes the popover.
    //
    // NOT the command_palette pattern, despite what this comment used to say:
    // the palette is mounted at the app root and never unmounts, while this
    // component sits under `<Show when=not_phone>` and is torn down every time
    // the viewport crosses the phone breakpoint. `window_event_listener`
    // registers no cleanup, so each crossing left another orphaned closure
    // holding `is_open` — a signal whose owner is gone — and every later
    // Escape ran `.set()` on it. Same defect `artifacts::preview` was crashing
    // on; found by the guard written for that one.
    let esc_handle = window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        if !is_open.get_untracked() {
            return;
        }
        if ev.key() == "Escape" {
            ev.prevent_default();
            is_open.set(false);
        }
    });
    on_cleanup(move || esc_handle.remove());

    view! {
        <Show when=move || bell_visible.get() fallback=|| ()>
            <button
                type="button"
                class="aleph-chrome-top aleph-no-drag \
                       fixed right-3 z-[50] flex items-center justify-center \
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

        <Show when=move || is_open.get() fallback=|| ()>
            // Backdrop — captures outside clicks. Transparent so the shell
            // remains visible; we just want the dismiss affordance.
            <div
                class="fixed inset-0 z-[54]"
                data-tauri-drag-region="false"
                on:click=move |_| is_open.set(false)
            />
            <div
                class="fixed top-12 right-3 z-[55] w-[min(360px,calc(100vw-32px))] \
                       glass bg-surface-overlay/85 border border-border \
                       rounded-2xl shadow-2xl overflow-hidden animate-pop-in"
                data-tauri-drag-region="false"
                role="dialog"
                aria-label=move || t_string!(use_i18n(), notifications.title).to_string()
            >
                <div class="flex items-center justify-between px-4 py-3 border-b border-border">
                    <h2 class="text-sm font-semibold text-text-primary">
                        {move || t_string!(use_i18n(), notifications.title).to_string()}
                    </h2>
                    <Show when=move || !list.get().is_empty() fallback=|| ()>
                        <button
                            type="button"
                            class="text-xs text-text-tertiary hover:text-text-primary transition-colors"
                            on:click=move |_| {
                                let keys: Vec<String> = list.get_untracked()
                                    .into_iter()
                                    .map(|a| a.key)
                                    .collect();
                                dismissed.update(|set| {
                                    for k in keys { set.insert(k); }
                                });
                            }
                        >
                            {move || t_string!(use_i18n(), notifications.dismiss_all).to_string()}
                        </button>
                    </Show>
                </div>

                <div class="max-h-[60vh] overflow-y-auto">
                    // Pending operator approvals — render first so a fresh
                    // approval prompt is impossible to miss.
                    {move || {
                        let approvals = pending_approvals.get();
                        if approvals.is_empty() {
                            view! { <div></div> }.into_any()
                        } else {
                            view! {
                                <ul class="divide-y divide-border">
                                    {approvals.into_iter().map(|a: PendingApprovalView| {
                                        view! {
                                            <li class="px-4 py-3">
                                                <ApprovalCard approval=a />
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                                </ul>
                            }.into_any()
                        }
                    }}
                    {move || {
                        let items = list.get();
                        if items.is_empty() && pending_approvals.get().is_empty() {
                            view! {
                                <div class="px-4 py-6 text-center text-sm text-text-tertiary">
                                    {move || t_string!(use_i18n(), notifications.empty).to_string()}
                                </div>
                            }.into_any()
                        } else if items.is_empty() {
                            view! { <div></div> }.into_any()
                        } else {
                            view! {
                                <ul class="divide-y divide-border">
                                    {items.into_iter().map(|alert| {
                                        let key_for_dismiss = alert.key.clone();
                                        let key_for_label = alert.key.clone();
                                        let level_class = match alert.level {
                                            AlertLevel::Critical => "bg-danger",
                                            AlertLevel::Warning => "bg-yellow-500",
                                            AlertLevel::Info => "bg-primary",
                                            AlertLevel::None => "bg-text-tertiary",
                                        };
                                        let message_owned = alert.message.clone();
                                        let count_owned = alert.count;
                                        view! {
                                            <li class="px-4 py-3 flex items-start gap-3 hover:bg-surface-sunken/40 transition-colors">
                                                <span
                                                    class=format!("mt-1.5 h-2 w-2 rounded-full flex-shrink-0 {level_class}")
                                                    aria-hidden="true"
                                                />
                                                <div class="flex-1 min-w-0">
                                                    <div class="flex items-center gap-2">
                                                        <span class="text-sm font-medium text-text-primary truncate">
                                                            {key_for_label}
                                                        </span>
                                                        {count_owned.map(|n| view! {
                                                            <span class="text-xs text-text-tertiary">
                                                                {format!("({n})")}
                                                            </span>
                                                        })}
                                                    </div>
                                                    {message_owned.map(|m| view! {
                                                        <p class="mt-1 text-xs text-text-secondary line-clamp-3">
                                                            {m}
                                                        </p>
                                                    })}
                                                </div>
                                                <button
                                                    type="button"
                                                    class="text-text-tertiary hover:text-text-primary text-xs px-1.5 py-0.5 rounded transition-colors"
                                                    on:click=move |_| {
                                                        let k = key_for_dismiss.clone();
                                                        dismissed.update(|set| { set.insert(k); });
                                                    }
                                                    aria-label="Dismiss"
                                                >
                                                    "×"
                                                </button>
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                                </ul>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </Show>
    }.into_any()
}

#[cfg(test)]
mod tests {
    // Component reactivity is exercised end-to-end through the visible_alerts
    // / unread_count tests in state/notifications.rs. This sentinel documents
    // the contract the component leans on so future refactors invalidate the
    // comment first:
    //   * bell visible ⇔ has_connected_once
    //   * badge shown ⇔ unread_count > 0
    //   * popover open ⇔ NotificationsState::is_open
    //   * dismiss-all writes every visible alert key into dismissed
    //   * per-item × inserts just that key
    #[test]
    fn contract_documented() {
        let _ = "see comment above";
    }
}
