//! Schema→widget form for the install config wizard. Net-new (no JSON-schema form
//! renderer existed). `fields_from` (pure) merges the always-available
//! `disclosure.secrets` with the optional `config_schema` (JSON Schema) into a flat
//! field list; the component dispatches each field to an existing primitive.
use leptos::prelude::*;
use serde_json::{Map, Value};

use crate::api::extensions::SecretDisclosure;
use crate::components::forms::FormField;
use crate::components::ui::SecretInput;

#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Text,
    Secret,
    Bool,
    Number,
    Select(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldSpec {
    pub name: String,
    pub label: String,
    pub help: String,
    pub required: bool,
    pub secret: bool,
    pub placeholder: String,
    pub default: Option<String>,
    pub field_type: FieldType,
    /// Console / signup page for this value, when the catalog declares one.
    pub how_to_get_url: Option<String>,
}

/// Build the form's fields. Field set = union of `disclosure.secrets` names and
/// `config_schema.properties` keys. Sensitivity comes from `secrets[*].sensitive`
/// (overriding schema type). `required` = name ∈ `missing` OR ∈ `schema.required`.
#[must_use]
pub fn fields_from(
    config_schema: Option<&Value>,
    secrets: &[SecretDisclosure],
    missing: &[String],
) -> Vec<FieldSpec> {
    let props = config_schema
        .and_then(|s| s.get("properties"))
        .and_then(Value::as_object);
    let schema_required: Vec<String> = config_schema
        .and_then(|s| s.get("required"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Ordered field names: disclosure.secrets first (the must-fill set), then any
    // extra schema properties not already covered.
    let mut names: Vec<String> = secrets.iter().map(|s| s.name.clone()).collect();
    if let Some(p) = props {
        for k in p.keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    }

    names
        .into_iter()
        .map(|name| {
            let secret_decl = secrets.iter().find(|s| s.name == name);
            let prop = props.and_then(|p| p.get(&name));
            let is_secret = secret_decl.map(|s| s.sensitive).unwrap_or(false);
            let required = missing.contains(&name) || schema_required.contains(&name);
            let help = prop
                .and_then(|p| p.get("description"))
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| secret_decl.map(|s| s.purpose.clone()))
                .unwrap_or_default();
            let default = prop
                .and_then(|p| p.get("default"))
                .and_then(Value::as_str)
                .map(String::from);
            let placeholder = prop
                .and_then(|p| p.get("placeholder").or_else(|| p.get("valueHint")))
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_default();
            let enum_vals: Option<Vec<String>> = prop
                .and_then(|p| p.get("enum"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                });
            let schema_type = prop
                .and_then(|p| p.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("string");
            let field_type = if is_secret {
                FieldType::Secret
            } else if let Some(opts) = enum_vals {
                FieldType::Select(opts)
            } else {
                match schema_type {
                    "boolean" => FieldType::Bool,
                    "integer" | "number" => FieldType::Number,
                    _ => FieldType::Text,
                }
            };
            FieldSpec {
                name: name.clone(),
                label: name,
                help,
                required,
                secret: is_secret,
                placeholder,
                default,
                field_type,
                how_to_get_url: secret_decl.and_then(|s| s.how_to_get_url.clone()),
            }
        })
        .collect()
}

/// Host portion of a URL, for use as link text. Falls back to the whole string
/// when it does not parse as `scheme://host/…` — the catalog is curated, but a
/// malformed entry should still render something clickable rather than nothing.
#[must_use]
pub fn link_host(url: &str) -> &str {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest.split('/').next().unwrap_or(rest))
}

/// Renders a JSON-Schema–derived form, dispatching each field to an existing
/// primitive widget. Values are written into `values` (a flat string map).
///
/// # Notes
/// - `FieldType::Bool` and `FieldType::Number` fall through to a plain text
///   input (v1: MCP env values are strings; the backend coerces).
/// - `SelectInput` from `forms.rs` requires `(&'static str, &'static str)` so
///   dynamic enum options use a raw `<select>` element instead.
/// - `TextInput` from `forms.rs` requires `&'static str` placeholders so
///   dynamic placeholders use a raw `<input>` element instead.
#[component]
#[must_use]
pub fn JsonSchemaForm(
    fields: Vec<FieldSpec>,
    values: RwSignal<Map<String, Value>>,
) -> impl IntoView {
    // Seed defaults once.
    let seed = fields.clone();
    Effect::new(move || {
        values.update(|m| {
            for f in &seed {
                if let Some(def) = &f.default {
                    m.entry(f.name.clone())
                        .or_insert_with(|| Value::String(def.clone()));
                }
            }
        });
    });

    view! {
        <div class="space-y-4">
            {fields.into_iter().map(|f| {
                let name = f.name.clone();
                let label = if f.required {
                    format!("{} *", f.label)
                } else {
                    f.label.clone()
                };
                let get = {
                    let name = name.clone();
                    move || {
                        values
                            .get()
                            .get(&name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    }
                };
                let set = {
                    let name = name.clone();
                    move |v: String| {
                        values.update(|m| {
                            m.insert(name.clone(), Value::String(v));
                        });
                    }
                };
                let placeholder = f.placeholder.clone();
                let help = f.help.clone();

                let widget = match f.field_type.clone() {
                    FieldType::Secret => {
                        if placeholder.is_empty() {
                            view! {
                                <SecretInput
                                    value=Signal::derive(get)
                                    on_change=set
                                    monospace=true
                                />
                            }
                            .into_any()
                        } else {
                            view! {
                                <SecretInput
                                    value=Signal::derive(get)
                                    on_change=set
                                    placeholder=placeholder
                                    monospace=true
                                />
                            }
                            .into_any()
                        }
                    }
                    FieldType::Select(opts) => {
                        // Dynamic options — use raw <select> (SelectInput requires &'static str).
                        view! {
                            <select
                                prop:value=Signal::derive(get)
                                on:change=move |ev| set(event_target_value(&ev))
                                class="w-full px-3 py-2 bg-surface-raised border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                            >
                                {opts.into_iter().map(|opt| {
                                    let label = opt.clone();
                                    view! { <option value=opt>{label}</option> }
                                }).collect_view()}
                            </select>
                        }
                        .into_any()
                    }
                    // Bool / Number / Text all fall through to a plain text input.
                    // Dynamic placeholder — use raw <input> (TextInput requires &'static str).
                    _ => {
                        view! {
                            <input
                                type="text"
                                prop:value=Signal::derive(get)
                                on:input=move |ev| set(event_target_value(&ev))
                                placeholder=placeholder
                                class="w-full px-3 py-2 bg-surface-raised border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30 focus:border-primary"
                            />
                        }
                        .into_any()
                    }
                };

                // The catalog's "where do I get this" link, rendered under the
                // input. Without it the Configure step asks for a key and says
                // nothing about where it comes from.
                // Link text is the URL's host, not a translated phrase: it needs
                // no locale file and it tells the user which console they are
                // about to open. The URL is sanitized before rendering so a
                // compromised catalog cannot inject a javascript:/data: link.
                let source_link = f.how_to_get_url.clone().and_then(|url| {
                    let safe = crate::components::markdown::sanitize_link_url(&url);
                    if safe.starts_with("#disallowed-") {
                        return None;
                    }
                    let label = format!("{} ↗", link_host(&safe));
                    Some(view! {
                        <a
                            href=safe
                            target="_blank"
                            rel="noopener noreferrer"
                            class="block text-xs text-primary hover:underline"
                        >
                            {label}
                        </a>
                    })
                });

                view! {
                    <FormField label=label help_text=(!help.is_empty()).then(|| help.clone())>
                        {widget}
                        {source_link}
                    </FormField>
                }
            }).collect_view()}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::extensions::SecretDisclosure;
    use serde_json::json;

    fn sd(name: &str, sensitive: bool) -> SecretDisclosure {
        SecretDisclosure {
            name: name.into(),
            purpose: format!("{name} purpose"),
            sensitive,
            how_to_get_url: None,
        }
    }

    /// Last hop of the catalog→form chain: a declared source URL has to reach
    /// the rendered field, and a field with no declared source stays bare.
    #[test]
    fn how_to_get_url_reaches_the_field() {
        let mut with_url = sd("AMAP_MAPS_API_KEY", true);
        with_url.how_to_get_url = Some("https://console.amap.com/dev/key/app".into());
        let fields = fields_from(None, &[with_url, sd("REGION", false)], &[]);
        assert_eq!(
            fields[0].how_to_get_url.as_deref(),
            Some("https://console.amap.com/dev/key/app")
        );
        assert!(fields[1].how_to_get_url.is_none());
    }

    #[test]
    fn link_host_strips_scheme_and_path() {
        assert_eq!(
            link_host("https://console.amap.com/dev/key/app"),
            "console.amap.com"
        );
        assert_eq!(
            link_host("https://platform.minimaxi.com"),
            "platform.minimaxi.com"
        );
        assert_eq!(link_host("not a url"), "not a url");
    }

    #[test]
    fn builds_from_disclosure_secrets_when_no_schema() {
        let secrets = vec![sd("GITHUB_TOKEN", true), sd("ACCOUNT", false)];
        let missing = vec!["GITHUB_TOKEN".to_string()];
        let fields = fields_from(None, &secrets, &missing);
        assert_eq!(fields.len(), 2);
        let tok = fields.iter().find(|f| f.name == "GITHUB_TOKEN").unwrap();
        assert!(tok.secret);
        assert!(tok.required); // present in `missing`
        assert_eq!(tok.field_type, FieldType::Secret);
        let acct = fields.iter().find(|f| f.name == "ACCOUNT").unwrap();
        assert!(!acct.secret);
        assert_eq!(acct.field_type, FieldType::Text);
    }

    #[test]
    fn schema_enriches_label_default_placeholder_and_type() {
        let schema = json!({
            "type": "object",
            "required": ["REGION"],
            "properties": {
                "REGION": { "type": "string", "description": "AWS region", "default": "us-east-1", "enum": ["us-east-1","eu-west-1"] },
                "GITHUB_TOKEN": { "type": "string", "description": "Token" }
            }
        });
        let secrets = vec![sd("GITHUB_TOKEN", true)];
        let fields = fields_from(Some(&schema), &secrets, &[]);
        let region = fields.iter().find(|f| f.name == "REGION").unwrap();
        assert!(region.required); // in schema.required
        assert_eq!(region.default.as_deref(), Some("us-east-1"));
        assert_eq!(
            region.field_type,
            FieldType::Select(vec!["us-east-1".into(), "eu-west-1".into()])
        );
        let tok = fields.iter().find(|f| f.name == "GITHUB_TOKEN").unwrap();
        assert!(tok.secret); // secret from disclosure even when schema-typed string
        assert_eq!(tok.field_type, FieldType::Secret);
    }

    #[test]
    fn secret_flag_forces_secret_type_over_schema_string() {
        let schema = json!({ "type":"object","properties": { "KEY": { "type":"string" } } });
        let fields = fields_from(Some(&schema), &[sd("KEY", true)], &[]);
        assert_eq!(fields[0].field_type, FieldType::Secret);
    }
}
