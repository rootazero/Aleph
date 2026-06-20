//! Top-level Extensions store mode (full-screen takeover, grouped with Teams).
pub mod browse;
pub mod installed;
pub mod model;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::extensions::ExtensionEntry;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

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
    // ── Install flow (Task 8) ─────────────────────────────────────────────
    /// Set by card/drawer Install; the `InstallFlow` Effect consumes it (→ None)
    /// and fires the first probe.
    pub install_target: RwSignal<Option<ExtensionEntry>>,
    /// In-flight extension id — persists across the multi-step flow so handlers
    /// never depend on `selected` (the drawer may close mid-flow).
    pub install_id: RwSignal<Option<String>>,
    /// In-flight entry — kept because the Configure step needs `config_schema`.
    pub install_entry: RwSignal<Option<ExtensionEntry>>,
    /// `Missing` branch's secret names, carried to the Configure step.
    pub install_missing: RwSignal<Vec<String>>,
    pub install_step: RwSignal<crate::components::extensions::install_flow::InstallStep>,
    pub disclosure: RwSignal<Option<crate::api::extensions::DisclosurePayload>>,
    pub config_values: RwSignal<serde_json::Map<String, serde_json::Value>>,
    pub installing: RwSignal<bool>,
    pub install_error: RwSignal<Option<String>>,
}

impl StoreState {
    fn new() -> Self {
        use crate::components::extensions::install_flow::InstallStep;
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
            install_target: RwSignal::new(None),
            install_id: RwSignal::new(None),
            install_entry: RwSignal::new(None),
            install_missing: RwSignal::new(Vec::new()),
            install_step: RwSignal::new(InstallStep::Hidden),
            disclosure: RwSignal::new(None),
            config_values: RwSignal::new(serde_json::Map::new()),
            installing: RwSignal::new(false),
            install_error: RwSignal::new(None),
        }
    }

    /// Entry point for the install flow. Captures the in-flight entry + id,
    /// resets per-install transient state, and triggers the `InstallFlow` Effect
    /// by setting `install_target`.
    pub fn start_install(self, entry: ExtensionEntry) {
        self.install_id.set(Some(entry.id.clone()));
        self.install_entry.set(Some(entry.clone()));
        self.install_missing.set(Vec::new());
        self.config_values.set(serde_json::Map::new());
        self.install_error.set(None);
        self.install_target.set(Some(entry));
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
                    on:click={
                        let navigate = navigate.clone();
                        move |_| {
                            // Leave-guard: if an install is in flight, confirm before navigating.
                            if store.installing.get() {
                                let ok = web_sys::window()
                                    .map(|w| {
                                        w.confirm_with_message(t_string!(
                                            i18n,
                                            extensions.leave_confirm
                                        ))
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
                <div class="flex-1">
                    <h1 class="font-serif text-2xl text-text-primary leading-tight">{t!(i18n, extensions.title)}</h1>
                    <p class="text-xs text-text-tertiary">{t!(i18n, extensions.subtitle)}</p>
                </div>
                <button
                    class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded-lg text-sm hover:text-text-primary"
                    on:click=move |_| store.show_installed.set(true)
                >
                    {t!(i18n, extensions.installed)}
                </button>
            </header>
            <div class="flex-1 overflow-y-auto px-6 pb-6">
                <div class="max-w-5xl mx-auto py-6">
                    <crate::views::extensions::browse::BrowsePane />
                </div>
            </div>
            <crate::components::extensions::detail_drawer::ExtensionDetailDrawer />
            <crate::components::extensions::install_flow::InstallFlow />
            <crate::views::extensions::installed::InstalledPanel />
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
