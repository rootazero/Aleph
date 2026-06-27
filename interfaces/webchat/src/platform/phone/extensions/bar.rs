//! Phone Extensions category chip bar (`/extensions` landing top row): a
//! horizontal scrolling chip strip that replaces the desktop left-column
//! `CategoryNav`. Chips drive the shared app-level `StoreState.category`
//! signal — identical behavior to the desktop nav, restored to the historical
//! horizontal form. I/O-only (R4): chips only set the filter signal.

use leptos::prelude::*;

use crate::components::extensions::labels::category_label;
use crate::i18n::use_i18n;
use crate::views::extensions::model::CATEGORIES;
use crate::views::extensions::StoreState;

#[component]
#[must_use]
pub fn PhoneCategoryBar() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

    // One chip per category value. `store` and `value` are Copy, so the inner
    // class/click closures capture them by copy — mirrors desktop `CategoryNav`.
    let chip = move |value: &'static str, label: String, emoji: &'static str| {
        view! {
            <button
                class=move || if store.category.get() == value { "chip chip-active" } else { "chip" }
                on:click=move |_| store.category.set(value.to_string())
            >
                <span>{emoji}</span>
                <span class="whitespace-nowrap">{label}</span>
            </button>
        }
    };

    view! {
        <div class="flex gap-2 overflow-x-auto cc-hide-scroll py-1">
            {chip("featured", category_label(i18n, "featured"), "★")}
            {chip("all", category_label(i18n, "all"), "🗂")}
            {CATEGORIES
                .iter()
                .map(|c| chip(c.value, category_label(i18n, c.value), c.emoji))
                .collect_view()}
        </div>
    }
}
