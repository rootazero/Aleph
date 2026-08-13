//! Generation Providers settings view — TTS / image / video / audio providers.
//!
//! ## Layout
//! - this module — list / cards / category tabs + the main `GenerationProvidersView`
//! - [`detail_view`] — `ProviderDetailView` for a configured provider
//! - [`preset_setup`] — `PresetSetupPanel` for unconfigured presets
//! - [`add_custom`] — `AddCustomProviderPanel` for non-preset providers
//! - [`settings_panel`] — `GenerationSettingsPanel` (thresholds + routing)

mod add_custom;
mod detail_view;
mod preset_setup;
mod settings_panel;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{GenerationProviderEntry, GenerationProvidersApi};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::context::DashboardState;
use crate::generation::GenerationType;
use crate::i18n::{t, t_string, use_i18n};
use crate::preset_providers::{PresetCatalog, PresetProvider};

use add_custom::AddCustomProviderPanel;
use detail_view::ProviderDetailView;
use preset_setup::PresetSetupPanel;
use settings_panel::GenerationSettingsPanel;

/// Extract base URL from a potentially full endpoint URL.
///
/// If the URL contains a versioned API path (`/v1/`, `/v2/`, etc.),
/// strip everything from the version segment onward. Otherwise return as-is.
///
/// Examples:
/// - `https://ai.t8star.cn/v1/audio/speech` → `https://ai.t8star.cn`
/// - `https://ai.t8star.cn/v2/videos/generations` → `https://ai.t8star.cn`
/// - `https://ai.t8star.cn` → `https://ai.t8star.cn`
/// - `https://api.openai.com/v1/images/generations` → `https://api.openai.com`
pub(super) fn extract_base_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    // Find the position of /v{digit}/ pattern
    let bytes = url.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        if bytes[i] == b'/' && bytes[i + 1] == b'v' && bytes[i + 2].is_ascii_digit() {
            // Check if it's followed by / or another digit + / (or end-of-string)
            let mut j = i + 3;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j == bytes.len() || bytes[j] == b'/' {
                return url[..i].to_string();
            }
        }
    }
    url.to_string()
}

#[component]
#[must_use]
pub fn GenerationProvidersView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // State
    let (providers, set_providers) = signal(Vec::<GenerationProviderEntry>::new());
    let (catalog, set_catalog) = signal(PresetCatalog::default());
    let (selected_category, set_selected_category) = signal(GenerationType::Image);
    let (selected_provider_id, set_selected_provider_id) = signal(Option::<String>::None);
    let (show_add_form, set_show_add_form) = signal(false);
    let (is_loading, set_is_loading) = signal(true);
    let (error_message, set_error_message) = signal(Option::<String>::None);
    // Live filter over the rows already in hand. 44 presets across five
    // category tabs is past the point where scrolling is the answer, and the
    // matcher is the shared one — so a query here ranks exactly the way the
    // same query ranks in the chat provider list and the TUI picker.
    let (search, set_search) = signal(String::new());

    // (Re)load providers + preset catalogue whenever the gateway is connected.
    // Subscribes to `is_connected` so a server restart (the only realistic catalog
    // mutation point — PRESETS is a Lazy<HashMap>, compile-time fixed) auto-refreshes
    // the cards. Disconnected ticks are no-ops; the loader stays visible.
    Effect::new(move |_| {
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            let (providers_res, presets_res) = futures::future::join(
                GenerationProvidersApi::list(&state),
                GenerationProvidersApi::list_presets(&state),
            )
            .await;
            match providers_res {
                Ok(list) => set_providers.set(list),
                Err(e) => set_error_message.set(Some(
                    crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                        format!("Failed to load providers: {e}")
                    }),
                )),
            }
            match presets_res {
                Ok(rows) => set_catalog.set(PresetCatalog::from_rows(rows)),
                Err(e) => set_error_message.set(Some(
                    crate::components::admin_refusal::settings_load_error(i18n, &e, |e| {
                        format!("Failed to load preset catalog: {e}")
                    }),
                )),
            }

            // Auto-select a default card on first load so the detail pane shows content
            // instead of the EmptyState (mirrors Embedding/Reranking). Prefer a configured
            // provider in the current category; otherwise fall back to its first preset card,
            // using the same configured-vs-preset id convention as the cards' on_click.
            // Post-`.await` — same shape, same hazard, and the same fix as the
            // `providers` and `embedding_providers` views (see
            // `crate::disposed_reads`). Four reads here, one probe: they share
            // an owner, so if the first survives so do the rest.
            let (Some(current), Some(cat), Some(prov), Some(cards)) = (
                selected_provider_id.try_get_untracked(),
                selected_category.try_get_untracked(),
                providers.try_get_untracked(),
                catalog.try_get_untracked(),
            ) else {
                return;
            };
            if current.is_none() {
                let pick = prov
                    .iter()
                    .filter(|p| p.effective_generation_type() == Some(cat))
                    .find(|p| !p.is_default_for.is_empty())
                    .or_else(|| {
                        prov.iter()
                            .find(|p| p.effective_generation_type() == Some(cat))
                    })
                    .map(|p| p.name.clone())
                    .or_else(|| {
                        cards.by_category(cat).first().map(|first| {
                            let id = first.id.clone();
                            if prov.iter().any(|p| p.name == id) {
                                id
                            } else {
                                format!("__preset__{id}")
                            }
                        })
                    });
                if let Some(sel) = pick {
                    set_selected_provider_id.set(Some(sel));
                }
            }

            set_is_loading.set(false);
        });
    });

    // Reload helper (configured providers only; preset catalog is static for the session)
    let reload = move || {
        spawn_local(async move {
            if let Ok(list) = GenerationProvidersApi::list(&state).await {
                set_providers.set(list);
            }
        });
    };

    // Get current category presets, narrowed by the search box.
    //
    // Category first, then the ranker: a query must never pull a video preset
    // into the image tab. An empty query is a no-op inside the matcher, so
    // this is the unconditional path rather than a branch on "is it empty".
    let current_presets = move || {
        catalog
            .get()
            .by_category_matching(selected_category.get(), &search.get())
    };

    // The custom (non-preset) providers in the selected category, after the
    // same filter. A free function of the signals rather than an inline block,
    // because the empty state has to know whether *either* list has rows —
    // telling the operator "nothing matches" above a list of matches is worse
    // than not having an empty state at all.
    let current_custom = move || {
        // Exclusion over the whole category, never the filtered view: a preset
        // the search hid is still a preset.
        let preset_ids: Vec<String> = catalog
            .get()
            .by_category(selected_category.get())
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let current_cat = selected_category.get();
        let owned: Vec<GenerationProviderEntry> = providers
            .get()
            .into_iter()
            .filter(|p| {
                !preset_ids.contains(&p.name) && p.effective_generation_type() == Some(current_cat)
            })
            .collect();
        aleph_protocol::providers::filter_rows(&owned, &search.get())
    };

    // Check if a preset is configured
    let is_configured = move |preset_id: &str| providers.get().iter().any(|p| p.name == preset_id);

    // Get provider entry for a preset
    let get_provider_entry =
        move |preset_id: &str| providers.get().into_iter().find(|p| p.name == preset_id);

    view! {
        <div class="flex h-full aleph-content-top aleph-md">
            // Left panel - Provider list + Generation settings
            <div class="flex flex-col w-5/12 min-w-[400px] border-r border-border aleph-md-list">
                // Header
                <div class="px-6 pb-4 border-b border-border">
                    <h1 class="text-2xl font-semibold text-text-primary">
                        {t!(i18n, settings.generation.title)}
                    </h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        {t!(i18n, settings.generation.description)}
                    </p>
                </div>

                // Category Tabs
                <div class="px-6 py-3 border-b border-border">
                    <div class="flex gap-2">
                        <CategoryTab
                            category=GenerationType::Image
                            selected=selected_category
                            on_select=set_selected_category
                        />
                        <CategoryTab
                            category=GenerationType::Video
                            selected=selected_category
                            on_select=set_selected_category
                        />
                        <CategoryTab
                            category=GenerationType::Audio
                            selected=selected_category
                            on_select=set_selected_category
                        />
                        <CategoryTab
                            category=GenerationType::Speech
                            selected=selected_category
                            on_select=set_selected_category
                        />
                        <CategoryTab
                            category=GenerationType::Transcription
                            selected=selected_category
                            on_select=set_selected_category
                        />
                    </div>
                </div>

                // Search — filters the preset rows already in hand, within the
                // selected category. Same matcher as the chat provider list.
                <div class="px-6 py-3 border-b border-border">
                    <input
                        type="text"
                        prop:value=move || search.get()
                        on:input=move |ev| set_search.set(event_target_value(&ev))
                        placeholder=move || t_string!(i18n, settings.generation.search_placeholder).to_string()
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                    />
                </div>

                // Content
                <div class="flex-1 overflow-auto">
                    // Provider cards (loading/error/list)
                    {move || {
                        if is_loading.get() {
                            view! {
                                <div class="flex items-center justify-center py-12">
                                    <div class="text-text-tertiary">{t!(i18n, settings.generation.loading_providers)}</div>
                                </div>
                            }.into_any()
                        } else if let Some(error) = error_message.get() {
                            view! {
                                <div class="p-6">
                                    <div class="p-4 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">{error}</div>
                                </div>
                            }.into_any()
                        } else {
                            let presets = current_presets();
                            // "Nothing matched" only when *neither* list has a
                            // row. A message above a populated custom section
                            // would be worse than no message.
                            let nothing_matched = presets.is_empty() && current_custom().is_empty();
                            view! {
                                <div class="p-6 space-y-4">
                                    <Show when=move || nothing_matched>
                                        <div class="py-8 text-center text-sm text-text-tertiary">
                                            {t!(i18n, settings.generation.no_search_match)}
                                        </div>
                                    </Show>
                                    <div class="grid grid-cols-1 gap-2">
                                        {presets.clone().into_iter().map(|preset| {
                                            let preset_id = preset.id.clone();
                                            let configured = is_configured(&preset_id);
                                            let entry = get_provider_entry(&preset_id);
                                            let is_selected = {
                                                let sel = selected_provider_id.get();
                                                sel.as_deref() == Some(&preset_id)
                                                    || sel.as_deref() == Some(&format!("__preset__{preset_id}"))
                                            };

                                            view! {
                                                <ProviderCard
                                                    preset=preset
                                                    is_configured=configured
                                                    entry=entry
                                                    is_selected=is_selected
                                                    on_click=move || {
                                                        // Configured preset → show detail; unconfigured → show setup form
                                                        if configured {
                                                            set_selected_provider_id.set(Some(preset_id.clone()));
                                                        } else {
                                                            set_selected_provider_id.set(Some(format!("__preset__{preset_id}")));
                                                        }
                                                        set_show_add_form.set(false);
                                                    }
                                                />
                                            }
                                        }).collect_view()}
                                    </div>

                                    // Custom providers (not matching any preset in current category)
                                    {move || {
                                        let custom = current_custom();
                                        if custom.is_empty() {
                                            view! { <div></div> }.into_any()
                                        } else {
                                            view! {
                                                <div class="pt-2">
                                                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                                                        {t!(i18n, settings.generation.custom_providers)}
                                                    </h2>
                                                    <div class="grid grid-cols-1 gap-2">
                                                        {custom.into_iter().map(|cp| {
                                                            let cp_name = cp.name.clone();
                                                            let cp_name_click = cp_name.clone();
                                                            let cp_name_check = cp_name.clone();
                                                            let cp_model = cp.config.models.first().cloned().unwrap_or_default();
                                                            let cp_color = cp.config.color.clone();
                                                            let is_default = !cp.is_default_for.is_empty();
                                                            let verified = cp.config.verified;

                                                            view! {
                                                                <ProviderRowCard
                                                                    name=cp_name
                                                                    icon_color=cp_color
                                                                    subtitle=cp_model
                                                                    is_selected=move || {
                                                                        selected_provider_id.get().as_deref() == Some(&cp_name_check)
                                                                    }
                                                                    is_configured=|| true
                                                                    dot=move || if verified { RowDot::Verified } else { RowDot::None }
                                                                    badge=move || {
                                                                        let state = BadgeState { is_default, verified };
                                                                        view! { <ProviderBadges state=state /> }.into_any()
                                                                    }
                                                                    on_click=move || {
                                                                        set_selected_provider_id.set(Some(cp_name_click.clone()));
                                                                        set_show_add_form.set(false);
                                                                    }
                                                                />
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                </div>
                                            }.into_any()
                                        }
                                    }}

                                    // Add Custom Provider button
                                    <div class="pt-2">
                                        <button
                                            on:click=move |_| {
                                                set_show_add_form.set(true);
                                                set_selected_provider_id.set(None);
                                            }
                                            class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary transition-colors"
                                        >
                                            {t!(i18n, settings.generation.add_custom)}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}

                    // Generation Settings (always visible, independent of provider loading)
                    <div class="px-6 pb-6 space-y-4">
                        <h2 class="text-lg font-semibold text-text-primary border-t border-border pt-6">
                            {t!(i18n, settings.generation.generation_settings)}
                        </h2>
                        <GenerationSettingsPanel />
                    </div>
                </div>
            </div>

            // Right panel - Provider details or Add form
            <div class="w-7/12 min-w-[320px] bg-surface aleph-md-detail">
                {move || {
                    if show_add_form.get() {
                        view! {
                            <AddCustomProviderPanel
                                category=selected_category.get()
                                on_added=move || {
                                    set_show_add_form.set(false);
                                    reload();
                                }
                                on_cancel=move || set_show_add_form.set(false)
                            />
                        }.into_any()
                    } else {
                        view! {
                            <ProviderDetailPanel
                                selected_id=selected_provider_id
                                providers=providers
                                catalog=catalog
                                on_reload=move || reload()
                            />
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

#[component]
fn CategoryTab(
    category: GenerationType,
    selected: ReadSignal<GenerationType>,
    on_select: WriteSignal<GenerationType>,
) -> impl IntoView {
    let is_selected = move || selected.get() == category;

    view! {
        <button
            class=move || {
                let base = "flex-1 flex flex-col items-center gap-1 px-3 py-2 rounded-lg font-medium transition-colors text-sm";
                if is_selected() {
                    format!("{base} bg-info text-white")
                } else {
                    format!("{base} bg-surface-raised text-text-secondary hover:bg-surface-sunken")
                }
            }
            on:click=move |_| on_select.set(category)
        >
            <span class="text-lg">{category.icon()}</span>
            <span>{category.display_name()}</span>
        </button>
    }
}

#[component]
fn ProviderCard(
    preset: PresetProvider,
    is_configured: bool,
    entry: Option<GenerationProviderEntry>,
    is_selected: bool,
    on_click: impl Fn() + 'static + Send,
) -> impl IntoView {
    let is_verified = entry.as_ref().is_some_and(|e| e.config.verified);
    let is_default = entry.as_ref().is_some_and(|e| !e.is_default_for.is_empty());

    let color = preset.color.clone();
    let name = preset.name.clone();
    let model = preset.default_model.clone();
    let icon = preset.icon;

    view! {
        <ProviderRowCard
            name=name
            icon_color=color
            icon_glyph=icon
            subtitle=model
            is_selected=move || is_selected
            is_configured=move || is_configured
            dot=move || if is_configured && is_verified { RowDot::Verified } else { RowDot::None }
            badge=move || {
                let state = BadgeState {
                    is_default: is_configured && is_default,
                    verified: is_configured && is_verified,
                };
                view! { <ProviderBadges state=state /> }.into_any()
            }
            on_click=on_click
        />
    }
}

#[component]
fn ProviderDetailPanel(
    selected_id: ReadSignal<Option<String>>,
    providers: ReadSignal<Vec<GenerationProviderEntry>>,
    catalog: ReadSignal<PresetCatalog>,
    on_reload: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let _state = expect_context::<DashboardState>();

    view! {
        <div class="h-full">
            {move || {
                if let Some(provider_id) = selected_id.get() {
                    // Unconfigured preset → show add form pre-filled with preset info
                    if let Some(preset_name) = provider_id.strip_prefix("__preset__") {
                        let preset = catalog.get().find(preset_name);
                        if let Some(preset) = preset {
                            return view! {
                                <PresetSetupPanel
                                    preset=preset
                                    on_added=move || on_reload()
                                />
                            }.into_any();
                        }
                    }

                    // Configured provider → show editable detail
                    let provider = providers.get().into_iter()
                        .find(|p| p.name == provider_id);

                    if let Some(provider) = provider {
                        view! {
                            <ProviderDetailView
                                provider=provider
                                catalog=catalog
                                on_reload=on_reload
                            />
                        }.into_any()
                    } else {
                        view! {
                            <EmptyState />
                        }.into_any()
                    }
                } else {
                    view! {
                        <EmptyState />
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn EmptyState() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex flex-1 items-center justify-center h-full">
            <div class="text-center text-text-secondary">
                <p class="text-lg">{t!(i18n, settings.generation.select_provider)}</p>
            </div>
        </div>
    }
}
