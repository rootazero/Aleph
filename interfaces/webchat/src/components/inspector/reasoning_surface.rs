//! Reasoning inspector surface — a run's think→act transcript, persistently.
//!
//! Opened by the ↗ affordance on the inline `ReasoningPanel` header. Reads
//! `chat.reasoning_text` (the same signal the inline panel shows) but keeps it
//! open in the side pane instead of the transcript tail, so it does not
//! auto-collapse or compete with the composer for space. `reasoning_text` is a
//! single active-run signal, so `run_id` is carried for identity/future-proofing
//! only — the data shown is always the current/last run's reasoning.

use crate::i18n::{t, use_i18n};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// Per-run reasoning (think→act) transcript.
#[component]
pub fn ReasoningInspector(run_id: String) -> impl IntoView {
    // `reasoning_text` is single-current; run_id is identity only (see module doc).
    let _ = run_id;
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex items-center gap-2 pb-2 border-b border-border/60">
                <span class="text-base shrink-0">"🧠"</span>
                <span class="flex-1 text-sm text-text-primary font-medium">
                    {t!(i18n, chat.reasoning)}
                </span>
            </div>
            {move || {
                let txt = chat.reasoning_text.get();
                if txt.trim().is_empty() {
                    view! {
                        <p class="text-xs text-text-tertiary italic">
                            {t!(i18n, common.inspector_reasoning_empty)}
                        </p>
                    }
                    .into_any()
                } else {
                    view! {
                        <pre class="text-xs whitespace-pre-wrap break-words font-mono
                                    text-text-secondary leading-relaxed">
                            {txt}
                        </pre>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}
