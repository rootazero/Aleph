//! Composer tier pill — the per-session execution tier, next to the model picker.
//!
//! Architecture (mirrors `components/model_picker.rs`, the canonical
//! "small picker in the composer" pattern):
//! 1. Pill shows the session's *effective* tier: the session override when set,
//!    otherwise the global tier.
//! 2. Click opens a popover; the three tiers are fetched from
//!    `config.get_tool_permissions` — labels and descriptions come FROM CORE
//!    (`builtin_tiers()`), never hardcoded here (R6: one core, many channels;
//!    R4: the panel picks an id and renders what core says about it).
//! 3. Selecting a tier writes `SessionIdentityMeta.custom["exec_tier"]` through
//!    the existing `sessions.patch` RPC — the same carrier as
//!    `custom["project_root"]`. A session tier REPLACES the global tier for that
//!    session (it can escalate as well as lower it) and takes effect on the next
//!    tool call.
//! 4. "Follow global" clears the override.
//!
//! The trap: a brand-new conversation has no `session_key` yet, so there is
//! nothing to patch. The choice is parked KEYED BY CONVERSATION and flushed as
//! soon as that conversation's first `chat.send` resolves a session key. Keying
//! is the whole point — a single parked value would be applied to whichever
//! conversation happened to be restored next, silently escalating a chat the
//! user never touched.

use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

use crate::api::sessions::set_exec_tier;
use crate::api::tool_permissions::{TierPreset, ToolPermissionsApi};
use crate::context::DashboardState;
use crate::state::sessions::{ConvId, SessionMap};
use crate::views::chat::state::ChatState;

/// The tier whose blast radius warrants a second click.
const FULL_TIER: &str = "full";

/// Short pill label for a tier id — the first word of core's label
/// ("Auto 自动" → "Auto"), so the composer chip stays compact. Falls back to
/// the raw id when the tier is unknown to this client (forward compatibility:
/// core may grow a fourth tier).
fn pill_label(tiers: &[TierPreset], id: &str) -> String {
    tiers
        .iter()
        .find(|t| t.id == id)
        .and_then(|t| t.label.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| id.to_string())
}

#[component]
#[must_use]
pub fn ExecTierPicker() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();

    let open = RwSignal::new(false);
    let tiers: RwSignal<Vec<TierPreset>> = RwSignal::new(Vec::new());
    let global_tier = RwSignal::new(String::new());
    // Tier id currently armed for the "are you sure" second click (Full only).
    let confirming: RwSignal<Option<String>> = RwSignal::new(None);
    // Choices made in a conversation that had no session key yet, keyed by the
    // conversation they were made in. Survives tab swaps (the composer is not
    // remounted) and can never be applied to the wrong chat.
    let parked: RwSignal<HashMap<ConvId, Option<String>>> = RwSignal::new(HashMap::new());

    // The global tier + the three presets. Fetched once on mount: the pill's
    // label depends on the global tier even before the popover is ever opened.
    spawn_local(async move {
        match ToolPermissionsApi::get_global(&dashboard).await {
            Ok(cfg) => {
                global_tier.set(cfg.exec_tier);
                tiers.set(cfg.tiers);
            }
            Err(e) => {
                web_sys::console::warn_1(&format!("Failed to load exec tiers: {e}").into());
            }
        }
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

    // Flush the parked choice of the conversation it was made in, once that
    // conversation's first `chat.send` resolves a session key. Re-runs on both
    // signals, so it also fires when the user switches back to a conversation
    // whose tier is still unflushed.
    Effect::new(move |_| {
        let session_key = chat.session_key.get();
        let Some(conv) = sessions.active.get() else {
            return;
        };
        let Some(session_key) = session_key else {
            return;
        };
        let Some(tier) = parked.try_update(|m| m.remove(&conv)).flatten() else {
            return;
        };
        persist(session_key, tier);
    });

    let select = move |id: Option<String>| {
        chat.session_exec_tier.set(id.clone());
        match chat.session_key.get_untracked() {
            // Live session — write through; nothing to park.
            Some(session_key) => persist(session_key, id),
            // Brand-new conversation: hold the choice against THIS conversation
            // until its first send resolves a session key.
            None => {
                if let Some(conv) = sessions.active_conv() {
                    parked.update(|m| {
                        m.insert(conv, id);
                    });
                }
            }
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
                on:click=move |_| open.update(|v| *v = !*v)
                class="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono
                       text-text-secondary border border-border
                       bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)]
                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title="工具执行档位 / Tool execution tier"
            >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <rect x="3" y="11" width="18" height="11" rx="2" />
                    <path d="M7 11V7a5 5 0 0 1 10 0v4" />
                </svg>
                <span>{move || pill_label(&tiers.get(), &effective.get())}</span>
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
                        <span class="font-medium">"跟随全局 / Follow global"</span>
                        <span class="text-text-tertiary text-[10px] font-mono">
                            {move || pill_label(&tiers.get(), &global_tier.get())}
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
                                        {tier.label.clone()}
                                    </div>
                                    // Core owns the wording (R4/R6) — we render it.
                                    <div class="text-[10px] leading-snug mt-0.5 whitespace-pre-line text-text-tertiary">
                                        {tier.description.clone()}
                                    </div>
                                    <Show when=move || is_confirming.get()>
                                        <div class="text-[10px] mt-1 font-semibold">
                                            "再点一次确认 / Click again to confirm"
                                        </div>
                                    </Show>
                                </button>
                            }
                        }
                    />

                    <div class="px-2.5 pt-1 text-[10px] leading-snug text-text-tertiary border-t border-border">
                        "沙箱命令硬底线（fork bomb / rm -rf / 抹盘）任何档位都无法关闭。"
                        <br />
                        "The sandbox command floor holds under every tier."
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers() -> Vec<TierPreset> {
        vec![
            TierPreset {
                id: "ask".to_string(),
                label: "Ask 请求".to_string(),
                description: "…".to_string(),
            },
            TierPreset {
                id: "auto".to_string(),
                label: "Auto 自动".to_string(),
                description: "…".to_string(),
            },
        ]
    }

    #[test]
    fn pill_label_takes_the_first_word_of_cores_label() {
        assert_eq!(pill_label(&tiers(), "auto"), "Auto");
        assert_eq!(pill_label(&tiers(), "ask"), "Ask");
    }

    #[test]
    fn unknown_tier_falls_back_to_its_id() {
        // Core could ship a fourth tier before this client knows about it; the
        // pill must degrade to the raw id rather than render blank.
        assert_eq!(pill_label(&tiers(), "paranoid"), "paranoid");
        assert_eq!(pill_label(&[], "auto"), "auto");
    }
}
