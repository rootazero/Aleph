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

use crate::api::ExecApprovalApi;
use crate::components::notification_bell::NotificationBell;
use crate::components::sidebar::AlertLevel;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::notifications::{visible_alerts, NotificationsState, PendingApprovalView};
use leptos::ev::keydown;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
#[must_use]
pub fn NotificationCenter() -> impl IntoView {
    let dashboard = use_context::<DashboardState>().expect("DashboardState not provided");
    let notif = use_context::<NotificationsState>().expect("NotificationsState not provided");

    let alerts = dashboard.alerts;
    let pending_approvals = dashboard.pending_approvals;
    let is_open = notif.is_open;
    let dismissed = notif.dismissed;

    // Visible alerts list (sorted). Re-evaluates on either signal change.
    let list = Memo::new(move |_| {
        let a = alerts.get();
        let d = dismissed.get();
        visible_alerts(&a, &d)
    });

    // ESC closes the popover. Mirrors the command_palette pattern.
    window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        if !is_open.get_untracked() {
            return;
        }
        if ev.key() == "Escape" {
            ev.prevent_default();
            is_open.set(false);
        }
    });

    view! {
        <div class="aleph-chrome-top fixed right-3 z-[50] max-sm:hidden">
            <NotificationBell />
        </div>

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
                       rounded-2xl shadow-2xl overflow-hidden animate-pop-in \
                       max-sm:inset-0 max-sm:top-0 max-sm:right-0 max-sm:w-full max-sm:h-full \
                       max-sm:rounded-none max-sm:border-0 max-sm:flex max-sm:flex-col \
                       max-sm:bg-surface-overlay"
                data-tauri-drag-region="false"
                role="dialog"
                aria-label=move || t_string!(use_i18n(), notifications.title).to_string()
            >
                <div class="flex items-center justify-between px-4 py-3 border-b border-border \
                            max-sm:pt-[calc(0.75rem+var(--safe-area-top))]">
                    <div class="flex items-center gap-1.5">
                        // Mobile-only back chevron — the full-screen sheet has no
                        // visible backdrop to tap, so this is the dismiss affordance.
                        <button
                            type="button"
                            class="hidden max-sm:flex items-center justify-center -ml-1.5 h-7 w-7 \
                                   rounded-full text-text-secondary hover:text-text-primary"
                            on:click=move |_| is_open.set(false)
                            aria-label="Back"
                        >
                            <svg width="20" height="20" viewBox="0 0 24 24" fill="none"
                                 stroke="currentColor" stroke-width="2"
                                 stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="15 18 9 12 15 6" />
                            </svg>
                        </button>
                        <h2 class="text-sm font-semibold text-text-primary">
                            {move || t_string!(use_i18n(), notifications.title).to_string()}
                        </h2>
                    </div>
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

                <div class="max-h-[60vh] overflow-y-auto \
                            max-sm:max-h-none max-sm:flex-1 \
                            max-sm:pb-[var(--safe-area-bottom)]">
                    // Pending operator approvals — render first so a fresh
                    // approval prompt is impossible to miss.
                    {move || {
                        let approvals = pending_approvals.get();
                        if approvals.is_empty() {
                            view! { <div></div> }.into_any()
                        } else {
                            view! {
                                <div class="hidden max-sm:block px-4 pt-3 pb-1 text-xs \
                                            font-medium uppercase tracking-wider text-text-tertiary">
                                    {move || t_string!(use_i18n(), notifications.section_pending).to_string()}
                                </div>
                                <ul class="divide-y divide-border">
                                    {approvals.into_iter().map(|a: PendingApprovalView| {
                                        let i18n = use_i18n();
                                        let id_once = a.id.clone();
                                        let id_session = a.id.clone();
                                        let id_deny = a.id.clone();
                                        let command = a.command.clone();
                                        let agent_id = a.agent_id.clone();
                                        let secs = (a.remaining_ms / 1000).to_string();
                                        view! {
                                            <li class="px-4 py-3">
                                                <div class="text-sm font-medium text-text-primary">
                                                    {t!(i18n, notifications.approval_header)}
                                                </div>
                                                <div class="font-mono text-sm my-1 text-primary">
                                                    {command}
                                                </div>
                                                <div class="text-xs text-text-secondary">
                                                    {t!(i18n, notifications.approval_requested_by)} ": " {agent_id}
                                                </div>
                                                <div class="text-xs text-text-tertiary mt-0.5">
                                                    {t!(i18n, notifications.approval_expires)} " " {secs} "s"
                                                </div>
                                                <div class="flex gap-2 mt-2">
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-primary hover:bg-primary-hover text-white text-xs font-semibold transition-colors"
                                                        on:click=move |_| {
                                                            let id = id_once.clone();
                                                            spawn_local(async move {
                                                                match ExecApprovalApi::resolve(&dashboard, id.clone(), "allow-once"
                                                                ).await {
                                                                    Ok(_) => {
                                                                        dashboard.pending_approvals.update(|l| l.retain(|x| x.id != id));
                                                                    }
                                                                    Err(e) => {
                                                                        web_sys::console::warn_1(&format!("Failed to resolve approval (allow-once): {e:?}").into());
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approval_allow_once)}
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-surface-raised hover:bg-surface-sunken text-text-primary text-xs border border-border transition-colors"
                                                        on:click=move |_| {
                                                            let id = id_session.clone();
                                                            spawn_local(async move {
                                                                match ExecApprovalApi::resolve(
                                                                    &dashboard, id.clone(), "allow-session"
                                                                ).await {
                                                                    Ok(_) => {
                                                                        dashboard.pending_approvals.update(|l| l.retain(|x| x.id != id));
                                                                    }
                                                                    Err(e) => {
                                                                        web_sys::console::warn_1(&format!("Failed to resolve approval (allow-session): {e:?}").into());
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approval_allow_session)}
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary text-xs transition-colors"
                                                        on:click=move |_| {
                                                            let id = id_deny.clone();
                                                            spawn_local(async move {
                                                                match ExecApprovalApi::resolve(
                                                                    &dashboard, id.clone(), "deny"
                                                                ).await {
                                                                    Ok(_) => {
                                                                        dashboard.pending_approvals.update(|l| l.retain(|x| x.id != id));
                                                                    }
                                                                    Err(e) => {
                                                                        web_sys::console::warn_1(&format!("Failed to resolve approval (deny): {e:?}").into());
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approval_deny)}
                                                    </button>
                                                </div>
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
                                <div class="hidden max-sm:block px-4 pt-3 pb-1 text-xs \
                                            font-medium uppercase tracking-wider text-text-tertiary">
                                    {move || t_string!(use_i18n(), notifications.section_recent).to_string()}
                                </div>
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
    }
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
