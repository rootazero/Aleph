//! The ordered model ladder editor, and the discovery sweep that feeds it.
//!
//! # The ladder is the multi-select
//!
//! `[providers.<id>] models` is an ordered list and the order is semantic:
//! `models[0]` is what a turn naming no model gets, the rest are the failover
//! rungs the walk descends. So "pick several models" and "declare a failover
//! ladder" are the same gesture, and this editor is where it happens — the
//! session model pin stays a single value with exactly one writer
//! (`select_model`), which this file does not touch.
//!
//! The roster it offers comes from the catalogue row (`entry.roster`), which
//! the backend merged through the same leaf the failover walk uses. It is an
//! offer, not a whitelist: `select_model` accepts ids nobody has heard of, so a
//! picker that refused them would be stricter than the tool, and the free-text
//! row below the list is how you name one.
//!
//! # Why the sweep verdict lives above this component
//!
//! There are two callers of `providers.modelsRefresh` on this page: the button
//! below, and the fire-and-forget sweep the save handler starts once a
//! provider has just been linked. The second one used to test its outcome with
//! `.is_ok()` — which is true even when every row in the answer is a failure,
//! because per-provider failures are rows and not RPC errors, on purpose. So
//! linking a vendor that was down reported nothing at all, and the comment
//! explaining that away pointed at a signal *local to this component*, which
//! the save handler could not reach.
//!
//! [`RefreshState`] is therefore owned by the parent and passed in, and both
//! writers derive it through [`RefreshState::settle`] — one function, so the
//! two paths cannot disagree about what a sweep said.

use aleph_protocol::providers::{
    DiscoveryFailureKind, ModelSource, ModelStatus, ModelsRefreshResult, ModelsRefreshRow,
    RefreshOutcome,
};

use crate::api::{CatalogEntry, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// What the last `providers.modelsRefresh` said about this provider.
#[derive(Clone)]
pub(super) enum RefreshState {
    Idle,
    Running,
    /// The sweep answered about this provider.
    Row(Box<ModelsRefreshRow>),
    /// The sweep succeeded and said nothing about this provider. It only visits
    /// providers that are enabled **and** have a resolvable key, so an absent
    /// row means "not swept" — which is a different thing from "no models", and
    /// reporting it as success would be the more expensive lie.
    NotSwept,
}

impl RefreshState {
    /// A sweep is in flight.
    pub(super) const fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Read one provider's verdict out of a sweep result.
    ///
    /// The single derivation both callers use. `ok` on the RPC says the sweep
    /// ran, never that it succeeded: a provider whose vendor is unreachable
    /// comes back as a row with `ok: false` and a `kind`, which is exactly the
    /// state a caller most needs to show and the one a bare `.is_ok()` check
    /// reads as success.
    pub(super) fn settle(result: &ModelsRefreshResult, provider: &str) -> Self {
        result
            .providers
            .iter()
            .find(|r| r.provider == provider)
            .map_or(Self::NotSwept, |r| Self::Row(Box::new(r.clone())))
    }
}

/// The ordered model ladder editor plus the roster it offers.
#[component]
pub(super) fn ModelLadder(
    /// Provider id this ladder belongs to; `None` while adding a brand-new
    /// custom provider, which has no catalogue row (and so no roster and
    /// nothing to refresh) until it is saved.
    provider_id: Option<String>,
    models: RwSignal<Vec<String>>,
    catalog: RwSignal<Vec<CatalogEntry>>,
    error: RwSignal<Option<String>>,
    /// Owned by the parent so the post-save sweep can report through the same
    /// badge this component's own button writes.
    refresh: RwSignal<RefreshState>,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let typed = RwSignal::new(String::new());

    let pid = provider_id.clone();
    let entry = Signal::derive(move || {
        let id = pid.as_ref()?;
        catalog
            .get()
            .into_iter()
            .find(|e| &e.id == id || e.aliases.iter().any(|a| a == id))
    });

    let add = move |id: String| {
        let id = id.trim().to_string();
        if id.is_empty() {
            return;
        }
        let mut current = models.get_untracked();
        if !current.iter().any(|m| m == &id) {
            current.push(id);
            models.set(current);
        }
    };

    // `StoredValue` keeps the handler `Copy`, which it has to be: it is
    // installed from inside a reactive block that re-runs whenever the
    // catalogue changes, and a handler owning a `String` could only be
    // installed once.
    let refresh_id = StoredValue::new(provider_id.clone());
    let on_refresh = move |_| {
        let Some(id) = refresh_id.get_value() else {
            return;
        };
        refresh.set(RefreshState::Running);
        spawn_local(async move {
            match ProvidersApi::models_refresh(&state, Some(id.clone())).await {
                Ok(result) => {
                    refresh.set(RefreshState::settle(&result, &id));
                    // The discovered ids reach the picker through the catalogue
                    // (the server folds the on-disk cache into `roster`), so a
                    // refresh is only half done until the row is re-read.
                    if let Ok(items) =
                        ProvidersApi::catalog(&state, crate::api::CatalogView::All).await
                    {
                        catalog.set(items);
                    }
                }
                Err(e) => {
                    refresh.set(RefreshState::Idle);
                    error.set(Some(crate::components::admin_refusal::settings_load_error(
                        i18n,
                        &e,
                        |e| format!("Failed to fetch models: {e}"),
                    )));
                }
            }
        });
    };

    let can_refresh = provider_id.is_some();

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-4 space-y-4">
            <h3 class="text-xs font-medium text-text-secondary uppercase tracking-wider">
                {t!(i18n, settings.providers.models_label)}
            </h3>
            <p class="text-xs text-text-tertiary">{t!(i18n, settings.providers.models_order_hint)}</p>

            // The selected ladder, in order.
            {move || {
                let list = models.get();
                if list.is_empty() {
                    return view! {
                        <p class="text-xs text-text-tertiary italic">
                            {t!(i18n, settings.providers.models_empty)}
                        </p>
                    }.into_any();
                }
                let last = list.len() - 1;
                view! {
                    <div class="space-y-1">
                        {list.into_iter().enumerate().map(|(idx, id)| {
                            let lifecycle = entry.get()
                                .and_then(|e| e.roster.iter().find(|r| r.id == id).cloned())
                                .map(|r| r.lifecycle);
                            let retired = lifecycle.as_ref().is_some_and(|l| l.status == ModelStatus::Deprecated);
                            let successor = lifecycle.and_then(|l| l.successor.map(|s| s.to_string()));
                            view! {
                                <div class="flex items-center gap-2 px-2.5 py-1.5 rounded-lg bg-surface-sunken border border-border">
                                    <span class="text-[10px] uppercase tracking-wider text-text-tertiary w-14 shrink-0">
                                        {if idx == 0 {
                                            t_string!(i18n, settings.providers.models_first).to_string()
                                        } else {
                                            format!("#{}", idx + 1)
                                        }}
                                    </span>
                                    <span class="flex-1 text-xs font-mono text-text-primary truncate">{id.clone()}</span>
                                    {retired.then(|| view! {
                                        <span class="text-[9px] uppercase tracking-wider text-warning shrink-0"
                                              title=successor.clone().unwrap_or_default()>
                                            {t!(i18n, settings.providers.models_retired)}
                                        </span>
                                    })}
                                    <button
                                        type="button"
                                        title=move || t_string!(i18n, settings.providers.models_move_up).to_string()
                                        prop:disabled=idx == 0
                                        on:click=move |_| {
                                            let mut cur = models.get_untracked();
                                            if idx > 0 && idx < cur.len() {
                                                cur.swap(idx - 1, idx);
                                                models.set(cur);
                                            }
                                        }
                                        class="px-1.5 text-text-tertiary hover:text-text-primary disabled:opacity-30"
                                    >"↑"</button>
                                    <button
                                        type="button"
                                        title=move || t_string!(i18n, settings.providers.models_move_down).to_string()
                                        prop:disabled=idx == last
                                        on:click=move |_| {
                                            let mut cur = models.get_untracked();
                                            if idx + 1 < cur.len() {
                                                cur.swap(idx, idx + 1);
                                                models.set(cur);
                                            }
                                        }
                                        class="px-1.5 text-text-tertiary hover:text-text-primary disabled:opacity-30"
                                    >"↓"</button>
                                    <button
                                        type="button"
                                        title=move || t_string!(i18n, settings.providers.models_remove).to_string()
                                        on:click=move |_| {
                                            let mut cur = models.get_untracked();
                                            if idx < cur.len() {
                                                cur.remove(idx);
                                                models.set(cur);
                                            }
                                        }
                                        class="px-1.5 text-text-tertiary hover:text-danger"
                                    >"✕"</button>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}

            // The roster this provider offers, minus what is already picked.
            {move || {
                let Some(e) = entry.get() else {
                    return view! { <span></span> }.into_any();
                };
                let chosen = models.get();
                let offer: Vec<_> = e.roster.into_iter()
                    .filter(|r| !chosen.iter().any(|m| m == &r.id))
                    .collect();
                if offer.is_empty() {
                    return view! { <span></span> }.into_any();
                }
                view! {
                    <div class="space-y-1">
                        <p class="text-[10px] uppercase tracking-wider text-text-tertiary">
                            {t!(i18n, settings.providers.models_offered)}
                        </p>
                        <div class="flex flex-wrap gap-1.5">
                            {offer.into_iter().map(|r| {
                                let id = r.id.clone();
                                let retired = r.lifecycle.status == ModelStatus::Deprecated;
                                let reference = reference_note(&r);
                                let hint = r.lifecycle.successor.map(|s| s.to_string()).unwrap_or_default();
                                let source = match r.source {
                                    ModelSource::Configured => t_string!(i18n, settings.providers.source_configured).to_string(),
                                    ModelSource::Discovered => t_string!(i18n, settings.providers.source_discovered).to_string(),
                                    ModelSource::PresetDefault
                                    | ModelSource::PresetFallback
                                    | ModelSource::PresetAux => t_string!(i18n, settings.providers.source_curated).to_string(),
                                };
                                view! {
                                    <button
                                        type="button"
                                        title=hint
                                        on:click=move |_| add(id.clone())
                                        class="flex items-center gap-1.5 px-2 py-1 rounded-md border border-border bg-surface-sunken hover:border-primary/40 transition-colors"
                                    >
                                        <span class="text-xs font-mono text-text-secondary">{r.id.clone()}</span>
                                        // The window and the price, when the curated tables have
                                        // them. Absent is rendered as *nothing at all* rather than
                                        // a zero: this is the only cell on the row that would be a
                                        // false claim if it guessed.
                                        {(!reference.is_empty()).then(|| view! {
                                            <span class="text-[9px] text-text-tertiary">{reference}</span>
                                        })}
                                        <span class="text-[9px] uppercase tracking-wider text-text-tertiary">{source}</span>
                                        {retired.then(|| view! {
                                            <span class="text-[9px] uppercase tracking-wider text-warning">
                                                {t!(i18n, settings.providers.models_retired)}
                                            </span>
                                        })}
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                }.into_any()
            }}

            // Free-text escape hatch. `select_model` accepts ids that are in no
            // roster, so this picker must too.
            <div class="flex gap-2">
                <input
                    type="text"
                    prop:value=move || typed.get()
                    on:input=move |ev| typed.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            add(typed.get_untracked());
                            typed.set(String::new());
                        }
                    }
                    placeholder=move || t_string!(i18n, settings.providers.models_add_placeholder).to_string()
                    class="flex-1 px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm font-mono focus:outline-none focus:ring-2 focus:ring-primary/30"
                />
                <button
                    type="button"
                    on:click=move |_| { add(typed.get_untracked()); typed.set(String::new()); }
                    class="px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-secondary hover:border-primary/40"
                >
                    {t!(i18n, settings.providers.models_add)}
                </button>
            </div>

            // Live discovery: a button only where one can succeed.
            {move || {
                if !can_refresh {
                    return view! { <span></span> }.into_any();
                }
                let discoverable = entry.get().is_some_and(|e| e.discoverable);
                if !discoverable {
                    return view! {
                        <p class="text-xs text-text-tertiary">
                            {t!(i18n, settings.providers.fetch_unsupported)}
                        </p>
                    }.into_any();
                }
                view! {
                    <div class="space-y-2">
                        <button
                            type="button"
                            on:click=on_refresh
                            prop:disabled=move || refresh.get().is_running()
                            class="px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm text-text-secondary hover:border-primary/40 disabled:opacity-50"
                        >
                            {move || if refresh.get().is_running() {
                                t_string!(i18n, settings.providers.fetching).to_string()
                            } else {
                                t_string!(i18n, settings.providers.fetch_models).to_string()
                            }}
                        </button>
                        {move || match refresh.get() {
                            RefreshState::Idle | RefreshState::Running => view! { <span></span> }.into_any(),
                            RefreshState::NotSwept => view! {
                                <p class="text-xs text-warning">{t!(i18n, settings.providers.fetch_not_swept)}</p>
                            }.into_any(),
                            RefreshState::Row(row) => {
                                let count = row.models.len();
                                let kind = row.kind.map(|k| failure_kind_label(i18n, k));
                                // The verdict is `ModelsRefreshRow::outcome`, not a local
                                // `match (ok, stale)`. This badge, the CLI's status column and
                                // the TUI's sentence each derived the tri-state themselves,
                                // which survives exactly until a fourth state exists.
                                match row.outcome() {
                                    RefreshOutcome::Live => view! {
                                        <p class="text-xs text-success">
                                            {t!(i18n, settings.providers.fetch_live)}
                                            " · "
                                            {count.to_string()}
                                        </p>
                                    }.into_any(),
                                    RefreshOutcome::Stale => view! {
                                        <div class="text-xs text-warning">
                                            <p>{t!(i18n, settings.providers.fetch_stale)}" · "{count.to_string()}</p>
                                            {kind.map(|k| view! { <p class="text-text-tertiary">{k}</p> })}
                                        </div>
                                    }.into_any(),
                                    // Nothing broke: this endpoint has no listing to fetch. The
                                    // button is already hidden for these rows, so reaching here
                                    // means the server answered about a provider whose preset
                                    // opts out — red would be a lie about a healthy vendor.
                                    RefreshOutcome::NotApplicable => view! {
                                        <div class="text-xs text-text-tertiary">
                                            {kind.map(|k| view! { <p>{k}</p> })}
                                        </div>
                                    }.into_any(),
                                    RefreshOutcome::Failed => view! {
                                        <div class="text-xs text-danger">
                                            <p>{t!(i18n, settings.providers.fetch_failed)}</p>
                                            {kind.map(|k| view! { <p class="text-text-tertiary">{k}</p> })}
                                        </div>
                                    }.into_any(),
                                }
                            }
                        }}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

/// The window and the price for one offerable id, or an empty string.
///
/// Both halves come from the contract's own formatters — the same two functions
/// the TUI picker and `aleph providers models` call — so the three faces print
/// one number each instead of three roundings of it. Empty when the curated
/// tables know neither, which is the normal state for an id scraped off a live
/// `/models` endpoint and the one case where saying nothing is the honest
/// answer.
fn reference_note(model: &aleph_protocol::providers::RosterModel) -> String {
    let window = model
        .capabilities
        .as_ref()
        .map(aleph_protocol::providers::ModelCapabilities::context_window_short);
    let price = model
        .cost
        .as_ref()
        .and_then(aleph_protocol::providers::RateCard::io_per_mtok_short);
    match (window, price) {
        (Some(w), Some(p)) => format!("{w} · {p}"),
        (Some(w), None) => w,
        (None, Some(p)) => p,
        (None, None) => String::new(),
    }
}

/// One localized sentence per discovery failure, because "no listing endpoint"
/// and "the request timed out" are the same prose but opposite advice: one is
/// never worth retrying, the other is.
fn failure_kind_label(
    i18n: leptos_i18n::I18nContext<crate::i18n::Locale>,
    kind: DiscoveryFailureKind,
) -> String {
    match kind {
        DiscoveryFailureKind::Unsupported => {
            t_string!(i18n, settings.providers.kind_unsupported).to_string()
        }
        DiscoveryFailureKind::MissingCredential => {
            t_string!(i18n, settings.providers.kind_missing_credential).to_string()
        }
        DiscoveryFailureKind::Transport => {
            t_string!(i18n, settings.providers.kind_transport).to_string()
        }
        DiscoveryFailureKind::Status => t_string!(i18n, settings.providers.kind_status).to_string(),
        DiscoveryFailureKind::Shape => t_string!(i18n, settings.providers.kind_shape).to_string(),
        DiscoveryFailureKind::Timeout => {
            t_string!(i18n, settings.providers.kind_timeout).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::providers::ModelsRefreshResult;

    fn row(provider: &str, ok: bool, kind: Option<DiscoveryFailureKind>) -> ModelsRefreshRow {
        ModelsRefreshRow {
            provider: provider.into(),
            ok,
            stale: false,
            fetched_at: None,
            models: Vec::new(),
            kind,
            error: kind.map(|_| "boom".to_string()),
        }
    }

    /// The defect this function exists to close: a sweep that *ran* and
    /// reported a failure is an `Ok` RPC, so the post-save path's `.is_ok()`
    /// test called it a success and reported nothing.
    #[test]
    fn a_failed_row_settles_as_a_failure_not_as_success() {
        let result = ModelsRefreshResult {
            providers: vec![row("openai", false, Some(DiscoveryFailureKind::Transport))],
        };
        match RefreshState::settle(&result, "openai") {
            RefreshState::Row(r) => {
                assert!(!r.ok);
                assert_eq!(r.kind, Some(DiscoveryFailureKind::Transport));
            }
            _ => panic!("a row for the provider we asked about must settle as Row"),
        }
    }

    #[test]
    fn a_sweep_that_skipped_us_is_not_a_success() {
        // "The sweep ran and said nothing about you" is its own answer. Reading
        // it as "no models" would claim the vendor offers none.
        let result = ModelsRefreshResult {
            providers: vec![row("anthropic", true, None)],
        };
        assert!(matches!(
            RefreshState::settle(&result, "openai"),
            RefreshState::NotSwept
        ));
    }

    #[test]
    fn the_matching_row_is_picked_out_of_a_full_sweep() {
        let result = ModelsRefreshResult {
            providers: vec![
                row("anthropic", false, Some(DiscoveryFailureKind::Timeout)),
                row("openai", true, None),
            ],
        };
        match RefreshState::settle(&result, "openai") {
            RefreshState::Row(r) => {
                assert_eq!(r.provider, "openai");
                assert!(r.ok);
            }
            _ => panic!("the row we asked about must win"),
        }
    }
}
