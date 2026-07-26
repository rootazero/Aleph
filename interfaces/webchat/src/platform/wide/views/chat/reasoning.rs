//! Collapsible reasoning panel — renders the model's streamed chain-of-thought.
//!
//! The Gateway already streams `reasoning` events (and `tool_summary` lines)
//! into [`ChatState::reasoning_text`] via [`super::events::subscribe_run_events`],
//! but until now that signal was captured and never shown. This component
//! closes that broken data-flow, porting the collapsible `ReasoningCard` UX from
//! DeepSeek-Reasonix to the web panel:
//!
//! - A pulsing header while the model is actively reasoning (`ChatPhase::Thinking`).
//! - A live tail preview (last few lines) during active reasoning, so the user
//!   sees thinking happen without expanding.
//! - Click-to-expand for the full transcript once it settles.
//!
//! `reasoning_text` is a single active-run signal (reset at each run start and
//! cleared on `clear`), so this panel represents the current/last turn's
//! reasoning — mounted once at the tail of [`super::messages::MessageList`].

use super::state::{ChatPhase, ChatState};
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

/// Number of trailing lines shown as a live preview while reasoning is active.
const PREVIEW_TAIL_LINES: usize = 2;

/// Return the last `n` lines of `text`, joined by newlines. UTF-8 safe — it
/// iterates over `str::lines()` rather than slicing by byte offset.
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Collapsible panel that surfaces [`ChatState::reasoning_text`].
#[component]
#[must_use]
pub(super) fn ReasoningPanel() -> impl IntoView {
    let i18n = use_i18n();
    let chat = expect_context::<ChatState>();
    // User-controlled expand toggle. Defaults collapsed; the active live-tail
    // preview gives streaming feedback without forcing a full expand.
    let expanded = RwSignal::new(false);

    let has_reasoning = move || !chat.reasoning_text.with(String::is_empty);
    // "Active" = the model is still reasoning (no answer text streaming yet).
    let active = move || chat.phase.get() == ChatPhase::Thinking;

    // Trailing-lines preview, UTF-8 safe (line iteration, never byte slicing).
    let preview = move || {
        chat.reasoning_text
            .with(|full| tail_lines(full, PREVIEW_TAIL_LINES))
    };

    let toggle = move |_: web_sys::MouseEvent| expanded.update(|e| *e = !*e);

    view! {
        <Show when=has_reasoning>
            <div class="max-w-5xl mx-auto w-full">
                <div class="rounded-xl glass-inset overflow-hidden">
                  // Header row — expand toggle.
                  <div class="flex items-center">
                    // Header — clickable toggle. Pulse dot animates while active.
                    <button
                        class="flex-1 min-w-0 flex items-center gap-2 px-3 py-2 text-left
                               hover:bg-surface-sunken/40 transition-colors"
                        on:click=toggle
                        title=move || if expanded.get() {
                            t_string!(i18n, chat.reasoning_collapse).to_string()
                        } else {
                            t_string!(i18n, chat.reasoning_expand).to_string()
                        }
                    >
                        <span class=move || {
                            if active() {
                                "inline-block w-2 h-2 rounded-full bg-primary animate-pulse shrink-0"
                            } else {
                                "inline-block w-2 h-2 rounded-full bg-text-tertiary/50 shrink-0"
                            }
                        }></span>
                        <span class="text-xs italic text-text-tertiary flex-1">
                            {move || if active() {
                                t_string!(i18n, chat.reasoning_active).to_string()
                            } else {
                                t_string!(i18n, chat.reasoning).to_string()
                            }}
                        </span>
                        // Chevron rotates when expanded.
                        <span
                            class="text-text-tertiary transition-transform text-[10px] shrink-0"
                            style=move || if expanded.get() { "transform: rotate(90deg)" } else { "" }
                        >
                            "\u{25B6}"
                        </span>
                    </button>
                  </div>

                    // Body — full transcript when expanded; live tail while
                    // actively reasoning; nothing once settled + collapsed.
                    <Show when=move || expanded.get()>
                        <pre class="px-3 pb-3 pt-1 text-xs leading-relaxed text-text-secondary
                                    whitespace-pre-wrap break-words font-mono
                                    max-h-64 overflow-y-auto border-t border-border/60">
                            {move || chat.reasoning_text.get()}
                        </pre>
                    </Show>
                    <Show when=move || !expanded.get() && active()>
                        <div class="px-3 pb-2.5 pt-0.5 text-xs leading-relaxed text-text-tertiary
                                    whitespace-pre-wrap break-words font-mono opacity-70">
                            {preview}
                        </div>
                    </Show>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::tail_lines;

    #[test]
    fn tail_lines_returns_all_when_fewer_than_n() {
        assert_eq!(tail_lines("a\nb", 3), "a\nb");
    }

    #[test]
    fn tail_lines_keeps_only_last_n() {
        assert_eq!(tail_lines("a\nb\nc\nd\ne", 3), "c\nd\ne");
    }

    #[test]
    fn tail_lines_empty_input() {
        assert_eq!(tail_lines("", 3), "");
    }

    #[test]
    fn tail_lines_is_utf8_safe() {
        // Multi-byte chars must not be sliced mid-codepoint.
        assert_eq!(
            tail_lines("第一行\n第二行\n第三行\n第四行", 2),
            "第三行\n第四行"
        );
    }
}
