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
//! * Catalog already arrives credential-filtered → no client-side filter
//!   pass and no second round-trip for "is this usable?".
//! * Selection persists in `ChatState` (memory-only for now); server-side
//!   `preferred_model` row is a follow-up.
//! * Each row carries a per-model link to the provider's homepage when
//!   available — turns the picker into a low-friction discovery surface.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::providers::{CatalogEntry, CatalogView, ModelOverride, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;

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
    // match (see `filter_catalog`); reset whenever the popover closes.
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
                Err(e) => load_error.set(Some(e)),
            }
            loading.set(false);
        });
    };

    let trigger_label = move || -> String {
        match chat.selected_model.get() {
            Some(mo) => match mo.provider() {
                Some(p) => format!("{}/{}", p, mo.model()),
                None => mo.model().to_string(),
            },
            None => "Default".to_string(),
        }
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
                <div class="absolute bottom-full mb-2 left-0 z-50 w-80 max-h-96 overflow-y-auto
                            glass rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                            p-2 space-y-1"
                    on:mouseleave=move |_| open.set(false)>
                    // Filter box — only meaningful once a non-empty catalog has
                    // loaded. Order-preserving substring filter (see
                    // `filter_catalog`), deliberately not fuzzy-ranked so the
                    // daemon's curated provider/model order survives.
                    {move || (!loading.get()
                        && load_error.get().is_none()
                        && !entries.get().is_empty())
                        .then(|| view! {
                            <input
                                type="text"
                                placeholder=move || {
                                    t_string!(i18n, model_picker.filter_placeholder).to_string()
                                }
                                class="w-full px-2.5 py-1.5 mb-1 rounded-md text-xs bg-surface-sunken \
                                       text-text-primary placeholder:text-text-tertiary outline-none \
                                       border border-border focus:border-primary/40"
                                on:input=move |ev| search.set(event_target_value(&ev))
                                on:keydown=move |ev| {
                                    if ev.key() == "Escape" {
                                        open.set(false);
                                    }
                                }
                                prop:value=move || search.get()
                            />
                        })}

                    // Default option
                    <button
                        on:click=move |_| clear_selection()
                        class=move || {
                            let base = "w-full text-left px-2.5 py-2 rounded-md text-xs \
                                        transition-colors flex items-center justify-between gap-2";
                            if chat.selected_model.get().is_none() {
                                format!("{base} bg-primary/10 text-primary border border-primary/30")
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
                            view! {
                                <For
                                    each=move || filter_catalog(&entries.get(), &search.get())
                                    key=|e: &CatalogEntry| e.id.clone()
                                    children=move |entry: CatalogEntry| {
                                        let provider_id = entry.id.clone();
                                        let display = entry.display_name.clone();
                                        let color = entry.color.clone();
                                        // Models to show: the roster the
                                        // backend computed for this entry.
                                        let models = roster(&entry);
                                        // A retired default must not read as a
                                        // live choice; the successor rides the
                                        // tooltip so the fix is one hover away.
                                        let retired = entry
                                            .lifecycle
                                            .is_deprecated()
                                            .then(|| entry.default_model.clone());
                                        let retired_note = entry.lifecycle.successor.clone();
                                        view! {
                                            <div class="pt-1.5">
                                                <div class="flex items-center gap-1.5 px-2.5 pb-1">
                                                    <span class="w-2 h-2 rounded-full"
                                                          style=format!("background: {}", color) />
                                                    <span class="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                                                        {display}
                                                    </span>
                                                </div>
                                                {models.into_iter().map(|model_id| {
                                                    let pid = provider_id.clone();
                                                    let mid = model_id.clone();
                                                    let pid_active = pid.clone();
                                                    let mid_active = mid.clone();
                                                    let is_active = move || {
                                                        matches!(
                                                            chat.selected_model.get(),
                                                            Some(ModelOverride::Qualified { provider, model })
                                                                if provider == pid_active && model == mid_active
                                                        )
                                                    };
                                                    let is_retired = retired.as_deref() == Some(mid.as_str());
                                                    let title = if is_retired {
                                                        retired_note.clone().map_or_else(
                                                            || "Retired by the vendor".to_string(),
                                                            |s| format!("Retired by the vendor — use {s}"),
                                                        )
                                                    } else {
                                                        String::new()
                                                    };
                                                    let display_text = model_id;
                                                    view! {
                                                        <button
                                                            title=title
                                                            on:click=move |_| select_entry(pid.clone(), mid.clone())
                                                            class=move || {
                                                                let base = "w-full text-left px-2.5 py-1.5 rounded-md \
                                                                            text-xs font-mono transition-colors \
                                                                            border";
                                                                if is_active() {
                                                                    format!("{base} bg-primary/10 text-primary border-primary/30")
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
                                                    }
                                                }).collect::<Vec<_>>()}
                                            </div>
                                        }
                                    }
                                />
                            }.into_any()
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
pub(crate) fn roster(entry: &CatalogEntry) -> Vec<String> {
    entry.roster.clone()
}

/// Order-preserving substring filter over the provider catalog.
///
/// Ported from hermes-agent's desktop model-picker, which sets cmdk's
/// `shouldFilter={false}` and does a manual substring match. The reason is
/// load-bearing: `providers.catalog` already returns providers (and each
/// provider's models) in a *curated* order — default-first, verified-first.
/// Fuzzy-ranking by match score would shuffle that into near-alphabetical
/// noise, so we deliberately keep the original order and only drop non-matches.
///
/// Matching rules:
/// * A provider whose `display_name` **or** `id` contains the query keeps all
///   of its models (the whole provider matched).
/// * Otherwise the provider is kept only if at least one model id matches, and
///   only the matching models are retained.
/// * Each kept entry's `models` is normalised to the list the picker actually
///   renders (see [`roster`]), so the view's fallback branch never has to
///   re-resolve it.
///
/// An empty or whitespace-only query returns the catalog untouched (clones),
/// so the no-filter render stays behaviourally identical to before.
pub(crate) fn filter_catalog(entries: &[CatalogEntry], query: &str) -> Vec<CatalogEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter_map(|e| {
            // Same resolution the view uses for the rendered model list.
            let models = roster(e);
            let provider_match =
                e.display_name.to_lowercase().contains(&q) || e.id.to_lowercase().contains(&q);
            if provider_match {
                return Some(CatalogEntry {
                    models,
                    ..e.clone()
                });
            }
            let matched: Vec<String> = models
                .into_iter()
                .filter(|m| m.to_lowercase().contains(&q))
                .collect();
            if matched.is_empty() {
                None
            } else {
                Some(CatalogEntry {
                    models: matched,
                    ..e.clone()
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, display: &str, default_model: &str, models: &[&str]) -> CatalogEntry {
        // Mirror the backend roster contract for fixtures: operator models,
        // else the preset default, else empty (BYO relay). The merge rules
        // themselves are core-side and tested there.
        let roster = if !models.is_empty() {
            models.iter().map(|m| m.to_string()).collect()
        } else if default_model.is_empty() {
            Vec::new()
        } else {
            vec![default_model.to_string()]
        };
        CatalogEntry {
            id: id.to_string(),
            display_name: display.to_string(),
            default_model: default_model.to_string(),
            base_url: String::new(),
            protocol: String::new(),
            color: String::new(),
            homepage: None,
            notes: None,
            modalities: Vec::new(),
            models: models.iter().map(|m| m.to_string()).collect(),
            fallback_models: Vec::new(),
            roster,
            has_api_key: false,
            verified: false,
            enabled: false,
            is_default: false,
            lifecycle: crate::api::providers::ModelLifecycle::default(),
            requires_explicit_model: false,
        }
    }

    fn catalog() -> Vec<CatalogEntry> {
        vec![
            entry(
                "openai",
                "OpenAI",
                "gpt-4o",
                &["gpt-4o", "gpt-4o-mini", "o1"],
            ),
            entry(
                "anthropic",
                "Anthropic",
                "claude-sonnet-4-6",
                &["claude-opus-4-8", "claude-sonnet-4-6"],
            ),
            entry("local", "Ollama", "llama3", &[]),
        ]
    }

    #[test]
    fn empty_query_returns_all_untouched() {
        let cat = catalog();
        let out = filter_catalog(&cat, "");
        assert_eq!(out.len(), 3);
        // Whitespace-only behaves the same.
        assert_eq!(filter_catalog(&cat, "   ").len(), 3);
        // Order preserved exactly.
        assert_eq!(out[0].id, "openai");
        assert_eq!(out[2].id, "local");
    }

    #[test]
    fn provider_name_match_keeps_all_models() {
        let out = filter_catalog(&catalog(), "anthropic");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "anthropic");
        assert_eq!(out[0].models.len(), 2);
    }

    #[test]
    fn model_substring_keeps_only_matching_models() {
        let out = filter_catalog(&catalog(), "mini");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "openai");
        assert_eq!(out[0].models, vec!["gpt-4o-mini".to_string()]);
    }

    #[test]
    fn preserves_curated_order_no_fuzzy_resort() {
        // "4" matches a model in both openai (gpt-4o*) and anthropic
        // (claude-*-4-*). Result must keep catalog order: openai before
        // anthropic, never re-sorted by score.
        let out = filter_catalog(&catalog(), "4");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "openai");
        assert_eq!(out[1].id, "anthropic");
    }

    #[test]
    fn empty_models_falls_back_to_default_model() {
        // "llama" only appears in the default_model of the models-less entry.
        let out = filter_catalog(&catalog(), "llama");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "local");
        assert_eq!(out[0].models, vec!["llama3".to_string()]);
    }

    #[test]
    fn no_match_returns_empty() {
        assert!(filter_catalog(&catalog(), "zzz-nonexistent").is_empty());
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(filter_catalog(&catalog(), "OPENAI").len(), 1);
        assert_eq!(filter_catalog(&catalog(), "OpEnAi").len(), 1);
    }

    #[test]
    fn roster_is_the_backend_field_rendered_verbatim() {
        // The merge semantics (operator list vs curated rungs, base_url-moved
        // guard) are core-side in `presets::model_ladder`; the picker must not
        // re-derive them. Whatever the backend sent is what renders — even an
        // empty roster for a bring-your-own-model relay.
        let mut e = entry("anthropic", "Anthropic", "claude-sonnet-5", &[]);
        e.roster = vec!["claude-sonnet-5".to_string(), "claude-opus-4-8".to_string()];
        assert_eq!(
            roster(&e),
            vec!["claude-sonnet-5".to_string(), "claude-opus-4-8".to_string()]
        );

        let byo = entry("replicate", "Replicate", "", &[]);
        assert!(roster(&byo).is_empty());
    }

    #[test]
    fn filter_searches_the_curated_roster_too() {
        let mut e = entry("anthropic", "Anthropic", "claude-sonnet-5", &[]);
        e.roster = vec!["claude-sonnet-5".to_string(), "claude-opus-4-8".to_string()];
        let out = filter_catalog(&[e], "opus");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].models, vec!["claude-opus-4-8".to_string()]);
    }
}
