//! Editor form for a single MoA preset — advisor rows, aggregator, advanced
//! knobs. Model dropdowns are deduplicated via [`super::options::available_options`].

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::moa::{MoaApi, MoaPresetDto, MoaSlotDto};
use crate::api::providers::CatalogEntry;
use crate::context::DashboardState;

use super::options::available_options;

fn blank_preset() -> MoaPresetDto {
    MoaPresetDto {
        enabled: true,
        advisors: vec![MoaSlotDto {
            provider: String::new(),
            model: String::new(),
        }],
        aggregator: MoaSlotDto {
            provider: String::new(),
            model: String::new(),
        },
        fanout: "per_iteration".to_string(),
        advisor_timeout_secs: 30,
        advisor_max_tokens: None,
        advisor_temperature: None,
        aggregator_temperature: None,
    }
}

/// All slots currently occupied in a preset (advisors + aggregator) — the
/// `used` list passed to [`available_options`] so every dropdown excludes
/// what's already picked elsewhere (its own value is unblocked via `keep`).
fn all_used(p: &MoaPresetDto) -> Vec<MoaSlotDto> {
    let mut v = p.advisors.clone();
    v.push(p.aggregator.clone());
    v
}

#[component]
#[must_use]
pub(super) fn MoaPresetEditor(
    catalog: RwSignal<Vec<CatalogEntry>>,
    initial_name: Option<String>,
    initial_preset: Option<MoaPresetDto>,
    initial_is_default: bool,
    on_saved: impl Fn() + 'static + Copy + Send,
    on_cancel: impl Fn() + 'static + Copy + Send,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let is_new = initial_name.is_none();

    let name = RwSignal::new(initial_name.unwrap_or_default());
    let preset = RwSignal::new(initial_preset.unwrap_or_else(blank_preset));
    let make_default = RwSignal::new(initial_is_default);
    let show_advanced = RwSignal::new(false);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);

    let add_advisor = move |_| {
        preset.update(|p| {
            p.advisors.push(MoaSlotDto {
                provider: String::new(),
                model: String::new(),
            })
        });
    };

    let handle_save = move |_| {
        let nm = name.get().trim().to_string();
        if nm.is_empty() {
            error.set(Some("Preset name is required".to_string()));
            return;
        }
        let p = preset.get();
        if p.enabled {
            if p.advisors.is_empty() {
                error.set(Some("At least one advisor is required".to_string()));
                return;
            }
            let incomplete = p
                .advisors
                .iter()
                .any(|s| s.provider.is_empty() || s.model.is_empty())
                || p.aggregator.provider.is_empty()
                || p.aggregator.model.is_empty();
            if incomplete {
                error.set(Some(
                    "Select a model for every advisor and the aggregator".to_string(),
                ));
                return;
            }
        }

        saving.set(true);
        error.set(None);
        let make_def = make_default.get();
        spawn_local(async move {
            match MoaApi::save_preset(&state, &nm, &p, make_def).await {
                Ok(()) => {
                    saving.set(false);
                    on_saved();
                }
                Err(e) => {
                    saving.set(false);
                    error.set(Some(format!("Failed to save preset: {e}")));
                }
            }
        });
    };

    view! {
        <div class="space-y-6">
            <h2 class="text-xl font-semibold text-text-primary">
                {if is_new { "New MoA preset" } else { "Edit MoA preset" }}
            </h2>

            {move || error.get().map(|e| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm">{e}</div>
            })}

            <div class="space-y-4 p-4 bg-surface-raised border border-border rounded-xl">
                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Name"</label>
                    <input
                        type="text"
                        prop:value=move || name.get()
                        prop:disabled=move || !is_new
                        on:input=move |ev| name.set(event_target_value(&ev))
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30 disabled:opacity-60"
                    />
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-2">"Advisors"</label>
                    <div class="space-y-2">
                        {move || {
                            let p = preset.get();
                            let used = all_used(&p);
                            let cat = catalog.get();
                            p.advisors.iter().enumerate().map(|(idx, slot)| {
                                let slot = slot.clone();
                                let used = used.clone();
                                let cat = cat.clone();
                                view! {
                                    <div class="flex items-center gap-2">
                                        <span class="text-xs text-text-tertiary w-20 shrink-0">
                                            {format!("Advisor {}", idx + 1)}
                                        </span>
                                        <SlotPicker
                                            catalog=cat
                                            used=used
                                            current=slot
                                            on_change=move |s| preset.update(|p| {
                                                if let Some(x) = p.advisors.get_mut(idx) {
                                                    *x = s;
                                                }
                                            })
                                        />
                                        <button
                                            on:click=move |_| preset.update(|p| {
                                                if idx < p.advisors.len() {
                                                    p.advisors.remove(idx);
                                                }
                                            })
                                            class="text-xs text-danger hover:underline shrink-0"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect_view()
                        }}
                    </div>
                    <button
                        on:click=add_advisor
                        class="mt-2 text-xs text-primary hover:underline"
                    >
                        "+ Add advisor"
                    </button>
                </div>

                <div>
                    <label class="block text-sm font-medium text-text-secondary mb-1">"Aggregator"</label>
                    {move || {
                        let p = preset.get();
                        let used = all_used(&p);
                        let cat = catalog.get();
                        view! {
                            <SlotPicker
                                catalog=cat
                                used=used
                                current=p.aggregator.clone()
                                on_change=move |s| preset.update(|p| p.aggregator = s)
                            />
                        }
                    }}
                </div>

                <label class="flex items-center gap-2 text-sm text-text-secondary">
                    <input
                        type="checkbox"
                        prop:checked=move || preset.get().enabled
                        on:change=move |ev| {
                            let checked = event_target_checked(&ev);
                            preset.update(|p| p.enabled = checked);
                        }
                        class="w-4 h-4"
                    />
                    "Enabled"
                </label>

                <label class="flex items-center gap-2 text-sm text-text-secondary">
                    <input
                        type="checkbox"
                        prop:checked=move || make_default.get()
                        on:change=move |ev| make_default.set(event_target_checked(&ev))
                        class="w-4 h-4"
                    />
                    "Set as default preset"
                </label>
            </div>

            <div class="p-4 bg-surface-raised border border-border rounded-xl">
                <button
                    on:click=move |_| show_advanced.update(|v| *v = !*v)
                    class="text-sm font-medium text-text-secondary hover:text-primary"
                >
                    {move || if show_advanced.get() { "▾ Advanced" } else { "▸ Advanced" }}
                </button>

                {move || {
                    if !show_advanced.get() {
                        return view! { <div></div> }.into_any();
                    }
                    view! {
                        <div class="mt-4 space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">"Fanout"</label>
                                <select
                                    prop:value=move || preset.get().fanout
                                    on:change=move |ev| {
                                        let v = event_target_value(&ev);
                                        preset.update(|p| p.fanout = v);
                                    }
                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                >
                                    <option value="per_iteration">"Per iteration"</option>
                                    <option value="user_turn">"Per user turn"</option>
                                </select>
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">"Advisor timeout (seconds)"</label>
                                <input
                                    type="number"
                                    min="1"
                                    prop:value=move || preset.get().advisor_timeout_secs.to_string()
                                    on:input=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).trim().parse::<u64>() {
                                            preset.update(|p| p.advisor_timeout_secs = v);
                                        }
                                    }
                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                />
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">"Advisor max tokens (optional)"</label>
                                <input
                                    type="number"
                                    min="1"
                                    prop:value=move || preset.get().advisor_max_tokens.map(|v| v.to_string()).unwrap_or_default()
                                    on:input=move |ev| {
                                        let v = event_target_value(&ev);
                                        let parsed = if v.trim().is_empty() { None } else { v.trim().parse::<u32>().ok() };
                                        preset.update(|p| p.advisor_max_tokens = parsed);
                                    }
                                    placeholder="model default"
                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                />
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">"Advisor temperature (optional)"</label>
                                <input
                                    type="number"
                                    step="0.1"
                                    prop:value=move || preset.get().advisor_temperature.map(|v| v.to_string()).unwrap_or_default()
                                    on:input=move |ev| {
                                        let v = event_target_value(&ev);
                                        let parsed = if v.trim().is_empty() { None } else { v.trim().parse::<f32>().ok() };
                                        preset.update(|p| p.advisor_temperature = parsed);
                                    }
                                    placeholder="model default"
                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                />
                            </div>

                            <div>
                                <label class="block text-sm font-medium text-text-secondary mb-1">"Aggregator temperature (optional)"</label>
                                <input
                                    type="number"
                                    step="0.1"
                                    prop:value=move || preset.get().aggregator_temperature.map(|v| v.to_string()).unwrap_or_default()
                                    on:input=move |ev| {
                                        let v = event_target_value(&ev);
                                        let parsed = if v.trim().is_empty() { None } else { v.trim().parse::<f32>().ok() };
                                        preset.update(|p| p.aggregator_temperature = parsed);
                                    }
                                    placeholder="model default"
                                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
                                />
                            </div>
                        </div>
                    }.into_any()
                }}
            </div>

            <div class="flex items-center gap-3">
                <button
                    on:click=handle_save
                    prop:disabled=move || saving.get()
                    class="px-6 py-2 bg-primary hover:bg-primary-hover disabled:bg-primary/50 text-white rounded-lg transition-colors disabled:cursor-not-allowed"
                >
                    {move || if saving.get() { "Saving…" } else { "Save" }}
                </button>
                <button
                    on:click=move |_| on_cancel()
                    class="px-6 py-2 bg-surface-sunken hover:bg-surface-sunken text-text-primary rounded-lg transition-colors"
                >
                    "Cancel"
                </button>
            </div>
        </div>
    }
}

#[component]
fn SlotPicker(
    catalog: Vec<CatalogEntry>,
    used: Vec<MoaSlotDto>,
    current: MoaSlotDto,
    on_change: impl Fn(MoaSlotDto) + 'static + Send,
) -> impl IntoView {
    let opts = available_options(&catalog, &used, Some(&current));
    let current_value = if current.provider.is_empty() || current.model.is_empty() {
        String::new()
    } else {
        format!("{}|{}", current.provider, current.model)
    };

    view! {
        <select
            prop:value=current_value
            on:change=move |ev| {
                let v = event_target_value(&ev);
                if let Some((provider, model)) = v.split_once('|') {
                    on_change(MoaSlotDto { provider: provider.to_string(), model: model.to_string() });
                }
            }
            class="flex-1 px-3 py-2 bg-surface-sunken border border-border rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-primary/30"
        >
            <option value="">"— select model —"</option>
            {opts.into_iter().map(|o| {
                let value = format!("{}|{}", o.provider, o.model);
                view! { <option value=value>{o.label}</option> }
            }).collect_view()}
        </select>
    }
}
