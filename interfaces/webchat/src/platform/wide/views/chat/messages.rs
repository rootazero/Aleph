//! Message rendering pieces — hero, list, single bubble.
//!
//! Extracted from `chat/view.rs` so the top-level [`super::view::ChatView`]
//! stays a thin mount + drop-zone shell. All components here are private to
//! the chat module (`pub(super)`).

use super::reasoning::ReasoningPanel;
use super::state::{ChatMessage, ChatPhase, ChatState, QueuedPrompt, HISTORY_PAGE};
use super::timeline::{self, TimelineRow};
use super::PlanArchiveCell;
use crate::components::markdown::TypewriterRenderer;
use crate::components::tool_card::ToolCard;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::layout::WorkspaceState;
use crate::state::user_directory::UserDirectoryState;
use crate::state::viewport::FormFactor;
use leptos::prelude::*;
use shared_ui_logic::state::chat_scroll::{scroll_action, ListCursor, ScrollAction};
use std::collections::HashMap;

/// Context carrier for the per-message attribution-grouping map
/// (message_id → show_header). Newtype so the context lookup is unambiguous.
/// `MessageList` computes it once per messages-change; `MessageBubble` reads it.
#[derive(Clone, Copy)]
struct AttributionMap(Memo<HashMap<String, bool>>);

/// Welcome hero — shown in the message area while a conversation is empty.
/// A breathing ℵ orb above a shimmering greeting, with a staggered reveal.
#[component]
#[must_use]
pub(super) fn ChatHero() -> impl IntoView {
    let i18n = use_i18n();
    let chat = expect_context::<ChatState>();
    // Starter prompts — clicking seeds the composer via `chat.seed_draft` so the
    // user lands on an editable draft instead of a blank box. Mirrors
    // hermes-desktop's `ChatEmptyState` suggestion cards. Small fixed set; each
    // tuple is (emoji, short label, seed prompt).
    let suggestions = [
        (
            "🔍",
            t_string!(i18n, chat.starter_search_label).to_string(),
            t_string!(i18n, chat.starter_search_prompt).to_string(),
        ),
        (
            "📝",
            t_string!(i18n, chat.starter_draft_label).to_string(),
            t_string!(i18n, chat.starter_draft_prompt).to_string(),
        ),
        (
            "💻",
            t_string!(i18n, chat.starter_code_label).to_string(),
            t_string!(i18n, chat.starter_code_prompt).to_string(),
        ),
        (
            "🧠",
            t_string!(i18n, chat.starter_recall_label).to_string(),
            t_string!(i18n, chat.starter_recall_prompt).to_string(),
        ),
    ];
    view! {
        <div class="h-full flex flex-col items-center justify-center px-6 pb-[var(--composer-clearance,150px)] text-center select-none">
            <div class="aleph-rise mb-7" style="animation-delay: 0s">
                <div class="aleph-hero-orb w-16 h-16 rounded-2xl flex items-center justify-center">
                    // Brand mark — Hebrew aleph, same glyph the app icon uses.
                    <svg viewBox="0 0 100 100" class="w-9 h-9" xmlns="http://www.w3.org/2000/svg">
                        <text x="50" y="78"
                              font-size="92"
                              font-family="'Frank Ruhl Libre','David Libre','Times New Roman','Times','Arial Hebrew',serif"
                              text-anchor="middle"
                              fill="white">"\u{05D0}"</text>
                    </svg>
                </div>
            </div>
            <div class="aleph-rise" style="animation-delay: 0.09s">
                <h2 class="aleph-hero-title text-[2rem] leading-tight font-semibold tracking-tight">
                    {t!(i18n, chat.hero_title)}
                </h2>
            </div>
            <p
                class="aleph-rise mt-3 text-sm text-text-tertiary max-w-sm leading-relaxed"
                style="animation-delay: 0.18s"
            >
                {t!(i18n, chat.hero_subtitle)}
            </p>
            // Suggestion chips — staggered in last; seed the composer on click.
            <div
                class="aleph-rise mt-7 flex flex-wrap items-center justify-center gap-2 max-w-lg"
                style="animation-delay: 0.27s"
            >
                {suggestions
                    .into_iter()
                    .map(|(icon, label, seed)| {
                        view! {
                            <button
                                class="flex items-center gap-1.5 px-3 py-1.5 rounded-full text-xs
                                       text-text-secondary glass-inset
                                       hover:text-text-primary hover:bg-surface-raised
                                       transition-colors"
                                on:click=move |_| chat.seed_draft(seed.to_string(), Vec::new())
                            >
                                <span>{icon}</span>
                                <span>{label}</span>
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}

/// Scrollable message list — stick-to-bottom variant.
///
/// Replaces the cycle-1 "always slam to scroll_height" behavior with the
/// pattern from openhuman's `useStickToBottom`: we only auto-scroll when
/// the user is already near the bottom (≤64px). If they've scrolled up to
/// read history, new content lands silently and a "↓ New messages" pill
/// appears so they can opt in.
#[component]
#[must_use]
pub(crate) fn MessageList() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let scroll_ref = NodeRef::<leptos::html::Div>::new();

    // The author label below (`MessageBubble`) is the only thing in this crate
    // that renders a `user_id`, so the fetch that resolves ids to names belongs
    // HERE — at the surface that renders it — not at whichever page happens to
    // remember. `ProjectRoomPage` and `SettingsTab` also call it, but neither
    // is on the path a user takes when they open a room session from the
    // ordinary chat sidebar (a room session legitimately appears there, via
    // `session_visible_to`). On that path `my_user_id` stayed `None`, which
    // makes `author_label`'s self-suppression vacuously false: every message,
    // including the viewer's own, got labelled with a raw `u-…` id.
    // `ensure_loaded` guards on `loading` + "already populated", so a third
    // call site is free. `use_context` (not `expect_context`) for the same
    // reason `MessageBubble` uses it: a storybook/test mount without the
    // directory or the socket must still render.
    if let (Some(dir), Some(dash)) = (
        use_context::<UserDirectoryState>(),
        use_context::<crate::context::DashboardState>(),
    ) {
        dir.ensure_loaded(dash);
    }

    // ---- "Load earlier messages" -------------------------------------------
    //
    // `chat.history` serves the TRAILING `history_limit` rows, so a transcript
    // longer than the window opens without its beginning. Before this control
    // existed the window was a hard-coded 50 with no way to widen it, and the
    // missing rows were reported nowhere — the conversation simply appeared to
    // start in the middle.
    //
    // Contexts are captured HERE, at render time, rather than looked up inside
    // the click handler: `use_context` resolves against the reactive owner, and
    // by the time a click runs, that lookup is no longer guaranteed to be the
    // one this component was mounted under. `use_context` (not `expect_`) for
    // the same reason the directory lookup above uses it — a storybook or test
    // mount without a socket must still render the list.
    let dash_ctx = use_context::<crate::context::DashboardState>();
    let sessions_ctx = use_context::<crate::state::sessions::SessionMap>();
    let workspace_ctx = use_context::<WorkspaceState>();
    let loading_earlier = RwSignal::new(false);
    // Write-only latch: it is what lets "you have reached the beginning" appear
    // only for someone who actually asked for more, instead of under every
    // short conversation that never had anything above it.
    let asked_for_earlier = RwSignal::new(false);
    let on_load_earlier = move |_| {
        if loading_earlier.get_untracked() {
            return;
        }
        let (Some(dash), Some(sessions), Some(key)) =
            (dash_ctx, sessions_ctx, chat.session_key.get_untracked())
        else {
            return;
        };
        let locale = i18n.get_locale_untracked();
        // Widen the window, then re-run the ONE hydration path. Deliberately not
        // a backwards page through the server's `before` cursor: hydration
        // replays assistant runs (appending, and expanding one row into many
        // narration/tool rows) and keys untraced rows by their index in the
        // page, so a prepended second page would both mis-order the transcript
        // and mint duplicate `<For>` keys. See `HISTORY_PAGE`.
        //
        // How far to widen: when the server reported how many rows are above,
        // take ALL of them — one press reaches the beginning, and the label
        // said what that would cost before it was pressed. A fixed page would
        // make a long transcript a staircase of identical presses with no way
        // to see how many remain. Only when the depth is unknown (a core that
        // does not report `total`) does the step fall back to one page.
        let step = chat
            .history_above
            .get_untracked()
            .filter(|n| *n > 0)
            .unwrap_or(HISTORY_PAGE);
        chat.history_limit.update(|n| *n += step);
        loading_earlier.set(true);
        asked_for_earlier.set(true);
        leptos::task::spawn_local(async move {
            // `hydrate_session_history` re-derives `history_has_more` from the
            // page it gets back, so reaching the start needs no read here —
            // which is what keeps this continuation write-only past the await
            // (see `crate::disposed_reads`).
            crate::components::chat_sidebar::hydrate_and_follow(
                dash,
                chat,
                workspace_ctx,
                sessions,
                key,
                locale,
            )
            .await;
            loading_earlier.set(false);
        });
    };

    // Memoized timeline: the flat message vector folded into day-separated
    // render rows. Recomputes only when `messages` changes (not on every
    // reactive read), so the per-day segmentation is paid once per update.
    let rows = Memo::new(move |_| {
        let msgs = chat.messages.get();
        let today = t_string!(i18n, chat.today).to_string();
        let yesterday = t_string!(i18n, chat.yesterday).to_string();
        timeline::derive_timeline(&msgs, &today, &yesterday)
    });

    // Telegram-style attribution grouping computed in one forward pass per
    // messages-change (id → show_header), so each team bubble looks up its
    // header decision in O(1) instead of reverse-walking the whole list.
    // Provided via context for `MessageBubble` to read.
    let attribution = Memo::new(move |_| {
        let pairs: Vec<(String, Option<String>)> = chat
            .messages
            .get()
            .iter()
            .map(|m| (m.id.clone(), m.agent_id.clone()))
            .collect();
        crate::views::chat::agent_identity::attribution_map(&pairs)
    });
    provide_context(AttributionMap(attribution));

    // True iff the viewport bottom is within 64px of scroll_height.
    let stuck_to_bottom = RwSignal::new(true);
    // Pulse incremented whenever new content lands while NOT stuck — drives
    // the "↓ New messages" pill so a user reading history sees it appear.
    let unseen_below = RwSignal::new(false);

    const STICK_THRESHOLD_PX: f64 = 64.0;

    // Scroll event handler updates stuck_to_bottom + clears unseen_below
    // when the user returns to the bottom on their own.
    let on_scroll = move |_ev: web_sys::Event| {
        if let Some(el) = scroll_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            let distance = f64::from(el.scroll_height())
                - f64::from(el.scroll_top())
                - f64::from(el.client_height());
            let near_bottom = distance <= STICK_THRESHOLD_PX;
            // Write only on a real edge. Every programmatic `set_scroll_top`
            // below fires this handler too, and both auto-scroll effects call
            // it at their tick rate — an unconditional `set` would notify the
            // pill's `Show` predicate ~30 times a second throughout a sweep to
            // tell it the same thing each time.
            if stuck_to_bottom.get_untracked() != near_bottom {
                stuck_to_bottom.set(near_bottom);
            }
            if near_bottom && unseen_below.get_untracked() {
                unseen_below.set(false);
            }
        }
    };

    // Reactive auto-scroll. The decision — follow / raise the pill / leave
    // alone — is `shared_ui_logic::state::chat_scroll::scroll_action`; read its
    // module doc for why the inputs are a `sends` counter and a conversation id
    // rather than anything derived from the message vector. This effect only
    // samples the signals and applies the verdict.
    //
    // `SessionMap` via `use_context` (not `expect_context`): a storybook mount
    // without the registry still scrolls, it simply never sees a conversation
    // switch — which for a mount that has only one conversation is correct.
    let sessions = use_context::<crate::state::sessions::SessionMap>();
    let observe = move || ListCursor {
        // `active` read reactively rather than through `SessionMap::active_conv`,
        // which is deliberately untracked for use inside the `'static` event
        // closures. Every input this effect branches on has to be subscribed
        // here, or the decision would be riding on a sibling signal happening to
        // fire on the same update — true today (`activate` rewrites `messages`)
        // and not a property anything guards.
        conv: sessions.and_then(|s| s.active.get()).map(|c| c.0),
        sends: chat.sends.get(),
        rows: chat.messages.with(Vec::len),
    };
    let last_cursor = RwSignal::new(ListCursor::default());
    Effect::new(move |_| {
        let next = observe();
        // `phase` is subscribed so the thinking indicator appearing/disappearing
        // — which changes the list's height without changing a row — still
        // re-pins a follower to the bottom.
        let _phase = chat.phase.get();
        let action = scroll_action(
            last_cursor.get_untracked(),
            next,
            stuck_to_bottom.get_untracked(),
        );
        last_cursor.set(next);
        match action {
            ScrollAction::PinToBottom => {
                stuck_to_bottom.set(true);
                unseen_below.set(false);
                if let Some(el) = scroll_ref.get() {
                    let el: &web_sys::HtmlElement = &el;
                    el.set_scroll_top(el.scroll_height());
                }
            }
            ScrollAction::MarkUnseen => unseen_below.set(true),
            ScrollAction::Leave => {}
        }
    });

    // Follow the typewriter sweep, not just the arrival of rows.
    //
    // The reveal is decoupled from `is_streaming` (see `state::typewriter`): it
    // keeps advancing after the last chunk lands, growing the bubble's height
    // with no further `messages` write to re-trigger the effect above. A reader
    // parked at the bottom therefore watched the tail of every answer slide out
    // from under the viewport. Subscribing to the animation tick pins it —
    // gated on there actually being a live cursor, so an idle transcript takes
    // no 30 fps dependency and a completed bubble stops the moment
    // `TypewriterClock::finish` drops its cursor.
    if let Some(clock) = use_context::<crate::state::typewriter::TypewriterClock>() {
        Effect::new(move |_| {
            clock.tick.track();
            if !clock.is_sweeping() || !stuck_to_bottom.get_untracked() {
                return;
            }
            if let Some(el) = scroll_ref.get() {
                let el: &web_sys::HtmlElement = &el;
                el.set_scroll_top(el.scroll_height());
            }
        });
    }

    // Jump-to-bottom button handler (also re-arms stickiness).
    let on_jump = move |_: web_sys::MouseEvent| {
        if let Some(el) = scroll_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            el.set_scroll_top(el.scroll_height());
            stuck_to_bottom.set(true);
            unseen_below.set(false);
        }
    };

    view! {
        <div class="relative h-full">
            // `overflow-x-hidden` is load-bearing: `overflow-y-auto` alone leaves
            // `overflow-x: visible`, which the CSS spec promotes to `auto` — so any
            // descendant a hair too wide (code block, table, sub-pixel rounding from
            // the centered `max-w-5xl`) grew a permanent horizontal scrollbar across
            // the bottom of the list. Wide children still scroll inside their own
            // `overflow-x:auto` boxes; the list itself never scrolls sideways.
            <div node_ref=scroll_ref class="absolute inset-0 overflow-y-auto overflow-x-hidden chat-scroll-fade" on:scroll=on_scroll>
                <Show
                    when=move || chat.messages.get().is_empty()
                    fallback=move || view! {
                        <div class=move || {
                            let top = if chat.team_id.get().is_some() {
                                // Roster bar floats over the top; clear its
                                // measured height (fallback ~2.75rem pre-observe).
                                "pt-[calc(var(--aleph-team-roster-h,2.75rem)+0.75rem)]".to_string()
                            } else {
                                "pt-6".to_string()
                            };
                            format!(
                                "w-full min-w-0 max-w-5xl mx-auto px-4 {top} \
                                 pb-[calc(var(--composer-clearance,150px)+1rem)] space-y-2"
                            )
                        }>
                            // Top of the transcript: the window's upper edge.
                            // Renders above the first row so "there is more
                            // above this" sits where the missing content would
                            // have been.
                            <Show when=move || chat.history_has_more.get()>
                                <div class="flex justify-center pb-2">
                                    <button
                                        class="px-3 py-1 rounded-full text-xs border border-border
                                               bg-surface-raised text-text-secondary
                                               hover:text-text-primary hover:bg-surface-sunken
                                               disabled:opacity-60 transition-colors"
                                        disabled=move || loading_earlier.get()
                                        on:click=on_load_earlier
                                    >
                                        {move || if loading_earlier.get() {
                                            t_string!(i18n, chat.loading_earlier).to_string()
                                        } else {
                                            // Name the number when the server
                                            // gave one: this press fetches
                                            // exactly that many, and a control
                                            // that says how deep the rest goes
                                            // is the difference between an exit
                                            // and a staircase. Unknown depth
                                            // keeps the unquantified wording,
                                            // which is honest about the one
                                            // page it will actually add.
                                            match chat.history_above.get() {
                                                Some(n) if n > 0 => t_string!(
                                                    i18n,
                                                    chat.load_earlier_n,
                                                    count = n as i64
                                                )
                                                .to_string(),
                                                _ => t_string!(i18n, chat.load_earlier).to_string(),
                                            }
                                        }}
                                    </button>
                                </div>
                            </Show>
                            <Show when=move || asked_for_earlier.get() && !chat.history_has_more.get()>
                                <div class="flex justify-center pb-2 text-[11px] text-text-tertiary">
                                    {move || t_string!(i18n, chat.history_start).to_string()}
                                </div>
                            </Show>
                            <For
                                each=move || rows.get()
                                key=timeline::row_key
                                children=move |row| match row {
                                    TimelineRow::DaySeparator { label, .. } => view! {
                                        <DaySeparator label=label />
                                    }.into_any(),
                                    TimelineRow::Message { id, has_plan_archive, role, clock, .. } => {
                                        if has_plan_archive {
                                            // Sunk plan archive — snapshot lookup is fine: a
                                            // finished/superseded plan capsule never streams.
                                            let lookup_id = id.clone();
                                            let snapshot = chat.messages.with_untracked(|m| {
                                                m.iter().find(|x| x.id == lookup_id).cloned()
                                            });
                                            match snapshot.and_then(|m| m.plan_archive.clone()) {
                                                Some(p) => view! { <PlanArchiveCell plan=p /> }.into_any(),
                                                None => ().into_any(),
                                            }
                                        } else if role == "system" {
                                            // Group-chat notice from the broadcaster
                                            // (storm-guard explanation, member failure).
                                            // Nobody's turn — a centered chip, never a
                                            // bubble attributed to an agent called "system".
                                            // Snapshot lookup: a system notice never streams.
                                            let lookup_id = id.clone();
                                            let snapshot = chat.messages.with_untracked(|m| {
                                                m.iter().find(|x| x.id == lookup_id).cloned()
                                            });
                                            // ChatMessage has no `Default` impl (out of scope
                                            // to add one) — a lookup miss renders nothing
                                            // rather than a synthesized placeholder message.
                                            match snapshot {
                                                Some(m) => view! { <SystemNoticeRow message=m /> }.into_any(),
                                                None => ().into_any(),
                                            }
                                        } else if role == "tool" {
                                            // Trace-less history fallback: a run with no
                                            // replayable trace persists its tool call/result
                                            // as standalone `role="tool"` rows. Render them
                                            // as a compact muted line, not a raw-JSON bubble.
                                            // (Live runs never reach here — their tool calls
                                            // flow through ToolCard / ToolLine.) Snapshot
                                            // lookup: a trace-less history row never streams.
                                            let lookup_id = id.clone();
                                            let snapshot = chat.messages.with_untracked(|m| {
                                                m.iter().find(|x| x.id == lookup_id).cloned()
                                            });
                                            match snapshot {
                                                Some(m) => view! { <ToolFallbackRow message=m /> }.into_any(),
                                                None => ().into_any(),
                                            }
                                        } else {
                                            let lookup_id = id.clone();
                                            let message = Memo::new(move |_| {
                                                chat.messages.with(|m| m.iter().find(|x| x.id == lookup_id).cloned())
                                            });
                                            view! { <MessageBubble message=message clock=clock /> }.into_any()
                                        }
                                    }
                                    TimelineRow::Narration { id, .. } => {
                                        let lookup_id = id.clone();
                                        let message = Memo::new(move |_| {
                                            chat.messages.with(|m| m.iter().find(|x| x.id == lookup_id).cloned())
                                        });
                                        view! { <NarrationRow message=message /> }.into_any()
                                    }
                                    TimelineRow::ToolLine { run_id, tool } => view! {
                                        <div class="px-1">
                                            <ToolCard run_id=run_id tool_id=tool.tool_id.clone() tool_name=tool.tool_name />
                                            // A live run's tool calls render HERE, not through
                                            // `ToolCallsBlock` (that path only rebuilds tool rows
                                            // from a message's `tool_calls`, i.e. history/replay).
                                            // The approval prompt has to hang off this row too, or
                                            // an `Ask`-tier turn shows "waiting for authorization"
                                            // with nothing to authorize it WITH.
                                            <ToolLineApproval tool_id=tool.tool_id />
                                        </div>
                                    }.into_any(),
                                    TimelineRow::ExploreGroup { key, run_id, tools, completed } => view! {
                                        <ExploreGroupRow key_id=key run_id=run_id tools=tools completed=completed />
                                    }.into_any(),
                                }
                            />
                            // Reasoning transcript — collapsible chain-of-thought
                            // for the active/last turn (renders only when present).
                            <ReasoningPanel />
                            // Thinking indicator
                            <Show when=move || chat.phase.get() == ChatPhase::Thinking>
                                <div class="flex items-center gap-2 text-text-secondary text-sm px-3 py-2">
                                    <span class="reading-dots"><span></span><span></span><span></span></span>
                                    {move || t_string!(i18n, chat.thinking).to_string()}
                                </div>
                                // Provider-retry status — replaces minutes of
                                // silent "thinking" during a provider outage.
                                <Show when=move || chat.provider_retry.get().is_some()>
                                    <div class="flex items-center gap-2 text-warning text-xs px-3 py-1" role="status">
                                        <span>"\u{26A0}"</span>
                                        {move || {
                                            chat.provider_retry.get().map(|n| format!(
                                                "{} ({} \u{00B7} {}/{})",
                                                t_string!(i18n, chat.provider_retrying),
                                                n.provider,
                                                n.attempt,
                                                n.max_attempts,
                                            ))
                                        }}
                                    </div>
                                </Show>
                            </Show>
                            // Queued indicator — the run has an id but the
                            // engine has not admitted it yet. A sibling of the
                            // Thinking block above, not a variant of it: the
                            // two phases are mutually exclusive, so exactly one
                            // of the two `<Show>` blocks renders.
                            <Show when=move || matches!(chat.phase.get(), ChatPhase::Queued { .. })>
                                <div class="flex items-center gap-2 text-text-secondary text-sm px-3 py-2">
                                    <span class="reading-dots"><span></span><span></span><span></span></span>
                                    {move || match chat.phase.get() {
                                        ChatPhase::Queued { ahead: 0 } => t_string!(i18n, chat.queued_next).to_string(),
                                        ChatPhase::Queued { ahead } => {
                                            t_string!(i18n, chat.queued_behind, count = ahead as i64).to_string()
                                        }
                                        _ => String::new(),
                                    }}
                                </div>
                            </Show>
                            // The question the agent is parked on (`ask_user`).
                            // Tail of the stream: it is always the newest thing
                            // that happened, and the turn cannot advance until
                            // it is answered.
                            <PendingAskCard />
                            // Pending follow-up ghosts — bottom of the stream,
                            // above the composer; flow into the transcript on
                            // insert. Replaces the old chip strip.
                            <QueuedGhosts />
                        </div>
                    }
                >
                    <ChatHero />
                </Show>
            </div>
            // "↓ New messages" pill — only when user scrolled up AND new
            // content has landed since they last looked at the bottom.
            <Show when=move || unseen_below.get() && !stuck_to_bottom.get()>
                <button
                    class="absolute left-1/2 -translate-x-1/2 bottom-[calc(var(--composer-clearance,150px)+0.5rem)] z-10
                           px-3 py-1.5 rounded-full text-xs font-medium
                           bg-primary text-white shadow-md hover:bg-primary-hover
                           transition-all flex items-center gap-1"
                    on:click=on_jump
                >
                    <span>"\u{2193}"</span>
                    <span>{t!(i18n, chat.new_messages)}</span>
                </button>
            </Show>
        </div>
    }
}

/// The `ask_user` question this conversation is waiting on, if any.
///
/// Its own component so the card appears *reactively*: the question lands mid-
/// turn, long after the transcript around it was rendered. Reading
/// `DashboardState::pending_clarifications` here re-runs the lookup whenever the
/// pending list changes — including when the question is answered, which is what
/// makes the card go away again.
#[component]
fn PendingAskCard() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dashboard = use_context::<crate::context::DashboardState>();

    let pending = Memo::new(move |_| {
        dashboard.and_then(|d| {
            crate::state::notifications::pending_ask_for_session(
                &d.pending_clarifications.get(),
                chat.session_key.get().as_deref(),
            )
            .cloned()
        })
    });

    view! {
        {move || pending.get().map(|ask| view! {
            <crate::components::ask_user_card::AskUserCard ask=ask />
        })}
    }
}

/// Pending follow-up prompts rendered as right-aligned "ghost" bubbles at the
/// tail of the conversation stream. They stay here until inserted: at a turn
/// boundary (Steer) they solidify into real user bubbles, or the user can ✕
/// remove / click-to-edit (pull back into the composer via `seed_draft`) /
/// Esc·⚡ force-insert. Replaces the old above-the-input chip strip so the
/// queue lives in the stream and never fights the sticky Todo panel for the
/// fixed bottom slot.
#[component]
fn QueuedGhosts() -> impl IntoView {
    use crate::views::chat::state::{queue_preview_label, queue_row_key};
    let chat = expect_context::<ChatState>();
    let form_factor = expect_context::<crate::state::viewport::FormFactorState>();
    let i18n = use_i18n();

    let enumerated = move || {
        let items: Vec<(usize, QueuedPrompt)> =
            chat.prompt_queue.get().into_iter().enumerate().collect();
        items
    };

    view! {
        <Show when=move || !chat.prompt_queue.get().is_empty()>
            <div class="space-y-2 pt-1">
                <For
                    each=enumerated
                    key=|(idx, e)| queue_row_key(*idx, e)
                    children=move |(idx, entry)| {
                        let label = queue_preview_label(&entry);
                        view! {
                            <div class="flex justify-end group">
                                <div
                                    class="relative max-w-[80%] px-3.5 py-2 rounded-2xl rounded-br-md text-sm
                                           border border-dashed border-primary/60 bg-primary/10 text-primary/90
                                           cursor-text transition-colors hover:bg-primary/15"
                                    title=move || t_string!(i18n, chat.queued).to_string()
                                    on:click=move |_| {
                                        // Edit: pull the full prompt (text + attachments) back
                                        // into the composer, dropping it from the queue in the
                                        // same step. Take-then-seed (rather than seed-then-remove
                                        // from a captured clone) means the prompt can never be
                                        // dropped from the queue without something receiving it.
                                        if let Some(entry) = chat.take_queued_prompt(idx) {
                                            chat.seed_draft(entry.text, entry.attachments);
                                        }
                                    }
                                >
                                    <span class="absolute -top-2 right-2 text-[9px] px-1.5 rounded-full
                                                 bg-surface-sunken border border-primary/50 text-primary/80">
                                        {(idx + 1).to_string()}
                                    </span>
                                    {label}
                                    <button
                                        class="absolute -top-2 -left-2 w-4 h-4 rounded-full bg-surface-raised
                                               border border-border text-text-tertiary text-[10px] leading-none
                                               flex items-center justify-center hover:text-danger hover:border-danger/50"
                                        title=move || t_string!(i18n, chat.remove).to_string()
                                        on:click=move |ev: web_sys::MouseEvent| {
                                            ev.stop_propagation();
                                            chat.remove_queued_prompt(idx);
                                        }
                                    >
                                        "✕"
                                    </button>
                                </div>
                            </div>
                        }
                    }
                />
                <div class="flex justify-end">
                    <span class="text-[10px] text-text-tertiary pr-1">
                        // `QueuedGhosts` is shared with the phone surface, which
                        // has no ArrowUp binding (and no arrow keys). Advertising
                        // a key that does nothing there is the "advertised but
                        // disabled" trap in reverse — the pointer affordance is
                        // real on both, the keyboard one only on wide.
                        {move || {
                            // `!= Phone`, not `== Wide`: `app.rs` picks the
                            // composer on exactly that predicate, so a tablet
                            // runs the wide composer — and has the binding.
                            if form_factor.form_factor.get() != FormFactor::Phone {
                                t_string!(i18n, chat.queue_hint_keyboard).to_string()
                            } else {
                                t_string!(i18n, chat.queue_hint).to_string()
                            }
                        }}
                    </span>
                </div>
            </div>
        </Show>
    }
}

/// Recover the run id from a message id. Handles both the live
/// `assistant-{run}` id and finalized `intermediate-{run}-{n}` step ids.
/// Returns the id unchanged for user messages.
pub(crate) fn run_id_from_message_id(message_id: &str) -> String {
    if let Some(r) = message_id.strip_prefix("assistant-") {
        return r.to_string();
    }
    if let Some(rest) = message_id.strip_prefix("intermediate-") {
        return match rest.rfind('-') {
            Some(pos) => rest[..pos].to_string(),
            None => rest.to_string(),
        };
    }
    message_id.to_string()
}

/// Calendar-day separator row — a centered pill anchoring the run of messages
/// that follow it to "Today" / "Yesterday" / an absolute date.
#[component]
fn DaySeparator(label: String) -> impl IntoView {
    view! {
        <div class="flex items-center gap-3 py-1.5 select-none">
            <span class="flex-1 h-px bg-gradient-to-r from-transparent to-border/60"></span>
            <span class="px-2.5 py-0.5 rounded-full text-[10px] font-medium uppercase tracking-wider
                         text-text-tertiary glass-inset">
                {label}
            </span>
            <span class="flex-1 h-px bg-gradient-to-l from-transparent to-border/60"></span>
        </div>
    }
}

/// A bubble's tool rows, each with the approval card it is blocked on (if any).
///
/// Its own component because the approval must appear *reactively*: an approval
/// request lands well after the tool row was rendered, so a match computed once
/// while building the bubble would never show up. Reading
/// `DashboardState::pending_approvals` here re-runs the match whenever the
/// pending list changes — including when the approval is resolved (from this
/// card or from the bell), which is what makes the card disappear again.
#[component]
fn ToolCallsBlock(run_id: String, tools: Vec<super::state::ToolCallEntry>) -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dashboard = use_context::<crate::context::DashboardState>();

    let tools_for_match = tools.clone();
    let approvals = Memo::new(move |_| match dashboard {
        Some(d) => super::tool_approvals::match_tool_approvals(
            &tools_for_match,
            chat.session_key.get().as_deref(),
            &d.pending_approvals.get(),
        ),
        None => HashMap::new(),
    });

    view! {
        <div class="mb-2 flex flex-col gap-1">
            {tools.into_iter().map(|tc| {
                let run_id = run_id.clone();
                let tool_id = tc.tool_id.clone();
                view! {
                    <ToolCard
                        run_id=run_id
                        tool_id=tc.tool_id.clone()
                        tool_name=tc.tool_name
                    />
                    // Inline permission prompt for the call this row is waiting
                    // on — resolved through the same RPC the bell uses.
                    {move || approvals.get().get(&tool_id).cloned().map(|a| view! {
                        <crate::components::approval_card::ApprovalCard approval=a />
                    })}
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

/// The inline permission prompt for a LIVE tool row (`TimelineRow::ToolLine`).
///
/// Same pairing rule as [`ToolCallsBlock`] — by harness call id, scoped to this
/// conversation's session — so a card can never appear under a tool call it does
/// not belong to. Renders nothing when this call is not waiting on an approval,
/// which is every call under the `Auto` and `Full` tiers.
#[component]
fn ToolLineApproval(tool_id: String) -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dashboard = use_context::<crate::context::DashboardState>();

    let approval = Memo::new(move |_| {
        let d = dashboard?;
        let session_key = chat.session_key.get()?;
        d.pending_approvals.get().into_iter().find(|p| {
            p.session_key == session_key && p.tool_call_id.as_deref() == Some(tool_id.as_str())
        })
    });

    view! {
        {move || approval.get().map(|a| view! {
            <crate::components::approval_card::ApprovalCard approval=a />
        })}
    }
}

/// Single message bubble. `clock` is a pre-resolved "HH:MM" label (empty for
/// undated/legacy rows) shown in the hover action bar.
///
/// `message` is a per-row `Memo` (minted by the `<For>` children closure,
/// keyed by the row's stable `id` — see `timeline::row_key`) rather than an
/// owned snapshot: the closure that builds this component's view tree runs
/// once per stable key, so anything that needs to keep showing *current*
/// content as the underlying message streams has to read `message`
/// reactively instead of capturing a value at closure-run time. Structural
/// facts that cannot change after a message is created (`role`, `agent_id`,
/// `id`) are still read once — see the "structural, read once" locals below.
#[component]
fn MessageBubble(message: Memo<Option<ChatMessage>>, clock: String) -> impl IntoView {
    let i18n = use_i18n();

    // Early exit if the row's id no longer resolves to a message (should not
    // happen — ids are stamped once — but a lookup miss must render nothing
    // rather than panic). A one-time check, not a reactive closure: the rest
    // of this component's structural shell (team layout, avatar, action bar)
    // is built once, the same way `bubble_align` is — see the module-level
    // note in `MessageBubble`'s doc comment.
    if message.with_untracked(|m| m.is_none()) {
        return ().into_any();
    }

    let is_user = move || message.with(|m| m.as_ref().is_some_and(|m| m.role == "user"));
    let has_error = move || message.with(|m| m.as_ref().is_some_and(|m| m.error.is_some()));
    let has_tools = move || message.with(|m| m.as_ref().is_some_and(|m| !m.tool_calls.is_empty()));
    let is_streaming = move || message.with(|m| m.as_ref().is_some_and(|m| m.is_streaming));
    let is_intermediate = move || message.with(|m| m.as_ref().is_some_and(|m| m.is_intermediate));

    // `bubble_align`/`bubble_class` are reactive closures (not plain values
    // computed once): `bubble_class` depends on `has_error`, which can flip
    // true after the bubble has already mounted (a run failing mid-stream),
    // and both are consumed purely as CSS class *attributes* — Leptos updates
    // those in place without unmounting the element, so recomputing them per
    // token costs a cheap string rebuild, not a remount.
    let bubble_align = move || {
        if is_user() {
            "flex justify-end"
        } else {
            "flex justify-start"
        }
    };
    // `min-w-0` lets a flex child shrink below its content's intrinsic width so
    // wide children (code blocks, tables) scroll internally via `overflow-x:auto`
    // instead of spilling past the right edge.
    //
    // Bubbles are reserved for the two rows that are conversational turns: the
    // user message (a compact right-aligned chip) and the run's standalone
    // *final answer* (a left bubble). A run's intermediate steps flow
    // bubble-less and dense as `NarrationRow` / `ToolLine` / `ExploreGroup`
    // rows instead (the opencode / claude-code transcript look).
    let bubble_class = move || {
        if is_user() {
            "min-w-0 max-w-[80%] rounded-2xl px-3.5 py-2 msg-glass-user".to_string()
        } else if has_error() {
            // Standalone final answer that errored — keep the bubble, full width
            // so long-form prose/markdown reads comfortably.
            "min-w-0 w-full rounded-2xl px-4 py-3 msg-glass-danger text-danger".to_string()
        } else {
            // Standalone final answer — the conversational reply keeps its bubble
            // but spans the full column; 80% crowded long markdown answers.
            "min-w-0 w-full rounded-2xl px-4 py-3 msg-glass text-text-primary".to_string()
        }
    };

    // Structural facts: `id` never changes after a message is created (same
    // invariant `TimelineRow::Message`'s field split relies on), so this is a
    // one-time, untracked read rather than a reactive closure.
    let message_run_id =
        message.with_untracked(|m| m.as_ref().map(|m| run_id_from_message_id(&m.id)));
    let run_for_cost = message_run_id.clone().unwrap_or_default();
    let run_for_halt = message_run_id.clone().unwrap_or_default();

    // Reactive: a message that streams into an assistant final-answer bubble
    // can gain tool calls after this row first mounts (the pre-Task-4 code
    // effectively re-read this on every token via the row's full remount —
    // this closure is what keeps that behavior without the remount).
    let tool_calls_view = move || {
        if !has_tools() {
            return None;
        }
        message.with(|m| {
            m.as_ref().map(|m| {
                let run_id = run_id_from_message_id(&m.id);
                let tools = m.tool_calls.clone();
                view! { <ToolCallsBlock run_id=run_id tools=tools /> }
            })
        })
    };

    let error_view = move || {
        message
            .with(|m| m.as_ref().and_then(|m| m.error.clone()))
            .map(|err| {
                view! {
                    <div class="mt-2 text-xs text-danger/80">{err}</div>
                }
            })
    };

    // Model info indicator (shows fallback when applicable). Reactive: it
    // lands after the bubble mounts (attached once the run resolves).
    let model_view = move || {
        if is_user() {
            return None;
        }
        message
            .with(|m| m.as_ref().and_then(|m| m.model_info.clone()))
            .map(|info| {
                if info.is_fallback {
                    let original = info.original_model.unwrap_or_default();
                    view! {
                        <div class="mt-1.5 text-[10px] leading-tight font-mono flex items-center gap-1">
                            <span style="text-decoration: line-through; opacity: 0.4;">{original}</span>
                            <span class="text-text-tertiary">{"\u{2192}"}</span>
                            <span style="color: #fde047;">{info.model}</span>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="mt-1.5 text-[10px] leading-tight font-mono text-text-tertiary">
                            {info.model}
                        </div>
                    }
                    .into_any()
                }
            })
    };

    // ---- G4: per-bubble hover actions (Copy + Retry) ----
    // Reach for ChatState so the retry button can pulse the composer
    // without prop-drilling a callback through MessageList → MessageBubble.
    let chat = expect_context::<ChatState>();

    // Project-room attribution (P2 Task 6/8): a display-name label above a
    // user bubble, shown only when the turn carries a DIFFERENT author than
    // the viewer — never for the viewer's own messages, and never for
    // assistant/system rows (those don't carry `author_user_id` at all —
    // see `ChatMessage::author_user_id`'s doc).
    //
    // A reactive closure, not a plain value computed once: `UserDirectoryState`
    // populates `my_user_id` / its name map asynchronously (`ensure_loaded`),
    // and a bubble routinely mounts before that resolves — freezing the "is
    // this my own message" check at its pre-fetch (`None`) state would
    // mislabel the viewer's own history on every fresh room visit.
    // `use_context` (not `expect_context`): a storybook/test mount without
    // `UserDirectoryState` provided simply renders no label.
    let author_label = move || {
        if !is_user() {
            return None;
        }
        let author = message.with(|m| m.as_ref().and_then(|m| m.author_user_id.clone()))?;
        let dir = use_context::<UserDirectoryState>()?;
        if dir.my_user_id.get().as_deref() == Some(author.as_str()) {
            return None;
        }
        Some(dir.display_name(&author))
    };

    // Cost + tokens for the run that produced this bubble (`run_complete`'s
    // summary; core does the pricing, we render it). Reactive — the summary
    // lands after the bubble mounts. Nothing renders for user bubbles, for runs
    // still in flight, or for a run core could not price: an absent figure is
    // honest, "$0.00" is not. `≈` whenever `cost_status != complete`, so a
    // partially-priced run is never passed off as exact.
    let cost_view = move || {
        if is_user() {
            return None;
        }
        let cost = chat.run_costs.with(|m| m.get(&run_for_cost).cloned())?;
        let money = cost.cost_label();
        let tokens = cost.tokens_label();
        if money.is_none() && tokens.is_none() {
            return None;
        }
        // Prefix reuse is the one cache number a user can act on: a session
        // that keeps re-creating its prefix is paying 1.25x for history it
        // already sent. Shown cumulatively as well as per-run, because the
        // first run of any session necessarily reads 0%.
        //
        // The hover text is localised, which it had not been: a `title=` whose
        // value arrives through a `let` was invisible to the English census
        // until that census learned to follow one binding hop
        // (`i18n_census::painted_identifiers`). `cache` is the *second* hop —
        // it reaches the screen inside `cost_title`'s own interpolation — and
        // is still outside what that scan can see; it is localised here because
        // a half-translated hover is worse than an untranslated one.
        let cache = match (cost.prefix_reuse(), chat.session_prefix_reuse()) {
            (Some(run), Some(session)) => t_string!(
                i18n,
                chat.cost_cache_fragment,
                read = cost.cache_read_tokens.to_string(),
                created = cost.cache_creation_tokens.to_string(),
                run = format!("{:.0}", run * 100.0),
                session = format!("{:.0}", session * 100.0),
            )
            .to_string(),
            _ => String::new(),
        };
        let status = if cost.is_exact() {
            String::new()
        } else {
            // Core's own `cost_status` token passes through verbatim — it is a
            // wire value, not copy, the same rule `RunHalt::label`'s
            // fall-through applies. Only the client's own "core said nothing"
            // word is ours to translate.
            let raw = cost.status.clone().unwrap_or_else(|| {
                t_string!(i18n, chat.cost_status_unknown).to_string()
            });
            t_string!(i18n, chat.cost_status_fragment, status = raw).to_string()
        };
        let title = t_string!(
            i18n,
            chat.cost_title,
            input = cost.input_tokens.to_string(),
            output = cost.output_tokens.to_string(),
            total = cost.total_tokens.to_string(),
            cache = cache,
            status = status,
        )
        .to_string();
        // Read-only meta line: the full token/cost breakdown it used to
        // open lived in the right pane's inspector, which no longer exists.
        // The `title` hover still carries the exact figures.
        Some(view! {
            <div class="mt-1 text-[10px] leading-tight font-mono text-text-tertiary \
                        flex items-center gap-1.5 tabular-nums"
                 title=title>
                {money}
                {tokens.map(|t| view! { <span class="opacity-70">{t}</span> })}
            </div>
        })
    };

    // Why this run stopped, when it did not stop cleanly. Sits beside
    // `cost_view` because it answers the other half of "what happened here" and
    // shares its lifetime exactly (same frame, same map key, same snapshot).
    // Reactive for the same reason: the terminal summary lands after the bubble
    // mounts. Renders nothing on the clean path — `parse_run_halt` returns
    // `None` for `"completed"`, for a core that never sent the field, and for a
    // failure core declined to characterise.
    let halt_view = move || {
        if is_user() {
            return None;
        }
        let halt = chat.run_halts.with(|m| m.get(&run_for_halt).cloned())?;
        let label = halt.label(i18n.get_locale());
        Some(view! {
            <div class="mt-1 text-[10px] leading-tight font-mono text-warning \
                        flex items-center gap-1 tabular-nums"
                 title=format!("terminate_reason: {}", halt.reason)>
                <span>"\u{26a0}\u{fe0f}"</span>
                <span>{label}</span>
            </div>
        })
    };

    // Team chat: Layout A — avatar disc outside the bubble + agent name above.
    // Only when message.agent_id is Some (team message). Zero regression on the
    // single-agent path (agent_id is None → layout_a is None).
    //
    // Structural, read once: `agent_id` never changes after a message is
    // created, so this whole branch selection is decided once rather than
    // re-derived reactively (unlike `tool_calls_view`/`error_view`/etc. above,
    // which read genuinely-mutable fields).
    //
    // `show_header` is read from the precomputed attribution map (O(1)); the
    // map is folded once per messages-change in `MessageList`. Falls back to
    // showing the header if the context is absent (e.g. storybook mount).
    let layout_a = message.with_untracked(|m| {
        m.as_ref().and_then(|m| {
            m.agent_id.as_ref().map(|aid| {
                let members = chat.team_members.get_untracked();
                let member = members.iter().find(|mm| &mm.agent_id == aid);
                let name = member
                    .map(|mm| mm.name.clone())
                    .unwrap_or_else(|| aid.clone());
                // Avatar glyph: the member's emoji if present and non-empty, else a
                // monogram derived from the name (computed at render below).
                let emoji = member
                    .and_then(|mm| mm.emoji.clone())
                    .filter(|e| !e.is_empty());
                let color = crate::views::chat::agent_identity::agent_color_for_id(aid);

                let show_header = use_context::<AttributionMap>()
                    .is_none_or(|am| am.0.get_untracked().get(&m.id).copied().unwrap_or(true));
                (name, emoji, color, show_header)
            })
        })
    });

    // One-shot click feedback — flip green + checkmark, auto-revert after a beat
    // so the user gets a clear "it worked" signal on an otherwise silent action.
    let copied = RwSignal::new(false);
    let retried = RwSignal::new(false);
    let on_copy = move |_: web_sys::MouseEvent| {
        let win = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        // Read the latest content at click time (untracked — this is an
        // imperative event handler, not a reactive render), so Copy always
        // grabs what's currently on screen even mid-stream.
        let copy_text =
            message.with_untracked(|m| m.as_ref().map(|m| m.content.clone()).unwrap_or_default());
        // Modern API — navigator.clipboard.writeText(text)
        let clipboard = win.navigator().clipboard();
        let _promise = clipboard.write_text(&copy_text);
        copied.set(true);
        set_timeout(
            move || copied.set(false),
            std::time::Duration::from_millis(1500),
        );
    };
    let on_retry = move |_: web_sys::MouseEvent| {
        chat.request_retry();
        retried.set(true);
        set_timeout(
            move || retried.set(false),
            std::time::Duration::from_millis(1500),
        );
    };
    // Retry only makes sense on a finalized assistant turn — streaming /
    // intermediate / error messages are noise. Reactive: `is_streaming`
    // flips false when the run finishes, and the button must appear then.
    let show_retry = move || !is_user() && !is_streaming() && !has_error() && !is_intermediate();
    // Structural, read once: `is_user` never changes after a message is
    // created, so the alignment side of the hover action bar is fixed here.
    let actions_align = if is_user() { "right-2" } else { "left-2" };
    let action_class = format!(
        "absolute -bottom-3 {actions_align} flex items-center gap-1 \
         opacity-0 group-hover:opacity-100 focus-within:opacity-100 \
         transition-opacity"
    );

    // One-shot rise+fade as the bubble mounts. Safe unconditionally now: the
    // row no longer remounts per token (stable `<For>` key, see
    // `timeline::row_key`), so this only ever plays once per bubble.
    let wrapper_class = move || format!("{} group relative aleph-msg-in", bubble_align());

    // Decompose layout_a so values can be used in a single view! without double-move.
    let is_team_msg = layout_a.is_some();
    let team_name = layout_a
        .as_ref()
        .map(|(n, ..)| n.clone())
        .unwrap_or_default();
    let team_emoji = layout_a.as_ref().and_then(|(_, e, _, _)| e.clone());
    let team_color = layout_a
        .as_ref()
        .map(|(_, _, c, _)| *c)
        .unwrap_or("#7c9cff");
    let team_show_header = layout_a.map(|(_, _, _, s)| s).unwrap_or(false);
    // Avatar glyph: agent emoji if present, else a name monogram (first char
    // uppercased). Mirrors `agent_identity`'s emoji → monogram fallback.
    let team_avatar = team_emoji.unwrap_or_else(|| {
        team_name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    });

    view! {
        <div class=wrapper_class>
            {if is_team_msg {
                // Layout A: avatar disc outside bubble + name above (Telegram-style).
                view! {
                    <div class="flex gap-2 items-start w-full min-w-0">
                        // Avatar disc on first message of a run; spacer on repeat.
                        {if team_show_header {
                            view! {
                                <div class="w-7 h-7 rounded-full flex items-center justify-center text-sm shrink-0"
                                     style=format!("background:{}1f;color:{}", team_color, team_color)>
                                    {team_avatar}
                                </div>
                            }.into_any()
                        } else {
                            view! { <div class="w-7 shrink-0"></div> }.into_any()
                        }}
                        // `flex-1` for the same reason the single-agent
                        // wrapper below carries `w-full`: `bubble_class` sizes
                        // the assistant bubble with `w-full`, a PERCENTAGE of
                        // this box, and a flex item defaults to shrink-to-fit —
                        // so without it the percentage resolved against the
                        // bubble's own content width and a short team answer
                        // rendered 72px wide instead of filling the column.
                        <div class="flex flex-col min-w-0 flex-1">
                            {team_show_header.then(|| view! {
                                <div class="text-[11px] font-semibold mb-0.5 ml-0.5"
                                     style=format!("color:{}", team_color)>
                                    {team_name}
                                </div>
                            })}
                            <div class=bubble_class>
                                {tool_calls_view}
                                // Assistant text — the paced renderer is mounted
                                // ONCE and follows the message reactively; the
                                // predecessor rebuilt the component (and its DOM
                                // subtree) on every streamed token.
                                <TypewriterRenderer message=message />
                                {error_view}
                                {model_view}
                                {cost_view}
                                {halt_view}
                            </div>
                        </div>
                    </div>
                }.into_any()
            } else {
                // Original layout for user and single-agent assistant messages.
                // The `flex-col items-end` wrapper only ever gains a visible
                // second child (the author label) on a `is_user` bubble —
                // `author_label()` is unconditionally `None` for assistant
                // rows, so this is a no-op wrapper there.
                //
                // `w-full min-w-0` is LOAD-BEARING, not cosmetic. Every width in
                // `bubble_class` is a PERCENTAGE of this wrapper (`max-w-[80%]`
                // for the user chip, `w-full` for the assistant answer). A flex
                // item defaults to `flex: 0 1 auto`, so without `w-full` this
                // wrapper is shrink-to-fit — its width IS the bubble's own
                // max-content width, and the percentage then resolves against
                // the very box it is supposed to constrain. Measured at an
                // 800px column: the user bubble came out 428px = 80% of its own
                // 535px natural width, wrapping a one-line message into two
                // short lines, and a short assistant answer collapsed to 72px
                // instead of spanning the column. `min-w-0` keeps the escape
                // hatch `bubble_class` documents (wide children scroll
                // internally instead of spilling past the right edge).
                view! {
                    <div class="flex flex-col items-end gap-0.5 w-full min-w-0">
                        {move || author_label().map(|name| view! {
                            <span class="text-[11px] text-text-tertiary mr-1">{name}</span>
                        })}
                        <div class=bubble_class>
                            {tool_calls_view}
                            {if is_user() {
                                // Structural, decided once: user messages never
                                // stream, so a one-time content snapshot is safe.
                                let content = message.with_untracked(|m| {
                                    m.as_ref().map(|m| m.content.clone()).unwrap_or_default()
                                });
                                view! {
                                    <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                                        {content}
                                    </div>
                                }.into_any()
                            } else {
                                // Assistant text — the paced renderer is mounted
                                // ONCE and follows the message reactively (see
                                // the team-layout branch above).
                                view! { <TypewriterRenderer message=message /> }.into_any()
                            }}
                            {error_view}
                            {model_view}
                            {cost_view}
                            {halt_view}
                        </div>
                    </div>
                }.into_any()
            }}
            // Hover action bar — timestamp + Copy (all bubbles) + Retry (final assistant).
            <div class=action_class>
                {(!clock.is_empty()).then(move || view! {
                    <span class="px-1 self-center text-[10px] text-text-tertiary tabular-nums leading-6">
                        {clock}
                    </span>
                })}
                <button
                    class=move || {
                        let base = "px-1.5 h-6 rounded-md border shadow-sm transition-colors \
                                    text-[11px] flex items-center gap-1";
                        if copied.get() {
                            format!("{base} bg-success-subtle border-success text-success")
                        } else {
                            format!(
                                "{base} bg-surface-raised border-border text-text-tertiary \
                                 hover:text-text-primary hover:bg-surface-sunken"
                            )
                        }
                    }
                    title=move || t_string!(i18n, chat.copy_message).to_string()
                    on:click=on_copy
                >
                    {move || if copied.get() {
                        view! {
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 24 24"
                                 fill="none" stroke="currentColor" stroke-width="2.5"
                                 stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="20 6 9 17 4 12"></polyline>
                            </svg>
                            <span>{t!(i18n, chat.copied)}</span>
                        }.into_any()
                    } else {
                        view! {
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 24 24"
                                 fill="none" stroke="currentColor" stroke-width="2"
                                 stroke-linecap="round" stroke-linejoin="round">
                                <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                            </svg>
                            <span>{t!(i18n, chat.copy)}</span>
                        }.into_any()
                    }}
                </button>
                {move || show_retry().then(|| view! {
                        <button
                            class=move || {
                                let base = "px-1.5 h-6 rounded-md border shadow-sm transition-colors \
                                            text-[11px] flex items-center gap-1";
                                if retried.get() {
                                    format!("{base} bg-primary/15 border-primary text-primary")
                                } else {
                                    format!(
                                        "{base} bg-surface-raised border-border text-text-tertiary \
                                         hover:text-text-primary hover:bg-surface-sunken"
                                    )
                                }
                            }
                            title=move || t_string!(i18n, chat.retry_last_prompt).to_string()
                            on:click=on_retry
                        >
                            <svg xmlns="http://www.w3.org/2000/svg"
                                 class=move || if retried.get() {
                                     "w-3 h-3 aleph-spin-once"
                                 } else {
                                     "w-3 h-3"
                                 }
                                 viewBox="0 0 24 24"
                                 fill="none" stroke="currentColor" stroke-width="2"
                                 stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="23 4 23 10 17 10"></polyline>
                                <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
                            </svg>
                            <span>{t!(i18n, chat.retry)}</span>
                        </button>
                    })}
            </div>
        </div>
    }
    .into_any()
}

/// Mid-turn narration — borderless inline process monologue, permanently retained in the conversation stream.
///
/// `message` is a per-row `Memo`, same reasoning as `MessageBubble` — see
/// its doc comment.
#[component]
fn NarrationRow(message: Memo<Option<ChatMessage>>) -> impl IntoView {
    // A lookup miss (should not happen — ids are stamped once) renders
    // nothing rather than panicking. One-time gate, same pattern as
    // `MessageBubble`'s mount-time check.
    if message.with_untracked(|m| m.is_none()) {
        return ().into_any();
    }
    view! {
        <div class="px-1 py-0.5 text-sm text-text-secondary leading-relaxed aleph-step-narration">
            <TypewriterRenderer message=message />
        </div>
    }
    .into_any()
}

/// A group-chat system notice: the broadcaster explaining why the conversation
/// stopped (depth / activation caps) or that a member's run failed. Rendered as
/// a centered, muted chip — the Telegram "X joined the group" register — so it
/// reads as chrome rather than as a participant speaking.
#[component]
fn SystemNoticeRow(message: ChatMessage) -> impl IntoView {
    view! {
        <div class="flex justify-center py-1">
            <span class="max-w-[80%] px-2.5 py-1 rounded-full text-[11px] text-text-tertiary
                         bg-surface-sunken/60 border border-border/40 text-center">
                {message.content}
            </span>
        </div>
    }
}

/// Trace-less history fallback: a run with no replayable trace persists each tool call/result as
/// standalone `role="tool"` rows. Rendered here as a compact grey line (icon + flattened payload),
/// rather than a full-width raw JSON bubble — keeping old conversations clean on re-open. The full payload is
/// visible via `title` hover. Live runs never reach here (their tool calls flow through ToolCard / ToolLine).
#[component]
fn ToolFallbackRow(message: ChatMessage) -> impl IntoView {
    let full = message.content.clone();
    // Collapse newlines/runs of whitespace into one line for the preview.
    // `split_whitespace` is UTF-8 safe, so CJK payloads aren't cut mid-char.
    let preview = full.split_whitespace().collect::<Vec<_>>().join(" ");
    view! {
        <div
            class="flex items-center gap-2 px-1 py-0.5 text-xs text-text-tertiary font-mono"
            title=full
        >
            <span class="shrink-0">"🔧"</span>
            <span class="min-w-0 truncate">{preview}</span>
        </div>
    }
}

/// Explore aggregate block — consecutive read-only tools collapsed into one expandable block (codex Exploring origin).
/// Expand state stored by group key in `ChatState::strip_open`, not as this
/// component's own signal: unlike the stabilized `Message`/`Narration` row
/// keys, `ExploreGroup`'s own key (`timeline::row_key`) still changes on the
/// group's own transitions (a tool starting/finishing, the group completing),
/// remounting this row while it is active — an in-component signal would
/// reset the toggle on every one of those.
#[component]
fn ExploreGroupRow(
    key_id: String,
    run_id: String,
    tools: Vec<crate::views::chat::state::ToolCallEntry>,
    completed: bool,
) -> impl IntoView {
    use crate::components::tool_card::{
        explore_entries, summarize_tools, tool_headline, ExploreOutcome, ToolKind,
    };
    let chat = expect_context::<ChatState>();
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();

    // What the block actually achieved, as opposed to whether it stopped.
    // `completed` only says "nothing is running any more"; a group whose reads
    // failed, or whose outcome frames were dropped and settled to `unknown`,
    // reaches it just the same and used to print an unqualified ✓ over the top.
    //
    // `completed` still gates the pulse, so this refines exactly one branch —
    // the ✓ — and leaves the in-flight rendering byte-identical. It has to:
    // `completed` is the wider condition (it also stays false while the source
    // turn is still streaming, i.e. while more reads may yet join the block),
    // and settling on any verdict there would be the same premature claim one
    // step earlier.
    let outcome = if completed {
        ExploreOutcome::of_all(tools.iter().map(|t| t.status.clone()))
    } else {
        ExploreOutcome::Running
    };

    let default_open = !completed;
    let open = {
        let k = key_id.clone();
        Memo::new(move |_| chat.strip_is_open(&k, default_open))
    };

    // Collapsed header summary: in-flight "Exploring… N items"; completed "Explored N items (Read×3 · Search×1)"
    let n = tools.len();
    let counts = summarize_tools(
        &tools
            .iter()
            .map(|t| (t.tool_id.clone(), t.tool_name.clone()))
            .collect::<Vec<_>>(),
    )
    .into_iter()
    .map(|(k, c)| {
        let label = match k {
            ToolKind::FileRead => t_string!(i18n, tool_card.cat_read).to_string(),
            ToolKind::Search => t_string!(i18n, tool_card.cat_search).to_string(),
            _ => t_string!(i18n, tool_card.cat_tool).to_string(),
        };
        format!("{label}×{c}")
    })
    .collect::<Vec<_>>()
    .join(" · ");
    let header = move || {
        if completed {
            format!(
                "{} {} {} ({})",
                t_string!(i18n, chat.explore_done),
                n,
                t_string!(i18n, chat.explore_items),
                counts.clone()
            )
        } else {
            format!(
                "{} {} {}",
                t_string!(i18n, chat.explore_running),
                n,
                t_string!(i18n, chat.explore_items)
            )
        }
    };

    // Expanded body entries: headline computed live from payload (merge logic is in pure functions).
    let entries = {
        let tools = tools.clone();
        let run = run_id.clone();
        Memo::new(move |_| {
            let items: Vec<(String, String, Option<String>, String)> = tools
                .iter()
                .map(|t| {
                    let kind = ToolKind::from_name(&t.tool_name);
                    let payload = workspace.and_then(|w| w.get_tool_payload(&run, &t.tool_id));
                    (
                        t.tool_id.clone(),
                        t.tool_name.clone(),
                        tool_headline(kind, &payload),
                        t.status.clone(),
                    )
                })
                .collect();
            explore_entries(&items)
        })
    };

    let k_for_toggle = key_id;
    view! {
        <div class="my-0.5">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-1 py-0.5 text-left text-sm
                       text-text-tertiary hover:text-text-secondary"
                on:click=move |_| chat.toggle_strip(&k_for_toggle, default_open)
            >
                {match outcome {
                    ExploreOutcome::Running => view! {
                        <span class="shrink-0 inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
                    }.into_any(),
                    ExploreOutcome::Ok => view! {
                        <span class="text-success shrink-0 text-[11px]">"✓"</span>
                    }.into_any(),
                    ExploreOutcome::Failed => view! {
                        <span class="text-danger shrink-0 text-[11px]">"✗"</span>
                    }.into_any(),
                    // Same muted dash `ToolCard` uses for a settled-unknown row,
                    // and the same tooltip, so one vocabulary covers both places
                    // a dropped outcome frame can surface.
                    ExploreOutcome::Unknown => view! {
                        <span
                            class="text-text-tertiary shrink-0 text-[11px]"
                            title=move || t_string!(i18n, tool_card.status_unknown).to_string()
                        >"–"</span>
                    }.into_any(),
                }}
                <span class="shrink-0">"🔍"</span>
                <span class="flex-1 min-w-0 truncate">{header}</span>
                <span class="shrink-0 text-[10px]">
                    {move || if open.get() { "▾" } else { "▸" }}
                </span>
            </button>
            <Show when=move || open.get()>
                <div class="pl-7 flex flex-col gap-0.5">
                    <For
                        each=move || entries.get()
                        key=|e| e.tool_ids.join(",")
                        children=move |e| {
                            // The name matters: `tool_icon` resolves web_fetch /
                            // skill / memory glyphs from it and only then falls
                            // back to the kind icon. This call site passed `""`,
                            // so those three families rendered the generic glyph.
                            let icon = crate::components::tool_card::tool_icon(&e.tool_name, e.kind);
                            // Read-only summary row: it used to open the tool
                            // detail in the right pane, which no longer exists.
                            // It carries its own outcome because a failed read
                            // inside a collapsed block is otherwise invisible —
                            // the row rendered identically whether the file was
                            // read or the call errored.
                            let (state_class, state_glyph) = match e.outcome {
                                ExploreOutcome::Running => ("text-primary", "·"),
                                ExploreOutcome::Ok => ("text-text-tertiary", ""),
                                ExploreOutcome::Failed => ("text-danger", "✗"),
                                ExploreOutcome::Unknown => ("text-text-tertiary", "–"),
                            };
                            view! {
                                <div class=format!(
                                    "flex items-center gap-2 px-1 py-0.5 text-xs min-w-0 {}",
                                    if e.outcome == ExploreOutcome::Failed {
                                        "text-danger"
                                    } else {
                                        "text-text-tertiary"
                                    }
                                )>
                                    <span class="shrink-0">{icon}</span>
                                    <span class="truncate">{e.label.clone()}</span>
                                    <span class=format!("shrink-0 text-[10px] {state_class}")>
                                        {state_glyph}
                                    </span>
                                </div>
                            }
                        }
                    />
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod run_id_tests {
    use super::run_id_from_message_id;

    #[test]
    fn strips_assistant_and_intermediate_prefixes() {
        assert_eq!(run_id_from_message_id("assistant-r1"), "r1");
        assert_eq!(run_id_from_message_id("intermediate-r1-3"), "r1");
        assert_eq!(run_id_from_message_id("intermediate-run-x-7"), "run-x");
        assert_eq!(run_id_from_message_id("user-0"), "user-0");
    }
}

#[cfg(test)]
mod bubble_width_tests {
    /// Every wrapper between a message row and its bubble must declare a
    /// definite width.
    ///
    /// `bubble_class` sizes both bubbles as a PERCENTAGE of their parent —
    /// `max-w-[80%]` for the user chip, `w-full` for the assistant answer. A
    /// flex item defaults to `flex: 0 1 auto`, i.e. shrink-to-fit, so a wrapper
    /// that declares no width of its own takes the width of its content — and
    /// the percentage then resolves against the very box it is meant to
    /// constrain. Measured in Chrome against this crate's compiled stylesheet
    /// at an 800px column, with the wrappers left indefinite:
    ///
    /// - a one-line user message came back 428px — exactly 80% of its own
    ///   535px natural width — and wrapped into two short lines;
    /// - a short assistant answer came back 72px instead of spanning 800px;
    /// - a short team answer came back 72px instead of 768px.
    ///
    /// None of that is visible to a type checker, nor to a rendering test that
    /// asserts on text: the markup is well-formed and every character is
    /// present, only the line breaks are wrong. Hence a source-level rule.
    ///
    /// Stated as a RULE over "wrappers that enclose `class=bubble_class`",
    /// deliberately not as a list of the two known wrappers: a third layout
    /// added later inherits the check instead of having to be told about it.
    #[test]
    fn every_wrapper_around_a_bubble_declares_a_definite_width() {
        // `production_lines` (not a hand-rolled cut at the first `#[cfg(test)]`)
        // is this crate's one answer to "which lines are production" — it walks
        // gated ITEMS, so it cannot be fooled by the class strings quoted in
        // this module's own doc comments. `no_guard_in_this_crate_hand_rolls_
        // the_cfg_test_cut` enforces that; it caught the first draft of this
        // guard doing exactly that.
        let src = include_str!("messages.rs");
        let production = crate::i18n_census::production_lines(src);

        // Walk the markup tracking the most recent `<div class="flex flex-col
        // …">` opener; when a `class=bubble_class` is reached, that opener is
        // the box its percentage widths resolve against.
        let mut current_wrapper: Option<(usize, String)> = None;
        let mut checked = 0usize;
        for (line_no, line) in production {
            let trimmed = line.trim().to_string();
            if trimmed.starts_with("<div class=\"flex flex-col") {
                current_wrapper = Some((line_no, trimmed.clone()));
            }
            if trimmed.starts_with("<div class=bubble_class") {
                let (wrapper_line, wrapper) = current_wrapper.clone().unwrap_or_else(|| {
                    panic!(
                        "messages.rs:{line_no}: a bubble with no enclosing \
                         flex-column wrapper — this guard can no longer tell \
                         what its percentage widths resolve against"
                    )
                });
                assert!(
                    wrapper.contains("w-full") || wrapper.contains("flex-1"),
                    "messages.rs:{wrapper_line}: this wrapper encloses a \
                     `bubble_class` bubble but declares no definite width, so \
                     `max-w-[80%]` / `w-full` inside it resolve against its own \
                     shrink-to-fit content width. Add `w-full` (or `flex-1`). \
                     Wrapper was: {wrapper}"
                );
                checked += 1;
            }
        }

        // Self-protection: "found nothing to check" and "everything passed"
        // are the same colour without this. Both layouts — single-agent and
        // team — route their bubble through `bubble_class`.
        assert!(
            checked >= 2,
            "expected at least the single-agent and team bubbles to be \
             checked, saw {checked} — the markup this guard scans has moved \
             and it is now reporting green on nothing"
        );
    }
}
