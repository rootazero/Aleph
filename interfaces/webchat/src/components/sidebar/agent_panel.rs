//! `<AgentPanel />` — the sidebar's live-agents section (Task 9, the Panel
//! face of the same list Task 8b draws in the TUI).
//!
//! This is **not** either of its two neighbours, and the name clash is
//! deliberate to call out (判据 §17: a wrong label costs more than a missing
//! one):
//! - `components/agents_sidebar.rs` is the **configured**-agents sidebar
//!   (create / select / set-default) — Aleph's own agent identities.
//! - `platform/wide/views/runtimes.rs` is the **installer** dashboard for
//!   runtime *capabilities* (node/python), via `api::runtimes`.
//!
//! This panel shows **live agents running under our ptys right now** —
//! `runtime.agents.list` / `runtime.agents.changed`, sampled by
//! `src/gateway/runtime/`. Subscribe idiom copied from
//! `platform/wide/views/teams/workers.rs::WorkersView`. This module never
//! sorts its own input: `shared_ui_logic::state::agent_panel::sort_entries`
//! is the only ordering operation here, called on a local clone — a
//! source-level guard (Task 10) fails the build if this file gains an
//! ordering call of its own (R2, mirrored from the TUI's
//! `widgets/agent_panel.rs`).
//!
//! The divider lives here too, not in `chat_sidebar.rs` — mounting
//! `<AgentPanel />` is the whole integration; the parent only wraps it and
//! the session list in a shared flex column. Cleanup pattern for the
//! `window_event_listener` pair copied from
//! `platform/wide/views/canvas/editor.rs:587-758`: hold the handles, drop
//! them in `on_cleanup` (Leptos 0.8's `window_event_listener` registers no
//! cleanup on its own).

use leptos::ev::{pointermove, pointerup};
use leptos::prelude::*;
use leptos::task::spawn_local;

use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState, RUNTIME_AGENTS_CHANGED_TOPIC};
use shared_ui_logic::state::agent_panel::{sort_entries, AgentPanelState};

use crate::api::runtime_agents::RuntimeAgentsApi;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

/// One glyph set, identical to the TUI's (`widgets/agent_panel.rs`,
/// R8-1/R9-6): `Unknown` gets its own glyph, never `Idle`'s — an
/// unrecognised state must never be misread as "nothing is happening here"
/// (判据 §8).
fn state_glyph(state: RuntimeAgentState) -> &'static str {
    match state {
        RuntimeAgentState::Blocked => "\u{25cf}", // ●
        RuntimeAgentState::Working => "\u{25d0}", // ◐
        RuntimeAgentState::Idle => "\u{25cb}",    // ○
        RuntimeAgentState::Unknown => "?",
    }
}

/// Colour-chip idiom (`teams/workers.rs:182-193`'s `state_chip_class`
/// pattern) rather than ratatui colours — Tailwind utility classes, same
/// four states kept visually distinct.
fn state_chip_class(state: RuntimeAgentState) -> &'static str {
    match state {
        RuntimeAgentState::Blocked => "bg-red-50 text-red-700 border border-red-200",
        RuntimeAgentState::Working => "bg-amber-50 text-amber-700 border border-amber-200",
        RuntimeAgentState::Idle => "bg-green-50 text-green-700 border border-green-200",
        RuntimeAgentState::Unknown => "bg-gray-50 text-gray-700 border border-gray-200",
    }
}

/// The dim manifest-version suffix for one entry, or `None`.
///
/// Same derivation as the TUI's (`interfaces/tui/src/tui/widgets/agent_panel.rs:68-71`,
/// 判据 §9/§12: one derivation, every face). `None` on either half — no
/// recognised agent, or a recognised agent with no bundled manifest version —
/// renders nothing, never a placeholder (判据 §17).
fn manifest_suffix(entry: &RuntimeAgentEntry) -> Option<String> {
    let agent = agent_detect::identify_agent(entry.agent.as_deref()?)?;
    agent_detect::manifest_version(agent)
}

#[component]
#[must_use]
pub fn AgentPanel() -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();

    let entries: RwSignal<Vec<RuntimeAgentEntry>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(true);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let panel_state = RwSignal::new(AgentPanelState::default());

    let refresh = move || {
        spawn_local(async move {
            match RuntimeAgentsApi::list(&dash).await {
                Ok(resp) => {
                    entries.set(resp.agents);
                    error.set(None);
                }
                Err(e) => {
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| e.to_string(),
                    )));
                }
            }
            loading.set(false);
        });
    };

    Effect::new(move |_| {
        if dash.is_connected.get() {
            refresh();
        } else {
            entries.set(Vec::new());
        }
    });

    // Re-subscribe on every (re)connect — mirrors WorkersView. Payload is
    // `{}` (R6-4): never read, always re-fetch on the signal.
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic(RUNTIME_AGENTS_CHANGED_TOPIC).await;
        });
    });

    let sub_id = dash.subscribe_events(move |evt| {
        if evt.topic == RUNTIME_AGENTS_CHANGED_TOPIC {
            refresh();
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));

    // ---- draggable divider -------------------------------------------
    //
    // Ratio is relative to THIS panel's own rendered height, not a
    // cross-component measurement: at drag-start, this panel's current
    // pixel height and its current `split_ratio` together imply the shared
    // flex space's total height (`start_height / start_ratio`), so the whole
    // interaction stays inside this file (R9-15) without a `NodeRef` on
    // `chat_sidebar.rs`'s wrapper. `with_split_ratio` is the only place that
    // clamps (`shared_ui_logic`) — every write here goes through it, so the
    // divider can never leave the panel too small to grab back (判据 §14).
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let drag_handles: StoredValue<Option<(WindowListenerHandle, WindowListenerHandle)>> =
        StoredValue::new(None);

    let stop_drag = move || {
        drag_handles.update_value(|slot| {
            if let Some((move_h, up_h)) = slot.take() {
                move_h.remove();
                up_h.remove();
            }
        });
    };

    let on_divider_pointerdown = move |ev: web_sys::PointerEvent| {
        ev.prevent_default();
        let Some(el) = container_ref.get_untracked() else {
            return;
        };
        let start_height = el.get_bounding_client_rect().height();
        let start_ratio = f64::from(panel_state.get_untracked().split_ratio);
        if start_height <= 0.0 || start_ratio <= 0.0 {
            return;
        }
        let total_space = start_height / start_ratio;
        let start_y = f64::from(ev.client_y());

        let move_h = window_event_listener(pointermove, move |ev: web_sys::PointerEvent| {
            if total_space <= 0.0 {
                return;
            }
            let delta = f64::from(ev.client_y()) - start_y;
            let ratio = ((start_height + delta) / total_space) as f32;
            panel_state.update(|s| *s = s.with_split_ratio(ratio));
        });
        let up_h = window_event_listener(pointerup, move |_ev: web_sys::PointerEvent| {
            stop_drag();
        });
        drag_handles.update_value(|slot| *slot = Some((move_h, up_h)));
    };

    on_cleanup(move || stop_drag());

    view! {
        <div
            node_ref=container_ref
            class="flex flex-col border-b border-border"
            style=move || format!(
                "flex: 0 0 {:.1}%; min-height: 0;",
                panel_state.get().split_ratio * 100.0
            )
        >
            <div class="px-3 pt-2 pb-1">
                <h2 class="text-[11px] font-semibold text-text-tertiary uppercase tracking-wider">
                    {t!(i18n, agent_panel.title)}
                </h2>
            </div>
            <div class="flex-1 overflow-y-auto px-3 pb-1 space-y-1 min-h-0">
                {move || {
                    if let Some(err) = error.get() {
                        return view! {
                            <div class="bg-red-50 border border-red-200 text-red-800 text-xs p-2 rounded-md">
                                {err}
                            </div>
                        }.into_any();
                    }
                    if loading.get() && entries.get().is_empty() {
                        return view! {
                            <div class="text-xs text-text-tertiary">"…"</div>
                        }.into_any();
                    }
                    let mut sorted = entries.get();
                    sort_entries(&mut sorted);
                    if sorted.is_empty() {
                        return view! {
                            <div class="text-xs text-text-tertiary">
                                {t!(i18n, agent_panel.empty)}
                            </div>
                        }.into_any();
                    }
                    view! {
                        <div class="space-y-1">
                            {sorted.into_iter().map(|entry| view! { <AgentRow entry=entry /> }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>
            <div
                class="h-1.5 cursor-row-resize hover:bg-primary/40 transition-colors shrink-0"
                on:pointerdown=on_divider_pointerdown
            />
        </div>
    }
}

#[component]
fn AgentRow(entry: RuntimeAgentEntry) -> impl IntoView {
    let i18n = use_i18n();
    let state = entry.state;
    let state_label = match state {
        RuntimeAgentState::Blocked => t_string!(i18n, agent_panel.state_blocked).to_string(),
        RuntimeAgentState::Working => t_string!(i18n, agent_panel.state_working).to_string(),
        RuntimeAgentState::Idle => t_string!(i18n, agent_panel.state_idle).to_string(),
        RuntimeAgentState::Unknown => t_string!(i18n, agent_panel.state_unknown).to_string(),
    };
    let version_suffix = manifest_suffix(&entry);

    view! {
        <div class="flex items-center gap-1.5 text-xs" title=entry.cwd.clone()>
            <span class=format!(
                "shrink-0 px-1 py-0 rounded text-[10px] leading-4 font-medium {}",
                state_chip_class(state)
            )>
                {format!("{} {}", state_glyph(state), state_label)}
            </span>
            <span class="truncate text-text-primary">{entry.label.clone()}</span>
            {version_suffix.map(|v| view! {
                <span class="text-text-tertiary shrink-0">{v}</span>
            })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::runtime::RuntimeAgentState as S;

    fn entry(session_id: &str, state: S) -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: session_id.to_string(),
            label: "claude".to_string(),
            cwd: String::new(),
            agent: None,
            state,
            updated_at: 0,
        }
    }

    /// Brief's Step-1 test. Weak on its own — proves `ui_logic` sorts, not
    /// that the Panel called it (R9-7) — the real R2 property is enforced by
    /// Task 10's source-scan guard on THIS file: no ordering call of its own
    /// is allowed here, only calling `sort_entries` on a local clone (see the
    /// module doc above — that guard is a raw text scan, so the forbidden
    /// tokens are deliberately not spelled out here).
    #[test]
    fn the_panel_renders_the_order_ui_logic_produced() {
        let mut entries = vec![entry("i", S::Idle), entry("b", S::Blocked)];
        sort_entries(&mut entries);
        assert_eq!(entries[0].state, RuntimeAgentState::Blocked);
    }

    /// `Unknown` must never render the same glyph as `Idle` (判据 §8): "I
    /// don't know" and "it's idle" are different facts.
    #[test]
    fn unknown_never_wears_idles_glyph() {
        assert_ne!(state_glyph(S::Unknown), state_glyph(S::Idle));
    }

    /// A recognised agent with no bundled manifest (`agent: None`) must
    /// render no suffix at all — never a placeholder (判据 §17).
    #[test]
    fn no_agent_label_means_no_manifest_suffix() {
        assert_eq!(manifest_suffix(&entry("s", S::Idle)), None);
    }
}
