//! Composer pills for the two session dials added after the tier and the mode:
//! reasoning depth and memory injection.
//!
//! Same pattern as `exec_tier_picker` / `mode_picker` — pill shows the dial's
//! effective position, the popover offers the ids core enumerated, a pick
//! writes `SessionIdentityMeta.custom[…]` through `sessions.patch`, and a
//! brand-new conversation carries the value on its first message instead
//! (`session_dials_for_send`).
//!
//! **One component for both**, parameterised by [`Dial`], rather than a third
//! and fourth copy of that ~230-line file. The copies were justified while the
//! two differed (the tier has a confirm-again step for `full`, the mode does
//! not); these two differ only in which signal they read, which key they patch
//! and which words they use, so a `match` covers it — and adding a fifth dial
//! is then a variant plus a match arm, with the compiler naming every place
//! that has to answer for it.
//!
//! # The one asymmetry worth reading
//!
//! The memory dial has a global to follow (`[memory] enabled`); the thinking
//! dial does not. Core resolves depth as request > session > *no directive at
//! all*, so clearing that override leaves the provider on its own default.
//! Rendering it as "follow global" would name a setting that does not exist,
//! which is why [`Dial::global`] returns `None` for `Think` and the
//! clear-the-override row uses different copy.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::sessions::{set_memory_mode, set_think_level};
use crate::api::tool_permissions::{DialPreset, ToolPermissionsApi, ToolPermissionsResponse};
use crate::components::dial_labels::{memory_desc, memory_label, think_desc, think_label};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, I18nCtx};
use crate::views::chat::state::ChatState;

/// Which session dial a [`DialPicker`] drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dial {
    /// Reasoning depth (`off` … `xhigh`).
    Think,
    /// Whether memory envelopes are injected (`on` / `off`).
    Memory,
}

impl Dial {
    /// The ids this dial can take, as core enumerated them. Empty against a
    /// core that predates the dial — which hides the pill, rather than showing
    /// a control with nothing behind it.
    fn presets(self, cfg: &ToolPermissionsResponse) -> Vec<DialPreset> {
        match self {
            Self::Think => cfg.think_levels.clone(),
            Self::Memory => cfg.memory_modes.clone(),
        }
    }

    /// Where the install-wide default for this dial sits, if it has one.
    ///
    /// `None` for `Think` on purpose — see the module doc. Returning the
    /// shipped default (`minimal`) here instead would be this client inventing
    /// a fact: nothing is sent when the override is clear, so no level is in
    /// force that a surface could name.
    fn global(self, cfg: &ToolPermissionsResponse) -> Option<String> {
        match self {
            Self::Think => None,
            Self::Memory => Some(cfg.memory.clone()).filter(|m| !m.is_empty()),
        }
    }

    /// The `ChatState` signal holding this session's override.
    fn signal(self, chat: &ChatState) -> RwSignal<Option<String>> {
        match self {
            Self::Think => chat.session_think_level,
            Self::Memory => chat.session_memory_mode,
        }
    }

    /// Persist (or clear) the override on a live session.
    async fn persist(
        self,
        dash: &DashboardState,
        session_key: &str,
        value: Option<&str>,
    ) -> Result<(), String> {
        match self {
            Self::Think => set_think_level(dash, session_key, value).await,
            Self::Memory => set_memory_mode(dash, session_key, value).await,
        }
    }

    fn label(self, i18n: I18nCtx, id: &str) -> String {
        match self {
            Self::Think => think_label(i18n, id),
            Self::Memory => memory_label(i18n, id),
        }
    }

    fn desc(self, i18n: I18nCtx, id: &str) -> String {
        match self {
            Self::Think => think_desc(i18n, id),
            Self::Memory => memory_desc(i18n, id),
        }
    }

    fn pill_title(self, i18n: I18nCtx) -> String {
        match self {
            Self::Think => t_string!(i18n, settings.policies.think_pill_title).to_string(),
            Self::Memory => t_string!(i18n, settings.policies.memory_pill_title).to_string(),
        }
    }

    /// Copy for the row that clears the override. Not the same sentence for
    /// both: one follows a configured global, the other stops sending anything.
    fn clear_label(self, i18n: I18nCtx) -> String {
        match self {
            Self::Think => t_string!(i18n, settings.policies.think_follow_default).to_string(),
            Self::Memory => t_string!(i18n, settings.policies.memory_follow_global).to_string(),
        }
    }

    fn note(self, i18n: I18nCtx) -> String {
        match self {
            Self::Think => t_string!(i18n, settings.policies.think_note).to_string(),
            Self::Memory => t_string!(i18n, settings.policies.memory_note).to_string(),
        }
    }

    /// What the pill shows when nothing is chosen and there is no global to
    /// fall back on. Only reachable for `Think`.
    fn unset_label(self, i18n: I18nCtx) -> String {
        match self {
            Self::Think => t_string!(i18n, settings.policies.think_unset_label).to_string(),
            // A memory dial with no global means an older core, and the pill is
            // already hidden in that case (empty `memory_modes`).
            Self::Memory => "—".to_string(),
        }
    }
}

/// Pill + popover for one session dial.
#[component]
#[must_use]
pub fn DialPicker(dial: Dial) -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let open = RwSignal::new(false);
    let presets: RwSignal<Vec<DialPreset>> = RwSignal::new(Vec::new());
    let global: RwSignal<Option<String>> = RwSignal::new(None);
    // A refused read leaves `presets` empty too, and the hide-on-empty rule
    // below would erase the whole control. The two causes need telling apart:
    // only one of them means "this build has no such dial".
    let refused = RwSignal::new(false);
    let selected = dial.signal(&chat);

    let load = move || {
        spawn_local(async move {
            match ToolPermissionsApi::get_global(&dashboard).await {
                Ok(cfg) => {
                    refused.set(false);
                    global.set(dial.global(&cfg));
                    presets.set(dial.presets(&cfg));
                }
                Err(e) => {
                    refused.set(crate::components::admin_refusal::is_admin_refusal(&e));
                    web_sys::console::warn_1(&format!("Failed to load session dial: {e}").into());
                }
            }
        });
    };

    // Gated on the socket being up: a bare on-mount fetch races the WS
    // handshake (see `exec_tier_picker`).
    Effect::new(move |_| {
        if !dashboard.is_connected.get() {
            return;
        }
        load();
    });

    // What the pill reads: the session's own choice, else the global, else the
    // dial's "nothing is in force" copy.
    let trigger = Memo::new(move |_| match selected.get().or_else(|| global.get()) {
        Some(id) if !id.is_empty() => dial.label(i18n, &id),
        _ => dial.unset_label(i18n),
    });

    let select = move |id: Option<String>| {
        selected.set(id.clone());
        // A live session is written through immediately; the first message of a
        // brand-new conversation carries the value itself.
        if let Some(session_key) = chat.session_key.get_untracked() {
            spawn_local(async move {
                if let Err(e) = dial.persist(&dashboard, &session_key, id.as_deref()).await {
                    web_sys::console::warn_1(
                        &format!("Failed to persist session dial: {e}").into(),
                    );
                }
            });
        }
        open.set(false);
    };

    view! {
        // An older core enumerates nothing for this dial — render nothing
        // rather than a pill with a blank label. A REFUSED read is a different
        // fact and keeps the pill: vanishing is the one outcome a reader cannot
        // tell apart from "this feature does not exist".
        <Show when=move || !presets.get().is_empty() || refused.get()>
        <div class="relative">
            <button
                on:click=move |_| {
                    let opening = !open.get_untracked();
                    open.set(opening);
                    // Refetch on open: the global can change in Settings and
                    // there is no config-changed subscription (same staleness
                    // workaround as the tier and mode pills).
                    if opening {
                        load();
                    }
                }
                class="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono
                       text-text-secondary border border-border
                       bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)]
                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title=move || dial.pill_title(i18n)
            >
                {move || match dial {
                    // Spark: depth of reasoning bought per turn.
                    Dial::Think => view! {
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                             stroke="currentColor" stroke-width="2"
                             stroke-linecap="round" stroke-linejoin="round">
                            <path d="M12 3l1.9 4.6L18.5 9l-4.6 1.9L12 15.5l-1.9-4.6L5.5 9l4.6-1.4z" />
                            <path d="M18 15l.9 2.1L21 18l-2.1.9L18 21l-.9-2.1L15 18l2.1-.9z" />
                        </svg>
                    }.into_any(),
                    // Stacked discs: what the turn is told it already knows.
                    Dial::Memory => view! {
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                             stroke="currentColor" stroke-width="2"
                             stroke-linecap="round" stroke-linejoin="round">
                            <ellipse cx="12" cy="5" rx="8" ry="3" />
                            <path d="M4 5v6c0 1.7 3.6 3 8 3s8-1.3 8-3V5" />
                            <path d="M4 11v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6" />
                        </svg>
                    }.into_any(),
                }}
                <span>{move || trigger.get()}</span>
                // A session override is a deliberate deviation from the
                // default — mark it, same as the tier and mode pills.
                <Show when=move || selected.get().is_some()>
                    <span class="w-1.5 h-1.5 rounded-full bg-primary" />
                </Show>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>

            // Click-outside catcher — see `exec_tier_picker` for why
            // `mouseleave` alone strands this popover open on a touch surface.
            {move || open.get().then(|| view! {
                <div class="fixed inset-0 z-40" on:click=move |_| open.set(false) />
            })}

            <Show when=move || open.get()>
                <div class="absolute bottom-full mb-2 left-0 z-50 w-80 max-h-96 overflow-y-auto
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                            p-2 space-y-1"
                    on:mouseleave=move |_| open.set(false)>

                    // Why there is nothing to choose from.
                    <Show when=move || refused.get()>
                        <div class="px-2.5 py-2 text-[11px] leading-snug text-text-tertiary">
                            {t!(i18n, settings.admin_refusal.read_dials)}
                        </div>
                    </Show>

                    // Clear-the-override row.
                    <button
                        on:click=move |_| select(None)
                        class=move || {
                            let base = "w-full text-left px-2.5 py-2 rounded-md text-xs \
                                        transition-colors flex items-center justify-between gap-2";
                            if selected.get().is_none() {
                                format!("{base} bg-primary/10 text-primary border border-primary/30")
                            } else {
                                format!("{base} hover:bg-surface-sunken text-text-secondary border border-transparent")
                            }
                        }
                    >
                        <span class="font-medium">{move || dial.clear_label(i18n)}</span>
                        // Only name a global when there is one. For the thinking
                        // dial this stays empty rather than printing a level
                        // nothing is sending.
                        <span class="text-text-tertiary text-[10px] font-mono">
                            {move || global.get().map(|g| dial.label(i18n, &g)).unwrap_or_default()}
                        </span>
                    </button>

                    <For
                        each=move || presets.get()
                        key=|p: &DialPreset| p.id.clone()
                        children=move |preset: DialPreset| {
                            let id_for_click = preset.id.clone();
                            let id_for_active = preset.id.clone();
                            let is_active = Memo::new(move |_| {
                                selected.get().as_deref() == Some(id_for_active.as_str())
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
                                        {dial.label(i18n, &preset.id)}
                                    </div>
                                    <div class="text-[10px] leading-snug mt-0.5 text-text-tertiary">
                                        {dial.desc(i18n, &preset.id)}
                                    </div>
                                </button>
                            }
                        }
                    />

                    <div class="px-2.5 pt-1 text-[10px] leading-snug text-text-tertiary border-t border-border">
                        {move || dial.note(i18n)}
                    </div>
                </div>
            </Show>
        </div>
        </Show>
    }
}
