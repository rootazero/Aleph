//! Message rendering pieces — hero, list, send-error banner, single bubble.
//!
//! Extracted from `chat/view.rs` so the top-level [`super::view::ChatView`]
//! stays a thin mount + drop-zone shell. All components here are private to
//! the chat module (`pub(super)`).

use super::state::{ChatMessage, ChatPhase, ChatSendErrorCode, ChatState};
use crate::components::markdown::{MarkdownRenderer, StreamingRenderer};
use crate::i18n::*;
use leptos::prelude::*;

/// Welcome hero — shown in the message area while a conversation is empty.
/// A breathing ℵ orb above a shimmering greeting, with a staggered reveal.
#[component]
pub(super) fn ChatHero() -> impl IntoView {
    view! {
        <div class="h-full flex flex-col items-center justify-center px-6 pb-10 text-center select-none">
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
                    "我们从哪里开始？"
                </h2>
            </div>
            <p
                class="aleph-rise mt-3 text-sm text-text-tertiary max-w-sm leading-relaxed"
                style="animation-delay: 0.18s"
            >
                "直接输入消息，或输入 / 调用命令与技能。"
            </p>
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
pub(super) fn MessageList() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();
    let scroll_ref = NodeRef::<leptos::html::Div>::new();
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
            let distance =
                f64::from(el.scroll_height()) - f64::from(el.scroll_top()) - f64::from(el.client_height());
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
        <div class="relative flex-1 min-h-0">
            <div node_ref=scroll_ref class="absolute inset-0 overflow-y-auto" on:scroll=on_scroll>
                <Show
                    when=move || chat.messages.get().is_empty()
                    fallback=move || view! {
                        <div class="max-w-3xl mx-auto px-4 py-6 space-y-4">
                            // Inline send-error banner (G2) — shown when the last
                            // outbound send failed; colour-coded by error code.
                            <SendErrorBanner />
                            <For
                                each=move || chat.messages.get()
                                key=|msg| format!("{}:{}:{}:{}:{}:{}", msg.id, msg.content.len(), msg.is_streaming, msg.is_intermediate, msg.tool_calls.len(), msg.model_info.is_some())
                                children=move |msg| {
                                    view! { <MessageBubble message=msg /> }
                                }
                            />
                            // Thinking indicator
                            <Show when=move || chat.phase.get() == ChatPhase::Thinking>
                                <div class="flex items-center gap-2 text-text-secondary text-sm px-3 py-2">
                                    <span class="inline-block w-2 h-2 rounded-full bg-primary animate-pulse"></span>
                                    {move || t_string!(i18n, chat.thinking).to_string()}
                                </div>
                            </Show>
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
                    class="absolute left-1/2 -translate-x-1/2 bottom-3 z-10
                           px-3 py-1.5 rounded-full text-xs font-medium
                           bg-primary text-white shadow-md hover:bg-primary-hover
                           transition-all flex items-center gap-1"
                    on:click=on_jump
                >
                    <span>"\u{2193}"</span>
                    <span>"New messages"</span>
                </button>
            </Show>
        </div>
    }
}

/// Inline banner for the most recent `ChatSendError`. Empty when none.
#[component]
fn SendErrorBanner() -> impl IntoView {
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
                                    title="Dismiss"
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

/// Single message bubble.
#[component]
fn MessageBubble(message: ChatMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let has_error = message.error.is_some();
    let has_tools = !message.tool_calls.is_empty();

    let bubble_align = if is_user {
        "flex justify-end"
    } else {
        "flex justify-start"
    };
    let bubble_style = if is_user {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-primary text-white"
    } else if has_error {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-danger-subtle text-danger border border-danger/20"
    } else if message.is_intermediate {
        "max-w-[80%] rounded-2xl px-3 py-2 bg-surface-raised/60 text-text-secondary text-sm italic"
    } else {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-surface-raised text-text-primary"
    };

    let tool_calls_view = if has_tools {
        let tools = message.tool_calls.clone();
        Some(view! {
            <div class="mb-2 space-y-1">
                {tools.into_iter().map(|tc| {
                    let status_color = match tc.status.as_str() {
                        "running" => "text-warning",
                        "completed" => "text-success",
                        "failed" => "text-danger",
                        _ => "text-text-secondary",
                    };
                    let status_icon = match tc.status.as_str() {
                        "running" => "\u{27F3}",
                        "completed" => "\u{2713}",
                        "failed" => "\u{2717}",
                        _ => "\u{00B7}",
                    };
                    let duration_text = tc.duration_ms
                        .map(|d| format!(" ({d}ms)"))
                        .unwrap_or_default();
                    view! {
                        <div class="flex items-center gap-2 text-xs font-mono">
                            <span class=status_color>
                                {status_icon}
                            </span>
                            <span class="text-text-secondary">{tc.tool_name.clone()}</span>
                            <span class="text-text-tertiary">{duration_text}</span>
                        </div>
                    }
                }).collect::<Vec<_>>()}
            </div>
        })
    } else {
        None
    };

    let content = message.content.clone();
    let is_streaming = message.is_streaming;
    let error = message.error.clone();
    let model_info = message.model_info.clone();

    let streaming_cursor = if is_streaming {
        Some(view! {
            <span class="inline-block w-1.5 h-4 bg-primary/60 animate-pulse ml-0.5 align-text-bottom"></span>
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
    let copy_text = content.clone();
    let on_copy = move |_: web_sys::MouseEvent| {
        let win = match web_sys::window() {
            Some(w) => w,
            None => return,
        };
        // Modern API — navigator.clipboard.writeText(text)
        let clipboard = win.navigator().clipboard();
        let _promise = clipboard.write_text(&copy_text);
    };
    let on_retry = move |_: web_sys::MouseEvent| {
        chat.request_retry();
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

    view! {
        <div class=format!("{bubble_align} group relative")>
            <div class=bubble_style>
                {tool_calls_view}

                // Message content — Markdown for assistant, plain text for user
                {if is_user {
                    view! {
                        <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
                            {content.clone()}
                        </div>
                    }.into_any()
                } else if is_streaming {
                    view! {
                        <StreamingRenderer content=content.clone() />
                    }.into_any()
                } else {
                    view! {
                        <MarkdownRenderer content=content.clone() />
                    }.into_any()
                }}

                // Streaming cursor
                {streaming_cursor}

                // Error message
                {error_view}

                // Model info (with fallback indicator)
                {model_view}
            </div>
            // Hover action bar — Copy (all bubbles) + Retry (final assistant).
            <div class=action_class>
                <button
                    class="px-1.5 h-6 rounded-md bg-surface-raised border border-border
                           text-[11px] text-text-tertiary hover:text-text-primary hover:bg-surface-sunken
                           shadow-sm transition-colors flex items-center gap-1"
                    title="Copy message"
                    on:click=on_copy
                >
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 24 24"
                         fill="none" stroke="currentColor" stroke-width="2"
                         stroke-linecap="round" stroke-linejoin="round">
                        <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                        <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                    </svg>
                    <span>"Copy"</span>
                </button>
                {if show_retry {
                    Some(view! {
                        <button
                            class="px-1.5 h-6 rounded-md bg-surface-raised border border-border
                                   text-[11px] text-text-tertiary hover:text-text-primary hover:bg-surface-sunken
                                   shadow-sm transition-colors flex items-center gap-1"
                            title="Retry last prompt"
                            on:click=on_retry
                        >
                            <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3" viewBox="0 0 24 24"
                                 fill="none" stroke="currentColor" stroke-width="2"
                                 stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="23 4 23 10 17 10"></polyline>
                                <path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path>
                            </svg>
                            <span>"Retry"</span>
                        </button>
                    })
                } else {
                    None
                }}
            </div>
        </div>
    }
}
