//! Message rendering pieces — hero, list, single bubble.
//!
//! Extracted from `chat/view.rs` so the top-level [`super::view::ChatView`]
//! stays a thin mount + drop-zone shell. All components here are private to
//! the chat module (`pub(super)`).

use super::reasoning::ReasoningPanel;
use super::state::{ChatMessage, ChatPhase, ChatState, QueuedPrompt};
use super::timeline::{self, TimelineRow};
use super::PlanArchiveCell;
use crate::components::markdown::TypewriterRenderer;
use crate::components::tool_card::ToolCard;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::layout::WorkspaceState;
use crate::state::viewport::FormFactor;
use leptos::prelude::*;
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
        ("🔍", "搜索网络", "帮我搜索今天的科技要闻，并总结 3 个重点"),
        ("📝", "起草文字", "帮我起草一封简短得体的工作邮件"),
        (
            "💻",
            "解释代码",
            "解释这段代码的作用，并指出潜在问题：\n\n```\n\n```",
        ),
        ("🧠", "回忆上下文", "我们上次聊到哪了？帮我回顾一下"),
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
            stuck_to_bottom.set(near_bottom);
            if near_bottom {
                unseen_below.set(false);
            }
        }
    };

    // Reactive auto-scroll — only when the user is already at the bottom, with
    // one exception: the user posting their OWN message re-arms stickiness.
    //
    // Without that exception, scrolling up to re-read something and then
    // sending left the reply off-screen behind the "new messages" pill — the
    // user had asked for it and then had to go hunt for it. Sending is an
    // unambiguous "I'm done reading back" signal (hermes-desktop's
    // `useChatScroll` force-scrolls on the same trigger).
    //
    // Detected by growth of the trailing-user-message count rather than by
    // hooking the send path, so every producer (composer, queued-prompt flush,
    // retry, voice, slash command) is covered by construction.
    let last_user_seq = RwSignal::new(0usize);
    Effect::new(move |_| {
        // Subscribe to message/phase changes (read both untracked-style for re-runs)
        let user_seq = chat
            .messages
            .with(|msgs| msgs.iter().filter(|m| m.role == "user").count());
        let _phase = chat.phase.get();
        let user_just_sent = user_seq > last_user_seq.get_untracked();
        last_user_seq.set(user_seq);
        if user_just_sent {
            stuck_to_bottom.set(true);
            unseen_below.set(false);
        }
        if let Some(el) = scroll_ref.get() {
            if user_just_sent || stuck_to_bottom.get_untracked() {
                let el: &web_sys::HtmlElement = &el;
                el.set_scroll_top(el.scroll_height());
            } else {
                unseen_below.set(true);
            }
        }
    });

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
                            <For
                                each=move || rows.get()
                                key=timeline::row_key
                                children=move |row| match row {
                                    TimelineRow::DaySeparator { label, .. } => view! {
                                        <DaySeparator label=label />
                                    }.into_any(),
                                    TimelineRow::Message { message, clock } => {
                                        if let Some(p) = message.plan_archive.clone() {
                                            view! { <PlanArchiveCell plan=p /> }.into_any()
                                        } else if message.role == "system" {
                                            // Group-chat notice from the broadcaster
                                            // (storm-guard explanation, member failure).
                                            // Nobody's turn — a centered chip, never a
                                            // bubble attributed to an agent called "system".
                                            view! { <SystemNoticeRow message=message /> }.into_any()
                                        } else if message.role == "tool" {
                                            // Trace-less history fallback: a run with no
                                            // replayable trace persists its tool call/result
                                            // as standalone `role="tool"` rows. Render them
                                            // as a compact muted line, not a raw-JSON bubble.
                                            // (Live runs never reach here — their tool calls
                                            // flow through ToolCard / ToolLine.)
                                            view! { <ToolFallbackRow message=message /> }.into_any()
                                        } else {
                                            view! { <MessageBubble message=message clock=clock /> }.into_any()
                                        }
                                    }
                                    TimelineRow::Narration { message } => view! {
                                        <NarrationRow message=message />
                                    }.into_any(),
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
#[component]
fn MessageBubble(message: ChatMessage, clock: String) -> impl IntoView {
    let i18n = use_i18n();
    let is_user = message.role == "user";
    let has_error = message.error.is_some();
    let has_tools = !message.tool_calls.is_empty();

    let bubble_align = if is_user {
        "flex justify-end"
    } else {
        "flex justify-start"
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
    let bubble_style = if is_user {
        "min-w-0 max-w-[80%] rounded-2xl px-3.5 py-2 msg-glass-user"
    } else if has_error {
        // Standalone final answer that errored — keep the bubble, full width
        // so long-form prose/markdown reads comfortably.
        "min-w-0 w-full rounded-2xl px-4 py-3 msg-glass-danger text-danger"
    } else {
        // Standalone final answer — the conversational reply keeps its bubble
        // but spans the full column; 80% crowded long markdown answers.
        "min-w-0 w-full rounded-2xl px-4 py-3 msg-glass text-text-primary"
    };
    let bubble_class = bubble_style.to_string();

    let message_run_id = run_id_from_message_id(&message.id);
    let run_for_cost = message_run_id.clone();

    let tool_calls_view = has_tools.then(|| {
        view! { <ToolCallsBlock run_id=message_run_id tools=message.tool_calls.clone() /> }
    });

    let content = message.content.clone();
    // Stable key for the typewriter reveal clock — survives the per-token
    // remount of a streaming bubble so the reveal sweep stays continuous, while
    // still distinguishing two steps that share the id `assistant-{run}`
    // (see `timeline::reveal_key`).
    let message_id = timeline::reveal_key(&message);
    let is_streaming = message.is_streaming;
    let error = message.error.clone();
    let model_info = message.model_info.clone();

    let error_view = error.map(|err| {
        view! {
            <div class="mt-2 text-xs text-danger/80">{err}</div>
        }
    });

    // Model info indicator (shows fallback when applicable)
    let model_view = if !is_user {
        model_info.map(|info| {
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
    } else {
        None
    };

    // ---- G4: per-bubble hover actions (Copy + Retry) ----
    // Reach for ChatState so the retry button can pulse the composer
    // without prop-drilling a callback through MessageList → MessageBubble.
    let chat = expect_context::<ChatState>();

    // Cost + tokens for the run that produced this bubble (`run_complete`'s
    // summary; core does the pricing, we render it). Reactive — the summary
    // lands after the bubble mounts. Nothing renders for user bubbles, for runs
    // still in flight, or for a run core could not price: an absent figure is
    // honest, "$0.00" is not. `≈` whenever `cost_status != complete`, so a
    // partially-priced run is never passed off as exact.
    let cost_view = (!is_user).then_some({
        move || {
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
            let cache = match (cost.prefix_reuse(), chat.session_prefix_reuse()) {
                (Some(run), Some(session)) => format!(
                    " · cache {} read / {} created · prefix reuse {:.0}% (session {:.0}%)",
                    cost.cache_read_tokens,
                    cost.cache_creation_tokens,
                    run * 100.0,
                    session * 100.0
                ),
                _ => String::new(),
            };
            let title = format!(
                "input {} · output {} · total {} tokens{}{}",
                cost.input_tokens,
                cost.output_tokens,
                cost.total_tokens,
                cache,
                if cost.is_exact() {
                    String::new()
                } else {
                    format!(" · cost {}", cost.status.as_deref().unwrap_or("unknown"))
                }
            );
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
        }
    });

    // Team chat: Layout A — avatar disc outside the bubble + agent name above.
    // Only when message.agent_id is Some (team message). Zero regression on the
    // single-agent path (agent_id is None → layout_a is None).
    //
    // `show_header` is read from the precomputed attribution map (O(1)); the
    // map is folded once per messages-change in `MessageList`. Falls back to
    // showing the header if the context is absent (e.g. storybook mount).
    let layout_a = message.agent_id.as_ref().map(|aid| {
        let members = chat.team_members.get_untracked();
        let member = members.iter().find(|m| &m.agent_id == aid);
        let name = member
            .map(|m| m.name.clone())
            .unwrap_or_else(|| aid.clone());
        // Avatar glyph: the member's emoji if present and non-empty, else a
        // monogram derived from the name (computed at render below).
        let emoji = member
            .and_then(|m| m.emoji.clone())
            .filter(|e| !e.is_empty());
        let color = crate::views::chat::agent_identity::agent_color_for_id(aid);

        let show_header = use_context::<AttributionMap>().is_none_or(|m| {
            m.0.get_untracked()
                .get(&message.id)
                .copied()
                .unwrap_or(true)
        });
        (name, emoji, color, show_header)
    });

    let copy_text = content.clone();
    // One-shot click feedback — flip green + checkmark, auto-revert after a beat
    // so the user gets a clear "it worked" signal on an otherwise silent action.
    let copied = RwSignal::new(false);
    let retried = RwSignal::new(false);
    let on_copy = move |_: web_sys::MouseEvent| {
        let win = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
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
    // intermediate / error messages are noise.
    let show_retry = !is_user && !is_streaming && !has_error && !message.is_intermediate;
    let actions_align = if is_user { "right-2" } else { "left-2" };
    let action_class = format!(
        "absolute -bottom-3 {actions_align} flex items-center gap-1 \
         opacity-0 group-hover:opacity-100 focus-within:opacity-100 \
         transition-opacity"
    );

    // One-shot rise+fade as the bubble mounts. Gated to non-streaming: the
    // keyed <For> recreates a streaming bubble on every token, so applying
    // the entrance there would replay it per chunk. User + finalized
    // assistant bubbles mount once, so it plays exactly once.
    let wrapper_class = if is_streaming {
        format!("{bubble_align} group relative")
    } else {
        format!("{bubble_align} group relative aleph-msg-in")
    };

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
                    <div class="flex gap-2 items-start">
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
                        <div class="flex flex-col min-w-0">
                            {team_show_header.then(|| view! {
                                <div class="text-[11px] font-semibold mb-0.5 ml-0.5"
                                     style=format!("color:{}", team_color)>
                                    {team_name}
                                </div>
                            })}
                            <div class=bubble_class>
                                {tool_calls_view}
                                // Assistant text — always the paced renderer; it
                                // keeps sweeping past stream completion and falls
                                // back to full Markdown for history/finished text.
                                <TypewriterRenderer content=content message_id=message_id is_streaming=is_streaming />
                                {error_view}
                                {model_view}
                                {cost_view}
                            </div>
                        </div>
                    </div>
                }.into_any()
            } else {
                // Original layout for user and single-agent assistant messages.
                view! {
                    <div class=bubble_class>
                        {tool_calls_view}
                        {if is_user {
                            view! {
                                <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                                    {content}
                                </div>
                            }.into_any()
                        } else {
                            // Assistant text — always the paced renderer; it keeps
                            // sweeping past stream completion and falls back to
                            // full Markdown for history/finished text.
                            view! { <TypewriterRenderer content=content message_id=message_id is_streaming=is_streaming /> }.into_any()
                        }}
                        {error_view}
                        {model_view}
                        {cost_view}
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
                {if show_retry {
                    Some(view! {
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
                    })
                } else {
                    None
                }}
            </div>
        </div>
    }
}

/// Mid-turn narration — borderless inline process monologue, permanently retained in the conversation stream.
#[component]
fn NarrationRow(message: ChatMessage) -> impl IntoView {
    let content = message.content.clone();
    // Per-step reveal identity, NOT the bare id — consecutive steps of one run
    // share the id `assistant-{run}` (see `timeline::reveal_key`).
    let message_id = timeline::reveal_key(&message);
    let is_streaming = message.is_streaming;
    view! {
        <div class="px-1 py-0.5 text-sm text-text-secondary leading-relaxed aleph-step-narration">
            <TypewriterRenderer content=content message_id=message_id is_streaming=is_streaming />
        </div>
    }
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
/// Expand state stored by group key in `ChatState::strip_open` (survives per-token remount).
#[component]
fn ExploreGroupRow(
    key_id: String,
    run_id: String,
    tools: Vec<crate::views::chat::state::ToolCallEntry>,
    completed: bool,
) -> impl IntoView {
    use crate::components::tool_card::{explore_entries, summarize_tools, tool_headline, ToolKind};
    let chat = expect_context::<ChatState>();
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();

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
            let items: Vec<(String, String, Option<String>)> = tools
                .iter()
                .map(|t| {
                    let kind = ToolKind::from_name(&t.tool_name);
                    let payload = workspace.and_then(|w| w.get_tool_payload(&run, &t.tool_id));
                    (
                        t.tool_id.clone(),
                        t.tool_name.clone(),
                        tool_headline(kind, &payload),
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
                {if completed {
                    view! { <span class="text-success shrink-0 text-[11px]">"✓"</span> }.into_any()
                } else {
                    view! { <span class="shrink-0 inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span> }.into_any()
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
                            let icon = crate::components::tool_card::tool_icon("", e.kind);
                            // Read-only summary row: it used to open the tool
                            // detail in the right pane, which no longer exists.
                            view! {
                                <div class="flex items-center gap-2 px-1 py-0.5 text-xs
                                            text-text-tertiary min-w-0">
                                    <span class="shrink-0">{icon}</span>
                                    <span class="truncate">{e.label.clone()}</span>
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
