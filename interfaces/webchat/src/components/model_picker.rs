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
use crate::views::chat::state::ChatState;

/// Pill + dropdown for selecting the per-turn chat model.
#[component]
pub fn ModelPicker() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let open = RwSignal::new(false);
    let entries: RwSignal<Vec<CatalogEntry>> = RwSignal::new(Vec::new());
    let loading = RwSignal::new(false);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);

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
        chat.selected_model.set(Some(ModelOverride::Qualified {
            provider,
            model,
        }));
        open.set(false);
    };

    let clear_selection = move || {
        chat.selected_model.set(None);
        open.set(false);
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
                       hover:bg-surface-sunken hover:text-text-primary transition-colors"
                title="Pick model for this turn"
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

            // Click-outside catcher
            {move || open.get().then(|| view! {
                <div class="fixed inset-0 z-40" on:click=move |_| open.set(false) />
            })}

            <Show when=move || open.get()>
                <div class="absolute bottom-full mb-2 left-0 z-50 w-80 max-h-96 overflow-y-auto
                            rounded-xl border border-border bg-surface-overlay/95 shadow-xl
                            backdrop-blur-md p-2 space-y-1">
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
                                    "loading catalog…"
                                </div>
                            }.into_any()
                        } else if let Some(err) = load_error.get() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-danger/80 text-center">
                                    {format!("error: {}", err)}
                                </div>
                            }.into_any()
                        } else if entries.get().is_empty() {
                            view! {
                                <div class="px-2.5 py-3 text-xs text-text-tertiary text-center">
                                    "no configured providers. Add a key in Settings → Providers."
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <For
                                    each=move || entries.get()
                                    key=|e: &CatalogEntry| e.id.clone()
                                    children=move |entry: CatalogEntry| {
                                        let provider_id = entry.id.clone();
                                        let display = entry.display_name.clone();
                                        let color = entry.color.clone();
                                        // Models to show: user-extended list if any, otherwise the default.
                                        let models = if entry.models.is_empty() {
                                            vec![entry.default_model.clone()]
                                        } else {
                                            entry.models.clone()
                                        };
                                        view! {
                                            <div class="pt-1.5">
                                                <div class="flex items-center gap-1.5 px-2.5 pb-1">
                                                    <span class="w-2 h-2 rounded-full"
                                                          style=format!("background: {}", color) />
                                                    <span class="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary">
                                                        {display.clone()}
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
                                                    let display_text = model_id.clone();
                                                    view! {
                                                        <button
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
