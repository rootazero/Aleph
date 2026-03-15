//! Setup Wizard
//!
//! First-run wizard that guides users through configuring their first AI provider.
//! 3-step flow: Select Provider -> Enter Credentials -> Enter Model

use leptos::prelude::*;
use leptos::task::spawn_local;
use crate::preset_data::{ProviderPreset, PRESETS, OAUTH_PRESETS};
use crate::components::api_key_input::ApiKeyInput;
use crate::context::DashboardState;
use crate::api::{ProvidersApi, ProviderConfig};

#[derive(Debug, Clone, PartialEq)]
enum WizardStep {
    SelectProvider,
    EnterCredentials,
    SelectModel,
    Complete,
}

/// Setup wizard overlay for first-run provider configuration
#[component]
pub fn SetupWizard(
    /// Called when wizard is closed or completed
    on_close: Callback<()>,
) -> impl IntoView {
    let step = RwSignal::new(WizardStep::SelectProvider);
    let selected_preset_name = RwSignal::new(None::<String>);
    let api_key = RwSignal::new(String::new());
    let base_url = RwSignal::new(String::new());
    let form_model = RwSignal::new(String::new());
    let is_saving = RwSignal::new(false);

    let state = expect_context::<DashboardState>();

    // Helper: look up the currently selected preset
    let get_preset = move || -> Option<&'static ProviderPreset> {
        selected_preset_name.get().and_then(|name| {
            PRESETS.iter().chain(OAUTH_PRESETS.iter()).find(|p| p.name == name)
        })
    };

    // Save: create provider + set as default
    let save_provider = {
        let state = state.clone();
        move || {
            let state = state.clone();
            is_saving.set(true);
            spawn_local(async move {
                if let Some(preset_name) = selected_preset_name.get() {
                    let preset = PRESETS.iter().chain(OAUTH_PRESETS.iter())
                        .find(|p| p.name == preset_name);
                    let protocol = preset.map(|p| p.protocol.to_string());
                    let color = preset.map(|p| p.icon_color.to_string());

                    let model = form_model.get();
                    let key_val = api_key.get();
                    let url_val = base_url.get();

                    let config = ProviderConfig {
                        protocol,
                        enabled: true,
                        model,
                        api_key: if key_val.is_empty() { None } else { Some(key_val) },
                        base_url: if url_val.is_empty() { None } else { Some(url_val) },
                        color,
                        timeout_seconds: None,
                        max_tokens: None,
                        temperature: None,
                        top_p: None,
                        top_k: None,
                    };
                    match ProvidersApi::create(&state, preset_name.clone(), config).await {
                        Ok(_) => {
                            let _ = ProvidersApi::set_default(&state, preset_name).await;
                            step.set(WizardStep::Complete);
                        }
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("Failed to create provider: {}", e).into(),
                            );
                        }
                    }
                }
                is_saving.set(false);
            });
        }
    };

    view! {
        <div
            class="wizard-overlay"
            tabindex="-1"
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Escape" {
                    on_close.run(());
                }
            }
        >
            <div class="wizard-modal">
                <button class="wizard-close" on:click=move |_| {
                    on_close.run(());
                }>"x"</button>

                <div class="wizard-header">
                    <h2>"Configure AI Provider"</h2>
                    <div class="wizard-steps">
                        <span class=move || if step.get() == WizardStep::SelectProvider { "step active" } else { "step" }>
                            "1. Provider"
                        </span>
                        <span class=move || if step.get() == WizardStep::EnterCredentials { "step active" } else { "step" }>
                            "2. Credentials"
                        </span>
                        <span class=move || if step.get() == WizardStep::SelectModel { "step active" } else { "step" }>
                            "3. Model"
                        </span>
                    </div>
                </div>

                <div class="wizard-content">
                    {move || match step.get() {
                        WizardStep::SelectProvider => {
                            view! {
                                <div class="wizard-step-provider">
                                    <p class="wizard-description">"Select an AI provider to get started:"</p>
                                    <div class="preset-grid">
                                        {PRESETS.iter().map(|preset| {
                                            let name = preset.name;
                                            let description = preset.description;
                                            let color = preset.icon_color;
                                            let needs_key = preset.needs_api_key;
                                            let preset_base_url = preset.base_url;
                                            let preset_model = preset.model;
                                            let initial = name.chars().next()
                                                .map(|c| c.to_uppercase().to_string())
                                                .unwrap_or_default();
                                            view! {
                                                <button
                                                    class="preset-card"
                                                    on:click=move |_| {
                                                        selected_preset_name.set(Some(name.to_string()));
                                                        base_url.set(preset_base_url.to_string());
                                                        form_model.set(preset_model.to_string());
                                                        if needs_key {
                                                            step.set(WizardStep::EnterCredentials);
                                                        } else {
                                                            // No key needed (e.g. Ollama): skip to model step
                                                            step.set(WizardStep::SelectModel);
                                                        }
                                                    }
                                                >
                                                    <span class="preset-icon" style=format!("background-color: {}", color)>
                                                        {initial.clone()}
                                                    </span>
                                                    <span class="preset-name">{name}</span>
                                                    <span class="preset-desc">{description}</span>
                                                </button>
                                            }
                                        }).collect_view()}
                                    </div>
                                </div>
                            }.into_any()
                        }
                        WizardStep::EnterCredentials => {
                            view! {
                                <div class="wizard-step-credentials">
                                    {move || get_preset().map(|preset| {
                                        let placeholder = preset.api_key_placeholder;
                                        view! {
                                            <p class="wizard-description">
                                                {format!("Enter your {} API key:", preset.name)}
                                            </p>
                                            <ApiKeyInput
                                                value=api_key
                                                placeholder=placeholder
                                            />
                                            <div class="wizard-actions">
                                                <button
                                                    class="btn-secondary"
                                                    on:click=move |_| {
                                                        step.set(WizardStep::SelectProvider);
                                                        api_key.set(String::new());
                                                    }
                                                >"Back"</button>
                                                <button
                                                    class="btn-primary"
                                                    disabled=move || api_key.get().is_empty()
                                                    on:click=move |_| {
                                                        step.set(WizardStep::SelectModel);
                                                    }
                                                >"Next"</button>
                                            </div>
                                        }
                                    })}
                                </div>
                            }.into_any()
                        }
                        WizardStep::SelectModel => {
                            let save_provider = save_provider.clone();
                            view! {
                                <div class="wizard-step-model">
                                    <p class="wizard-description">"Enter a model name:"</p>
                                    <div>
                                        <input
                                            type="text"
                                            prop:value=move || form_model.get()
                                            on:input=move |ev| form_model.set(event_target_value(&ev))
                                            class="input"
                                            placeholder="e.g. gpt-4o, claude-sonnet-4-20250514"
                                        />
                                        <p class="text-xs text-text-tertiary mt-1">
                                            {move || get_preset().map(|p| format!("Recommended: {}", p.model)).unwrap_or_default()}
                                        </p>
                                    </div>
                                    <div class="wizard-actions">
                                        <button
                                            class="btn-secondary"
                                            on:click=move |_| {
                                                if get_preset().map(|p| p.needs_api_key).unwrap_or(true) {
                                                    step.set(WizardStep::EnterCredentials);
                                                } else {
                                                    step.set(WizardStep::SelectProvider);
                                                }
                                            }
                                        >"Back"</button>
                                        <button
                                            class="btn-primary"
                                            disabled=move || form_model.get().is_empty() || is_saving.get()
                                            on:click={
                                                let save_provider = save_provider.clone();
                                                move |_| save_provider()
                                            }
                                        >
                                            {move || if is_saving.get() { "Saving..." } else { "Confirm" }}
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                        WizardStep::Complete => {
                            view! {
                                <div class="wizard-step-complete">
                                    <h3>"Provider configured!"</h3>
                                    <p>"Your AI provider is ready to use."</p>
                                    <div class="wizard-actions">
                                        <button
                                            class="btn-primary"
                                            on:click=move |_| on_close.run(())
                                        >"Get Started"</button>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </div>
    }
}
