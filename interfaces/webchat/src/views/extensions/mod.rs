//! Top-level Extensions store mode (full-screen takeover, grouped with Teams).
pub mod browse;
pub mod model;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::extensions::ExtensionEntry;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};

/// Shared store state, provided by `ExtensionsView`, consumed by browse/drawer/installed.
#[derive(Clone, Copy)]
pub struct StoreState {
    pub entries: RwSignal<Vec<ExtensionEntry>>,
    pub loading: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub category: RwSignal<String>,
    pub kind_filter: RwSignal<String>,
    pub trust_filter: RwSignal<String>,
    pub query: RwSignal<String>,
    pub selected: RwSignal<Option<ExtensionEntry>>,
    pub show_installed: RwSignal<bool>,
}

impl StoreState {
    fn new() -> Self {
        Self {
            entries: RwSignal::new(Vec::new()),
            loading: RwSignal::new(true),
            error: RwSignal::new(None),
            category: RwSignal::new("featured".to_string()),
            kind_filter: RwSignal::new("all".to_string()),
            trust_filter: RwSignal::new("all".to_string()),
            query: RwSignal::new(String::new()),
            selected: RwSignal::new(None),
            show_installed: RwSignal::new(false),
        }
    }
}

#[component]
#[must_use]
pub fn ExtensionsView() -> impl IntoView {
    let _state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let store = StoreState::new();
    provide_context(store);
    let navigate = use_navigate();

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden bg-surface aleph-content-top">
            <header class="px-6 py-3 border-b border-border flex items-center gap-4">
                <button
                    class="text-sm text-text-secondary hover:text-text-primary transition-colors"
                    on:click=move |_| navigate("/chat", Default::default())
                >
                    {t!(i18n, extensions.back_to_chat)}
                </button>
                <div>
                    <h1 class="font-serif text-2xl text-text-primary leading-tight">{t!(i18n, extensions.title)}</h1>
                    <p class="text-xs text-text-tertiary">{t!(i18n, extensions.subtitle)}</p>
                </div>
            </header>
            <div class="flex-1 overflow-y-auto px-6 pb-6">
                <div class="max-w-5xl mx-auto py-6">
                    <crate::views::extensions::browse::BrowsePane />
                </div>
            </div>
            <crate::components::extensions::detail_drawer::ExtensionDetailDrawer />
        </div>
    }
}

#[component]
#[must_use]
pub fn ExtensionsSidebar() -> impl IntoView {
    // Minimal secondary column; the store's own topbar (chips/search/installed) lives in the
    // main area per the mockup. Category quick-nav is added with browse in Task 5.
    view! { <div class="flex flex-col h-full"></div> }
}
