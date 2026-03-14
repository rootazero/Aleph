//! Grouped model selector dropdown
//!
//! Displays discovered models grouped by capability (Chat, Vision, Tools, Other).
//! Shared between the setup wizard and the settings form.

use leptos::prelude::*;

/// A discovered model for display
#[derive(Debug, Clone, PartialEq)]
pub struct ModelOption {
    pub id: String,
    pub name: Option<String>,
    pub capabilities: Vec<String>,
    pub source: String,
}

/// Group models by capability
fn group_models(models: &[ModelOption]) -> Vec<(&'static str, Vec<&ModelOption>)> {
    let groups = [
        ("Embedding", "embedding"),
        ("Chat", "chat"),
        ("Vision", "vision"),
        ("Tools", "tools"),
        ("Image", "image"),
        ("Video", "video"),
        ("Speech", "speech"),
    ];

    let mut result: Vec<(&'static str, Vec<&ModelOption>)> = Vec::new();
    let mut categorized = std::collections::HashSet::new();

    for (label, cap) in &groups {
        let group_models: Vec<&ModelOption> = models
            .iter()
            .filter(|m| m.capabilities.iter().any(|c| c == cap))
            .collect();
        if !group_models.is_empty() {
            for m in &group_models {
                categorized.insert(&m.id);
            }
            result.push((label, group_models));
        }
    }

    // "Other" for uncategorized
    let other: Vec<&ModelOption> = models
        .iter()
        .filter(|m| !categorized.contains(&m.id))
        .collect();
    if !other.is_empty() {
        result.push(("Other", other));
    }

    result
}

/// Grouped model dropdown selector
#[component]
pub fn ModelSelector(
    /// Available models
    models: Signal<Vec<ModelOption>>,
    /// Currently selected model ID
    selected: RwSignal<Option<String>>,
    /// Recommended/default model ID
    #[prop(optional)]
    recommended: Option<Signal<Option<String>>>,
    /// Show refresh button
    #[prop(default = false)]
    show_refresh: bool,
    /// Refresh callback
    #[prop(optional)]
    on_refresh: Option<Callback<()>>,
    /// Whether refresh is in progress
    #[prop(optional)]
    refreshing: Option<Signal<bool>>,
    /// Show "custom model" input fallback
    #[prop(default = true)]
    allow_custom: bool,
) -> impl IntoView {
    let show_custom_input = RwSignal::new(false);
    let custom_model = RwSignal::new(String::new());
    let recommended = recommended.unwrap_or(Signal::derive(|| None));
    let refreshing = refreshing.unwrap_or(Signal::derive(|| false));

    view! {
        <div class="model-selector">
            <div class="model-selector-header">
                <label class="form-label">"Model"</label>
                {move || show_refresh.then(|| {
                    let on_refresh = on_refresh.clone();
                    view! {
                        <button
                            class="btn-icon btn-xs"
                            title="Refresh model list"
                            disabled=move || refreshing.get()
                            on:click=move |_| {
                                if let Some(ref cb) = on_refresh {
                                    cb.run(());
                                }
                            }
                        >
                            {move || if refreshing.get() { "⟳" } else { "↻" }}
                        </button>
                    }
                })}
            </div>

            {move || {
                if show_custom_input.get() {
                    view! {
                        <div class="custom-model-input">
                            <input
                                type="text"
                                class="form-input"
                                placeholder="Enter model name..."
                                prop:value=move || custom_model.get()
                                on:input=move |ev| {
                                    let val = event_target_value(&ev);
                                    custom_model.set(val.clone());
                                    selected.set(Some(val));
                                }
                            />
                            <button
                                class="btn-link btn-xs"
                                on:click=move |_| show_custom_input.set(false)
                            >
                                "Back to list"
                            </button>
                        </div>
                    }.into_any()
                } else {
                    let models_val = models.get();
                    let groups = group_models(&models_val);
                    view! {
                        <select
                            class="form-select"
                            on:change=move |ev| {
                                let val = event_target_value(&ev);
                                if val == "__custom__" {
                                    show_custom_input.set(true);
                                } else if !val.is_empty() {
                                    selected.set(Some(val));
                                }
                            }
                        >
                            <option value="" disabled selected=move || selected.get().is_none()>
                                "Select a model..."
                            </option>
                            {groups.into_iter().map(|(label, group)| {
                                let recommended_id = recommended.get();
                                view! {
                                    <optgroup label=label>
                                        {group.into_iter().map(|m| {
                                            let is_recommended = recommended_id.as_ref() == Some(&m.id);
                                            let is_selected = selected.get().as_ref() == Some(&m.id);
                                            let base_name = m.name.clone().unwrap_or_else(|| m.id.clone());
                                            let display = if m.source == "preset" {
                                                format!("{} [Preset]", base_name)
                                            } else {
                                                base_name
                                            };
                                            let display = if is_recommended {
                                                format!("* {}", display)
                                            } else {
                                                display
                                            };
                                            let id = m.id.clone();
                                            view! {
                                                <option
                                                    value=id
                                                    selected=is_selected
                                                >
                                                    {display}
                                                </option>
                                            }
                                        }).collect_view()}
                                    </optgroup>
                                }
                            }).collect_view()}
                            {allow_custom.then(|| view! {
                                <option value="__custom__">"Enter custom model name..."</option>
                            })}
                        </select>
                    }.into_any()
                }
            }}
        </div>
    }
}
