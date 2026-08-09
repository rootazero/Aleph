//! Panel MoA (Mixture-of-Agents) settings page — visual preset list + editor.
//!
//! ## Layout
//! - this module — `MoaView` (preset cards + global `save_traces` toggle)
//! - [`options`] — pure model-option dedup logic (unit-tested)
//! - [`preset_editor`] — `MoaPresetEditor` form for creating/editing one preset

mod options;
mod preset_editor;

use std::collections::HashSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;

use crate::api::moa::{MoaApi, MoaConfigDto, MoaPresetDto, MoaSlotDto};
use crate::api::{CatalogEntry, CatalogView, ModelOverride, ProvidersApi};
use crate::context::DashboardState;
use crate::views::chat::state::ChatState;

use preset_editor::MoaPresetEditor;

/// What the right-hand panel currently shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EditorTarget {
    New,
    /// Prefill the editor from an existing preset but save under a new name.
    Clone(String),
    Existing(String),
}

#[component]
#[must_use]
pub fn MoaView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    // Only consumer so far is the refused-load copy below; this page's own
    // strings are still hard-coded English.
    let i18n = crate::i18n::use_i18n();
    // B1 "Use in chat": arm the preset on the chat session's model selector,
    // then navigate to chat. `ChatState` is provided at app root (Copy);
    // `navigate` is stored so per-card callbacks can invoke it.
    let chat = expect_context::<ChatState>();
    let navigate = StoredValue::new(use_navigate());

    let config = RwSignal::new(MoaConfigDto::default());
    let catalog = RwSignal::new(Vec::<CatalogEntry>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(Option::<String>::None);
    let editing = RwSignal::new(Option::<EditorTarget>::None);

    let reload = move || {
        spawn_local(async move {
            match MoaApi::list_presets(&state).await {
                Ok(cfg) => config.set(cfg),
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to reload MoA config: {e}"),
                ))),
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
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load MoA config: {e}"),
                ))),
            }
            match cat_res {
                Ok(list) => catalog.set(list),
                Err(e) => error.set(Some(crate::components::admin_refusal::settings_load_error(
                    i18n,
                    &e,
                    |e| format!("Failed to load model catalog: {e}"),
                ))),
            }
            loading.set(false);
        });
    });

    // Distinct (provider, model) pairs available across configured providers —
    // an enabled preset needs at least one advisor plus a distinct aggregator,
    // i.e. at least 2 distinct models.
    let configured_model_count = move || {
        let mut seen: HashSet<(String, String)> = HashSet::new();
        // Exclude the synthetic `moa` pseudo-provider row so its preset-name
        // "models" can't satisfy the >= 2 create-gate (mirrors options.rs).
        for entry in catalog
            .get()
            .iter()
            .filter(|e| e.enabled && e.has_api_key && e.id != "moa")
        {
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
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to update save_traces: {e}")
                    }),
                )),
            }
        });
    };

    let set_default = move |name: String| {
        spawn_local(async move {
            match MoaApi::set_default(&state, &name).await {
                Ok(()) => reload(),
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to set default preset: {e}")
                    }),
                )),
            }
        });
    };

    let delete_preset = move |name: String| {
        spawn_local(async move {
            match MoaApi::delete_preset(&state, &name).await {
                Ok(()) => reload(),
                Err(e) => error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to delete preset: {e}")
                    }),
                )),
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
                        // Clone prefills the form from `name` but leaves the name
                        // blank (is_new) so Save creates a fresh preset.
                        EditorTarget::Clone(name) => {
                            (None, config.get().presets.get(name).cloned())
                        }
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
                                            let name_dup = name.clone();
                                            let name_default = name.clone();
                                            let name_delete = name.clone();
                                            let name_activate = name.clone();
                                            view! {
                                                <PresetCard
                                                    name=name.clone()
                                                    preset=preset
                                                    is_default=is_default
                                                    on_edit=move || editing.set(Some(EditorTarget::Existing(name_edit.clone())))
                                                    on_duplicate=move || editing.set(Some(EditorTarget::Clone(name_dup.clone())))
                                                    on_set_default=move || set_default(name_default.clone())
                                                    on_delete=move || delete_preset(name_delete.clone())
                                                    on_activate=move || {
                                                        // Park the request; the chat view's restore_from applies it
                                                        // after activating the session (a direct selected_model set
                                                        // would be overwritten by that restore).
                                                        chat.pending_model_override.set(Some(ModelOverride::Qualified {
                                                            provider: "moa".to_string(),
                                                            model: name_activate.clone(),
                                                        }));
                                                        navigate.with_value(|nav| nav("/", Default::default()));
                                                    }
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
    on_duplicate: impl Fn() + 'static + Send,
    on_set_default: impl Fn() + 'static + Send,
    on_delete: impl Fn() + 'static + Send,
    on_activate: impl Fn() + 'static + Send,
) -> impl IntoView {
    let slot_chip = |slot: &MoaSlotDto| format!("{} / {}", slot.provider, slot.model);
    let aggregator_chip = slot_chip(&preset.aggregator);
    let enabled = preset.enabled;
    // Model calls on a CONSULTATION iteration: each advisor + the aggregator
    // (disabled = aggregator acts alone). Deliberately not "per turn" — how
    // many turns cost this is the `fanout` cadence's business, so the badge
    // states the unit it actually knows and the tooltip names the multiplier.
    let call_count = if enabled {
        preset.advisors.len() + 1
    } else {
        1
    };
    let cadence_hint = if !enabled {
        "Advisors disabled — the aggregator acts alone.".to_string()
    } else if preset.fanout == "user_turn" {
        format!("{call_count} model calls (advisors + aggregator) on the one consultation per run.")
    } else if let Some(n) = preset.fanout.strip_prefix("every_n:") {
        format!(
            "{call_count} model calls (advisors + aggregator) on each consultation — every {n} tool iterations."
        )
    } else {
        format!(
            "{call_count} model calls (advisors + aggregator) on each consultation — every tool iteration."
        )
    };

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
                    {if enabled {
                        view! {
                            <button
                                on:click=move |_| on_activate()
                                class="text-xs font-medium text-primary hover:underline"
                            >
                                "Use in chat"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }}
                    {if !is_default {
                        view! {
                            <button
                                on:click=move |_| on_set_default()
                                class="text-xs text-text-secondary hover:text-primary"
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
                        on:click=move |_| on_duplicate()
                        class="text-xs text-text-secondary hover:text-primary"
                    >
                        "Duplicate"
                    </button>
                    <button
                        on:click=move |_| on_delete()
                        class="text-xs text-danger hover:underline"
                    >
                        "Delete"
                    </button>
                </div>
            </div>
            <div class="flex flex-wrap items-center gap-1.5 text-xs">
                {preset.advisors.iter().map(|slot| {
                    let chip = slot_chip(slot);
                    view! {
                        <span class="px-2 py-1 rounded bg-info/10 text-info">{chip}</span>
                    }
                }).collect_view()}
                <span class="px-2 py-1 rounded bg-primary/10 text-primary">{format!("Σ {aggregator_chip}")}</span>
                <span
                    class="px-2 py-1 rounded bg-surface-sunken text-text-tertiary"
                    title=cadence_hint
                >
                    {format!("×{call_count}/consult")}
                </span>
            </div>
        </div>
    }
}
