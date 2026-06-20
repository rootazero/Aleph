//! Store-specific secret helpers over the shared `crate::secrets` vault pipeline.
//!
//! Aleph already has a canonical secret-injection pipeline: secrets live in the
//! encrypted vault (`SharedTokenManager`), are referenced in text as
//! `{{secret:NAME}}`, and are resolved at the host boundary by
//! `crate::secrets::render_with_secrets` (which also records each injection for
//! leak detection). The Extensions Store reuses that pipeline rather than
//! inventing a parallel one. This module only adds the store's naming
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

/// Namespaced vault secret name for an extension config field.
///
/// Format: `ext.{kind}.{sanitized_id}.{field}` — guaranteed valid as a
/// `{{secret:NAME}}` placeholder name so it round-trips through the canonical
/// secret parser.
pub fn field_key(kind: ExtensionKind, id: &str, field: &str) -> String {
    format!("ext.{}.{}.{}", kind.as_str(), sanitize(id), sanitize(field))
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
        assert_eq!(k, "ext.mcp.mcp-official_io.github.a_b.GITHUB_TOKEN");
        assert_eq!(
            secret_ref(&k),
            "{{secret:ext.mcp.mcp-official_io.github.a_b.GITHUB_TOKEN}}"
        );
    }

    #[test]
    fn ref_round_trips_through_canonical_parser() {
        // The store's name must be accepted by the same parser MCP spawn uses.
        let k = field_key(ExtensionKind::Mcp, "weird id:with/slashes", "API.KEY");
        let refs = crate::secrets::extract_secret_refs(&secret_ref(&k)).unwrap();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, k);
    }
}
