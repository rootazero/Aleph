use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use crate::api::extensions::{ExtensionEntry, ExtensionsApi};
use crate::components::extensions::card::ExtensionCard;
use crate::components::extensions::chips::{category_label, FilterSegs, StoreSearch};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, Locale};
use crate::views::extensions::model::{apply_filters, featured_picks, group_into_shelves, Filters};
use crate::views::extensions::StoreState;
use leptos_i18n::I18nContext;

pub(crate) fn load_catalog(
    state: DashboardState,
    store: StoreState,
    i18n: I18nContext<Locale>,
    quiet: bool,
) {
    if !quiet {
        store.loading.set(true);
    }
    store.error.set(None);
    spawn_local(async move {
        match ExtensionsApi::catalog(&state, json!({})).await {
            Ok(list) => {
                store.entries.set(list);
                if !quiet {
                    store.loading.set(false);
                }
            }
            Err(e) => {
                let prefix = t_string!(i18n, extensions.error.catalog_load).to_string();
                store.error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("{prefix}: {e}")
                    }),
                ));
                if !quiet {
                    store.loading.set(false);
                }
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
            load_catalog(state, store, i18n, false);
        } else {
            store.loading.set(false);
        }
    });

    // Filtered view (chip/filter signals drive these already).
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
        // Chrome: search (left) + installed entry (right) on one row, filter
        // segments below (category nav lives in the left column now). The
        // installed button stays in the scrollable content area so Windows'
        // fixed notification bell (z-[50]) never overlaps it.
        <div class="flex flex-col gap-3 mb-4">
            <div class="flex items-center justify-between gap-4">
                <StoreSearch />
                <button
                    class="px-3 py-1.5 bg-surface-sunken text-text-secondary rounded-lg text-sm hover:text-text-primary whitespace-nowrap flex-shrink-0"
                    on:click=move |_| store.show_installed.set(true)
                >
                    {t!(i18n, extensions.installed)}
                </button>
            </div>
            <FilterSegs />
        </div>

        // Error banner (unchanged from Task 4)
        {move || store.error.get().map(|err| view! {
            <div class="p-3 bg-danger-subtle border border-border rounded text-danger text-sm mb-4">{err}</div>
        })}

        // Body: loading → empty → featured/shelves → flat grid
        {move || {
            if store.loading.get() {
                view! {
                    <div class="flex items-center justify-center py-12">
                        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary"></div>
                    </div>
                }.into_any()
            } else if filtered().is_empty() {
                view! {
                    <div class="text-center py-12 border border-dashed border-border rounded-xl">
                        <div class="text-4xl mb-4">"🧩"</div>
                        <p class="text-text-secondary">{t!(i18n, extensions.empty)}</p>
                    </div>
                }.into_any()
            } else {
                let entries = store.entries.get();
                let category = store.category.get();
                let query = store.query.get();
                let kind_filter = store.kind_filter.get();
                let trust_filter = store.trust_filter.get();

                let featured_view = category == "featured"
                    && query.trim().is_empty()
                    && kind_filter == "all"
                    && trust_filter == "all";

                if featured_view {
                    let featured = featured_picks(&entries, 3);
                    let shelves = group_into_shelves(&entries);
                    view! {
                        <div class="space-y-8">
                            {(!featured.is_empty()).then(|| view! {
                                <div>
                                    <h2 class="font-serif text-lg text-text-primary mb-3">
                                        {t!(i18n, extensions.featured)}
                                    </h2>
                                    <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
                                        <For
                                            each=move || featured.clone()
                                            key=|e: &ExtensionEntry| e.id.clone()
                                            children=move |e| view! { <ExtensionCard entry=e /> }
                                        />
                                    </div>
                                </div>
                            })}
                            {shelves.into_iter().map(|(cat, items)| {
                                let shelf_title = category_label(i18n, cat);
                                view! {
                                    <div>
                                        <h2 class="font-serif text-lg text-text-primary mb-3">
                                            {shelf_title}
                                        </h2>
                                        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
                                            <For
                                                each=move || items.clone()
                                                key=|e: &ExtensionEntry| e.id.clone()
                                                children=move |e| view! { <ExtensionCard entry=e /> }
                                            />
                                        </div>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                } else {
                    // Flat filtered grid (Task 4 body)
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
            }
        }}
    }
}
