// Overview Tab — identity, model config, and inference parameters editor

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;
use crate::api::agents::AgentsApi;
use crate::context::DashboardState;

#[component]
pub fn OverviewTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Editable fields
    let emoji = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let theme = RwSignal::new(String::new());
    let primary_model = RwSignal::new(String::new());
    let fallbacks = RwSignal::new(String::new());
    let temperature = RwSignal::new(String::new());
    let max_tokens = RwSignal::new(String::new());
    let top_p = RwSignal::new(String::new());
    let top_k = RwSignal::new(String::new());
    let is_saving = RwSignal::new(false);
    let save_message = RwSignal::new(Option::<(bool, String)>::None);

    // Load agent detail
    let id_for_load = agent_id.clone();
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        let id = id_for_load.clone();
        spawn_local(async move {
            if let Ok(detail) = AgentsApi::get(&dash, &id).await {
                let def = &detail.definition;
                name.set(def.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string());

                if let Some(identity) = def.get("identity") {
                    emoji.set(identity.get("emoji").and_then(|v| v.as_str()).unwrap_or("").to_string());
                    description.set(identity.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string());
                    theme.set(identity.get("theme").and_then(|v| v.as_str()).unwrap_or("").to_string());
                }

                if let Some(mc) = def.get("model_config") {
                    primary_model.set(mc.get("primary").and_then(|v| v.as_str()).unwrap_or("").to_string());
                    if let Some(fb) = mc.get("fallbacks").and_then(|v| v.as_array()) {
                        let fbs: Vec<String> = fb.iter().filter_map(|v| v.as_str().map(String::from)).collect();
                        fallbacks.set(fbs.join(", "));
                    }
                } else if let Some(model) = def.get("model").and_then(|v| v.as_str()) {
                    primary_model.set(model.to_string());
                }

                if let Some(params) = def.get("params") {
                    if let Some(v) = params.get("temperature").and_then(|v| v.as_f64()) {
                        temperature.set(format!("{}", v));
                    }
                    if let Some(v) = params.get("max_tokens").and_then(|v| v.as_u64()) {
                        max_tokens.set(format!("{}", v));
                    }
                    if let Some(v) = params.get("top_p").and_then(|v| v.as_f64()) {
                        top_p.set(format!("{}", v));
                    }
                    if let Some(v) = params.get("top_k").and_then(|v| v.as_u64()) {
                        top_k.set(format!("{}", v));
                    }
                }
            }
        });
    });

    // Save handler
    let id_for_save = agent_id.clone();
    let handle_save = move |_: web_sys::MouseEvent| {
        is_saving.set(true);
        save_message.set(None);
        let id = id_for_save.clone();
        let dash = state;

        let fb_list: Vec<String> = fallbacks.get()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut patch = json!({
            "name": name.get(),
            "identity": {
                "emoji": emoji.get(),
                "description": description.get(),
                "theme": theme.get(),
            },
        });

        let pm = primary_model.get();
        if !pm.is_empty() {
            patch["model_config"] = json!({
                "primary": pm,
                "fallbacks": fb_list,
            });
        }

        let mut params = serde_json::Map::new();
        if let Ok(v) = temperature.get().parse::<f32>() {
            params.insert("temperature".to_string(), json!(v));
        }
        if let Ok(v) = max_tokens.get().parse::<u32>() {
            params.insert("max_tokens".to_string(), json!(v));
        }
        if let Ok(v) = top_p.get().parse::<f32>() {
            params.insert("top_p".to_string(), json!(v));
        }
        if let Ok(v) = top_k.get().parse::<u32>() {
            params.insert("top_k".to_string(), json!(v));
        }
        if !params.is_empty() {
            patch["params"] = serde_json::Value::Object(params);
        }

        spawn_local(async move {
            match AgentsApi::update(&dash, &id, patch).await {
                Ok(()) => save_message.set(Some((true, "Saved successfully".to_string()))),
                Err(e) => save_message.set(Some((false, e))),
            }
            is_saving.set(false);
        });
    };

    view! {
        <div class="space-y-6">
            // Identity section
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">"Identity"</h2>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Agent ID"</label>
                        <div class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-tertiary font-mono text-sm select-all">
                            {agent_id.clone()}
                        </div>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Emoji"</label>
                        <input
                            type="text"
                            prop:value=move || emoji.get()
                            on:input=move |ev| emoji.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary text-lg"
                            placeholder="🤖"
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Display Name"</label>
                        <input
                            type="text"
                            prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder="Agent name"
                        />
                    </div>
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Description"</label>
                        <textarea
                            prop:value=move || description.get()
                            on:input=move |ev| description.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary resize-none"
                            rows="2"
                            placeholder="What this agent specializes in..."
                        />
                    </div>
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Theme / Tagline"</label>
                        <input
                            type="text"
                            prop:value=move || theme.get()
                            on:input=move |ev| theme.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder="Write clean, efficient code"
                        />
                    </div>
                </div>
            </div>

            // Model Configuration
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">"Model Configuration"</h2>
                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Primary Model"</label>
                        <input
                            type="text"
                            prop:value=move || primary_model.get()
                            on:input=move |ev| primary_model.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono text-sm"
                            placeholder="claude-opus-4"
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Fallback Models"</label>
                        <input
                            type="text"
                            prop:value=move || fallbacks.get()
                            on:input=move |ev| fallbacks.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono text-sm"
                            placeholder="claude-sonnet-4, gpt-4o"
                        />
                        <p class="mt-1 text-xs text-text-tertiary">"Comma-separated list of fallback models"</p>
                    </div>
                </div>
            </div>

            // Inference Parameters
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">"Inference Parameters"</h2>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Temperature"</label>
                        <input
                            type="number"
                            step="0.1"
                            min="0"
                            max="2"
                            prop:value=move || temperature.get()
                            on:input=move |ev| temperature.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder="0.7"
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Max Tokens"</label>
                        <input
                            type="number"
                            prop:value=move || max_tokens.get()
                            on:input=move |ev| max_tokens.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder="4096"
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Top P"</label>
                        <input
                            type="number"
                            step="0.05"
                            min="0"
                            max="1"
                            prop:value=move || top_p.get()
                            on:input=move |ev| top_p.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder="0.95"
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">"Top K"</label>
                        <input
                            type="number"
                            prop:value=move || top_k.get()
                            on:input=move |ev| top_k.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder="50"
                        />
                    </div>
                </div>
            </div>

            // Status message and save button
            {move || save_message.get().map(|(success, msg)| {
                let class = if success {
                    "p-3 bg-success-subtle border border-success/30 rounded-lg text-success text-sm"
                } else {
                    "p-3 bg-danger-subtle border border-danger/20 rounded-lg text-danger text-sm"
                };
                view! { <div class=class>{msg}</div> }
            })}

            <div class="flex justify-end pt-4 border-t border-border">
                <button
                    on:click=handle_save
                    disabled=move || is_saving.get()
                    class="px-6 py-2 bg-primary text-white rounded-lg hover:bg-primary-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                >
                    {move || if is_saving.get() { "Saving..." } else { "Save Changes" }}
                </button>
            </div>
        </div>
    }
}
