//! Generation Providers settings view — TTS / image / video / audio providers.
//!
//! ## Layout
//! - this module — list / cards / category tabs + the main `GenerationProvidersView`
//! - [`picker`] — the "add a provider" disclosure and the panel/picker
//!   partition. The 44 presets used to render as cards behind five tabs, which
//!   put 14 of them in front of an operator looking for the one they had set up
//! - [`detail_view`] — `ProviderDetailView` for a configured provider
//! - [`preset_setup`] — `PresetSetupPanel` for unconfigured presets
//! - [`add_custom`] — `AddCustomProviderPanel` for non-preset providers
//! - [`settings_panel`] — `GenerationSettingsPanel` (thresholds + routing)

mod add_custom;
mod detail_view;
mod picker;
mod preset_setup;
mod settings_panel;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::{GenerationProviderEntry, GenerationProvidersApi};
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::context::DashboardState;
use crate::generation::GenerationType;
use crate::i18n::{t, use_i18n};
use crate::preset_providers::{PresetCatalog, PresetProvider};

use add_custom::AddCustomProviderPanel;
use detail_view::ProviderDetailView;
use picker::CategoryPicker;
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
    // Whether the "add a provider" disclosure is expanded. Owned here because
    // the first-load seed below has to reach it.
    let picker_open = RwSignal::new(false);
    // Generation settings are page-level, not a provider row, and expanded they
    // are three controls and a save button tall. That was invisible while the
    // panel above them was 14 preset cards deep; with the panel listing only
    // what the operator configured, leaving them open makes the page look like
    // its subject is thresholds. Collapsed by default, one click away.
    let settings_open = RwSignal::new(false);
    // Seed it open **once**, after the first load, when the operator has no
    // generation providers at all — otherwise a fresh install renders a left
    // panel holding one collapsed button. A seed rather than a derived
    // predicate: a signal that recomputed would snap back open every time the
    // operator closed it while still configuring their first provider.
    let seeded = RwSignal::new(false);
    Effect::new(move |_| {
        if is_loading.get() || seeded.get_untracked() {
            return;
        }
        seeded.set(true);
        if providers.get_untracked().is_empty() {
            picker_open.set(true);
        }
    });

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

            // Auto-select a provider on first load so the detail pane shows
            // content instead of the EmptyState: the category's default, else
            // any provider configured in it.
            //
            // Deliberately no fall-back to the category's first *preset*. That
            // used to be right, because the panel rendered every preset as a
            // card and the fallback simply pre-selected the top one. The panel
            // now lists configured rows only, so the same fallback would open a
            // setup form for a provider that appears nowhere on the left — and
            // the case it fired in, nothing configured, is exactly the case the
            // picker seeds itself open for.
            //
            // Post-`.await` — same shape, same hazard, and the same fix as the
            // `providers` and `embedding_providers` views (see
            // `crate::disposed_reads`). Three reads here, one probe: they share
            // an owner, so if the first survives so do the rest.
            let (Some(current), Some(cat), Some(prov)) = (
                selected_provider_id.try_get_untracked(),
                selected_category.try_get_untracked(),
                providers.try_get_untracked(),
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
                    .map(|p| p.name.clone());
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

    // The presets this category lists: configured only. The rest are one click
    // away in the picker — see [`picker`] for why this catalogue is safe to
    // collapse and the embedding one is not.
    let current_presets =
        move || picker::listed(&catalog.get(), &providers.get(), selected_category.get());

    // The custom (non-preset) providers in the selected category. No filter:
    // the panel now holds only rows the operator set up themselves, which is a
    // handful, and the search box moved into the picker — the surface with a
    // catalogue to sift.
    let current_custom = move || {
        let preset_ids: Vec<String> = catalog
            .get()
            .by_category(selected_category.get())
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let current_cat = selected_category.get();
        providers
            .get()
            .into_iter()
            .filter(|p| {
                !preset_ids.contains(&p.name) && p.effective_generation_type() == Some(current_cat)
            })
            .collect::<Vec<GenerationProviderEntry>>()
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
                            // Read once: the `Show` guard and the grid below
                            // must agree, and re-deriving would let a refetch
                            // between them render a heading over nothing.
                            let has_presets = !presets.is_empty();
                            view! {
                                <div class="p-6 space-y-4">
                                    // Add a provider — button + the searchable
                                    // catalogue it reveals, scoped to the tab.
                                    // Top of the panel because it is the
                                    // action; the sections below are content.
                                    <CategoryPicker
                                        catalog=catalog
                                        providers=providers
                                        category=selected_category
                                        selected=set_selected_provider_id
                                        show_add_form=set_show_add_form
                                        open=picker_open
                                    />

                                    <Show when=move || has_presets>
                                        <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider">
                                            {t!(i18n, settings.generation.configured_providers)}
                                        </h2>
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

                    // Generation settings — page-level, so they sit below the
                    // provider sections behind a disclosure rather than
                    // competing with them. Outside the loading/error block on
                    // purpose: these settings come from a different RPC and are
                    // still reachable when the provider list fails to load.
                    <div class="px-6 pb-6">
                        <button
                            on:click=move |_| {
                                let next = !settings_open.get_untracked();
                                settings_open.set(next);
                            }
                            class="w-full flex items-center gap-2 border-t border-border pt-6 text-left text-lg font-semibold text-text-primary hover:text-primary transition-colors"
                        >
                            <svg class="w-4 h-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2"
                                    d=move || if settings_open.get() { "M19 9l-7 7-7-7" } else { "M9 5l7 7-7 7" } />
                            </svg>
                            {t!(i18n, settings.generation.generation_settings)}
                        </button>
                        // Hidden, not unmounted. `Show` would drop the panel
                        // and take any unsaved slider with it — collapsing a
                        // section is not a discard, and there is nothing on
                        // screen to say one happened. The save button stays
                        // with its controls on purpose: alone under a collapsed
                        // section it would offer to save what you cannot see.
                        <div class="mt-4 space-y-4" class:hidden=move || !settings_open.get()>
                            <GenerationSettingsPanel />
                        </div>
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
            icon_glyph=Some(icon)
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
