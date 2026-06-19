use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use crate::api::extensions::{ExtensionEntry, ExtensionsApi};
use crate::components::extensions::card::ExtensionCard;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::views::extensions::model::{apply_filters, Filters};
use crate::views::extensions::StoreState;

fn load_catalog(state: DashboardState, store: StoreState) {
    store.loading.set(true);
    store.error.set(None);
    spawn_local(async move {
        match ExtensionsApi::catalog(&state, json!({})).await {
            Ok(list) => {
                store.entries.set(list);
                store.loading.set(false);
            }
            Err(e) => {
                store.error.set(Some(format!("Failed to load catalog: {e}")));
                store.loading.set(false);
            }
        }
    });
}

#[component]
#[must_use]
pub fn BrowsePane() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let store = expect_context::<StoreState>();
    let i18n = use_i18n();

    Effect::new(move || {
        if state.is_connected.get() {
            load_catalog(state, store);
        } else {
            store.loading.set(false);
        }
    });

    // Filtered view (Task 5 binds the chip/filter signals; here Filters reads them already).
    let filtered = move || {
        let f = Filters {
            category: store.category.get(),
            kind: store.kind_filter.get(),
            trust: store.trust_filter.get(),
            query: store.query.get(),
        };
        apply_filters(&store.entries.get(), &f)
    };

    view! {
        {move || store.error.get().map(|err| view! {
            <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm mb-4">{err}</div>
        })}
        {move || {
            if store.loading.get() {
                view! { <div class="flex items-center justify-center py-12"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div></div> }.into_any()
            } else if filtered().is_empty() {
                view! {
                    <div class="text-center py-12 border border-dashed border-border rounded-xl">
                        <div class="text-4xl mb-4">"🧩"</div>
                        <p class="text-text-secondary">{t!(i18n, extensions.empty)}</p>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
                        <For
                            each=move || filtered()
                            key=|e: &ExtensionEntry| e.id.clone()
                            children=move |e| view! { <ExtensionCard entry=e /> }
                        />
                    </div>
                }.into_any()
            }
        }}
    }
}
