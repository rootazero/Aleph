//! Per-spawn secret injection for MCP server environments.
//!
//! MCP env values may carry `{{secret:NAME}}` references (written by the
//! Extensions Store install flow; never plaintext). This module resolves them
//! into a fresh env map at spawn time, using the shared
//! `crate::secrets::AsyncSecretResolver` pipeline (same vault + leak detection
//! as HTTP-header and WASM credential injection). The resolved values are
//! placed only into the spawned child's env — never the daemon's own process
//! env.

use std::collections::HashMap;

use crate::secrets::{render_with_secrets, AsyncSecretResolver};

/// Resolve `{{secret:NAME}}` placeholders in MCP env values just before spawn.
///
/// - Values with no `{{secret:` marker pass through unchanged (process-env
///   `${VAR}` expansion is handled separately, upstream).
/// - A value whose placeholder cannot be resolved — no resolver wired, or the
///   secret is missing/errors — is **dropped** (the key is omitted, with a
///   warning). The child is never spawned with an unresolved placeholder or a
///   leaked literal; fail-closed.
pub async fn resolve_secret_env(
    env: &HashMap<String, String>,
    resolver: Option<&dyn AsyncSecretResolver>,
) -> HashMap<String, String> {
    let mut out = HashMap::with_capacity(env.len());
    for (key, value) in env {
        if !value.contains("{{secret:") {
            // rust-doctor-disable-next-line excessive-clone
            out.insert(key.clone(), value.clone());
            continue;
        }
        match resolver {
            Some(r) => match render_with_secrets(value, r).await {
                Ok((rendered, _injected)) => {
                    // rust-doctor-disable-next-line excessive-clone
                    out.insert(key.clone(), rendered);
                }
                Err(e) => tracing::warn!(
                    key = %key,
                    error = %e,
                    "MCP secret env reference unresolved; omitting key from child env"
                ),
            },
            None => tracing::warn!(
                key = %key,
                "MCP env has a {{secret:..}} reference but no resolver is wired; omitting key"
            ),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::types::{DecryptedSecret, SecretError};

    struct Fake;
    #[async_trait::async_trait]
    impl AsyncSecretResolver for Fake {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            if name == "ext.mcp.x.TOKEN" {
                Ok(DecryptedSecret::new("plain-value".to_string()))
            } else {
                Err(SecretError::NotFound(name.to_string()))
            }
        }
    }

    fn env_with(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[tokio::test]
    async fn resolves_only_secret_refs_and_drops_unresolved() {
        let env = env_with(&[
            ("TOKEN", "{{secret:ext.mcp.x.TOKEN}}"),
            ("PLAIN", "literal"),
            ("MISSING", "{{secret:ext.mcp.x.NOPE}}"),
        ]);
        let out = resolve_secret_env(&env, Some(&Fake)).await;
        assert_eq!(out.get("TOKEN").map(String::as_str), Some("plain-value"));
        assert_eq!(out.get("PLAIN").map(String::as_str), Some("literal"));
        // unresolved secret dropped — never spawned as a literal placeholder
        assert!(!out.contains_key("MISSING"));
    }

    #[tokio::test]
    async fn no_resolver_drops_secret_refs_but_keeps_plain() {
        let env = env_with(&[
            ("TOKEN", "{{secret:ext.mcp.x.TOKEN}}"),
            ("PLAIN", "literal"),
        ]);
        let out = resolve_secret_env(&env, None).await;
        assert!(!out.contains_key("TOKEN"));
        assert_eq!(out.get("PLAIN").map(String::as_str), Some("literal"));
    }
}
