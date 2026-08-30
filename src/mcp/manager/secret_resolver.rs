//! Per-spawn secret injection for MCP server environments and remote headers.
//!
//! MCP env values and remote-transport headers may carry `{{secret:NAME}}`
//! references (written by the Extensions Hub install flow; never plaintext).
//! This module resolves them into a fresh map at spawn/connect time, using the
//! shared `crate::secrets::AsyncSecretResolver` pipeline (same vault + leak
//! detection as WASM credential injection). Resolved values reach only the
//! spawned child's env or that connection's header map — never the daemon's own
//! process env.

use std::collections::HashMap;

use crate::secrets::{render_with_secrets, AsyncSecretResolver};

/// Resolve `{{secret:NAME}}` placeholders in a name→value map (stdio env, or a
/// remote transport's HTTP headers).
///
/// - Values with no `{{secret:` marker pass through unchanged (process-env
///   `${VAR}` expansion is handled separately, upstream).
/// - A value whose placeholder cannot be resolved — no resolver wired, or the
///   secret is missing/errors — is **dropped** (the key is omitted, with a
///   warning). The server is never reached with an unresolved placeholder or a
///   leaked literal; fail-closed.
/// - A *resolved* value that contains CR/LF/NUL is rejected (key dropped
///   with a warning). CR/LF would fail HTTP `HeaderValue::from_str` at
///   request time — failing fast at spawn time names the offending key
///   for the operator. NUL would be silently truncated by the kernel
///   for `cmd.env()` on Unix, leaving the spawned child with a
///   half-secret; the truncated value is worse than missing.
pub async fn resolve_secret_map(
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
                    if let Some(bad) = first_unsafe_byte(&rendered) {
                        tracing::warn!(
                            key = %key,
                            byte = bad as u32,
                            "resolved MCP secret contains CR/LF/NUL; omitting key"
                        );
                        continue;
                    }
                    // rust-doctor-disable-next-line excessive-clone
                    out.insert(key.clone(), rendered);
                }
                Err(e) => tracing::warn!(
                    key = %key,
                    error = %e,
                    "MCP secret reference unresolved; omitting key"
                ),
            },
            None => tracing::warn!(
                key = %key,
                "MCP value has a {{secret:..}} reference but no resolver is wired; omitting key"
            ),
        }
    }
    out
}

fn first_unsafe_byte(s: &str) -> Option<u8> {
    s.bytes().find(|b| matches!(b, b'\r' | b'\n' | 0))
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
        let out = resolve_secret_map(&env, Some(&Fake)).await;
        assert_eq!(out.get("TOKEN").map(String::as_str), Some("plain-value"));
        assert_eq!(out.get("PLAIN").map(String::as_str), Some("literal"));
        // unresolved secret dropped — never spawned as a literal placeholder
        assert!(!out.contains_key("MISSING"));
    }

    /// The same resolver serves remote-transport headers: an `Authorization`
    /// header stored as a `{{secret:..}}` reference must arrive as a real bearer
    /// value, and an unresolvable one must be dropped rather than sent literally.
    #[tokio::test]
    async fn resolves_remote_transport_headers() {
        let headers = env_with(&[
            ("Authorization", "Bearer {{secret:ext.mcp.x.TOKEN}}"),
            ("X-Region", "us"),
            ("X-Bad", "{{secret:ext.mcp.x.NOPE}}"),
        ]);
        let out = resolve_secret_map(&headers, Some(&Fake)).await;
        assert_eq!(
            out.get("Authorization").map(String::as_str),
            Some("Bearer plain-value")
        );
        assert_eq!(out.get("X-Region").map(String::as_str), Some("us"));
        assert!(!out.contains_key("X-Bad"));
    }

    #[tokio::test]
    async fn no_resolver_drops_secret_refs_but_keeps_plain() {
        let env = env_with(&[
            ("TOKEN", "{{secret:ext.mcp.x.TOKEN}}"),
            ("PLAIN", "literal"),
        ]);
        let out = resolve_secret_map(&env, None).await;
        assert!(!out.contains_key("TOKEN"));
        assert_eq!(out.get("PLAIN").map(String::as_str), Some("literal"));
    }
}
