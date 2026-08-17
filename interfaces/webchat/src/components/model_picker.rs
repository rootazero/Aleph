//! Chat-window model picker — compact pill + popover for picking the
//! per-turn provider/model.
//!
//! Architecture (openclaw parity, Rust port):
//! 1. Pill button shows the *current selection* (or "Default" when none).
//! 2. Click opens a popover; on first open we fetch `providers.catalog`
//!    with `view: "configured"` (only credentialed + verified providers).
//! 3. Clicking a row writes `ChatState.selected_model` as
//!    [`ModelOverride::Qualified`]. The composer reads this when sending,
//!    so the daemon's run loop short-circuits its fallback chain.
//! 4. The "Default" row clears the override — falls back to the agent's
//!    configured model.
//!
//! Differences from openclaw's `<select>`:
//! * Catalog already arrives credential-filtered → the client-side pass is a
//!   search box, not a usability check, and there is no second round-trip.
//! * Selection persists in `ChatState` (memory-only for now); server-side
//!   `preferred_model` row is a follow-up.
//! * Each row carries a per-model link to the provider's homepage when
//!   available — turns the picker into a low-friction discovery surface.
//!
//! ## Keyboard walk
//!
//! The popover is two levels deep on screen (a provider heading, then its
//! model ids) and one level deep to the keyboard: [`walk_targets`] flattens it
//! into the rows ↑/↓ can actually land on, with `Default` at index 0 because
//! that is where it is drawn. The highlight moves only through
//! [`step_highlight`], shared with the settings disclosure and the ⌘K palette.
//!
//! The flattening is why the provider loop is a plain `map` and not `<For>`:
//! a row's flat index depends on how many models every *preceding* provider
//! offers, so narrowing an earlier provider's roster shifts every later index.
//! Keyed by provider id alone, `<For>` would keep those children and their
//! captured — now stale — offsets; keying by `(id, offset)` would rebuild the
//! list on exactly the keystrokes that change it, which is what `map` already
//! does, for less.

use leptos::html::{Div, Input};
use leptos::prelude::*;
use leptos::task::spawn_local;

use aleph_protocol::providers::search::filter_catalog;

use crate::api::providers::{CatalogEntry, CatalogView, ModelOverride, ProvidersApi, RosterModel};
use crate::components::picker_nav::{
    publish_more_below, row_dom_id, scroll_row_into_view, step_highlight,
};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;

/// Namespace for this popover's row ids — see [`row_dom_id`].
const LIST: &str = "model";

/// A row the keyboard walk can land on, in the order the popover draws them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickTarget {
    /// Clear the per-turn override and fall back to the agent's chain. Always
    /// index 0 — the row is rendered above the list and outside the filter, so
    /// it is reachable even while the catalogue is loading or empty.
    Default,
    Model {
        provider: String,
        model: String,
    },
}

/// Flatten the filtered catalogue into the rows ↑/↓ walk.
///
/// Takes the same `filter_catalog` the renderer takes, and in the same order,
/// so index *n* here is the *n*th button on screen. Deriving the walk from a
/// second traversal of the same data is the failure this exists to prevent:
/// the roster `filter_catalog` returns is already narrowed to the matching
/// ids, and a hand-written "all models of the matching providers" would be a
/// longer list than the one being drawn.
#[must_use]
pub(crate) fn walk_targets(entries: &[CatalogEntry], query: &str) -> Vec<PickTarget> {
    let mut out = vec![PickTarget::Default];
    for entry in filter_catalog(entries, query) {
        for model in roster(&entry) {
            out.push(PickTarget::Model {
                provider: entry.id.clone(),
                model: model.id,
            });
        }
    }
    out
}

/// Pill + dropdown for selecting the per-turn chat model.
#[component]
#[must_use]
pub fn ModelPicker() -> impl IntoView {
    let i18n = use_i18n();
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let open = RwSignal::new(false);
    let entries: RwSignal<Vec<CatalogEntry>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(false);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    // Live filter term for the popover's search box. Order-preserving substring
    // match (`aleph_protocol::providers::search`); reset when the popover closes.
    let search = RwSignal::new(String::new());

    // Fetch catalog on first open. Generation-counter staleness is the next
    // step — for the inaugural cut, we cache forever within the session.
    let load_catalog = move || {
        if !entries.get_untracked().is_empty() || loading.get_untracked() {
            return;
        }
        loading.set(true);
        load_error.set(None);
        spawn_local(async move {
            match ProvidersApi::catalog(&dashboard, CatalogView::Configured).await {
                Ok(items) => entries.set(items),
                Err(e) => load_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                )),
            }
            loading.set(false);
        });
    };

    // What the pill names, in the same precedence core resolves
    // (`effective_model_directive`): this turn's pick, else the conversation's
    // `select_model` pin, else nothing chosen at all.
    //
    // The middle arm is the one that was missing. A pin is per-session state
    // the model itself sets (R8) and the run loop honours from the next run on,
    // so a pill that only knew about its own per-turn override answered
    // "Default" for a conversation that was pinned — naming, of all the models
    // available, the one that was not going to serve.
    let trigger_label = move || -> String {
        if let Some(mo) = chat.selected_model.get() {
            return match mo.provider() {
                Some(p) => format!("{}/{}", p, mo.model()),
                None => mo.model().to_string(),
            };
        }
        chat.session_model_pin
            .get()
            .unwrap_or_else(|| "Default".to_string())
    };

    let select_entry = move |provider: String, model: String| {
        chat.selected_model
            .set(Some(ModelOverride::Qualified { provider, model }));
        open.set(false);
    };

    let clear_selection = move || {
        chat.selected_model.set(None);
        open.set(false);
    };

    // Clear the filter every time the popover closes so the next open starts
    // from the full catalog (mirrors `command_palette.rs`'s reset-on-close).
    Effect::new(move |_| {
        if !open.get() {
            search.set(String::new());
        }
    });

    let highlight = RwSignal::new(0usize);
    let popover_ref = NodeRef::<Div>::new();
    let search_ref = NodeRef::<Input>::new();
    let more_below = RwSignal::new(false);

    // The flat walk, shared by the keyboard handler and by the per-row
    // `is_highlighted` predicates below.
    let targets = Memo::new(move |_| walk_targets(&entries.get(), &search.get()));

    // A new list means a new first row (see `preset_picker` for the full
    // argument). `Default` is index 0, so a narrowed search lands the highlight
    // on "clear the override" rather than on whatever survived the filter —
    // deliberate: the alternative is arming Enter on a model the operator has
    // not looked at.
    Effect::new(move |_| {
        targets.track();
        highlight.set(0);
    });

    let remeasure = move || publish_more_below(popover_ref, more_below);

    // Focus so the popover can receive keys at all, and re-measure the well.
    // Both re-run when the catalogue lands, because the search box only exists
    // once it has: an effect that only tracked `open` would fire while the
    // popover still says "loading" and find nothing to focus.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let _ = loading.get();
        let _ = entries.get();
        targets.track();
        request_animation_frame(move || {
            if let Some(el) = search_ref.get_untracked() {
                let _ = el.focus();
            } else if let Some(el) = popover_ref.get_untracked() {
                // No search box yet (loading / empty / error). The container
                // is `tabindex=-1` so Escape and the Default row stay reachable.
                let _ = el.focus();
            }
            remeasure();
        });
    });

    let commit = move |target: &PickTarget| match target {
        PickTarget::Default => clear_selection(),
        PickTarget::Model { provider, model } => select_entry(provider.clone(), model.clone()),
    };

    // Scoped to the popover subtree, not the window: this component unmounts
    // with the composer, and `window_event_listener` registers no cleanup.
    let on_key = move |ev: web_sys::KeyboardEvent| {
        if !open.get_untracked() {
            return;
        }
        let current = targets.get_untracked();
        match ev.key().as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                let next = step_highlight(current.len(), highlight.get_untracked(), 1);
                highlight.set(next);
                scroll_row_into_view(LIST, next);
            }
            "ArrowUp" => {
                ev.prevent_default();
                let next = step_highlight(current.len(), highlight.get_untracked(), -1);
                highlight.set(next);
                scroll_row_into_view(LIST, next);
            }
            "Enter" => {
                ev.prevent_default();
                if let Some(target) =
                    current.get(step_highlight(current.len(), highlight.get_untracked(), 0))
                {
                    commit(target);
                }
            }
            "Escape" => {
                ev.prevent_default();
                open.set(false);
            }
            _ => {}
        }
    };

    view! {
        <div class="relative">
            <button
                on:click=move |_| {
                    let next = !open.get_untracked();
                    open.set(next);
                    if next {
                        load_catalog();
                    }
                }
                class="flex items-center gap-1 px-2 py-1 rounded-md text-xs font-mono
                       text-text-secondary border border-border
                       bg-surface-raised backdrop-blur-[var(--glass-blur-chrome)]
                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title=move || t_string!(i18n, model_picker.pick_model_title).to_string()
            >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 2L2 7l10 5 10-5-10-5z" />
                    <path d="M2 17l10 5 10-5" />
                    <path d="M2 12l10 5 10-5" />
                </svg>
                <span class="max-w-[200px] truncate">{trigger_label}</span>
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none"
                     stroke="currentColor" stroke-width="2"
                     stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9" />
                </svg>
            </button>

            <Show when=move || open.get()>
                <div node_ref=popover_ref
                    tabindex="-1"
                    on:keydown=on_key
                    on:scroll=move |_| remeasure()
                    class="absolute bottom-full mb-2 left-0 z-50 w-80 max-h-96 overflow-y-auto
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                            p-2 space-y-1 outline-none"
                    class:aleph-scroll-more=move || more_below.get()
                    on:mouseleave=move |_| open.set(false)>
                    // Filter box — only meaningful once a non-empty catalog has
                    // loaded. Order-preserving substring filter, deliberately
                    // not fuzzy-ranked so the daemon's curated provider/model
                    // order survives. The matcher is shared with the TUI's
                    // picker (`aleph_protocol::providers::search`): two
                    // independently written filters do not merely look
                    // different, they disagree about which row a bare Enter
                    // selects.
                    {move || (!loading.get()
                        && load_error.get().is_none()
                        && !entries.get().is_empty())
                        .then(|| view! {
                            <input
                                node_ref=search_ref
                                type="text"
                                placeholder=move || {
                                    t_string!(i18n, model_picker.filter_placeholder).to_string()
                                }
                                class="w-full px-2.5 py-1.5 mb-1 rounded-md text-xs bg-surface-sunken \
                                       text-text-primary placeholder:text-text-tertiary outline-none \
                                       border border-border focus:border-primary/40"
                                on:input=move |ev| search.set(event_target_value(&ev))
                                prop:value=move || search.get()
                            />
                        })}

                    // Default option — index 0 of the keyboard walk.
                    <button
                        id=row_dom_id(LIST, 0)
                        on:click=move |_| clear_selection()
                        class=move || {
                            let base = "w-full text-left px-2.5 py-2 rounded-md text-xs \
                                        transition-colors flex items-center justify-between gap-2";
                            if chat.selected_model.get().is_none() {
                                format!("{base} bg-primary/10 text-primary border border-primary/30")
                            } else if highlight.get() == 0 {
                                format!("{base} bg-surface-sunken text-text-primary border border-border")
                            } else {
                                format!("{base} hover:bg-surface-sunken text-text-secondary border border-transparent")
                            }
                        }
                    >
                        <span class="font-medium">"Default"</span>
                        <span class="text-text-tertiary text-[10px]">"agent fallback chain"</span>
                    </button>

                    // Loading / error / list
                    {move || {
                        if loading.get() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, model_picker.loading_catalog)}
                                </div>
                            }.into_any()
                        } else if let Some(err) = load_error.get() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-danger/80 text-center">
                                    {format!("error: {err}")}
                                </div>
                            }.into_any()
                        } else if entries.get().is_empty() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, model_picker.no_providers)}
                                </div>
                            }.into_any()
                        } else if filter_catalog(&entries.get(), &search.get()).is_empty() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    {t!(i18n, model_picker.no_match)}
                                </div>
                            }.into_any()
                        } else {
                            // Plain `map` with a running flat index, not
                            // `<For>` — see the module doc. `flat` starts at 1
                            // because the Default row above took index 0.
                            let mut flat = 1usize;
                            let mut groups: Vec<AnyView> = Vec::new();
                            for entry in filter_catalog(&entries.get(), &search.get()) {
                                let provider_id = entry.id.clone();
                                let display = entry.display_name.clone();
                                let color = entry.color.clone();
                                // Models to show: the roster the backend
                                // computed for this entry, already narrowed to
                                // the matching ids by the shared matcher.
                                let mut rows: Vec<AnyView> = Vec::new();
                                for model in roster(&entry) {
                                    let idx = flat;
                                    flat += 1;
                                    let pid = provider_id.clone();
                                    let mid = model.id.clone();
                                    let pid_active = pid.clone();
                                    let mid_active = mid.clone();
                                    let is_active = move || {
                                        matches!(
                                            chat.selected_model.get(),
                                            Some(ModelOverride::Qualified { provider, model })
                                                if provider == pid_active && model == mid_active
                                        )
                                    };
                                    // Retirement is per model id now, not per
                                    // provider default: the roster carries each
                                    // id's own lifecycle, so a live id sitting
                                    // under a retired default is no longer
                                    // mislabelled (and vice versa). The successor
                                    // rides the tooltip so the fix is one hover
                                    // away.
                                    let is_retired = model.lifecycle.is_deprecated();
                                    let title = if is_retired {
                                        model.lifecycle.successor.as_ref().map_or_else(
                                            || "Retired by the vendor".to_string(),
                                            |s| format!("Retired by the vendor — use {s}"),
                                        )
                                    } else {
                                        String::new()
                                    };
                                    let display_text = model.id;
                                    rows.push(view! {
                                        <button
                                            id=row_dom_id(LIST, idx)
                                            title=title
                                            on:click=move |_| select_entry(pid.clone(), mid.clone())
                                            class=move || {
                                                let base = "w-full text-left px-2.5 py-1.5 rounded-md \
                                                            text-xs font-mono transition-colors \
                                                            border";
                                                if is_active() {
                                                    format!("{base} bg-primary/10 text-primary border-primary/30")
                                                } else if highlight.get() == idx {
                                                    format!("{base} bg-surface-sunken text-text-primary border-border")
                                                } else {
                                                    format!("{base} hover:bg-surface-sunken text-text-secondary border-transparent")
                                                }
                                            }
                                        >
                                            {display_text}
                                            {is_retired.then(|| view! {
                                                <span class="ml-1.5 text-[9px] uppercase tracking-wider text-warning">
                                                    "retired"
                                                </span>
                                            })}
                                        </button>
                                    }.into_any());
                                }
                                groups.push(view! {
                                    <div class="pt-1.5">
                                        <div class="flex items-center gap-1.5 px-2.5 pb-1">
                                            <span class="w-2 h-2 rounded-full"
                                                  style=format!("background: {}", color) />
                                            <span class="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                                                {display}
                                            </span>
                                        </div>
                                        {rows}
                                    </div>
                                }.into_any());
                            }
                            view! { <div>{groups}</div> }.into_any()
                        }
                    }}
                </div>
            </Show>
        </div>
    }
}

/// The model ids the picker offers for one provider.
///
/// This is the backend-computed `roster` field rendered verbatim — the merge
/// rules (operator list vs curated fallback rungs, base_url-moved guard) live
/// in `presets::model_ladder` on the core side, shared with the failover
/// walk, so the picker can never recommend ids the walk would refuse to dial.
#[must_use]
pub(crate) fn roster(entry: &CatalogEntry) -> Vec<RosterModel> {
    entry.roster.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::providers::ModelSource;

    /// A catalogue row as `providers.catalog` would report it. Built through
    /// serde rather than a struct literal so a field added later with a serde
    /// default does not have to be repeated here.
    fn entry(id: &str, models: &[&str]) -> CatalogEntry {
        let mut e: CatalogEntry = serde_json::from_value(serde_json::json!({
            "id": id,
            "display_name": id,
            "default_model": models.first().copied().unwrap_or_default(),
            "base_url": "",
            "protocol": "openai",
            "color": "#808080",
            "has_api_key": true,
            "verified": true,
            "enabled": true,
            "is_default": false,
        }))
        .expect("the ten required catalogue fields are all present");
        e.roster = models
            .iter()
            .map(|m| RosterModel::new(*m, ModelSource::PresetFallback))
            .collect();
        e
    }

    fn model(provider: &str, model: &str) -> PickTarget {
        PickTarget::Model {
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn an_empty_catalogue_still_offers_default() {
        // Loading, empty and errored popovers all draw the Default row, so the
        // walk must contain it even when there is nothing to filter.
        assert_eq!(walk_targets(&[], ""), vec![PickTarget::Default]);
    }

    #[test]
    fn default_is_index_zero_and_models_follow_in_render_order() {
        let cat = [entry("alpha", &["a1", "a2"]), entry("beta", &["b1"])];
        assert_eq!(
            walk_targets(&cat, ""),
            vec![
                PickTarget::Default,
                model("alpha", "a1"),
                model("alpha", "a2"),
                model("beta", "b1"),
            ]
        );
    }

    /// The property that rules out `<For key=provider_id>`: narrowing an
    /// earlier provider's roster moves every later provider's flat index, so a
    /// keyed child holding a captured index would light the wrong row.
    #[test]
    fn narrowing_an_earlier_roster_shifts_later_indices() {
        let cat = [entry("alpha", &["a1", "a2"]), entry("beta", &["b1"])];
        let wide = walk_targets(&cat, "");
        let narrow = walk_targets(&cat, "a2");

        assert_eq!(
            wide.iter().position(|t| *t == model("alpha", "a2")),
            Some(2)
        );
        assert_eq!(
            narrow.iter().position(|t| *t == model("alpha", "a2")),
            Some(1),
            "the surviving model moved up once its sibling was filtered out"
        );
    }

    /// The walk must be the buttons on screen, not "every model of every
    /// matching provider": `filter_catalog` narrows the roster it returns, and
    /// the renderer draws that narrowed roster.
    #[test]
    fn a_model_query_walks_only_the_matching_ids() {
        let cat = [entry("alpha", &["gpt-4o", "gpt-4o-mini"])];
        assert_eq!(
            walk_targets(&cat, "mini"),
            vec![PickTarget::Default, model("alpha", "gpt-4o-mini")]
        );
    }

    #[test]
    fn an_empty_query_offers_every_row() {
        // Same contract the settings picker's `offer` closure owes: browsing
        // must not require knowing a vendor's name first.
        let cat = [entry("alpha", &["a1"]), entry("beta", &["b1", "b2"])];
        assert_eq!(walk_targets(&cat, "").len(), 4);
    }
}
