//! Generic channel configuration renderer for a single instance.
//!
//! `ChannelConfigTemplate` receives a `&'static ChannelDefinition` plus an
//! `instance_id` string identifying the concrete channel instance. It renders
//! a configuration form: connection status, all config fields, save/connect/
//! disconnect/delete actions. The `render_field()` dispatcher maps each
//! `FieldKind` to the appropriate form component.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Value};

use crate::components::forms::{
    ErrorMessageDynamic, FormField, NumberInput, SaveButton, SelectInput, SettingsSection,
    SwitchInput, TextInput,
};
use crate::components::ui::channel_status::{ChannelStatus, ChannelStatusBadge};
use crate::components::ui::SecretInput;
use crate::components::ui::TagListInput;
use crate::context::DashboardState;

use super::definitions::{ChannelDefinition, FieldDef, FieldKind};

// ---------------------------------------------------------------------------
// ChannelConfigTemplate
// ---------------------------------------------------------------------------

/// Generic channel configuration panel driven by a `ChannelDefinition`.
///
/// Loads the current config from Gateway via `config.get`, renders all fields
/// through the `render_field` dispatcher, and provides Save / Connect /
/// Disconnect / Delete actions.
#[component]
pub fn ChannelConfigTemplate(
    definition: &'static ChannelDefinition,
    instance_id: String,
    #[prop(optional)] on_deleted: Option<Callback<()>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // ---- state signals ----
    let field_values = RwSignal::new(serde_json::Map::new());
    let loading = RwSignal::new(true);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let success = RwSignal::new(Option::<String>::None);
    let channel_status = RwSignal::new(ChannelStatus::Disconnected);
    let connecting = RwSignal::new(false);
    let deleting = RwSignal::new(false);

    // Dynamic identifiers based on instance_id
    let channel_id: String = instance_id.clone();
    let config_section: String = format!("channels.{}", instance_id);

    // Extract top-level section and sub-key for config.get
    let (top_section, channel_sub_key): (String, String) = config_section
        .split_once('.')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap_or((config_section.clone(), String::new()));

    // ---- Effect: load config on mount ----
    {
        let section = top_section.clone();
        let sub_key = channel_sub_key.clone();
        let channel_id_for_load = channel_id.clone();
        spawn_local(async move {
            match state
                .rpc_call("config.get", json!({ "section": section }))
                .await
            {
                Ok(val) => {
                    let channel_val = if sub_key.is_empty() {
                        Some(&val)
                    } else {
                        val.get(&sub_key)
                    };
                    if let Some(obj) = channel_val.and_then(|v| v.as_object()) {
                        field_values.set(obj.clone());
                    }
                    loading.set(false);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load config for {}: {}", channel_id_for_load, e)
                            .into(),
                    );
                    loading.set(false);
                }
            }
        });
    }

    // Fetch channel status on mount
    {
        let channel_id_for_status = channel_id.clone();
        spawn_local(async move {
            match state
                .rpc_call(
                    "channels.status",
                    json!({ "channel_id": channel_id_for_status }),
                )
                .await
            {
                Ok(val) => {
                    if let Some(s) = val.as_str() {
                        channel_status.set(ChannelStatus::from_str(s));
                    } else if let Some(s) = val.get("status").and_then(|v| v.as_str()) {
                        channel_status.set(ChannelStatus::from_str(s));
                    }
                }
                Err(_) => {
                    // Channel status endpoint may not exist yet; keep Disconnected
                }
            }
        });
    }

    // ---- Save handler ----
    let config_section_for_save = config_section.clone();
    let on_save = move || {
        if !state.is_connected.get() {
            return;
        }
        saving.set(true);
        error.set(None);
        success.set(None);

        let values = field_values.get();
        let section = config_section_for_save.clone();

        spawn_local(async move {
            let mut patch_obj = serde_json::Map::new();
            for (key, value) in values.iter() {
                patch_obj.insert(key.clone(), value.clone());
            }

            let params = json!({
                "path": section,
                "patch": Value::Object(patch_obj),
            });

            match state.rpc_call("config.patch", params).await {
                Ok(_) => {
                    success.set(Some("Configuration saved successfully.".to_string()));
                }
                Err(e) => {
                    error.set(Some(format!("Failed to save configuration: {}", e)));
                }
            }
            saving.set(false);
        });
    };

    // Store channel_id in a signal so handlers can access it without move issues
    let channel_id_sig = StoredValue::new(channel_id.clone());

    // ---- Connect handler ----
    let on_connect = move || {
        if !state.is_connected.get() {
            return;
        }
        connecting.set(true);
        error.set(None);
        success.set(None);
        channel_status.set(ChannelStatus::Connecting);

        let id = channel_id_sig.get_value();
        spawn_local(async move {
            match state
                .rpc_call("channel.start", json!({ "channel_id": id }))
                .await
            {
                Ok(_) => {
                    channel_status.set(ChannelStatus::Connected);
                    success.set(Some("Channel connected.".to_string()));
                }
                Err(e) => {
                    channel_status.set(ChannelStatus::Error);
                    error.set(Some(format!("Failed to connect: {}", e)));
                }
            }
            connecting.set(false);
        });
    };

    // ---- Disconnect handler ----
    let on_disconnect = move || {
        if !state.is_connected.get() {
            return;
        }
        error.set(None);
        success.set(None);

        let id = channel_id_sig.get_value();
        spawn_local(async move {
            match state
                .rpc_call("channel.stop", json!({ "channel_id": id }))
                .await
            {
                Ok(_) => {
                    channel_status.set(ChannelStatus::Disconnected);
                    success.set(Some("Channel disconnected.".to_string()));
                }
                Err(e) => {
                    error.set(Some(format!("Failed to disconnect: {}", e)));
                }
            }
        });
    };

    // ---- Delete handler ----
    let on_delete = move || {
        if !state.is_connected.get() {
            return;
        }
        deleting.set(true);
        error.set(None);
        success.set(None);

        let id = channel_id_sig.get_value();
        spawn_local(async move {
            match state
                .rpc_call("channel.delete", json!({ "channel_id": id }))
                .await
            {
                Ok(_) => {
                    if let Some(cb) = on_deleted {
                        cb.run(());
                    }
                }
                Err(e) => {
                    error.set(Some(format!("Failed to delete instance: {}", e)));
                    deleting.set(false);
                }
            }
        });
    };

    // ---- Pre-compute static view data ----
    let icon_svg = definition.icon_svg;
    let brand_color = definition.brand_color;
    let docs_url = definition.docs_url;
    let fields = definition.fields;

    // ---- Build the success signal for display ----
    let success_signal: Signal<Option<String>> = success.into();

    // Channel ID label
    let channel_id_label = channel_id.clone();

    view! {
        <div class="space-y-6">
            // ---- Instance identifier ----
            <div class="text-xs text-text-tertiary font-mono">{channel_id_label}</div>

            // ---- Connection status card ----
            <div class="p-4 bg-surface-raised border border-border rounded-xl">
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-3">
                        <div class="w-10 h-10 rounded-full bg-surface-sunken flex items-center justify-center">
                            <svg
                                width="20"
                                height="20"
                                viewBox="0 0 24 24"
                                fill="none"
                                stroke=brand_color
                                stroke-width="2"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                inner_html=icon_svg
                            />
                        </div>
                        <div>
                            <div class="text-sm font-medium text-text-primary">"Connection Status"</div>
                            <ChannelStatusBadge status=channel_status.into() />
                        </div>
                    </div>
                    <div class="flex items-center gap-2">
                        {move || {
                            let st = channel_status.get();
                            match st {
                                ChannelStatus::Connected | ChannelStatus::Connecting => {
                                    view! {
                                        <button
                                            on:click=move |_| on_disconnect()
                                            class="px-3 py-1.5 text-sm border border-danger/30 text-danger rounded-lg hover:bg-danger-subtle transition-colors"
                                        >
                                            "Disconnect"
                                        </button>
                                    }.into_any()
                                }
                                _ => {
                                    view! {
                                        <button
                                            on:click=move |_| on_connect()
                                            disabled=move || connecting.get()
                                            class="px-3 py-1.5 text-sm bg-primary text-text-inverse rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors"
                                        >
                                            {move || if connecting.get() { "Connecting..." } else { "Connect" }}
                                        </button>
                                    }.into_any()
                                }
                            }
                        }}
                    </div>
                </div>
            </div>

            // ---- Error / Success messages ----
            <ErrorMessageDynamic error=error.into() />
            {move || success_signal.get().map(|msg| view! {
                <div class="p-4 bg-success-subtle border border-success/30 rounded-lg text-success text-sm">
                    {msg}
                </div>
            })}

            // ---- Loading state OR field section ----
            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">"Loading configuration..."</div>
                        </div>
                    }.into_any()
                } else if fields.is_empty() {
                    view! {
                        <div class="p-4 bg-primary-subtle border border-primary/20 rounded-xl text-sm text-info">
                            "This channel uses a custom configuration page."
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <SettingsSection title="Configuration">
                            {fields
                                .iter()
                                .map(|field| render_field(field, field_values))
                                .collect_view()}
                        </SettingsSection>
                    }.into_any()
                }
            }}

            // ---- Action bar: Save + Delete + docs link ----
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <SaveButton
                        on_click=move || on_save()
                        loading=saving.into()
                        text="Save Configuration"
                    />
                    <button
                        on:click=move |_| on_delete()
                        disabled=move || deleting.get()
                        class="px-4 py-2 text-sm border border-danger/30 text-danger rounded-lg hover:bg-danger-subtle disabled:opacity-50 transition-colors"
                    >
                        {move || if deleting.get() { "Deleting..." } else { "Delete Instance" }}
                    </button>
                </div>
                <a
                    href=docs_url
                    target="_blank"
                    rel="noopener noreferrer"
                    class="text-sm text-text-tertiary hover:text-primary transition-colors inline-flex items-center gap-1"
                >
                    "Documentation"
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>
                        <polyline points="15 3 21 3 21 9"/>
                        <line x1="10" y1="14" x2="21" y2="3"/>
                    </svg>
                </a>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// render_field — dispatches to the matching form component by FieldKind
// ---------------------------------------------------------------------------

fn render_field(
    field: &'static FieldDef,
    field_values: RwSignal<serde_json::Map<String, Value>>,
) -> impl IntoView {
    let key: &'static str = field.key;

    // Compute the label (append " *" for required fields)
    let label: &'static str = if field.required {
        // Leak is acceptable: FieldDef is static data, finite count
        Box::leak(format!("{} *", field.label).into_boxed_str())
    } else {
        field.label
    };

    // For #[prop(optional)] fields we pass the raw &'static str.
    // Empty strings are treated as "no value" by the components.
    let help: &'static str = field.help;
    let placeholder: &'static str = field.placeholder;

    // Shared setter
    let set_value = move |val: Value| {
        field_values.update(|map| {
            map.insert(key.to_string(), val);
        });
    };

    match field.kind {
        // -- Text --
        FieldKind::Text => {
            let get_val = move || -> String {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            view! {
                <FormField label=label help_text=help>
                    <TextInput
                        value=Signal::derive(get_val)
                        on_change=move |v: String| set_value(Value::String(v))
                        placeholder=placeholder
                    />
                </FormField>
            }
            .into_any()
        }

        // -- URL (same as Text but monospace) --
        FieldKind::Url => {
            let get_val = move || -> String {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            view! {
                <FormField label=label help_text=help>
                    <TextInput
                        value=Signal::derive(get_val)
                        on_change=move |v: String| set_value(Value::String(v))
                        placeholder=placeholder
                        monospace=true
                    />
                </FormField>
            }
            .into_any()
        }

        // -- Secret --
        FieldKind::Secret => {
            let get_val = move || -> String {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            view! {
                <FormField label=label help_text=help>
                    <SecretInput
                        value=Signal::derive(get_val)
                        on_change=move |v: String| set_value(Value::String(v))
                        placeholder=placeholder
                        monospace=true
                    />
                </FormField>
            }
            .into_any()
        }

        // -- Number --
        FieldKind::Number { min, max } => {
            let default_num: i32 = field.default_value.parse().unwrap_or(0);
            let get_val = move || -> i32 {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(default_num as i64) as i32
            };
            view! {
                <FormField label=label help_text=help>
                    <NumberInput
                        value=Signal::derive(get_val)
                        on_change=move |v: i32| set_value(json!(v))
                        min=min
                        max=max
                    />
                </FormField>
            }
            .into_any()
        }

        // -- Toggle --
        FieldKind::Toggle => {
            let default_bool = field.default_value == "true";
            let get_val = move || -> bool {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(default_bool)
            };
            view! {
                <FormField label=label help_text=help>
                    <SwitchInput
                        checked=Signal::derive(get_val)
                        on_change=move |v: bool| set_value(Value::Bool(v))
                    />
                </FormField>
            }
            .into_any()
        }

        // -- TagList --
        FieldKind::TagList => {
            let get_val = move || -> Vec<String> {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                v.as_str()
                                    .map(|s| s.to_string())
                                    .or_else(|| v.as_i64().map(|n| n.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let on_tags_change = move |tags: Vec<String>| {
                let arr: Vec<Value> = tags
                    .iter()
                    .map(|t| {
                        if let Ok(n) = t.parse::<i64>() {
                            Value::Number(n.into())
                        } else {
                            Value::String(t.clone())
                        }
                    })
                    .collect();
                set_value(Value::Array(arr));
            };
            view! {
                <FormField label=label help_text=help>
                    <TagListInput
                        tags=Signal::derive(get_val)
                        on_change=on_tags_change
                        placeholder=placeholder
                    />
                </FormField>
            }
            .into_any()
        }

        // -- Select --
        FieldKind::Select => {
            let get_val = move || -> String {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let options: Vec<(&'static str, &'static str)> = field.options.to_vec();
            view! {
                <FormField label=label help_text=help>
                    <SelectInput
                        value=Signal::derive(get_val)
                        on_change=move |v: String| set_value(Value::String(v))
                        options=options
                    />
                </FormField>
            }
            .into_any()
        }
    }
}
