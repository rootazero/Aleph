//! Message rendering pieces — hero, list, send-error banner, single bubble.
//!
//! Extracted from `chat/view.rs` so the top-level [`super::view::ChatView`]
//! stays a thin mount + drop-zone shell. All components here are private to
//! the chat module (`pub(super)`).

use super::reasoning::ReasoningPanel;
use super::state::{ChatMessage, ChatPhase, ChatSendErrorCode, ChatState, QueuedPrompt};
use super::PlanArchiveCell;
use super::timeline::{self, TimelineRow};
use crate::components::markdown::{MarkdownRenderer, TypewriterRenderer};
use crate::components::tool_card::ToolCard;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
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
    // Starter prompts — clicking seeds the composer via `chat.draft_seed` so the
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
                                on:click=move |_| chat.draft_seed.set(Some(seed.to_string()))
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
    let sessions = expect_context::<SessionMap>();
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

    // Reactive auto-scroll — only when the user is already at the bottom.
    Effect::new(move |_| {
        // Subscribe to message/phase changes (read both untracked-style for re-runs)
        let _msgs = chat.messages.get();
        let _phase = chat.phase.get();
        if let Some(el) = scroll_ref.get() {
            if stuck_to_bottom.get_untracked() {
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
            <div node_ref=scroll_ref class="absolute inset-0 overflow-y-auto chat-scroll-fade" on:scroll=on_scroll>
                <Show
                    when=move || chat.messages.get().is_empty()
                    fallback=move || view! {
                        <div class=move || {
                            let top = if chat.team_id.get().is_some() {
                                // Roster bar floats over the top; clear its
                                // measured height (fallback ~2.75rem pre-observe).
                                "pt-[calc(var(--aleph-team-roster-h,2.75rem)+0.75rem)]".to_string()
                            } else if sessions.tab_strip_visible() {
                                // pt-14 = band (~33px) + headroom
                                "pt-14".to_string()
                            } else {
                                "pt-6".to_string()
                            };
                            format!(
                                "max-w-3xl mx-auto px-4 {top} \
                                 pb-[calc(var(--composer-clearance,150px)+1rem)] space-y-2"
                            )
                        }>
                            // Inline send-error banner (G2) — shown when the last
                            // outbound send failed; colour-coded by error code.
                            <SendErrorBanner />
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
                                        } else {
                                            view! { <MessageBubble message=message clock=clock /> }.into_any()
                                        }
                                    }
                                    TimelineRow::StepStrip { steps, completed, .. } => view! {
                                        <StepStrip steps=steps completed=completed />
                                    }
                                    .into_any(),
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

/// Inline banner for the most recent `ChatSendError`. Empty when none.
#[component]
fn SendErrorBanner() -> impl IntoView {
    let i18n = use_i18n();
    let chat = expect_context::<ChatState>();
    view! {
        <Show when=move || chat.send_error.get().is_some()>
            {move || {
                let err = chat.send_error.get();
                err.map(|e| {
                    let is_warning = matches!(e.code, ChatSendErrorCode::PromptReview);
                    let class_str = if is_warning {
                        "px-3 py-2 rounded-lg border text-sm bg-warning-subtle border-warning/30 text-warning"
                    } else {
                        "px-3 py-2 rounded-lg border text-sm bg-danger-subtle border-danger/30 text-danger"
                    };
                    view! {
                        <div class=class_str role="alert">
                            <div class="flex items-start gap-2">
                                <span class="font-mono text-[10px] uppercase tracking-wider opacity-70 shrink-0 pt-0.5">
                                    {format!("{:?}", e.code).to_lowercase()}
                                </span>
                                <span class="flex-1">{e.message}</span>
                                <button
                                    class="opacity-60 hover:opacity-100 shrink-0"
                                    title=move || t_string!(i18n, chat.dismiss).to_string()
                                    on:click=move |_| {
                                        chat.send_error.set(None);
                                        chat.error_message.set(None);
                                    }
                                >
                                    "\u{2715}"
                                </button>
                            </div>
                        </div>
                    }
                })
            }}
        </Show>
    }
}

/// Pending follow-up prompts rendered as right-aligned "ghost" bubbles at the
/// tail of the conversation stream. They stay here until inserted: at a turn
/// boundary (Steer) they solidify into real user bubbles, or the user can ✕
/// remove / click-to-edit (pull back into the composer via `draft_seed`) /
/// Esc·⚡ force-insert. Replaces the old above-the-input chip strip so the
/// queue lives in the stream and never fights the sticky Todo panel for the
/// fixed bottom slot.
#[component]
fn QueuedGhosts() -> impl IntoView {
    use crate::views::chat::state::queue_preview_label;
    let chat = expect_context::<ChatState>();
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
                    key=|(idx, e)| format!("{}:{}", idx, e.text)
                    children=move |(idx, entry)| {
                        let label = queue_preview_label(&entry);
                        let edit_text = entry.text.clone();
                        let edit_attachments = entry.attachments.clone();
                        view! {
                            <div class="flex justify-end group">
                                <div
                                    class="relative max-w-[80%] px-3.5 py-2 rounded-2xl rounded-br-md text-sm
                                           border border-dashed border-primary/60 bg-primary/10 text-primary/90
                                           cursor-text transition-colors hover:bg-primary/15"
                                    title=move || t_string!(i18n, chat.queued).to_string()
                                    on:click=move |_| {
                                        // Edit: pull the full prompt (text + attachments) back
                                        // into the composer, drop it from the queue. The composer's
                                        // attachments are backed by `chat.pending_attachments`;
                                        // `draft_seed` feeds the textarea. Restoring both avoids
                                        // silently dropping a queued prompt's files on edit.
                                        chat.draft_seed.set(Some(edit_text.clone()));
                                        chat.pending_attachments.set(edit_attachments.clone());
                                        chat.remove_queued_prompt(idx);
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
                        {move || t_string!(i18n, chat.queue_hint).to_string()}
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

/// Single message bubble. `clock` is a pre-resolved "HH:MM" label (empty for
/// undated/legacy rows) shown in the hover action bar.
#[component]
fn MessageBubble(
    message: ChatMessage,
    clock: String,
    /// True when this bubble is one of a run's intermediate steps rendered
    /// inside the [`StepStrip`]. Steps flow bubble-less and dense; the user
    /// message and the run's standalone *final answer* keep their bubble.
    #[prop(optional)]
    in_strip: bool,
) -> impl IntoView {
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
    // *final answer* (a left bubble). A run's intermediate steps — folded into
    // the step strip — flow bubble-less and dense (the opencode / claude-code
    // transcript look), so the live streaming echo no longer wears card chrome.
    let bubble_style = if is_user {
        "min-w-0 max-w-[80%] rounded-2xl px-3.5 py-2 msg-glass-user"
    } else if in_strip {
        // Intermediate step inside the run's step strip — no bubble.
        if has_error {
            "min-w-0 w-full px-2 py-1 text-danger border-l-2 border-danger/40"
        } else if message.is_intermediate {
            "min-w-0 w-full px-2 py-0.5 text-text-secondary text-sm"
        } else {
            "min-w-0 w-full px-2 py-1 text-text-primary"
        }
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

    // Tool calls render as ToolCard rows. WorkspaceState (when present)
    // lets a card look up its captured args/result payload; without it
    // (e.g. storybook) cards degrade to header-only.
    let workspace = use_context::<WorkspaceState>();
    let message_run_id = run_id_from_message_id(&message.id);

    // Left-side counterpart to the right-side StepCards: an iteration-tagged
    // bubble still gets a stable dom id + a reactive highlight ring so the
    // right panel can scroll-focus and cross-highlight it. The visible `#N`
    // label was dropped — it added a line of height per step without earning
    // it; focus is now driven entirely from the right StepCards.
    let msg_iteration = message.iteration;
    let focused = {
        let run = message_run_id.clone();
        Memo::new(move |_| match (workspace, msg_iteration) {
            (Some(ws), Some(it)) => ws.is_step_focused(&run, it),
            _ => false,
        })
    };
    let bubble_class_reactive = {
        let base = bubble_class;
        move || {
            if focused.get() {
                format!("{base} ring-2 ring-primary/60")
            } else {
                base.clone()
            }
        }
    };
    let bubble_dom_id: Option<String> = match (msg_iteration, is_user) {
        (Some(it), false) => Some(format!("step-{message_run_id}-{it}")),
        _ => None,
    };

    let tool_calls_view = if has_tools {
        let tools = message.tool_calls.clone();
        let run_for_cards = message_run_id;
        Some(view! {
            <div class="mb-2 flex flex-col gap-1">
                {tools.into_iter().map(|tc| {
                    view! {
                        <ToolCard
                            run_id=run_for_cards.clone()
                            tool_id=tc.tool_id.clone()
                            tool_name=tc.tool_name
                        />
                    }
                }).collect::<Vec<_>>()}
            </div>
        })
    } else {
        None
    };

    let content = message.content.clone();
    // Stable key for the typewriter reveal clock — survives the per-token
    // remount of a streaming bubble so the reveal sweep stays continuous.
    let message_id = message.id.clone();
    let is_streaming = message.is_streaming;
    let error = message.error.clone();
    let model_info = message.model_info.clone();

    let streaming_cursor = if is_streaming {
        Some(view! {
            <span class="inline-block w-[3px] h-4 rounded-full bg-gradient-to-b from-primary to-primary/40 animate-pulse ml-0.5 align-text-bottom"></span>
        })
    } else {
        None
    };

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
                            <div class=bubble_class_reactive id=bubble_dom_id>
                                {tool_calls_view}
                                {if is_streaming {
                                    view! { <TypewriterRenderer content=content message_id=message_id /> }.into_any()
                                } else {
                                    view! { <MarkdownRenderer content=content /> }.into_any()
                                }}
                                {streaming_cursor}
                                {error_view}
                                {model_view}
                            </div>
                        </div>
                    </div>
                }.into_any()
            } else {
                // Original layout for user and single-agent assistant messages.
                view! {
                    <div class=bubble_class_reactive id=bubble_dom_id>
                        {tool_calls_view}
                        {if is_user {
                            view! {
                                <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                                    {content}
                                </div>
                            }.into_any()
                        } else if is_streaming {
                            view! { <TypewriterRenderer content=content message_id=message_id /> }.into_any()
                        } else {
                            view! { <MarkdownRenderer content=content /> }.into_any()
                        }}
                        {streaming_cursor}
                        {error_view}
                        {model_view}
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

/// A run's intermediate steps folded into a bounded, internally-scrolling
/// strip. Running (`completed == false`) → expanded + scrollable, so a long
/// run keeps the chat column short. Done (`completed == true`) → collapsed to a
/// single summary line the user can click to expand.
#[component]
fn StepStrip(steps: Vec<ChatMessage>, completed: bool) -> impl IntoView {
    // Default open while the run streams; collapse once it's done. The
    // expand/collapse choice is keyed by `run_id` on `ChatState`, NOT a
    // strip-local signal: `timeline::row_key` folds in content length, so this
    // row remounts on every streamed token — a local signal would re-open a
    // strip the user just collapsed mid-run. The hoisted override survives the
    // remount (see `ChatState::strip_open`).
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let run_id = steps
        .first()
        .map(|m| run_id_from_message_id(&m.id))
        .unwrap_or_default();
    let open = {
        let run = run_id.clone();
        Memo::new(move |_| chat.strip_is_open(&run, !completed))
    };
    let count = steps.len();
    let word = if count == 1 {
        t_string!(i18n, chat.step).to_string()
    } else {
        t_string!(i18n, chat.steps).to_string()
    };
    let summary = format!("{count} {word}");

    // Stick the inner scroll window to its bottom so a running strip shows the
    // latest step, not the first 220px. The row remounts on every streamed
    // token (row_key folds in content length), so this Effect re-runs and
    // re-pins to the bottom each update while the run is live; once complete it
    // stops remounting, leaving the user free to scroll back through the steps.
    let scroll_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        if open.get() && !completed {
            if let Some(el) = scroll_ref.get() {
                let el: &web_sys::HtmlElement = &el;
                el.set_scroll_top(el.scroll_height());
            }
        }
    });

    view! {
        <div class="my-1">
            <div class="w-full rounded-lg glass-inset">
                <button
                    type="button"
                    class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                           text-[11px] uppercase tracking-wider text-text-tertiary
                           hover:text-text-secondary"
                    on:click=move |_| chat.toggle_strip(&run_id, !completed)
                >
                    <span>{summary}</span>
                    <span class="ml-auto">
                        {move || if open.get() { "▾" } else { "▸" }}
                    </span>
                </button>
                <Show when=move || open.get()>
                    <div node_ref=scroll_ref class="max-h-[220px] overflow-y-auto px-2 pb-2 flex flex-col gap-1">
                        {steps
                            .clone()
                            .into_iter()
                            .map(|m| view! { <MessageBubble message=m clock=String::new() in_strip=true /> })
                            .collect_view()}
                    </div>
                </Show>
            </div>
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
