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
//!    session and takes effect on the next tool call. It may escalate as well
//!    as lower it **for an operator**; a non-operator's resolved tier is
//!    clamped to the global one (`turn_permissions::resolve_exec_tier`), so a
//!    member can only ever arm the gate further, never disarm it.
//! 4. "Follow global" clears the override.
//!
//! Step 2's fetch is admin-gated, which for a member means REFUSED. That is a
//! state this component renders (see `restricted`) rather than a warning it
//! logs: an empty option list is indistinguishable from "this product has one
//! choice", and saying nothing at all is how the member's only visible control
//! over their own approval gate quietly disappeared. **Known gap:** the tier id
//! CATALOG is still operator-only, so a member cannot pick a stricter tier from
//! this pill even though the server would honour it — closing that needs a
//! member-reachable catalog read, which is a wire change, not a copy fix.
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
use shared_ui_logic::authz::is_admin_required;

#[component]
#[must_use]
pub fn ExecTierPicker() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let open = RwSignal::new(false);
    let tiers: RwSignal<Vec<TierPreset>> = RwSignal::new(Vec::new());
    let global_tier = RwSignal::new(String::new());
    // The server refused the catalog because this caller is not an operator —
    // distinct from "the fetch has not happened yet", which is what an empty
    // `tiers` used to mean for both.
    let restricted = RwSignal::new(false);
    // Tier id currently armed for the "are you sure" second click (Full only).
    let confirming: RwSignal<Option<String>> = RwSignal::new(None);

    // The global tier + the selectable ids.
    //
    // `config.get_tool_permissions` is admin-gated, so for a member this call
    // is REFUSED — and until 2026-08-08 the refusal was swallowed into a
    // `console.warn`, leaving `tiers` empty and the popover showing a single
    // "follow global" entry. The dial did not become unavailable, only
    // invisible: the write paths (`sessions.patch` and `chat.send`'s
    // `exec_tier`) are member-open, so the member kept the capability and lost
    // the control. Now the refusal is a state the popover can render.
    let load = move || {
        spawn_local(async move {
            match ToolPermissionsApi::get_global(&dashboard).await {
                Ok(cfg) => {
                    global_tier.set(cfg.exec_tier);
                    tiers.set(cfg.tiers);
                    restricted.set(false);
                }
                Err(e) => {
                    if is_admin_required(&e) {
                        restricted.set(true);
                    } else {
                        web_sys::console::warn_1(&format!("Failed to load exec tiers: {e}").into());
                    }
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
                <span>{move || tier_label(i18n, &effective.get())}</span>
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

                    // The refusal, said out loud. Before this the popover
                    // simply rendered the row above and nothing else, which
                    // reads as "there is only one choice" — a claim about the
                    // product rather than about this caller's permissions.
                    <Show when=move || restricted.get()>
                        <p class="px-2.5 py-2 text-[11px] leading-relaxed text-text-tertiary">
                            "执行档位由 operator 配置,当前角色读取不到可选档位清单。"
                            "本会话跟随全局档位,且不会超过它。"
                        </p>
                    </Show>

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
