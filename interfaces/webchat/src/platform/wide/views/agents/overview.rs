// Overview Tab — identity, model config, and inference parameters editor

use crate::api::agents::AgentsApi;
use crate::api::providers::{CatalogEntry, CatalogView, ProvidersApi};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

/// Get the effective model list for a catalog entry: use models if present, otherwise fall back to [`default_model`].
fn effective_models(e: &CatalogEntry) -> Vec<String> {
    if e.models.is_empty() {
        vec![e.default_model.clone()]
    } else {
        e.models.clone()
    }
}

#[component]
#[must_use]
pub fn OverviewTab(agent_id: String) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Editable fields
    let emoji = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    // "" = inherit system default; otherwise "provider|model" (catalog selected item)
    let selected_model = RwSignal::new(String::new());
    let model_touched = RwSignal::new(false);
    let catalog: RwSignal<Vec<CatalogEntry>> = RwSignal::new(Vec::new());
    let is_saving = RwSignal::new(false);
    let save_message = RwSignal::new(Option::<(bool, String)>::None);

    // Fetch catalog once
    {
        let dash = state;
        spawn_local(async move {
            if let Ok(items) = ProvidersApi::catalog(&dash, CatalogView::Configured).await {
                catalog.set(items);
            }
        });
    }

    // Load agent detail
    let id_for_load = agent_id.clone();
    let dash = state;
    Effect::new(move || {
        if !dash.is_connected.get() {
            return;
        }
        let id = id_for_load.clone();
        spawn_local(async move {
            if let Ok(detail) = AgentsApi::get(&dash, &id).await {
                let def = &detail.definition;
                name.set(
                    def.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                );

                if let Some(identity) = def.get("identity") {
                    emoji.set(
                        identity
                            .get("emoji")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                    description.set(
                        identity
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    );
                }

                // Read stored model: Qualified object -> "provider|model"; Legacy string / absent -> leave empty = inherit
                if let Some(mv) = def.get("model") {
                    if let Some(obj) = mv.as_object() {
                        let p = obj.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                        let m = obj.get("model").and_then(|v| v.as_str()).unwrap_or("");
                        if !p.is_empty() && !m.is_empty() {
                            selected_model.set(format!("{p}\u{1f}{m}"));
                        }
                    }
                    // Legacy bare string: no provider context → leave empty (=inherit); user can re-pick.
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

        let sel = selected_model.get();
        let model_patch = if sel.is_empty() {
            serde_json::Value::Null // inherit system default
        } else if let Some((p, m)) = sel.split_once('\u{1f}') {
            json!({ "provider": p, "model": m })
        } else {
            serde_json::Value::Null
        };

        let mut patch = json!({
            "name": name.get(),
            "identity": {
                "emoji": emoji.get(),
                "description": description.get(),
            },
        });

        // Only write the model key when the user actually changes the dropdown:
        // untouched -> key absent -> backend AgentPatch.model = None -> preserve original value (legacy/qualified).
        if model_touched.get() {
            patch["model"] = model_patch;
        }

        spawn_local(async move {
            match AgentsApi::update(&dash, &id, patch).await {
                Ok(()) => save_message.set(Some((
                    true,
                    t_string!(i18n, agents.overview.saved).to_string(),
                ))),
                Err(e) => save_message.set(Some((false, e))),
            }
            is_saving.set(false);
        });
    };

    view! {
        <div class="space-y-6">
            // Identity section
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, agents.overview.title)}</h2>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.agent_id)}</label>
                        <div class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-tertiary font-mono text-sm select-all">
                            {agent_id}
                        </div>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.emoji)}</label>
                        <input
                            type="text"
                            prop:value=move || emoji.get()
                            on:input=move |ev| emoji.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary text-lg"
                            placeholder=move || t_string!(i18n, agents.overview.emoji_placeholder).to_string()
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.display_name)}</label>
                        <input
                            type="text"
                            prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary"
                            placeholder=move || t_string!(i18n, agents.overview.name_placeholder).to_string()
                        />
                    </div>
                    <div class="md:col-span-2">
                        <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.description)}</label>
                        <textarea
                            prop:value=move || description.get()
                            on:input=move |ev| description.set(event_target_value(&ev))
                            class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary resize-none"
                            rows="2"
                            placeholder=move || t_string!(i18n, agents.overview.description_placeholder).to_string()
                        />
                    </div>
                </div>
            </div>

            // Model Configuration
            <div class="bg-surface-raised border border-border rounded-xl p-6">
                <h2 class="text-lg font-semibold text-text-primary mb-4">{t!(i18n, agents.overview.model_config)}</h2>
                <div class="space-y-2">
                    <label class="block text-sm font-medium text-text-secondary mb-1">{t!(i18n, agents.overview.primary_model)}</label>
                    <select
                        prop:value=move || selected_model.get()
                        on:change=move |ev| { selected_model.set(event_target_value(&ev)); model_touched.set(true); }
                        class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary font-mono text-sm"
                    >
                        <option value="">"继承系统默认 (inherit system default)"</option>
                        {move || {
                            catalog.get().into_iter().flat_map(|entry: CatalogEntry| {
                                let provider_id = entry.id.clone();
                                let models = effective_models(&entry);
                                let dn = entry.display_name;
                                models.into_iter().map(move |m| {
                                    let val = format!("{provider_id}\u{1f}{m}");
                                    let label = format!("{dn} / {m}");
                                    view! { <option value=val>{label}</option> }
                                }).collect::<Vec<_>>()
                            }).collect::<Vec<_>>()
                        }}
                    </select>
                    {move || {
                        let sel = selected_model.get();
                        let in_catalog = sel.is_empty() || catalog.get().iter().any(|e| {
                            effective_models(e).iter().any(|m| format!("{}\u{1f}{}", e.id, m) == sel)
                        });
                        (!in_catalog).then(|| view! {
                            <p class="mt-1 text-xs text-danger/80">
                                "\u{26a0} 当前选中的 model 已失效(provider 被删/禁用),保存后将回退系统默认"
                            </p>
                        })
                    }}
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
                    {move || if is_saving.get() { t_string!(i18n, common.saving).to_string() } else { t_string!(i18n, common.save).to_string() }}
                </button>
            </div>
        </div>
    }
}
