//! Composer mode pill — the per-session usage mode (chat / work / code),
//! next to the exec-tier pill.
//!
//! Clone of `exec_tier_picker` (the canonical "small picker in the composer"
//! pattern), for the tier's third twin:
//! 1. Pill shows the session's *effective* mode: the session override when
//!    set, otherwise the global `[policies] mode`.
//! 2. The mode IDS and their order come from `config.get_tool_permissions`
//!    (the same fetch as the tier pill — one wire shape, one decoder); copy
//!    resolves per locale in `components::mode_labels` (R4/R6).
//! 3. Selecting a mode writes `SessionIdentityMeta.custom["session_mode"]`
//!    through `sessions.patch`; "Follow global" clears the override. A mode
//!    shapes the tool *presentation* surface only — approvals stay with the
//!    exec tier — so no selection needs a second confirming click.
//! 4. First-message trap, same as the tier: a brand-new conversation has no
//!    session to patch, so the mode also rides on `ChatApi::send` and the
//!    server stamps it onto the session it creates.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::sessions::set_session_mode;
use crate::api::tool_permissions::{ModePreset, ToolPermissionsApi};
use crate::components::mode_labels::{mode_desc, mode_label};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;

#[component]
#[must_use]
pub fn ModePicker() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let open = RwSignal::new(false);
    let modes: RwSignal<Vec<ModePreset>> = RwSignal::new(Vec::new());
    let global_mode = RwSignal::new(String::new());

    // The global default mode + the selectable ids (same RPC the tier pill
    // uses — the response carries both dials).
    let load = move || {
        spawn_local(async move {
            match ToolPermissionsApi::get_global(&dashboard).await {
                Ok(cfg) => {
                    global_mode.set(cfg.mode);
                    modes.set(cfg.modes);
                }
                Err(e) => {
                    web_sys::console::warn_1(&format!("Failed to load session modes: {e}").into());
                }
            }
        });
    };

    // Initial fetch, gated on the socket being up (a bare on-mount fetch races
    // the WS handshake — see exec_tier_picker).
    Effect::new(move |_| {
        if !dashboard.is_connected.get() {
            return;
        }
        load();
    });

    // Effective mode: the session override wins over the global default.
    let effective = Memo::new(move |_| {
        chat.session_mode
            .get()
            .unwrap_or_else(|| global_mode.get())
    });

    let persist = move |session_key: String, mode: Option<String>| {
        spawn_local(async move {
            if let Err(e) = set_session_mode(&dashboard, &session_key, mode.as_deref()).await {
                web_sys::console::warn_1(&format!("Failed to persist session mode: {e}").into());
            }
        });
    };

    let select = move |id: Option<String>| {
        chat.session_mode.set(id.clone());
        // A live session is written through immediately; a first message
        // carries the mode itself (`ChatApi::send`) — see the module doc.
        if let Some(session_key) = chat.session_key.get_untracked() {
            persist(session_key, id);
        }
        open.set(false);
    };

    view! {
        <div class="relative">
            <button
                on:click=move |_| {
                    let opening = !open.get_untracked();
                    open.set(opening);
                    // Refetch on open: the global default can change in
                    // Settings and there is no config-changed subscription
                    // (same staleness workaround as the tier pill).
                    if opening {
                        load();
                    }
                }
                class="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono
                       text-text-secondary border border-border
                       bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)]
                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title=move || t_string!(i18n, settings.policies.mode_pill_title).to_string()
            >
                // Layout-grid glyph: three panes — a nod to the three modes.
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <rect x="3" y="3" width="7" height="7" rx="1" />
                    <rect x="14" y="3" width="7" height="7" rx="1" />
                    <rect x="3" y="14" width="18" height="7" rx="1" />
                </svg>
                <span>{move || mode_label(i18n, &effective.get())}</span>
                // A session override is a deliberate deviation from the global
                // default — mark it, same as the tier pill's dot.
                <Show when=move || chat.session_mode.get().is_some()>
                    <span class="w-1.5 h-1.5 rounded-full bg-primary" />
                </Show>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>

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
                            if chat.session_mode.get().is_none() {
                                format!("{base} bg-primary/10 text-primary border border-primary/30")
                            } else {
                                format!("{base} hover:bg-surface-sunken text-text-secondary border border-transparent")
                            }
                        }
                    >
                        <span class="font-medium">
                            {t!(i18n, settings.policies.mode_follow_global)}
                        </span>
                        <span class="text-text-tertiary text-[10px] font-mono">
                            {move || mode_label(i18n, &global_mode.get())}
                        </span>
                    </button>

                    <For
                        each=move || modes.get()
                        key=|m: &ModePreset| m.id.clone()
                        children=move |mode: ModePreset| {
                            let id_for_click = mode.id.clone();
                            let id_for_active = mode.id.clone();
                            let is_active = Memo::new(move |_| {
                                chat.session_mode.get().as_deref() == Some(id_for_active.as_str())
                            });
                            view! {
                                <button
                                    on:click=move |_| select(Some(id_for_click.clone()))
                                    class=move || {
                                        let base = "w-full text-left px-2.5 py-2 rounded-md \
                                                    transition-colors border";
                                        if is_active.get() {
                                            format!("{base} bg-primary/10 text-primary border-primary/30")
                                        } else {
                                            format!("{base} hover:bg-surface-sunken text-text-secondary border-transparent")
                                        }
                                    }
                                >
                                    <div class="text-xs font-medium">
                                        {mode_label(i18n, &mode.id)}
                                    </div>
                                    <div class="text-[10px] leading-snug mt-0.5 text-text-tertiary">
                                        {mode_desc(i18n, &mode.id)}
                                    </div>
                                </button>
                            }
                        }
                    />

                    <div class="px-2.5 pt-1 text-[10px] leading-snug text-text-tertiary border-t border-border">
                        {t!(i18n, settings.policies.mode_note)}
                    </div>
                </div>
            </Show>
        </div>
    }
}
