//! Resolving `{{secret:NAME}}` references in a plugin's stored configuration.
//!
//! # Two forms of one value
//!
//! `plugins.toml` is plaintext, and its doc has said since the store was
//! written that a value a `config_ui_hints` entry marks `sensitive` belongs in
//! the vault as a `{{secret:NAME}}` reference. Nothing resolved one, so the
//! reference reached the plugin verbatim: a plugin configured the documented
//! way received the literal string `{{secret:SLACK_TOKEN}}` as its API key.
//!
//! So a setting has a **stored form** (the placeholder) and a **runtime form**
//! (the resolved value), and the conversion may happen at exactly one edge —
//! the one where the value is handed to plugin code.
//!
//! # Which callers get which form
//!
//! `ExtensionManager::plugin_settings` has four consumers and they do not want
//! the same thing:
//!
//! | caller | form | why |
//! |---|---|---|
//! | `publish_plugin_settings` → `PluginLoader` | runtime | the WASM guest / MCP child needs the real value |
//! | `hooks::executor` → `settings_env` | runtime | the hook subprocess needs the real value |
//! | `plugin_manage(config_get / show)` | **stored** | this text goes into the model's context |
//! | `plugin.config.get` RPC | **stored** | this text goes to Panel |
//!
//! Resolving inside `plugin_settings` itself would have been one line and
//! would have piped every configured secret into the transcript and the
//! settings UI — the display faces are exactly the ones that must keep the
//! placeholder. That is why this is a separate function with a name that says
//! which side it is on, rather than a flag on the existing one.
//!
//! # Failure direction
//!
//! A reference that cannot be resolved (no vault, no such key, locked vault)
//! **drops the key** with a warning, rather than passing the literal
//! `{{secret:...}}` through. Mirrors `mcp::manager::secret_resolver`: a plugin
//! that reads a missing setting fails in its own terms, whereas one handed a
//! placeholder sends it to a remote host as if it were a credential.

use crate::secrets::AsyncSecretResolver;

/// Marker that identifies a value needing resolution. Checked before doing any
/// work so the overwhelmingly common case — settings with no secrets at all —
/// costs one substring search per value.
const MARKER: &str = "{{secret:";

/// Resolve every `{{secret:NAME}}` reference in a settings object.
///
/// Returns a new object; the stored configuration is never mutated. Non-string
/// values pass through untouched, and nested objects/arrays are walked so a
/// reference inside a structured setting is not silently missed.
pub async fn resolve_settings(
    settings: &serde_json::Value,
    resolver: Option<&dyn AsyncSecretResolver>,
    plugin_id: &str,
) -> serde_json::Value {
    if !contains_reference(settings) {
        // rust-doctor-disable-next-line excessive-clone
        return settings.clone();
    }
    resolve_value(settings, resolver, plugin_id).await
}

/// Whether any string anywhere in this value carries the marker.
fn contains_reference(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => s.contains(MARKER),
        serde_json::Value::Array(items) => items.iter().any(contains_reference),
        serde_json::Value::Object(map) => map.values().any(contains_reference),
        _ => false,
    }
}

/// Recursive worker. Boxed because `async fn` cannot recurse directly.
fn resolve_value<'a>(
    value: &'a serde_json::Value,
    resolver: Option<&'a dyn AsyncSecretResolver>,
    plugin_id: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send + 'a>> {
    Box::pin(async move {
        match value {
            serde_json::Value::String(s) if s.contains(MARKER) => {
                match resolver {
                    Some(r) => match crate::secrets::render_with_secrets(s, r).await {
                        Ok((rendered, _injected)) => serde_json::Value::String(rendered),
                        Err(e) => {
                            tracing::warn!(
                                plugin_id, error = %e,
                                "plugin setting references a secret that could not be \
                                 resolved; the key is omitted rather than passed through \
                                 as a literal placeholder"
                            );
                            serde_json::Value::Null
                        }
                    },
                    None => {
                        tracing::warn!(
                            plugin_id,
                            "plugin setting references a secret but no vault is available; \
                             the key is omitted"
                        );
                        serde_json::Value::Null
                    }
                }
            }
            serde_json::Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(resolve_value(item, resolver, plugin_id).await);
                }
                serde_json::Value::Array(out)
            }
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (key, item) in map {
                    let resolved = resolve_value(item, resolver, plugin_id).await;
                    // Drop rather than emit null: a plugin reading an absent
                    // key takes its own default, while one reading `null` may
                    // well send it onward as a credential.
                    if !resolved.is_null() {
                        out.insert(key.clone(), resolved);
                    }
                }
                serde_json::Value::Object(out)
            }
            // rust-doctor-disable-next-line excessive-clone
            other => other.clone(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::types::{DecryptedSecret, SecretError};
    use async_trait::async_trait;
    use serde_json::json;

    struct Fake;

    #[async_trait]
    impl AsyncSecretResolver for Fake {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            if name == "SLACK_TOKEN" {
                Ok(DecryptedSecret::new("xoxb-real"))
            } else {
                Err(SecretError::NotFound(name.to_string()))
            }
        }
    }

    #[tokio::test]
    async fn a_reference_is_replaced_by_the_vault_value() {
        let settings = json!({"api_key": "{{secret:SLACK_TOKEN}}", "endpoint": "https://x"});
        let out = resolve_settings(&settings, Some(&Fake), "p").await;
        assert_eq!(out["api_key"], json!("xoxb-real"));
        assert_eq!(out["endpoint"], json!("https://x"), "plain values pass through");
    }

    #[tokio::test]
    async fn an_unresolvable_reference_drops_the_key_rather_than_leaking_the_placeholder() {
        let settings = json!({"api_key": "{{secret:MISSING}}", "endpoint": "https://x"});
        let out = resolve_settings(&settings, Some(&Fake), "p").await;
        assert!(
            out.get("api_key").is_none(),
            "an unresolved reference must not reach the plugin at all: passing \
             `{{{{secret:MISSING}}}}` through means the plugin sends that literal \
             to a remote host as a credential"
        );
        assert_eq!(out["endpoint"], json!("https://x"));
    }

    #[tokio::test]
    async fn with_no_vault_the_reference_is_dropped_not_passed_through() {
        let settings = json!({"api_key": "{{secret:SLACK_TOKEN}}"});
        let out = resolve_settings(&settings, None, "p").await;
        assert!(out.get("api_key").is_none());
    }

    #[tokio::test]
    async fn settings_without_references_are_returned_unchanged() {
        let settings = json!({"a": 1, "b": "plain", "c": {"d": true}});
        let out = resolve_settings(&settings, Some(&Fake), "p").await;
        assert_eq!(out, settings);
    }

    /// A reference nested inside a structured setting must resolve too — a
    /// walker that only looked at top-level strings would silently hand the
    /// placeholder to the plugin, which is the failure this module exists to
    /// prevent.
    #[tokio::test]
    async fn a_nested_reference_resolves() {
        let settings = json!({"auth": {"headers": ["{{secret:SLACK_TOKEN}}"]}});
        let out = resolve_settings(&settings, Some(&Fake), "p").await;
        assert_eq!(out["auth"]["headers"][0], json!("xoxb-real"));
    }

    /// Partial interpolation: the reference is one part of a longer string.
    #[tokio::test]
    async fn a_reference_embedded_in_a_larger_string_resolves_in_place() {
        let settings = json!({"header": "Bearer {{secret:SLACK_TOKEN}}"});
        let out = resolve_settings(&settings, Some(&Fake), "p").await;
        assert_eq!(out["header"], json!("Bearer xoxb-real"));
    }

    /// No model-facing or UI-facing surface may ask for the resolved form.
    ///
    /// The two mistakes here are not symmetric. Using the *stored* form on a
    /// runtime edge hands a plugin the literal `{{secret:NAME}}`, which fails
    /// loudly in that plugin's own terms. Using the *resolved* form on a
    /// display face writes a decrypted credential into the model's context or
    /// into Panel, and nothing anywhere reports it. Only the silent direction
    /// is worth a guard.
    ///
    /// A whole-directory rule rather than a list of the two known faces:
    /// everything under `builtin_tools/` is by construction something the model
    /// reads, and everything under `gateway/handlers/` is something a client
    /// reads. There are zero violations today, so the broad rule costs nothing
    /// — and a list of file names would go stale the first time someone adds a
    /// third face.
    #[test]
    fn no_model_or_client_facing_surface_reads_the_resolved_form() {
        let mut offenders = Vec::new();
        let mut scanned = 0usize;

        for dir in ["src/builtin_tools", "src/gateway/handlers"] {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
            let mut stack = vec![root];
            while let Some(path) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&path) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|e| e == "rs") {
                        let Ok(src) = std::fs::read_to_string(&p) else {
                            continue;
                        };
                        scanned += 1;
                        // Comments are documentation, not calls: this very
                        // guard is described in prose near some of these files.
                        let code: String = src
                            .replace('\r', "")
                            .lines()
                            .filter(|l| !l.trim_start().starts_with("//"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if code.contains("plugin_settings_for_runtime") {
                            offenders.push(p.display().to_string());
                        }
                    }
                }
            }
        }

        assert!(
            scanned > 50,
            "scanned only {scanned} files — the walk stopped finding sources and \
             would now pass by looking at nothing"
        );
        assert!(
            offenders.is_empty(),
            "these model-facing / client-facing files read the RESOLVED plugin \
             settings, which puts decrypted credentials into a transcript or a \
             settings page: {offenders:?}\nUse `plugin_settings` (stored form, \
             placeholders intact) on any surface a human or a model reads."
        );
    }
}
