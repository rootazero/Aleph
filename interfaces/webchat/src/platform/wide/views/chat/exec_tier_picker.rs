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
//! Step 2's fetch reaches a member: `config.get_tool_permissions` is carved out
//! of the otherwise-admin `config.` family (`method_admin::MEMBER_CARVE_OUTS`)
//! and the handler narrows the response to the four id enumerations, dropping
//! the two server-global policy axes Settings → Policies edits. So the catalogue
//! is real for a member and this pill offers the full ladder — which matters
//! because the write paths were member-open all along: gating only the READ left
//! the door open and the menu locked.
//!
//! The `Refused` catalogue state below is no longer a member's normal case, and
//! it is deliberately kept: it is what an unauthorized or downgraded connection
//! renders, and the alternative — an empty option list — is indistinguishable
//! from "this product has one choice". Saying nothing at all is how the only
//! visible control over one's own approval gate quietly disappeared once
//! already.
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
use crate::views::chat::exec_tier_picker_state::{CatalogState, PickerState, SaveState, Selection};
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
    let picker = RwSignal::new(PickerState::new(chat.session_key.get_untracked()));
    let save_refused = RwSignal::new(false);

    let load = move || {
        let mut state = picker.get_untracked();
        state.switch_session(chat.session_key.get_untracked());
        let request = state.begin_load();
        picker.set(state);
        spawn_local(async move {
            let result = ToolPermissionsApi::get_global(&dashboard).await;
            // try_get also rejects a disposed component. Check the live session
            // before any write, even if its reset Effect has not run yet.
            if chat.session_key.try_get_untracked() != Some(request.session_key.clone()) {
                return;
            }
            let Some(mut state) = picker.try_get_untracked() else {
                return;
            };
            let catalog = match &result {
                Ok(cfg) if cfg.session_tier_presets().is_empty() => CatalogState::Empty,
                Ok(_) => CatalogState::Ready,
                Err(error) if crate::components::admin_refusal::is_admin_refusal(error) => {
                    CatalogState::Refused
                }
                Err(_) => CatalogState::Failed,
            };
            if !state.finish_load(&request, catalog) {
                return;
            }
            if let Ok(cfg) = result {
                tiers.set(cfg.session_tier_presets().to_vec());
                global_tier.set(cfg.exec_tier);
            }
            picker.set(state);
        });
    };

    Effect::new(move |_| {
        let session_key = chat.session_key.get();
        let connected = dashboard.is_connected.get();
        let mut state = picker.get_untracked();
        state.switch_session(session_key);
        picker.set(state);
        save_refused.set(false);
        open.set(false);
        if connected {
            load();
        }
    });

    let effective = Memo::new(move |_| {
        chat.session_exec_tier
            .get()
            .unwrap_or_else(|| global_tier.get())
    });

    let apply_selection = move |selection: Selection| match selection {
        Selection::Ignored | Selection::Confirm => {}
        Selection::Commit(tier) => {
            chat.session_exec_tier.set(tier);
            save_refused.set(false);
            open.set(false);
        }
        Selection::Persist { request, tier } => {
            save_refused.set(false);
            spawn_local(async move {
                let Some(session_key) = request.session_key.as_deref() else {
                    return;
                };
                let result = set_exec_tier(&dashboard, session_key, tier.as_deref()).await;
                if chat.session_key.try_get_untracked() != Some(request.session_key.clone()) {
                    return;
                }
                let Some(mut state) = picker.try_get_untracked() else {
                    return;
                };
                if !state.accepts_save(&request) {
                    return;
                }
                let committed = state.complete_save(&request, result.is_ok());
                save_refused.set(result.as_ref().err().is_some_and(|error| {
                    crate::components::admin_refusal::is_admin_refusal(error)
                }));
                picker.set(state);
                if let Some(tier) = committed {
                    chat.session_exec_tier.set(tier);
                    open.set(false);
                } else {
                    open.set(true);
                }
            });
        }
    };

    let select = move |id: Option<String>| {
        let mut state = picker.get_untracked();
        state.switch_session(chat.session_key.get_untracked());
        let needs_confirmation = id.as_deref() == Some(FULL_TIER);
        let selection = state.choose(id, needs_confirmation);
        picker.set(state);
        apply_selection(selection);
    };

    let retry_save = move || {
        let mut state = picker.get_untracked();
        state.switch_session(chat.session_key.get_untracked());
        let selection = state.retry();
        picker.set(state);
        save_refused.set(false);
        apply_selection(selection);
    };

    Effect::new(move |_| {
        if !open.get() {
            picker.update(|state| state.close());
        }
    });

    let saving = Memo::new(move |_| picker.get().save == SaveState::Saving);
    let can_follow = Memo::new(move |_| {
        !saving.get()
            && matches!(
                picker.get().catalog,
                CatalogState::Ready | CatalogState::Empty
            )
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
                <Show when=move || saving.get()>
                    <span role="status" class="text-text-tertiary">{t!(i18n, settings.policies.exec_tier_saving)}</span>
                </Show>
                <Show when=move || !saving.get()>
                    <span>{move || {
                        let id = effective.get();
                        if id.is_empty() { "—".to_string() } else { tier_label(i18n, &id) }
                    }}</span>
                </Show>
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

                    <div role="status" aria-live="polite" class="px-2.5 text-[11px] leading-snug text-text-secondary">
                        {move || match picker.get().catalog {
                            CatalogState::Loading => t_string!(i18n, settings.policies.exec_tier_loading).to_string(),
                            CatalogState::Ready => String::new(),
                            CatalogState::Empty => t_string!(i18n, settings.policies.exec_tier_empty).to_string(),
                            CatalogState::Refused => t_string!(i18n, settings.admin_refusal.read_tiers).to_string(),
                            CatalogState::Failed => t_string!(i18n, settings.policies.exec_tier_load_failed).to_string(),
                        }}
                    </div>
                    <Show when=move || matches!(picker.get().catalog, CatalogState::Empty | CatalogState::Refused | CatalogState::Failed)>
                        <button type="button" on:click=move |_| load()
                            class="px-2.5 py-2 text-xs text-primary hover:bg-surface-sunken rounded-md">
                            {t!(i18n, settings.policies.exec_tier_retry)}
                        </button>
                    </Show>
                    <Show when=move || picker.get().save == SaveState::Failed>
                        <div role="alert" class="px-2.5 py-2 text-xs text-danger">
                            {move || if save_refused.get() {
                                t_string!(i18n, settings.policies.exec_tier_save_refused).to_string()
                            } else {
                                t_string!(i18n, settings.policies.exec_tier_save_failed).to_string()
                            }}
                            <button type="button" on:click=move |_| retry_save()
                                class="block py-1 text-primary hover:underline">
                                {t!(i18n, settings.policies.exec_tier_retry)}
                            </button>
                        </div>
                    </Show>

                    // Follow-global row — clears the session override.
                    <button
                        disabled=move || !can_follow.get()
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
                        each=move || if picker.get().catalog == CatalogState::Ready { tiers.get() } else { Vec::new() }
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
                                picker.get().confirming.as_deref() == Some(id_for_confirm.as_str())
                            });
                            view! {
                                <button
                                    disabled=move || saving.get()
                                    on:click=move |_| select(Some(id_for_click.clone()))
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
