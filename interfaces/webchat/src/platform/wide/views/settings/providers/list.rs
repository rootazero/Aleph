//! Left-panel list sections — Subscription / Configured.
//!
//! Both render a vertical stack of provider cards over the rows
//! `providers.catalog` sent. They take the shared `catalog` + `providers` +
//! `selected` signals from the parent `ProvidersView` and emit click handlers
//! that mutate `selected` (unconfigured → `__preset__<id>`, configured → the
//! real config key).
//!
//! The unconfigured remainder of the catalogue lives in [`super::picker`], not
//! here: 56 rows of it drowned the two the operator came for.
//!
//! # Why the sections are these two and not "preset vs custom"
//!
//! The server merges presets, operator-defined providers and the MoA pseudo
//! row into one list and does **not** mark which is which, so that partition is
//! not recoverable here — and re-deriving it would mean keeping a copy of the
//! preset table, which is the drift this page was rewritten to delete. What
//! *is* on the row is whether the operator has a `[providers.<id>]` section for
//! it (a non-empty `models` ladder) and how it authenticates (`auth_kind`), so
//! those are the cuts: sign-in providers, and providers you have set up.

use crate::api::{AuthKind, CatalogEntry, ProviderInfo};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::ProvidersApi;

/// The config key a catalogue row edits, when the operator has one.
///
/// A preset configured under an alias (`kimi` for `moonshot`, `codex` for
/// `chatgpt`) attaches to the canonical catalogue row server-side, so the row's
/// id is not necessarily the key `providers.list` reports. The aliases come off
/// the row itself — this used to be a `codex → chatgpt` literal in this crate,
/// which covered exactly one of the vendors that have alternative names.
pub(super) fn configured_key(entry: &CatalogEntry, providers: &[ProviderInfo]) -> Option<String> {
    providers
        .iter()
        .find(|p| p.name == entry.id || entry.aliases.contains(&p.name))
        .map(|p| p.name.clone())
}

/// Rows this page can edit.
///
/// The MoA pseudo-provider (`protocol == "moa"`) is dropped: it is a virtual
/// multiplexer over other providers' credentials with no config section of its
/// own, so a settings row for it would open an editor that can only write
/// nonsense. Same exclusion, same reason, as `moa::options::available_options`.
///
/// No query parameter: the panel now lists only sign-in rows and configured
/// rows — at most a dozen — and the search box moved into [`super::picker`],
/// which is the surface with 56 rows to sift.
fn editable(catalog: &[CatalogEntry]) -> Vec<CatalogEntry> {
    catalog
        .iter()
        .filter(|e| e.protocol != "moa")
        .cloned()
        .collect()
}

/// True when the operator has a `[providers.<id>]` section for this row.
///
/// `models` is the operator's ladder and the wire rejects an empty one, so a
/// non-empty ladder is exactly "there is a config entry" — as opposed to
/// `has_api_key`, which is also true for a key sitting in an env var for a
/// provider nobody has configured.
pub(super) fn is_configured(entry: &CatalogEntry) -> bool {
    !entry.models.is_empty()
}

fn badge_state(entry: &CatalogEntry) -> BadgeState {
    BadgeState {
        is_default: entry.is_default,
        verified: entry.verified,
    }
}

#[component]
pub(super) fn SubscriptionLoginSection(
    catalog: RwSignal<Vec<CatalogEntry>>,
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    // Live OAuth connection state, keyed by the catalogue id. A row is only
    // here because the SERVER said `auth_kind == oauth`, so this map grows and
    // shrinks with the catalogue instead of with a list in this file.
    let oauth_connected: RwSignal<Vec<(String, bool)>> = RwSignal::new(Vec::new());

    Effect::new(move || {
        let ids: Vec<String> = catalog
            .get()
            .iter()
            .filter(|e| e.auth_kind == AuthKind::OAuth)
            .map(|e| e.id.clone())
            .collect();
        let state = expect_context::<DashboardState>();
        for id in ids {
            spawn_local(async move {
                // A failed status read is not "disconnected" — it is no answer,
                // so the row simply keeps whatever it last knew rather than
                // claiming the subscription is gone.
                if let Ok(status) = ProvidersApi::oauth_status(&state, id.clone()).await {
                    // `try_update`, not read-then-set: one task per OAuth row
                    // lands here and a read/write pair split around the `await`
                    // would let the last writer erase its siblings' answers.
                    // `None` means the scope is gone (page tearing down).
                    oauth_connected.try_update(|rows| {
                        match rows.iter_mut().find(|(k, _)| *k == id) {
                            Some(row) => row.1 = status.connected,
                            None => rows.push((id, status.connected)),
                        }
                    });
                }
            });
        }
    });

    view! {
        {move || {
            let rows: Vec<CatalogEntry> = editable(&catalog.get())
                .into_iter()
                .filter(|e| e.auth_kind == AuthKind::OAuth)
                .collect();
            if rows.is_empty() {
                return view! { <div></div> }.into_any();
            }
            view! {
                <div>
                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                        {t!(i18n, settings.providers.subscription_login)}
                    </h2>
                    <div class="grid grid-cols-1 gap-2">
                        {rows.into_iter().map(|entry| {
                            // `StoredValue` so every slot below can be `FnMut`:
                            // a row renders four closures and a `String` can
                            // only be moved into one of them.
                            let id = StoredValue::new(entry.id.clone());
                            let row = StoredValue::new(entry.clone());
                            let state = badge_state(&entry);
                            let subtitle = entry.notes.clone()
                                .unwrap_or_else(|| entry.default_model.clone());
                            // A subscription row counts as live when the token
                            // is there OR the provider verified — the two are
                            // written by different flows.
                            let live = move || {
                                state.verified
                                    || oauth_connected.get().iter()
                                        .any(|(k, c)| *c && id.with_value(|i| i == k))
                            };
                            view! {
                                <ProviderRowCard
                                    name=entry.display_name.clone()
                                    icon_color=entry.color.clone()
                                    subtitle=subtitle
                                    is_selected=move || {
                                        let sel = selected.get();
                                        id.with_value(|i| {
                                            sel.as_deref() == Some(i.as_str())
                                                || sel.as_deref() == Some(&format!("__preset__{i}"))
                                        })
                                    }
                                    is_configured=live
                                    dot=move || if live() { RowDot::Verified } else { RowDot::None }
                                    badge=move || {
                                        let state = BadgeState { verified: live(), ..state };
                                        view! { <ProviderBadges state=state /> }.into_any()
                                    }
                                    large_icon=true
                                    trailing=move || view! {
                                        <svg class="w-4 h-4 text-text-tertiary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"/>
                                        </svg>
                                    }.into_any()
                                    on_click=move || {
                                        let target = row.with_value(|e| {
                                            configured_key(e, &providers.get())
                                                .unwrap_or_else(|| format!("__preset__{}", e.id))
                                        });
                                        selected.set(Some(target));
                                    }
                                />
                            }
                        }).collect_view()}
                    </div>
                </div>
            }.into_any()
        }}
    }
}

/// The catalogue rows the operator has actually configured.
///
/// The unconfigured remainder used to render here as a "Quick setup" stack of
/// 56 cards, which made this panel a scroll well whose least findable rows were
/// the ones being used. It moved to [`super::picker`]; nothing else about a row
/// changed, so a provider deleted from the detail pane empties its ladder,
/// drops out of this list, and is offered again by the picker.
#[component]
pub(super) fn ConfiguredList(
    catalog: RwSignal<Vec<CatalogEntry>>,
    providers: RwSignal<Vec<ProviderInfo>>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || {
            let mine: Vec<CatalogEntry> = editable(&catalog.get())
                .into_iter()
                .filter(|e| e.auth_kind != AuthKind::OAuth)
                .filter(is_configured)
                .collect();
            if mine.is_empty() {
                return view! { <div></div> }.into_any();
            }
            let known = providers.get();
            view! {
                <div>
                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                        {t!(i18n, settings.providers.configured_providers)}
                    </h2>
                    <div class="grid grid-cols-1 gap-2">
                        {mine.into_iter().map(|e| {
                            let key = configured_key(&e, &known);
                            view! { <CatalogRow entry=e configured_key=key selected=selected /> }
                        }).collect_view()}
                    </div>
                </div>
            }.into_any()
        }}
    }
}

/// One catalogue row. `configured_key` is `Some` when the operator has a config
/// section for it — resolved by the caller because it needs `providers.list`,
/// which the row itself cannot see.
#[component]
fn CatalogRow(
    entry: CatalogEntry,
    configured_key: Option<String>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let id = entry.id.clone();
    let target = configured_key
        .clone()
        .unwrap_or_else(|| format!("__preset__{id}"));
    let target_click = target.clone();
    let self_key = configured_key.unwrap_or_else(|| entry.id.clone());
    let subtitle = if entry.default_model.is_empty() {
        entry
            .notes
            .clone()
            .unwrap_or_else(|| entry.base_url.clone())
    } else {
        entry.default_model.clone()
    };
    let configured = is_configured(&entry);
    let verified = entry.verified;
    let state = badge_state(&entry);

    view! {
        <ProviderRowCard
            name=entry.display_name.clone()
            icon_color=entry.color.clone()
            subtitle=subtitle
            is_selected=move || {
                let sel = selected.get();
                sel.as_deref() == Some(self_key.as_str()) || sel.as_deref() == Some(target.as_str())
            }
            is_configured=move || configured
            dot=move || if verified {
                RowDot::Verified
            } else if configured {
                RowDot::Inactive
            } else {
                RowDot::None
            }
            badge=move || view! { <ProviderBadges state=state /> }.into_any()
            on_click=move || selected.set(Some(target_click.clone()))
        />
    }
}
