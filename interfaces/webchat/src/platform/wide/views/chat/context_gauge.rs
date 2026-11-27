use super::state::ChatState;
use crate::i18n::{t_string, use_i18n};
use leptos::prelude::*;

/// Live last-call prompt-cache hit rate chip beside the context vial.
#[component]
#[must_use]
pub fn CacheStat() -> impl IntoView {
    let i18n = use_i18n();
    let chat = expect_context::<ChatState>();
    view! {
        <Show when=move || chat.live_cache_pct.get().is_some()>
            {move || {
                let pct = chat.live_cache_pct.get()?;
                let color = if pct < 50 {
                    "var(--color-warning)"
                } else {
                    "var(--color-text-tertiary)"
                };
                Some(view! {
                    <div
                        class="flex items-center select-none"
                        title=t_string!(i18n, chat.cache_hit_title, pct = pct).to_string()
                    >
                        <span class="text-[10px] tabular-nums" style=format!("color: {color}")>
                            {format!("cache {pct}%")}
                        </span>
                    </div>
                })
            }}
        </Show>
    }
}
