use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::components::extensions::labels::category_label;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::extensions::model::CATEGORIES;
use crate::views::extensions::StoreState;

/// Vertical category navigation for the Aleph Hub left column.
///
/// A "Back to Chat" exit guide is pinned at the very top (the Hub's only way
/// out of its full-screen takeover), followed by the two "global" entries
/// (Featured / All) and the functional-category facets below dividers. Each
/// facet drives `store.category` — identical behavior to the old horizontal
/// CategoryChips, just relocated to the left column to declutter the main area.
#[component]
#[must_use]
pub fn CategoryNav() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let navigate = use_navigate();

    let item = move |value: &'static str, label: String, emoji: &'static str| {
        let active = move || store.category.get() == value;
        view! {
            <button
                class=move || {
                    let base = "w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm";
                    if active() {
                        format!("{base} nav-tile-active")
                    } else {
                        format!("{base} nav-tile")
                    }
                }
                on:click=move |_| store.category.set(value.to_string())
            >
                <span class="flex-shrink-0 w-5 text-center">{emoji}</span>
                <span class="flex-1 text-left truncate">{label}</span>
            </button>
        }
    };

    view! {
        <nav class="flex flex-col h-full overflow-y-auto px-2 py-3 gap-0.5">
            // Exit affordance: pinned above the facets and styled with an
            // always-on primary tint + border so it reads as the way out of
            // the Hub takeover (a guide), not a selectable category tile.
            <button
                class="w-full flex items-center px-3 py-2 rounded-lg text-sm font-medium
                       text-primary bg-primary/10 border border-primary/40
                       hover:bg-primary/20 hover:border-primary/60 transition-colors"
                on:click={
                    let navigate = navigate.clone();
                    move |_| {
                        // Leave-guard: if an install is in flight, confirm before navigating.
                        if store.installing.get() {
                            let ok = web_sys::window()
                                .map(|w| {
                                    w.confirm_with_message(t_string!(i18n, extensions.leave_confirm))
                                        .unwrap_or(false)
                                })
                                .unwrap_or(false);
                            if !ok {
                                return;
                            }
                        }
                        navigate("/chat", Default::default());
                    }
                }
            >
                {t!(i18n, extensions.back_to_chat)}
            </button>
            <div class="my-2 border-t border-border"></div>
            {item("featured", category_label(i18n, "featured"), "★")}
            {item("all", category_label(i18n, "all"), "🗂")}
            <div class="my-2 border-t border-border"></div>
            {CATEGORIES
                .iter()
                .map(|c| item(c.value, category_label(i18n, c.value), c.emoji))
                .collect_view()}
        </nav>
    }
}
