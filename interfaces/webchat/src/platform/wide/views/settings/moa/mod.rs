//! Panel MoA (Mixture-of-Agents) settings page — visual preset list + editor.
//!
//! ## Layout
//! - this module — `MoaView` (preset cards + global `save_traces` toggle)
//! - [`options`] — pure model-option dedup logic (unit-tested)
//! - [`preset_editor`] — `MoaPresetEditor` form for creating/editing one preset

mod options;
mod preset_editor;

pub use options::{available_options, SlotOption};

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::moa::{MoaApi, MoaConfigDto, MoaPresetDto, MoaSlotDto};
use crate::api::{CatalogEntry, CatalogView, ProvidersApi};
use crate::context::DashboardState;

use preset_editor::MoaPresetEditor;

/// What the right-hand panel currently shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorTarget {
    New,
    Existing(String),
}

#[component]
#[must_use]
pub fn MoaView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    let config = RwSignal::new(MoaConfigDto::default());
    let catalog = RwSignal::new(Vec::<CatalogEntry>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let editing = RwSignal::new(Option::<EditorTarget>::None);

    let reload = move || {
        spawn_local(async move {
            match MoaApi::list_presets(&state).await {
                Ok(cfg) => config.set(cfg),
                Err(e) => error.set(Some(format!("Failed to reload MoA config: {e}"))),
            }
        });
    };

    Effect::new(move |_| {
        if !state.is_connected.get() {
            return;
        }
        spawn_local(async move {
            let (cfg_res, cat_res) = futures::future::join(
                MoaApi::list_presets(&state),
                ProvidersApi::catalog(&state, CatalogView::Configured),
            )
            .await;
            match cfg_res {
                Ok(cfg) => config.set(cfg),
                Err(e) => error.set(Some(format!("Failed to load MoA config: {e}"))),
            }
            match cat_res {
                Ok(list) => catalog.set(list),
                Err(e) => error.set(Some(format!("Failed to load model catalog: {e}"))),
            }
            loading.set(false);
        });
    });

    // Distinct (provider, model) pairs available across configured providers —
    // an enabled preset needs at least one advisor plus a distinct aggregator,
    // i.e. at least 2 distinct models.
    let configured_model_count = move || {
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for entry in catalog.get().iter().filter(|e| e.enabled && e.has_api_key) {
            for model in &entry.models {
                seen.insert((entry.id.clone(), model.clone()));
            }
        }
        seen.len()
    };

    let toggle_save_traces = move |_| {
        let next = !config.get().save_traces;
        spawn_local(async move {
            match MoaApi::set_save_traces(&state, next).await {
                Ok(()) => config.update(|c| c.save_traces = next),
                Err(e) => error.set(Some(format!("Failed to update save_traces: {e}"))),
            }
        });
    };

    let set_default = move |name: String| {
        spawn_local(async move {
            match MoaApi::set_default(&state, &name).await {
                Ok(()) => reload(),
                Err(e) => error.set(Some(format!("Failed to set default preset: {e}"))),
            }
        });
    };

    let delete_preset = move |name: String| {
        spawn_local(async move {
            match MoaApi::delete_preset(&state, &name).await {
                Ok(()) => reload(),
                Err(e) => error.set(Some(format!("Failed to delete preset: {e}"))),
            }
        });
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto space-y-6">
            <div class="flex items-start justify-between gap-4">
                <div>
                    <h1 class="text-2xl font-semibold text-text-primary">"MoA"</h1>
                    <p class="mt-1 text-sm text-text-secondary">
                        "Mixture-of-Agents: multiple advisor models consult before one aggregator model responds."
                    </p>
                </div>
                <label class="flex items-center gap-2 text-sm text-text-secondary shrink-0">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().save_traces
                        on:change=toggle_save_traces
                        class="w-4 h-4"
                    />
                    "Save advisor traces"
                </label>
            </div>

            {move || error.get().map(|e| view! {
                <div class="p-4 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            {move || {
                if let Some(target) = editing.get() {
                    let (initial_name, initial_preset) = match &target {
                        EditorTarget::New => (None, None),
                        EditorTarget::Existing(name) => {
                            (Some(name.clone()), config.get().presets.get(name).cloned())
                        }
                    };
                    let is_default = matches!(&target, EditorTarget::Existing(name) if config.get().default_preset.as_deref() == Some(name));
                    view! {
                        <MoaPresetEditor
                            catalog=catalog
                            initial_name=initial_name
                            initial_preset=initial_preset
                            initial_is_default=is_default
                            on_saved=move || {
                                editing.set(None);
                                reload();
                            }
                            on_cancel=move || editing.set(None)
                        />
                    }.into_any()
                } else if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12 text-text-tertiary">
                            "Loading…"
                        </div>
                    }.into_any()
                } else {
                    let cfg = config.get();
                    let mut names: Vec<String> = cfg.presets.keys().cloned().collect();
                    names.sort();
                    let can_create = configured_model_count() >= 2;

                    view! {
                        <div class="space-y-4">
                            {if names.is_empty() {
                                view! {
                                    <div class="p-6 bg-surface-raised border border-border rounded-xl text-center text-text-secondary text-sm">
                                        "No MoA presets yet. Create one to enable multi-model consultation."
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="grid grid-cols-1 gap-3">
                                        {names.into_iter().map(|name| {
                                            let preset = cfg.presets.get(&name).cloned();
                                            let Some(preset) = preset else { return view!{ <div></div> }.into_any(); };
                                            let is_default = cfg.default_preset.as_deref() == Some(name.as_str());
                                            let name_edit = name.clone();
                                            let name_default = name.clone();
                                            let name_delete = name.clone();
                                            view! {
                                                <PresetCard
                                                    name=name.clone()
                                                    preset=preset
                                                    is_default=is_default
                                                    on_edit=move || editing.set(Some(EditorTarget::Existing(name_edit.clone())))
                                                    on_set_default=move || set_default(name_default.clone())
                                                    on_delete=move || delete_preset(name_delete.clone())
                                                />
                                            }.into_any()
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }}

                            {if !can_create {
                                view! {
                                    <div class="p-3 bg-warning-subtle border border-warning/20 rounded-lg text-warning text-xs">
                                        "Configure at least 2 models across your providers (Settings → Providers) before creating a MoA preset."
                                    </div>
                                }.into_any()
                            } else {
                                view! { <div></div> }.into_any()
                            }}

                            <button
                                on:click=move |_| editing.set(Some(EditorTarget::New))
                                prop:disabled=move || !can_create
                                class="w-full px-4 py-3 border-2 border-dashed border-border rounded-lg text-text-secondary hover:border-primary hover:text-primary disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:border-border disabled:hover:text-text-secondary transition-colors"
                            >
                                "+ New preset"
                            </button>
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn PresetCard(
    name: String,
    preset: MoaPresetDto,
    is_default: bool,
    on_edit: impl Fn() + 'static + Send,
    on_set_default: impl Fn() + 'static + Send,
    on_delete: impl Fn() + 'static + Send,
) -> impl IntoView {
    let slot_chip = |slot: &MoaSlotDto| format!("{} / {}", slot.provider, slot.model);
    let advisor_chips: Vec<String> = preset.advisors.iter().map(slot_chip).collect();
    let aggregator_chip = slot_chip(&preset.aggregator);
    let enabled = preset.enabled;

    view! {
        <div class="p-4 bg-surface-raised border border-border rounded-xl space-y-3">
            <div class="flex items-center justify-between gap-3">
                <div class="flex items-center gap-2 min-w-0">
                    <span class="font-medium text-text-primary truncate">{name}</span>
                    {if is_default {
                        view! {
                            <span class="px-2 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wider bg-primary/15 text-primary shrink-0">
                                "Default"
                            </span>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }}
                    {if !enabled {
                        view! {
                            <span class="px-2 py-0.5 rounded-full text-[10px] font-bold uppercase tracking-wider bg-surface-sunken text-text-tertiary shrink-0">
                                "Disabled"
                            </span>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }}
                </div>
                <div class="flex items-center gap-2 shrink-0">
                    {if !is_default {
                        view! {
                            <button
                                on:click=move |_| on_set_default()
                                class="text-xs text-primary hover:underline"
                            >
                                "Set default"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }}
                    <button
                        on:click=move |_| on_edit()
                        class="text-xs text-text-secondary hover:text-primary"
                    >
                        "Edit"
                    </button>
                    <button
                        on:click=move |_| on_delete()
                        class="text-xs text-danger hover:underline"
                    >
                        "Delete"
                    </button>
                </div>
            </div>
            <div class="flex flex-wrap gap-1.5 text-xs">
                {advisor_chips.into_iter().map(|chip| view! {
                    <span class="px-2 py-1 rounded bg-info/10 text-info">{chip}</span>
                }).collect_view()}
                <span class="px-2 py-1 rounded bg-primary/10 text-primary">{format!("Σ {aggregator_chip}")}</span>
            </div>
        </div>
    }
}
