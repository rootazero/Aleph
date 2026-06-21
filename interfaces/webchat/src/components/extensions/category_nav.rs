use leptos::prelude::*;

use crate::components::extensions::labels::category_label;
use crate::i18n::use_i18n;
use crate::views::extensions::model::CATEGORIES;
use crate::views::extensions::StoreState;

/// Vertical category navigation for the Aleph Hub left column.
///
/// Two "global" entries (Featured / All) are pinned on top, then the 13
/// functional-category facets below a divider. Each entry drives
/// `store.category` — identical behavior to the old horizontal CategoryChips,
/// just relocated to the left column to declutter the main area.
#[component]
#[must_use]
pub fn CategoryNav() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

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
