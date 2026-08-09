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
use crate::components::ui::AgentBindingSelector;
use crate::components::ui::ConfirmButton;
use crate::components::ui::SecretInput;
use crate::components::ui::TagListInput;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

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
#[must_use]
pub fn ChannelConfigTemplate(
    definition: &'static ChannelDefinition,
    instance_id: String,
    #[prop(optional)] on_deleted: Option<Callback<()>>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

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
    let config_section: String = format!("channels.{instance_id}");

    // Extract top-level section and sub-key for config.get
    let (top_section, channel_sub_key): (String, String) = config_section
        .split_once('.')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap_or((config_section.clone(), String::new()));

    // ---- Effect: load config + status when connected ----
    {
        let section = top_section;
        let sub_key = channel_sub_key;
        let channel_id_for_load = channel_id.clone();
        let channel_id_for_status = channel_id.clone();
        Effect::new(move || {
            if !state.is_connected.get() {
                return;
            }
            let section = section.clone();
            let sub_key = sub_key.clone();
            let channel_id_for_load = channel_id_for_load.clone();
            let channel_id_for_status = channel_id_for_status.clone();
            spawn_local(async move {
                // Load config
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
                            &format!("Failed to load config for {channel_id_for_load}: {e}").into(),
                        );
                        loading.set(false);
                    }
                }

                // Fetch channel status
                match state
                    .rpc_call(
                        "channels.status",
                        json!({ "channel_id": channel_id_for_status }),
                    )
                    .await
                {
                    Ok(val) => {
                        if let Some(s) = val.as_str() {
                            channel_status.set(ChannelStatus::parse(s));
                        } else if let Some(s) = val.get("status").and_then(|v| v.as_str()) {
                            channel_status.set(ChannelStatus::parse(s));
                        }
                    }
                    Err(_) => {
                        // Channel status endpoint may not exist yet; keep Disconnected
                    }
                }
            });
        });
    }

    // ---- Save handler ----
    let config_section_for_save = config_section;
    let on_save = move || {
        if !state.is_connected.get() {
            return;
        }
        saving.set(true);
        error.set(None);
        success.set(None);

        let values = field_values.get();
        let section = config_section_for_save.clone();

        // Secret fields use stable-echo semantics (matches provider panels): the
        // input starts empty, an empty value means "keep existing key", and the
        // `has_<field>` presence flags from config.get must never be persisted.
        let secret_keys: std::collections::HashSet<&'static str> = definition
            .fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::Secret))
            .map(|f| f.key)
            .collect();

        spawn_local(async move {
            let mut patch_obj = serde_json::Map::new();
            for (key, value) in values.iter() {
                if key.starts_with("has_") {
                    continue; // presence flag, never persisted
                }
                if secret_keys.contains(key.as_str())
                    && value.as_str().map(str::is_empty).unwrap_or(false)
                {
                    continue; // empty secret = keep existing key
                }
                patch_obj.insert(key.clone(), value.clone());
            }

            let params = json!({
                "path": section,
                "patch": Value::Object(patch_obj),
            });

            match state.rpc_call("config.patch", params).await {
                Ok(_) => {
                    success.set(Some(
                        t_string!(i18n, channel_config.toast_saved).to_string(),
                    ));
                }
                Err(e) => {
                    error.set(Some(format!(
                        "{}{}",
                        t_string!(i18n, channel_config.toast_save_failed),
                        e
                    )));
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
                    success.set(Some(
                        t_string!(i18n, channel_config.toast_connected).to_string(),
                    ));
                }
                Err(e) => {
                    channel_status.set(ChannelStatus::Error);
                    error.set(Some(format!(
                        "{}{}",
                        t_string!(i18n, channel_config.toast_connect_failed),
                        e
                    )));
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
                    success.set(Some(
                        t_string!(i18n, channel_config.toast_disconnected).to_string(),
                    ));
                }
                Err(e) => {
                    error.set(Some(format!(
                        "{}{}",
                        t_string!(i18n, channel_config.toast_disconnect_failed),
                        e
                    )));
                }
            }
        });
    };

    // ---- Delete handler ----
    let confirming = RwSignal::new(false);
    let on_confirm_delete = move || {
        if !state.is_connected.get() {
            return;
        }
        deleting.set(true);
        error.set(None);
        success.set(None);

        let id = channel_id_sig.get_value();
        spawn_local(async move {
            match state.rpc_call("channel.delete", json!({ "id": id })).await {
                Ok(_) => {
                    if let Some(cb) = on_deleted {
                        cb.run(());
                    }
                }
                Err(e) => {
                    error.set(Some(format!(
                        "{}{}",
                        t_string!(i18n, channel_config.toast_delete_failed),
                        e
                    )));
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
                            <div class="text-sm font-medium text-text-primary">{t!(i18n, channel_config.connection_status)}</div>
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
                                            {t!(i18n, channel_config.disconnect)}
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
                                            {move || if connecting.get() { t_string!(i18n, channel_config.connecting).to_string() } else { t_string!(i18n, channel_config.connect).to_string() }}
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

            // ---- Agent binding selector ----
            <AgentBindingSelector channel_id=channel_id />

            // ---- Loading state OR field section ----
            {move || {
                if loading.get() {
                    view! {
                        <div class="flex items-center justify-center py-12">
                            <div class="text-text-tertiary">{t!(i18n, channel_config.loading)}</div>
                        </div>
                    }.into_any()
                } else if fields.is_empty() {
                    view! {
                        <div class="p-4 bg-primary-subtle border border-primary/20 rounded-xl text-sm text-info">
                            {t!(i18n, channel_config.custom_config_page)}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <SettingsSection title=t_string!(i18n, channel_config.config_title).to_string() description=None>
                            {fields
                                .iter()
                                .map(|field| render_field(field, field_values))
                                .collect_view()}
                        </SettingsSection>
                    }.into_any()
                }
            }}

            // ---- Channel Pairing section ----
            <ChannelPairingSection channel_id=channel_id_sig />

            // ---- Action bar: Save + Delete + docs link ----
            <div class="flex items-center justify-between">
                <div class="flex items-center gap-2">
                    <SaveButton
                        on_click=move || on_save()
                        loading=saving.into()
                    />
                    {move || if confirming.get() {
                        view! {
                            <ConfirmButton confirming=confirming on_confirm=on_confirm_delete />
                        }.into_any()
                    } else {
                        view! {
                            <button
                                on:click=move |_| confirming.set(true)
                                disabled=move || deleting.get()
                                class="px-4 py-2 text-sm border border-danger/30 text-danger rounded-lg hover:bg-danger-subtle disabled:opacity-50 transition-colors"
                            >
                                {move || if deleting.get() { t_string!(i18n, channel_config.deleting).to_string() } else { t_string!(i18n, channel_config.delete_instance).to_string() }}
                            </button>
                        }.into_any()
                    }}
                </div>
                <a
                    href=docs_url
                    target="_blank"
                    rel="noopener noreferrer"
                    class="text-sm text-text-tertiary hover:text-primary transition-colors inline-flex items-center gap-1"
                >
                    {t!(i18n, channel_config.documentation)}
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
    let label: String = if field.required {
        format!("{} *", field.label)
    } else {
        field.label.to_string()
    };

    // For #[prop(optional)] fields we pass owned String.
    // Empty strings are treated as "no value" by the components.
    let help: Option<String> = if field.help.is_empty() {
        None
    } else {
        Some(field.help.to_string())
    };
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

        // -- Secret (stable-echo: never pre-fills the stored secret) --
        FieldKind::Secret => {
            let i18n = use_i18n();
            // config.get reports `has_<key>` instead of the plaintext secret.
            let has_key = move || -> bool {
                field_values
                    .get()
                    .get(&format!("has_{key}"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            };
            // Editable value always starts empty; empty on save = keep existing.
            let get_val = move || -> String {
                field_values
                    .get()
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let configured_ph = t_string!(i18n, settings.providers.key_configured_hint).to_string();
            let unset_ph = if placeholder.is_empty() {
                t_string!(i18n, settings.providers.key_unset_hint).to_string()
            } else {
                placeholder.to_string()
            };
            view! {
                <FormField label=label help_text=help>
                    // SecretInput's placeholder is non-reactive, so re-render on has_key.
                    {move || {
                        let ph = if has_key() { configured_ph.clone() } else { unset_ph.clone() };
                        view! {
                            <SecretInput
                                value=Signal::derive(get_val)
                                on_change=move |v: String| set_value(Value::String(v))
                                placeholder=ph
                                monospace=true
                            />
                        }
                    }}
                    <p class="mt-1 text-xs text-text-tertiary">
                        {move || if has_key() {
                            t_string!(i18n, settings.providers.key_status_configured).to_string()
                        } else {
                            t_string!(i18n, settings.providers.key_status_unset).to_string()
                        }}
                    </p>
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
                    .and_then(serde_json::Value::as_i64)
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
                    .and_then(serde_json::Value::as_bool)
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
                                    .map(std::string::ToString::to_string)
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

// ---------------------------------------------------------------------------
// ChannelPairingSection — approve pairing codes & manage approved senders
// ---------------------------------------------------------------------------

/// Inline section for approving channel pairing codes and listing approved senders.
#[component]
fn ChannelPairingSection(channel_id: StoredValue<String>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let pairing_code_input = RwSignal::new(String::new());
    let approving = RwSignal::new(false);
    let pairing_error = RwSignal::new(Option::<String>::None);
    let pairing_success = RwSignal::new(Option::<String>::None);

    // Which Aleph principal this sender will speak as. Empty = the owner,
    // which is the single-user default and what every approval did before this
    // control existed.
    //
    // This is the CHANNEL half of P0's identity link. Its consumer
    // (`inbound_router::executor` → `ScopeAttribution::personal`) has been live
    // since P1 with nothing producing for it, so a member's Telegram turns were
    // filed under the operator: their sessions in the operator's list, their
    // messages in the operator's memory partition, and the operator's curated
    // MEMORY.md injected into their prompts.
    let pairing_user_id = RwSignal::new(String::new());
    let dir = expect_context::<crate::state::user_directory::UserDirectoryState>();
    dir.ensure_loaded(state);

    // Approved senders list
    let approved_senders: RwSignal<Vec<ApprovedSenderInfo>> = RwSignal::new(Vec::new());
    let refresh_approved = RwSignal::new(0u32);

    // Load approved senders
    Effect::new(move |_| {
        let _ = refresh_approved.get();
        let ch_id = channel_id.get_value();
        spawn_local(async move {
            if let Ok(val) = state
                .rpc_call("channel.pairing.approved", json!({ "channel": ch_id }))
                .await
            {
                if let Some(arr) = val.get("senders").and_then(|v| v.as_array()) {
                    let senders: Vec<ApprovedSenderInfo> = arr
                        .iter()
                        .map(|v| ApprovedSenderInfo {
                            sender_id: v
                                .get("sender_id")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string(),
                            approved_at: v
                                .get("approved_at")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                        .collect();
                    approved_senders.set(senders);
                }
            }
        });
    });

    // Approve handler
    let on_approve = move || {
        let code = pairing_code_input.get().trim().to_uppercase();
        if code.is_empty() {
            pairing_error.set(Some(
                t_string!(i18n, channel_config.pairing_enter_code).to_string(),
            ));
            return;
        }
        approving.set(true);
        pairing_error.set(None);
        pairing_success.set(None);

        let ch_id = channel_id.get_value();
        let bind_to = pairing_user_id.get();
        spawn_local(async move {
            // Omit the key entirely when unset — the server's `COALESCE` writes
            // the owner, which is exactly the single-user behaviour, and an
            // explicit `null` and an absent key must not diverge here.
            let mut params = json!({ "channel": ch_id, "code": code });
            if !bind_to.is_empty() {
                params["user_id"] = json!(bind_to);
            }
            match state.rpc_call("channel.pairing.approve", params).await {
                Ok(_) => {
                    pairing_success.set(Some(
                        t_string!(i18n, channel_config.pairing_approved).to_string(),
                    ));
                    pairing_code_input.set(String::new());
                    refresh_approved.update(|n| *n += 1);
                }
                Err(e) => {
                    pairing_error.set(Some(format!(
                        "{}{}",
                        t_string!(i18n, channel_config.pairing_approve_failed),
                        e
                    )));
                }
            }
            approving.set(false);
        });
    };

    view! {
        <SettingsSection title=t_string!(i18n, channel_config.pairing_title).to_string() description=None>
            // Approve pairing code input
            <div class="space-y-3">
                <p class="text-sm text-text-secondary">
                    {t!(i18n, channel_config.pairing_intro)}
                </p>
                <div class="flex items-center gap-2">
                    <input
                        type="text"
                        placeholder=move || t_string!(i18n, channel_config.pairing_placeholder).to_string()
                        class="flex-1 px-3 py-2 text-sm font-mono tracking-widest bg-surface border border-border rounded-lg focus:outline-none focus:border-primary text-text-primary uppercase"
                        prop:value=move || pairing_code_input.get()
                        on:input=move |ev| pairing_code_input.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                on_approve();
                            }
                        }
                    />
                    <button
                        on:click=move |_| on_approve()
                        disabled=move || approving.get()
                        class="px-4 py-2 text-sm bg-primary text-text-inverse rounded-lg hover:bg-primary-hover disabled:opacity-50 transition-colors whitespace-nowrap"
                    >
                        {move || if approving.get() { t_string!(i18n, channel_config.approving).to_string() } else { t_string!(i18n, channel_config.approve).to_string() }}
                    </button>
                </div>

                // Which principal this sender speaks as. Rendered only when a
                // second person exists — on a single-user install the control
                // has exactly one option and would be pure noise.
                {move || {
                    let people = dir.selectable();
                    (people.len() > 1).then(|| view! {
                        <div class="flex items-center gap-2">
                            <label class="text-sm text-text-secondary whitespace-nowrap">
                                {t!(i18n, channel_config.pairing_speaks_as)}
                            </label>
                            <select
                                class="flex-1 px-3 py-2 text-sm bg-surface border border-border rounded-lg focus:outline-none focus:border-primary text-text-primary"
                                prop:value=move || pairing_user_id.get()
                                on:change=move |ev| pairing_user_id.set(event_target_value(&ev))
                            >
                                <option value="">{t!(i18n, channel_config.pairing_speaks_as_owner)}</option>
                                {people.into_iter().map(|(uid, name)| view! {
                                    <option value=uid.clone()>{name}</option>
                                }).collect_view()}
                            </select>
                        </div>
                    })
                }}

                // Error / Success
                {move || pairing_error.get().map(|e| view! {
                    <div class="text-sm text-danger">{e}</div>
                })}
                {move || pairing_success.get().map(|msg| view! {
                    <div class="text-sm text-success">{msg}</div>
                })}
            </div>

            // Approved senders list
            {move || {
                let senders = approved_senders.get();
                if senders.is_empty() {
                    view! {
                        <div class="text-sm text-text-tertiary mt-3">
                            {t!(i18n, channel_config.no_approved_senders)}
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <div class="mt-3 space-y-1">
                            <div class="text-xs text-text-tertiary font-medium uppercase tracking-wider mb-2">
                                {t!(i18n, channel_config.approved_senders)}
                            </div>
                            {senders.into_iter().map(|s| {
                                let sender_id = s.sender_id;
                                let ch_id_for_revoke = channel_id.get_value();
                                let sid_for_revoke = sender_id.clone();
                                view! {
                                    <div class="flex items-center justify-between px-3 py-2 bg-surface-raised border border-border rounded-lg">
                                        <div class="flex items-center gap-2">
                                            <span class="w-2 h-2 rounded-full bg-success flex-shrink-0" />
                                            <span class="text-sm font-mono text-text-primary">{sender_id}</span>
                                        </div>
                                        <button
                                            on:click=move |_| {
                                                let ch = ch_id_for_revoke.clone();
                                                let sid = sid_for_revoke.clone();
                                                spawn_local(async move {
                                                    let _ = state.rpc_call(
                                                        "channel.pairing.revoke",
                                                        json!({ "channel": ch, "sender_id": sid }),
                                                    ).await;
                                                    refresh_approved.update(|n| *n += 1);
                                                });
                                            }
                                            class="text-xs text-danger hover:text-danger-hover transition-colors"
                                        >
                                            {t!(i18n, channel_config.revoke)}
                                        </button>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }
            }}
        </SettingsSection>
    }
}

#[derive(Debug, Clone)]
struct ApprovedSenderInfo {
    sender_id: String,
    #[allow(dead_code)]
    approved_at: String,
}
