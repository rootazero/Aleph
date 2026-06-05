//! Queued-prompt preview strip rendered above the textarea.
//!
//! When the user lines up follow-up prompts while a run is active, each one
//! shows as a numbered chip with its own ✕. Drained automatically when the
//! turn settles (see `ChatState::dequeue_prompt_front` +
//! `shared_ui_logic::state::should_auto_drain_on_settle`); the ✕ lets the
//! user drop one before it fires.

use leptos::prelude::*;

use crate::i18n::*;
use crate::views::chat::state::QueuedPrompt;

/// One-line label for a queued chip: the trimmed prompt text, or an
/// attachment-count fallback when the prompt is attachments-only.
///
/// Pure — unit-tested below; the chip renders whatever this returns.
pub(super) fn queue_label(entry: &QueuedPrompt) -> String {
    const MAX: usize = 64;
    let text = entry.text.trim();
    if !text.is_empty() {
        // UTF-8-safe truncation (P7) — never slice mid-codepoint.
        let truncated: String = text.chars().take(MAX).collect();
        if truncated.chars().count() < text.chars().count() {
            format!("{truncated}…")
        } else {
            truncated
        }
    } else {
        let n = entry.attachments.len();
        format!("📎 {n}")
    }
}

/// Horizontal chip strip, one chip per queued prompt. Mounted by the
/// composer just under the attachment preview bar.
#[component]
pub(super) fn QueuedPromptBar(queue: RwSignal<Vec<QueuedPrompt>>) -> impl IntoView {
    let i18n = use_i18n();
    let on_remove = move |idx: usize| {
        queue.update(|list| {
            if idx < list.len() {
                list.remove(idx);
            }
        });
    };

    // Hoist the enumerated collection out of the `view!` macro: a turbofish
    // (`::<Vec<_>>`) inside a `view!` attribute trips the rstml tag parser,
    // which reads the `<`/`>` as HTML tags. Keeping the angle brackets in plain
    // Rust here avoids that.
    let enumerated = move || {
        let items: Vec<(usize, QueuedPrompt)> = queue.get().into_iter().enumerate().collect();
        items
    };

    view! {
        <Show when=move || !queue.get().is_empty()>
            <div class="flex flex-wrap items-center gap-2 mb-2">
                <span class="text-xs text-text-tertiary pl-1">
                    {move || t_string!(i18n, chat.queued).to_string()}
                    " · "
                    {move || queue.get().len().to_string()}
                </span>
                <For
                    each=enumerated
                    key=|(idx, e)| format!("{}:{}", idx, e.text)
                    children=move |(idx, entry)| {
                        let label = queue_label(&entry);
                        view! {
                            <div class="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg
                                        bg-surface-raised border border-border text-xs text-text-secondary">
                                <span class="text-text-tertiary tabular-nums">{(idx + 1).to_string()}</span>
                                <span class="max-w-[200px] truncate">{label}</span>
                                <button
                                    class="ml-0.5 p-0.5 rounded hover:bg-danger/10 hover:text-danger transition-colors"
                                    title=move || t_string!(i18n, chat.remove).to_string()
                                    on:click=move |_| on_remove(idx)
                                >
                                    <svg xmlns="http://www.w3.org/2000/svg" class="w-3 h-3"
                                         viewBox="0 0 20 20" fill="currentColor">
                                        <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
                                    </svg>
                                </button>
                            </div>
                        }
                    }
                />
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::chat::state::PendingAttachment;

    fn prompt(text: &str, attachments: usize) -> QueuedPrompt {
        QueuedPrompt {
            text: text.to_string(),
            attachments: (0..attachments)
                .map(|i| PendingAttachment {
                    name: format!("f{i}"),
                    mime_type: "text/plain".into(),
                    data_base64: String::new(),
                    size: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn label_uses_trimmed_text() {
        assert_eq!(queue_label(&prompt("  hello  ", 0)), "hello");
    }

    #[test]
    fn label_truncates_long_text_on_codepoint_boundary() {
        let long = "a".repeat(100);
        let out = queue_label(&prompt(&long, 0));
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 65); // 64 chars + ellipsis
    }

    #[test]
    fn label_falls_back_to_attachment_count() {
        assert_eq!(queue_label(&prompt("   ", 2)), "📎 2");
    }
}
