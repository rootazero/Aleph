//! Composer tier pill — the per-session execution tier, next to the model picker.
//!
//! Architecture (mirrors `components/model_picker.rs`, the canonical
//! "small picker in the composer" pattern):
//! 1. Pill shows the session's *effective* tier: the session override when set,
//!    otherwise the global tier.
//! 2. Click opens a popover; the tier IDS and their order are fetched from
//!    `config.get_tool_permissions` — core owns the id set and every permission
//!    verdict behind it (R6: one core, many channels). The COPY is ours: resolved
//!    per locale in `components::exec_tier_labels` (R4: the panel picks an id and
//!    renders it for its reader).
//! 3. Selecting a tier writes `SessionIdentityMeta.custom["exec_tier"]` through
//!    the existing `sessions.patch` RPC — the same carrier as
//!    `custom["project_root"]`. A session tier REPLACES the global tier for that
//!    session (it can escalate as well as lower it) and takes effect on the next
//!    tool call.
//! 4. "Follow global" clears the override.
//!
//! The trap: a brand-new conversation has no `session_key` yet, so there is
//! nothing to patch — and the FIRST turn is the one the user armed the gate
//! for. No amount of client-side parking fixes that: the run resolves its tier
//! when it starts, so a value written after `chat.send` returns is already too
//! late. The tier therefore rides ON the message (`ChatApi::send`'s `exec_tier`,
//! same shape as `project_root`), and the server stamps it onto the session it
//! creates — which is also what makes it survive a reload.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::sessions::set_exec_tier;
use crate::api::tool_permissions::{TierPreset, ToolPermissionsApi};
use crate::components::exec_tier_labels::{tier_desc, tier_label, FULL_TIER};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;

#[component]
#[must_use]
pub fn ExecTierPicker() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let open = RwSignal::new(false);
    let tiers: RwSignal<Vec<TierPreset>> = RwSignal::new(Vec::new());
    let global_tier = RwSignal::new(String::new());
    // Tier id currently armed for the "are you sure" second click (Full only).
    let confirming: RwSignal<Option<String>> = RwSignal::new(None);
    // The read came back refused for lack of operator privilege. Distinct from
    // "no tiers exist": an empty list rendered as a popover holding nothing but
    // a blank-labelled "follow global" row, which reads as a broken control
    // rather than a withheld one.
    let refused = RwSignal::new(false);

    // The global tier + the selectable ids.
    let load = move || {
        spawn_local(async move {
            match ToolPermissionsApi::get_global(&dashboard).await {
                Ok(cfg) => {
                    refused.set(false);
                    global_tier.set(cfg.exec_tier);
                    tiers.set(cfg.tiers);
                }
                Err(e) => {
                    refused.set(crate::components::admin_refusal::is_admin_refusal(&e));
                    web_sys::console::warn_1(&format!("Failed to load exec tiers: {e}").into());
                }
            }
        });
    };

    // Initial fetch, gated on the socket being up: a bare fetch on mount races
    // the WebSocket handshake, loses, and leaves the popover permanently empty —
    // there is no second chance to ask. Re-runs when `is_connected` flips, so a
    // reconnect also refreshes.
    Effect::new(move |_| {
        if !dashboard.is_connected.get() {
            return;
        }
        load();
    });

    // Effective tier: the session override wins over the global tier.
    let effective = Memo::new(move |_| {
        chat.session_exec_tier
            .get()
            .unwrap_or_else(|| global_tier.get())
    });

    let persist = move |session_key: String, tier: Option<String>| {
        spawn_local(async move {
            if let Err(e) = set_exec_tier(&dashboard, &session_key, tier.as_deref()).await {
                web_sys::console::warn_1(&format!("Failed to persist session tier: {e}").into());
            }
        });
    };

    let select = move |id: Option<String>| {
        chat.session_exec_tier.set(id.clone());
        // A live session is written through immediately. A conversation with no
        // session key yet needs no bookkeeping here: the composer carries the
        // tier on the send itself (`ChatApi::send`), and the server stamps it
        // onto the session it creates. Parking the choice client-side could
        // never have governed the first turn anyway — the run resolves its tier
        // when it starts, and by then the parked value has not been written.
        if let Some(session_key) = chat.session_key.get_untracked() {
            persist(session_key, id);
        }
        confirming.set(None);
        open.set(false);
    };

    // Full never asks again — the same explicit confirmation the settings page
    // requires. (The sandbox command-policy floor still holds; see the popover
    // footer.)
    let on_tier_click = move |id: String| {
        if id == FULL_TIER && confirming.get_untracked().as_deref() != Some(FULL_TIER) {
            confirming.set(Some(id));
            return;
        }
        select(Some(id));
    };

    // Reset the arming state whenever the popover closes, so it never reopens
    // one click away from Full.
    Effect::new(move |_| {
        if !open.get() {
            confirming.set(None);
        }
    });

    view! {
        <div class="relative">
            <button
                on:click=move |_| {
                    let opening = !open.get_untracked();
                    open.set(opening);
                    // `MainContent` keeps the chat container mounted (it switches
                    // modes by CSS `display`), so the connect-gated Effect above
                    // runs once per socket — the global tier goes stale the moment
                    // Settings writes a new one. Refetch on open: there is no
                    // config-changed event to subscribe to.
                    if opening {
                        load();
                    }
                }
                class="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono
                       text-text-secondary border border-border
                       bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)]
                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title=move || t_string!(i18n, settings.policies.exec_tier_pill_title).to_string()
            >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <rect x="3" y="11" width="18" height="11" rx="2" />
                    <path d="M7 11V7a5 5 0 0 1 10 0v4" />
                </svg>
                // An unresolved tier renders as a dash, not as nothing: an
                // empty span left the pill looking like a control whose label
                // failed to paint.
                <span>{move || {
                    let id = effective.get();
                    if id.is_empty() { "—".to_string() } else { tier_label(i18n, &id) }
                }}</span>
                // A session override is a deliberate deviation from the global
                // policy — mark it so it can't be mistaken for the default.
                <Show when=move || chat.session_exec_tier.get().is_some()>
                    <span class="w-1.5 h-1.5 rounded-full bg-primary" />
                </Show>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>

            // Click-outside catcher — the repo's standard popover dismissal
            // (`theme_toggle`, `chat_sidebar`, `team_task_strip`). `mouseleave`
            // below is a hover affordance and a finger never produces one, so
            // without this the popover could not be dismissed at all on the
            // phone composer: it stayed up, covering the thread, until the user
            // guessed to tap the pill again.
            {move || open.get().then(|| view! {
                <div class="fixed inset-0 z-40" on:click=move |_| open.set(false) />
            })}

            <Show when=move || open.get()>
                <div class="absolute bottom-full mb-2 left-0 z-50 w-80 max-h-96 overflow-y-auto
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                            p-2 space-y-1"
                    on:mouseleave=move |_| open.set(false)>

                    // Why the list below is empty, when it is empty because the
                    // server refused rather than because there is nothing to
                    // show. Without this the popover was a silent degradation:
                    // one row, no label, no explanation.
                    <Show when=move || refused.get()>
                        <div class="px-2.5 py-2 text-[11px] leading-snug text-text-tertiary">
                            {t!(i18n, settings.admin_refusal.read_tiers)}
                        </div>
                    </Show>

                    // Follow-global row — clears the session override.
                    <button
                        on:click=move |_| select(None)
                        class=move || {
                            let base = "w-full text-left px-2.5 py-2 rounded-md text-xs \
                                        transition-colors flex items-center justify-between gap-2";
                            if chat.session_exec_tier.get().is_none() {
                                format!("{base} bg-primary/10 text-primary border border-primary/30")
                            } else {
                                format!("{base} hover:bg-surface-sunken text-text-secondary border border-transparent")
                            }
                        }
                    >
                        <span class="font-medium">
                            {t!(i18n, settings.policies.exec_tier_follow_global)}
                        </span>
                        <span class="text-text-tertiary text-[10px] font-mono">
                            {move || tier_label(i18n, &global_tier.get())}
                        </span>
                    </button>

                    <For
                        each=move || tiers.get()
                        key=|t: &TierPreset| t.id.clone()
                        children=move |tier: TierPreset| {
                            let id_for_click = tier.id.clone();
                            let id_for_active = tier.id.clone();
                            let id_for_confirm = tier.id.clone();
                            let is_active = Memo::new(move |_| {
                                chat.session_exec_tier.get().as_deref() == Some(id_for_active.as_str())
                            });
                            // Memo (not a bare closure) so both the class fn and
                            // the <Show> below can read it — a closure would move.
                            let is_confirming = Memo::new(move |_| {
                                confirming.get().as_deref() == Some(id_for_confirm.as_str())
                            });
                            view! {
                                <button
                                    on:click=move |_| on_tier_click(id_for_click.clone())
                                    class=move || {
                                        let base = "w-full text-left px-2.5 py-2 rounded-md \
                                                    transition-colors border";
                                        if is_confirming.get() {
                                            format!("{base} bg-danger/10 text-danger border-danger/40")
                                        } else if is_active.get() {
                                            format!("{base} bg-primary/10 text-primary border-primary/30")
                                        } else {
                                            format!("{base} hover:bg-surface-sunken text-text-secondary border-transparent")
                                        }
                                    }
                                >
                                    <div class="text-xs font-medium">
                                        {tier_label(i18n, &tier.id)}
                                    </div>
                                    <div class="text-[10px] leading-snug mt-0.5 text-text-tertiary">
                                        {tier_desc(i18n, &tier.id)}
                                    </div>
                                    <Show when=move || is_confirming.get()>
                                        <div class="text-[10px] mt-1 font-semibold">
                                            {t!(i18n, settings.policies.exec_tier_confirm_again)}
                                        </div>
                                    </Show>
                                </button>
                            }
                        }
                    />

                    <div class="px-2.5 pt-1 text-[10px] leading-snug text-text-tertiary border-t border-border">
                        {t!(i18n, settings.policies.exec_tier_floor_note)}
                    </div>
                </div>
            </Show>
        </div>
    }
}
