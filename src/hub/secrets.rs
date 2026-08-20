//! Hub-specific secret helpers over the shared `crate::secrets` vault pipeline.
//!
//! Aleph already has a canonical secret-injection pipeline: secrets live in the
//! encrypted vault (`SharedTokenManager`), are referenced in text as
//! `{{secret:NAME}}`, and are resolved at the host boundary by
//! `crate::secrets::render_with_secrets` (which also records each injection for
//! leak detection). The Extensions Hub reuses that pipeline rather than
//! inventing a parallel one. This module only adds the hub's naming
//! convention: a namespaced, placeholder-safe vault name per extension config
//! field, plus the `{{secret:NAME}}` reference written into mcp_config.json.
//!
//! MCP env values store the *reference* (`{{secret:NAME}}`), never plaintext;
//! the reference resolves per-server into that child's env only at spawn — see
//! `src/mcp/manager/secret_resolver.rs`.

use crate::hub::types::ExtensionKind;

/// Map any char outside the placeholder-safe charset (`[A-Za-z0-9_.-]`,
/// enforced by `crate::secrets::extract_secret_refs`) to `_`.
fn sanitize(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Short stable hash of `id` to make `field_key` collision-resistant.
///
/// `sanitize` alone is not injective: `"foo bar"`, `"foo+bar"`, and `"foo_bar"`
/// all map to the same sanitized id, which means three distinct extension ids
/// would share a single vault key and overwrite each other's secrets on
/// install. We append the first 16 hex chars of a domain-separated SHA-256;
/// the collision space is 2^64 and a preimage search against the domain
/// separator is what an attacker would face.
fn id_hash(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"aleph-hub-secret-key::");
    hasher.update(id.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Namespaced vault secret name for an extension config field.
///
/// Format: `ext.{kind}.{sanitized_id}.{hash(id)}.{field}` — guaranteed valid
/// as a `{{secret:NAME}}` placeholder name (so it round-trips through the
/// canonical secret parser) AND collision-resistant across ids that sanitize
/// to the same string (`"foo bar"`, `"foo+bar"`, `"foo_bar"`).
pub fn field_key(kind: ExtensionKind, id: &str, field: &str) -> String {
    format!(
        "ext.{}.{}.{}.{}",
        kind.as_str(),
        sanitize(id),
        id_hash(id),
        sanitize(field)
    )
}

/// The `{{secret:NAME}}` reference written into config (never plaintext).
pub fn secret_ref(name: &str) -> String {
    format!("{{{{secret:{name}}}}}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_key_is_namespaced_and_placeholder_safe() {
        let k = field_key(
            ExtensionKind::Mcp,
            "mcp-official:io.github.a/b",
            "GITHUB_TOKEN",
        );
        // Hash suffix is fixed for a fixed id; assert only the surrounding
        // shape so the test does not break if the hash algorithm is tuned.
        assert!(k.starts_with("ext.mcp.mcp-official_io.github.a_b."));
        assert!(k.ends_with(".GITHUB_TOKEN"));
        assert_eq!(k.len(), "ext.mcp.mcp-official_io.github.a_b.".len() + 16 + ".GITHUB_TOKEN".len());
        assert!(secret_ref(&k).starts_with("{{secret:ext.mcp.mcp-official_io.github.a_b."));
        assert!(secret_ref(&k).ends_with(".GITHUB_TOKEN}}"));
    }

    #[test]
    fn ref_round_trips_through_canonical_parser() {
        // The hub's name must be accepted by the same parser MCP spawn uses.
        let k = field_key(ExtensionKind::Mcp, "weird id:with/slashes", "API.KEY");
        let refs = crate::secrets::extract_secret_refs(&secret_ref(&k)).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, k);
    }

    /// Regression: three distinct ids that `sanitize` to the same string MUST
    /// produce three distinct vault keys. Without the hash suffix, an attacker
    /// could install an extension whose id collides with an existing secret and
    /// silently overwrite it.
    #[test]
    fn field_keys_are_collision_resistant_across_sanitize_collisions() {
        let a = field_key(ExtensionKind::Mcp, "foo bar", "TOKEN");
        let b = field_key(ExtensionKind::Mcp, "foo+bar", "TOKEN");
        let c = field_key(ExtensionKind::Mcp, "foo_bar", "TOKEN");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
