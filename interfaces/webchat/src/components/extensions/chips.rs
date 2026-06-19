use leptos::prelude::*;
use leptos_i18n::I18nContext;

use crate::i18n::{t_string, use_i18n, Locale};
use crate::views::extensions::model::CATEGORIES;
use crate::views::extensions::StoreState;

/// Localize a category value string using literal i18n key paths.
/// Covers "featured", "all", and all 13 CATEGORIES values.
/// Used for BOTH chip labels (CategoryChips) and shelf titles (browse.rs).
pub fn category_label(i18n: I18nContext<Locale>, value: &str) -> String {
    match value {
        "featured"      => t_string!(i18n, extensions.cat.featured).to_string(),
        "all"           => t_string!(i18n, extensions.cat.all).to_string(),
        "search"        => t_string!(i18n, extensions.cat.search).to_string(),
        "developer"     => t_string!(i18n, extensions.cat.developer).to_string(),
        "data"          => t_string!(i18n, extensions.cat.data).to_string(),
        "productivity"  => t_string!(i18n, extensions.cat.productivity).to_string(),
        "writing"       => t_string!(i18n, extensions.cat.writing).to_string(),
        "communication" => t_string!(i18n, extensions.cat.communication).to_string(),
        "knowledge"     => t_string!(i18n, extensions.cat.knowledge).to_string(),
        "files"         => t_string!(i18n, extensions.cat.files).to_string(),
        "design"        => t_string!(i18n, extensions.cat.design).to_string(),
        "automation"    => t_string!(i18n, extensions.cat.automation).to_string(),
        "finance"       => t_string!(i18n, extensions.cat.finance).to_string(),
        "utilities"     => t_string!(i18n, extensions.cat.utilities).to_string(),
        "other"         => t_string!(i18n, extensions.cat.other).to_string(),
        _               => value.to_string(),
    }
}

#[component]
#[must_use]
pub fn CategoryChips() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let chip = move |value: &'static str, label: String, emoji: &'static str| {
        let active = move || store.category.get() == value;
        view! {
            <button
                class=move || if active() {
                    "flex items-center gap-1 px-3 py-1.5 rounded-full text-sm bg-text-primary text-surface whitespace-nowrap"
                } else {
                    "flex items-center gap-1 px-3 py-1.5 rounded-full text-sm bg-surface-sunken text-text-secondary hover:text-text-primary whitespace-nowrap"
                }
                on:click=move |_| store.category.set(value.to_string())
            >
                <span>{emoji}</span>
                <span>{label}</span>
            </button>
        }
    };
    view! {
        <div class="flex gap-2 overflow-x-auto pb-2">
            {chip("featured", category_label(i18n, "featured"), "★")}
            {CATEGORIES.iter().map(|c| chip(c.value, category_label(i18n, c.value), c.emoji)).collect_view()}
        </div>
    }
}

#[component]
#[must_use]
pub fn FilterSegs() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    let seg = move |sig: RwSignal<String>, value: &'static str, label: String| {
        let active = move || sig.get() == value;
        view! {
            <button
                class=move || if active() {
                    "px-2.5 py-1 rounded-md text-xs font-mono bg-text-primary text-surface"
                } else {
                    "px-2.5 py-1 rounded-md text-xs font-mono text-text-secondary hover:text-text-primary"
                }
                on:click=move |_| sig.set(value.to_string())
            >{label}</button>
        }
    };
    view! {
        <div class="flex items-center gap-4">
            <div class="flex items-center gap-1 bg-surface-sunken rounded-lg p-1">
                {seg(store.kind_filter, "all",    t_string!(i18n, extensions.cat.all).to_string())}
                {seg(store.kind_filter, "skill",  t_string!(i18n, extensions.kind.skill).to_string())}
                {seg(store.kind_filter, "plugin", t_string!(i18n, extensions.kind.plugin).to_string())}
                {seg(store.kind_filter, "mcp",    t_string!(i18n, extensions.kind.mcp).to_string())}
            </div>
            <div class="flex items-center gap-1 bg-surface-sunken rounded-lg p-1">
                {seg(store.trust_filter, "all",       t_string!(i18n, extensions.cat.all).to_string())}
                {seg(store.trust_filter, "official",  t_string!(i18n, extensions.trust.official).to_string())}
                {seg(store.trust_filter, "verified",  t_string!(i18n, extensions.trust.verified).to_string())}
                {seg(store.trust_filter, "community", t_string!(i18n, extensions.trust.community).to_string())}
            </div>
        </div>
    }
}

#[component]
#[must_use]
pub fn StoreSearch() -> impl IntoView {
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();
    // Filtering is in-memory + reactive, so the query signal can update on every input
    // (no network debounce needed; `apply_filters` is cheap over the cached list).
    view! {
        <input
            class="w-full max-w-md px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:ring-2 focus:ring-primary/30"
            prop:value=move || store.query.get()
            placeholder=move || t_string!(i18n, extensions.search_placeholder).to_string()
            on:input=move |ev| store.query.set(event_target_value(&ev))
        />
    }
}
