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

use leptos::ev::{pointercancel, pointermove, pointerup};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;

use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState, RUNTIME_AGENTS_CHANGED_TOPIC};
use shared_ui_logic::state::agent_panel::{
    entry_name, quiet_age, sort_entries, state_glyph, AgentPanelState, QuietAge, QuietUnit,
};

use crate::api::runtime_agents::RuntimeAgentsApi;
use crate::components::mode_sidebar::PanelMode;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, I18nCtx};

// The glyph table used to live here as a byte-identical twin of the TUI's,
// with no test spanning the two (判据 §1). Both faces now call
// `shared_ui_logic::state::agent_panel::state_glyph`, imported above.

/// Which of two very different failures the panel is showing.
///
/// D8: this surface used to fold both into one red box while the TUI has
/// kept them apart since R8-6. They are not the same fact and they do not
/// have the same remedy — one is answered by getting operator privilege,
/// the other by fixing the connection — so a user shown the wrong one goes
/// looking in the wrong place (判据 §17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelFailure {
    /// The gateway's operator gate returned a verdict. Not an outage.
    Refused,
    /// Everything else: transport, timeout, a decode this client could not
    /// do, or a server error carrying some other code.
    Unavailable,
}

/// Split a failed `runtime.agents.list` by its JSON-RPC error **code**.
///
/// The same rule and the same constant as the TUI's `agent_panel_data`
/// (`interfaces/tui/src/tui/mod.rs`) — by code, never by matching words in
/// the message (P8). This function cannot see the message at all, so that
/// discipline is structural here rather than something a later edit has to
/// remember.
///
/// `None` is `Unavailable` by construction, and that is the load-bearing
/// half: `RpcFailure` carries no code for anything this client minted
/// itself, and "I could not ask" must never be promoted to "you may not"
/// (判据 §8).
fn classify_failure(code: Option<i32>) -> PanelFailure {
    if code == Some(aleph_protocol::jsonrpc::AUTH_REQUIRED) {
        PanelFailure::Refused
    } else {
        PanelFailure::Unavailable
    }
}

/// Open one session in the terminal view: name the session, and hand back the
/// route that switches the panel to it.
///
/// D3: both ends of this were finished and there was no line between them —
/// the row that tells you an agent is BLOCKED was a plain `<div>`, so the one
/// thing a person wants to do on reading it had no path on this page at all
/// (判据 §7).
///
/// One function rather than two statements at the call site, because "which
/// session" and "which panel" have to agree and a second call site is where
/// they would stop agreeing. Returning the route instead of navigating keeps
/// this testable off the browser: `use_navigate` needs a router context, a
/// signal write does not.
fn open_in_terminal(selection: RwSignal<Option<String>>, session_id: &str) -> &'static str {
    selection.set(Some(session_id.to_string()));
    PanelMode::Terminal.path()
}

/// The chip class for each failure, mirroring the TUI's colour split
/// (`warning` for a verdict, `error` for an outage) in Tailwind terms.
/// The two must never collapse to one string — that collapse IS D8.
const fn failure_class(failure: PanelFailure) -> &'static str {
    match failure {
        PanelFailure::Refused => {
            "bg-amber-50 border border-amber-200 text-amber-800 text-xs p-2 rounded-md"
        }
        PanelFailure::Unavailable => {
            "bg-red-50 border border-red-200 text-red-800 text-xs p-2 rounded-md"
        }
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

#[component]
#[must_use]
pub fn AgentPanel() -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();

    let entries: RwSignal<Vec<RuntimeAgentEntry>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(true);
    // Two signals, not one, because they render differently and a single
    // `Option<String>` cannot say which of the two it is holding without a
    // second look at the message text — the exact re-derivation P8 forbids.
    // At most one is ever `Some`: `refresh` writes the pair together.
    let refused: RwSignal<Option<String>> = RwSignal::new(None);
    let unavailable: RwSignal<Option<String>> = RwSignal::new(None);
    let panel_state = RwSignal::new(AgentPanelState::default());

    let refresh = move || {
        spawn_local(async move {
            loading.set(true);
            match RuntimeAgentsApi::list(&dash).await {
                Ok(resp) => {
                    entries.set(resp.agents);
                    refused.set(None);
                    unavailable.set(None);
                }
                Err(failure) => {
                    // The CODE picks the face; `admin_refusal` still owns the
                    // COPY, unchanged — it replaces the operator-gate
                    // sentence with the localized explanation and passes
                    // every other message through verbatim, so a transport
                    // error still names its own cause.
                    let copy = crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &failure.message,
                        |e| e.to_string(),
                    );
                    match classify_failure(failure.code) {
                        PanelFailure::Refused => {
                            refused.set(Some(copy));
                            unavailable.set(None);
                        }
                        PanelFailure::Unavailable => {
                            unavailable.set(Some(copy));
                            refused.set(None);
                        }
                    }
                }
            }
            loading.set(false);
        });
    };

    Effect::new(move |_| {
        if dash.is_connected.get() {
            refresh();
        } else {
            // Disconnect must fall through to the loading branch below, not
            // the empty-list one — "not connected" and "confirmed zero
            // agents" are different facts (判据 §8/§9; the exact collapse
            // R8-11 forbids on the TUI face). Both failure signals are
            // intentionally left as-is: a disconnect is not what produced
            // them.
            entries.set(Vec::new());
            loading.set(true);
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
    #[allow(clippy::type_complexity)]
    let drag_handles: StoredValue<
        Option<(
            WindowListenerHandle,
            WindowListenerHandle,
            WindowListenerHandle,
        )>,
    > = StoredValue::new(None);

    let stop_drag = move || {
        drag_handles.update_value(|slot| {
            if let Some((move_h, up_h, cancel_h)) = slot.take() {
                move_h.remove();
                up_h.remove();
                cancel_h.remove();
            }
        });
    };

    let on_divider_pointerdown = move |ev: web_sys::PointerEvent| {
        ev.prevent_default();
        // A prior drag that never saw its `pointerup` — the browser claimed
        // a touch gesture mid-drag and fired only `pointercancel`, or the
        // pointer was lost outside the window — must be torn down before
        // starting a new one. Without this, the line below overwrites
        // `drag_handles` and DROPS the previous `WindowListenerHandle`
        // triple; those have no `Drop` impl (`leptos_dom-0.8.8`'s
        // `WindowListenerHandle::remove` only runs on an explicit call), so
        // the orphaned listeners leak for the life of the page and neither
        // `stop_drag` nor `on_cleanup` can ever reach them again (Task 9
        // review finding 3).
        stop_drag();
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
        // On touch, `prevent_default()` on `pointerdown` does not by itself
        // stop the browser from claiming the gesture (e.g. for page scroll)
        // — when it does, `pointerup` never arrives and only `pointercancel`
        // fires. Without this listener the stale `pointermove` handler above
        // keeps resizing the panel on every later pointer move with nothing
        // held down (a "sticky drag"), and the orphaned pair then leaks on
        // the next `pointerdown` per the comment above.
        let cancel_h = window_event_listener(pointercancel, move |_ev: web_sys::PointerEvent| {
            stop_drag();
        });
        drag_handles.update_value(|slot| *slot = Some((move_h, up_h, cancel_h)));
    };

    on_cleanup(stop_drag);

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
                    // Refused first: when both somehow hold a value, a
                    // verdict is the more specific fact and the one with an
                    // action behind it.
                    if let Some(msg) = refused.get() {
                        return view! {
                            <div
                                class=failure_class(PanelFailure::Refused)
                                data-agent-panel-failure="refused"
                                role="alert"
                            >
                                {msg}
                            </div>
                        }.into_any();
                    }
                    if let Some(msg) = unavailable.get() {
                        return view! {
                            <div
                                class=failure_class(PanelFailure::Unavailable)
                                data-agent-panel-failure="unavailable"
                                role="alert"
                            >
                                {msg}
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
                class="h-1.5 cursor-row-resize hover:bg-primary/40 transition-colors shrink-0 touch-none"
                on:pointerdown=on_divider_pointerdown
            />
        </div>
    }
}

#[component]
fn AgentRow(entry: RuntimeAgentEntry) -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();
    // `use_navigate` returns a non-`Copy` closure; the row's click handler is
    // `move` and outlives this body, so it is parked in a `StoredValue` the
    // same way `views/agents/mod.rs` and `views/settings/moa/mod.rs` do.
    let navigate = StoredValue::new(use_navigate());
    let session_id = entry.session_id.clone();
    let state = entry.state;
    let state_label = match state {
        RuntimeAgentState::Blocked => t_string!(i18n, agent_panel.state_blocked).to_string(),
        RuntimeAgentState::Working => t_string!(i18n, agent_panel.state_working).to_string(),
        RuntimeAgentState::Idle => t_string!(i18n, agent_panel.state_idle).to_string(),
        RuntimeAgentState::Unknown => t_string!(i18n, agent_panel.state_unknown).to_string(),
    };

    // A `<button>`, not a `<div>`: this is a control now, and a div with a
    // click handler is unreachable by keyboard and invisible to a screen
    // reader.
    view! {
        <button
            type="button"
            class="w-full flex items-center gap-1.5 text-xs text-left rounded px-1 -mx-1 hover:bg-surface-hover"
            title=entry.cwd.clone()
            on:click=move |_| {
                let route = open_in_terminal(dash.terminal_selection, &session_id);
                navigate.with_value(|nav| nav(route, leptos_router::NavigateOptions::default()));
            }
        >
            <span class=format!(
                "shrink-0 px-1 py-0 rounded text-[10px] leading-4 font-medium {}",
                state_chip_class(state)
            )>
                {format!("{} {}", state_glyph(state), state_label)}
            </span>
            <span class="truncate text-text-primary">{entry_name(&entry)}</span>
            // The quiet age qualifies the state, it does not replace it: a
            // Working agent silent for five minutes still reads Working.
            // Nothing here turns time into a state (spec R2-3).
            {quiet_age(entry.quiet_since, crate::views::chat::timeline::now_millis())
                .map(|age| {
                    view! {
                        <span class="shrink-0 text-text-tertiary">{quiet_text(i18n, age)}</span>
                    }
                })}
        </button>
    }
}

/// The Panel's words for a [`QuietAge`], resolved through this surface's own
/// locale files.
///
/// I2: the previous version rendered `shared_ui_logic`'s composed `"quiet 3m"`
/// directly. That crate has no i18n, so the string was English on a surface
/// where Settings -> General moves everything else, and the crate's own
/// `hardcoded_english_line_ratchet` could not see it because the literal lived
/// in a different crate (判据 §18). The NUMBER and the UNIT come from
/// `shared_ui_logic` — one derivation for both faces — and only the words are
/// local.
///
/// One key per unit rather than one key with a unit token, because the two
/// languages put the number and the unit in different places and a shared
/// template would force one of them into the other's word order.
fn quiet_text(i18n: I18nCtx, age: QuietAge) -> String {
    let n = i64::try_from(age.value).unwrap_or(i64::MAX);
    match age.unit {
        QuietUnit::Seconds => t_string!(i18n, agent_panel.quiet_seconds, n = n).to_string(),
        QuietUnit::Minutes => t_string!(i18n, agent_panel.quiet_minutes, n = n).to_string(),
        QuietUnit::Hours => t_string!(i18n, agent_panel.quiet_hours, n = n).to_string(),
        QuietUnit::Days => t_string!(i18n, agent_panel.quiet_days, n = n).to_string(),
    }
}

// `entry_name` used to live here as a byte-identical twin of the TUI's, and
// this doc claimed "same order and same reasoning as the TUI's `entry_name`" —
// a claim about another crate's file with nothing enforcing it (判据 §1 / §9).
// Both faces now call `shared_ui_logic::state::agent_panel::entry_name`,
// imported above, and reading `entry.program` here again is what
// `no_frontend_derives_its_own_agent_row_name` (alephcore,
// `src/gateway/runtime/tests.rs`) fails on.

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::runtime::RuntimeAgentState as S;
    use leptos::prelude::Owner;

    fn entry(session_id: &str, state: S) -> RuntimeAgentEntry {
        RuntimeAgentEntry {
            session_id: session_id.to_string(),
            label: "claude".to_string(),
            cwd: String::new(),
            agent: None,
            program: None,
            state,
            updated_at: 0,
            quiet_since: None,
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

    // The fallback-chain test moved WITH the function it tests, to
    // `shared_ui_logic::state::agent_panel`'s
    // `a_row_prefers_the_probed_program_then_the_agent_then_the_label`. A copy
    // here would have asserted this face's copy of the chain, which is how two
    // byte-identical copies stayed free to drift while both looked tested. What
    // remains this face's own business is that its markup CALLS the shared
    // derivation — the source-scan half in alephcore's
    // `no_frontend_derives_its_own_agent_row_name`.

    /// D3: the agent panel and the terminal were both fully built and there
    /// was no wire between them — the row that tells you an agent is BLOCKED
    /// was a plain `<div>`, so the one thing a person wants to do on reading
    /// it (go look at that terminal) had no path on this page at all
    /// (判据 §7: two ends complete, no line in the middle).
    ///
    /// Two halves, both necessary:
    ///
    /// * the behaviour — clicking names the session and hands back the route
    ///   that switches the panel, as ONE function, so "which session" and
    ///   "which mode" cannot drift into two call sites that disagree;
    /// * the wiring — a source check that `AgentRow`'s markup actually calls
    ///   it. Without this half, deleting `on:click` leaves the function
    ///   perfectly tested and completely unreachable, which is the exact
    ///   defect this task exists to fix (判据 §4: assert the effect arrived,
    ///   not that a helper is correct).
    #[test]
    fn agent_row_click_selects_the_session_and_switches_mode() {
        let owner = Owner::new();
        owner.set();

        let selection = RwSignal::new(None::<String>);
        let route = open_in_terminal(selection, "sess-7");

        assert_eq!(selection.get_untracked().as_deref(), Some("sess-7"));
        assert_eq!(
            crate::components::mode_sidebar::PanelMode::from_path(route),
            crate::components::mode_sidebar::PanelMode::Terminal,
            "the route this hands back must classify as the terminal panel"
        );

        // Production half of the file only: this test's own source mentions
        // both tokens, and a scan that reads itself certifies nothing.
        //
        // `i18n_census::production_lines`, NOT a `split("#[cfg(test)]")` of my
        // own. This crate has a guard against exactly that hand-rolled cut
        // (`no_guard_in_this_crate_hand_rolls_the_cfg_test_cut`) and it caught
        // the first version of this test: cutting at the first marker walks
        // off the end of whatever it could not see and then reports a clean
        // pass for it — a guard that under-scans silently is 判据 §3, and this
        // crate already paid for it once.
        let production: String =
            crate::i18n_census::production_lines(include_str!("agent_panel.rs"))
                .into_iter()
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n");
        let row = production
            .split("fn AgentRow")
            .nth(1)
            .expect("the AgentRow component must exist");
        assert!(
            row.contains("on:click"),
            "AgentRow must carry a click handler, or the panel and the \
             terminal are two finished ends with no line between them"
        );
        assert!(
            row.contains("open_in_terminal"),
            "AgentRow's click handler must go through `open_in_terminal`, \
             not spell the selection write and the route out a second time"
        );
    }

    /// `Unknown` must never render the same glyph as `Idle` (判据 §8): "I
    /// don't know" and "it's idle" are different facts. The table itself now
    /// lives in `shared_ui_logic` and its distinctness across all four states
    /// is pinned there (`glyphs_are_distinct_and_unknown_is_not_idle`); this
    /// keeps the assertion on the Panel's own face so adopting the shared
    /// table cannot quietly drop the property this surface cared about.
    #[test]
    fn unknown_never_wears_idles_glyph() {
        assert_ne!(state_glyph(S::Unknown), state_glyph(S::Idle));
    }

    /// D8: the Panel used to fold "the operator gate said no" and "the call
    /// did not come back" into one red box, while the TUI has kept them
    /// apart since R8-6 (`widgets/agent_panel.rs`'s test of the same name).
    /// A verdict is not an outage: one is answered by getting operator
    /// privilege, the other by fixing the connection, and a user shown the
    /// wrong one goes looking in the wrong place (判据 §17).
    ///
    /// The split is decided by the JSON-RPC error CODE, never by matching
    /// words in the message (P8) — same code and same rule as the TUI's
    /// `agent_panel_data`. `classify_failure` cannot even see the message,
    /// so that discipline is structural here rather than remembered.
    ///
    /// Reddens if: both faces are given the same class again; if a
    /// locally-minted failure (`code: None` — socket down, decode failed)
    /// is read as a verdict; or if a server error with some other code is.
    #[test]
    fn refused_and_unavailable_render_differently() {
        assert_eq!(
            classify_failure(Some(aleph_protocol::jsonrpc::AUTH_REQUIRED)),
            PanelFailure::Refused
        );
        // A server error that is not the operator gate.
        assert_eq!(classify_failure(Some(-32603)), PanelFailure::Unavailable);
        // Locally minted (no socket, timeout, decode) — `RpcFailure` never
        // carries a code for these, and "I could not ask" is not "you may
        // not" (判据 §8).
        assert_eq!(classify_failure(None), PanelFailure::Unavailable);

        assert_ne!(
            failure_class(PanelFailure::Refused),
            failure_class(PanelFailure::Unavailable),
            "the two failures must not share one rendering"
        );
    }
}
