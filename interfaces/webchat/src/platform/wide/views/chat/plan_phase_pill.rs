//! Composer plan pill — enter the read-only planning phase, next to the mode
//! and exec-tier pills.
//!
//! A **toggle**, not a picker, and the difference is not cosmetic: the other
//! two composer dials are preferences the user sets and re-sets, while this one
//! is a position inside one piece of work that the *server* moves the user out
//! of. So:
//!
//! 1. There is no "follow global" rung and no `[policies]` default to fetch —
//!    an unstamped session is building, and nobody can sensibly declare an
//!    install "always planning".
//! 2. The pill mirrors `chat.session_plan_phase`, which the sidebar's
//!    session-list Effect refreshes from the store. That is the whole read
//!    path: an approved plan handoff clears the phase server-side, and this
//!    control learns about it the same way it would learn about any other
//!    session change. It must never re-assert a cached value — see
//!    `shared_ui_logic::state::session_dials_for_send`.
//! 3. Turning it ON is unprivileged (it only ever subtracts). Turning it OFF
//!    here is the *escape hatch*, not the handoff: it abandons planning without
//!    approving anything. The handoff proper is the approval card the agent
//!    raises with `scratchpad { action: "request_build" }`.
//! 4. First-message trap, same as the other two: a brand-new conversation has
//!    no session to patch, so the phase also rides on `ChatApi::send` and the
//!    server stamps it onto the session it creates.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::sessions::set_plan_phase;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;

/// The stored value that means "this session is planning". One spelling,
/// matching core's `PlanPhase::id()`; the absence of any value means building.
const PLANNING: &str = "planning";

#[component]
#[must_use]
pub fn PlanPhasePill() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    let planning = Memo::new(move |_| chat.session_plan_phase.get().is_some_and(|p| p == PLANNING));

    let toggle = move |_| {
        let next = !planning.get_untracked();
        // Optimistic local flip so the banner appears on the same click that
        // caused it. The sidebar Effect is the authority and will correct this
        // on the next session-list refresh — including when the server clears
        // the phase on its own after an approved handoff.
        chat.session_plan_phase
            .set(next.then(|| PLANNING.to_string()));
        // A live session is written through immediately. A conversation with no
        // session key yet needs no bookkeeping: the composer carries the phase
        // on the send itself, and the server stamps it onto the session it
        // creates. Parking it client-side could never have governed the first
        // turn — which is exactly the turn "plan before you touch anything" is
        // about.
        if let Some(session_key) = chat.session_key.get_untracked() {
            spawn_local(async move {
                if let Err(e) = set_plan_phase(&dashboard, &session_key, next).await {
                    web_sys::console::warn_1(&format!("Failed to persist plan phase: {e}").into());
                }
            });
        }
    };

    view! {
        <button
            class=move || {
                let base = "px-2 py-1 rounded-lg text-xs font-medium transition-colors \
                            flex items-center gap-1 flex-shrink-0 border";
                if planning.get() {
                    format!("{base} border-accent text-accent bg-accent/10")
                } else {
                    format!(
                        "{base} border-transparent text-text-tertiary \
                         hover:text-text-primary hover:bg-surface-sunken"
                    )
                }
            }
            title=move || t_string!(i18n, chat.plan_phase.hint)
            aria-pressed=move || if planning.get() { "true" } else { "false" }
            on:click=toggle
        >
            // Clipboard-with-checklist: the plan, not a lock. The phase is a
            // stage of work, and an icon that reads "blocked" would misdescribe
            // what the user just chose.
            <svg xmlns="http://www.w3.org/2000/svg" class="w-3.5 h-3.5"
                 viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 5H7a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-2" />
                <rect x="9" y="3" width="6" height="4" rx="1" />
                <path d="M9 12h6" />
                <path d="M9 16h4" />
            </svg>
            {move || t!(i18n, chat.plan_phase.label)}
        </button>
    }
}

/// The banner shown above the composer while the session is planning.
///
/// Separate from the pill because it answers a different question. The pill
/// says "this is on"; the banner says **what that means and how it ends** —
/// and without the second sentence a user watching tool calls get refused has
/// no way to know the refusals are the feature working.
#[component]
#[must_use]
pub fn PlanPhaseBanner() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let planning = Memo::new(move |_| chat.session_plan_phase.get().is_some_and(|p| p == PLANNING));

    view! {
        <Show when=move || planning.get()>
            <div class="mb-2 px-3 py-2 rounded-lg text-xs bg-accent/10 text-accent
                        border border-accent/30 flex items-start gap-2">
                <span class="font-medium flex-shrink-0">
                    {move || t!(i18n, chat.plan_phase.label)}
                </span>
                <span class="opacity-90">{move || t!(i18n, chat.plan_phase.banner)}</span>
            </div>
        </Show>
    }
}
