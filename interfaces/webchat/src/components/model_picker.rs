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

use leptos::prelude::*;
use leptos::task::spawn_local;

use aleph_protocol::providers::search::filter_catalog;

use crate::api::providers::{CatalogEntry, CatalogView, ModelOverride, ProvidersApi, RosterModel};
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
                                        // backend computed for this entry,
                                        // already narrowed to the matching ids
                                        // by the shared matcher.
                                        let models = roster(&entry);
                                        view! {
                                            <div class="pt-1.5">
                                                <div class="flex items-center gap-1.5 px-2.5 pb-1">
                                                    <span class="w-2 h-2 rounded-full"
                                                          style=format!("background: {}", color) />
                                                    <span class="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                                                        {display}
                                                    </span>
                                                </div>
                                                {models.into_iter().map(|model| {
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
pub(crate) fn roster(entry: &CatalogEntry) -> Vec<RosterModel> {
    entry.roster.clone()
}
