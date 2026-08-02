//! `server/discover` — capability discovery and the era probe.
//!
//! Modern servers **must** implement `server/discover`; it reports the protocol
//! versions, capabilities, and identity a client would otherwise have learned
//! from the `initialize` handshake. Aleph uses it for both of its jobs:
//!
//! 1. **Capability discovery.** One round-trip replaces the handshake and tells
//!    the connection whether the server serves tools, resources, and prompts.
//! 2. **Era detection.** On stdio there is no HTTP status code to fall back on,
//!    so the spec prescribes probing with `server/discover` and treating any
//!    reply that is *not* a recognized modern one as "this server is legacy —
//!    fall back to `initialize`".

use serde::Deserialize;
use serde_json::Value;

use crate::mcp::protocol::{ServerCapabilities, ServerInfo};

use super::META_SERVER_INFO;

/// The discovery method name.
pub const DISCOVER_METHOD: &str = "server/discover";

/// A server's answer to `server/discover`.
///
/// Every member is optional-by-default on the wire: a server that omits
/// `capabilities` is reporting "none", not a malformed response, and refusing
/// to parse such a reply would fail the era probe for a server that is in fact
/// modern.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResult {
    /// Protocol revisions the server implements. The client picks one of these.
    #[serde(default)]
    pub supported_versions: Vec<String>,
    /// What the server serves: tools, resources, prompts.
    #[serde(default)]
    pub capabilities: ServerCapabilities,
    /// Natural-language guidance for the model on how to use this server.
    #[serde(default)]
    pub instructions: Option<String>,
    /// `_meta`, which is where a modern server reports its own identity.
    #[serde(default, rename = "_meta")]
    pub meta: Option<Value>,
}

impl DiscoverResult {
    /// The server's self-reported name and version.
    ///
    /// Self-reported and unverified by the protocol: display, logging, and
    /// debugging only. It must never gate behavior or a security decision.
    #[must_use]
    pub fn server_info(&self) -> Option<ServerInfo> {
        let raw = self.meta.as_ref()?.get(META_SERVER_INFO)?;
        serde_json::from_value(raw.clone()).ok()
    }
}

/// Which dialect a `server/discover` answer commits the connection to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionChoice {
    /// Speak the modern, stateless shape at this revision.
    Modern(String),
    /// The server answered `server/discover` but offers only revisions older
    /// than the modern one — it is dual-era, and the `initialize` handshake is
    /// the way in.
    Legacy,
    /// No mutually supported revision: everything the server offers is newer
    /// than what Aleph implements.
    Incompatible(Vec<String>),
}

/// Pick the revision to speak from what the server says it supports.
///
/// Version strings are ISO-8601 dates, so lexicographic order is chronological
/// order — an unknown revision *newer* than Aleph's is a genuine
/// incompatibility, while unknown *older* ones mean the server is dual-era and
/// the legacy handshake still works.
///
/// A server that answers `server/discover` but names no versions at all is
/// taken at its word that it is modern: it implemented a method that only
/// exists in this revision.
#[must_use]
pub fn select_version(supported: &[String], preferred: &str) -> VersionChoice {
    if supported.is_empty() || supported.iter().any(|v| v == preferred) {
        return VersionChoice::Modern(preferred.to_string());
    }

    if supported.iter().all(|v| v.as_str() < preferred) {
        return VersionChoice::Legacy;
    }

    VersionChoice::Incompatible(supported.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::modern::MCP_MODERN_PROTOCOL_VERSION;
    use serde_json::json;

    fn parse(raw: Value) -> DiscoverResult {
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn parses_a_full_discover_result() {
        let result = parse(json!({
            "resultType": "complete",
            "supportedVersions": ["2026-07-28"],
            "capabilities": {"tools": {}, "resources": {}},
            "_meta": {
                "io.modelcontextprotocol/serverInfo": {
                    "name": "ExampleServer",
                    "version": "1.0.0"
                }
            },
            "instructions": "This server provides weather utilities.",
            "ttlMs": 3_600_000,
            "cacheScope": "public"
        }));

        assert_eq!(result.supported_versions, vec!["2026-07-28"]);
        assert!(result.capabilities.tools.is_some());
        assert!(result.capabilities.resources.is_some());
        assert!(result.capabilities.prompts.is_none());
        assert_eq!(
            result.instructions.as_deref(),
            Some("This server provides weather utilities.")
        );
        let info = result.server_info().unwrap();
        assert_eq!(info.name, "ExampleServer");
    }

    #[test]
    fn parses_a_minimal_discover_result() {
        // A modern server that reports nothing must still probe as modern;
        // refusing to parse would send Aleph down the legacy path.
        let result = parse(json!({"resultType": "complete"}));

        assert!(result.supported_versions.is_empty());
        assert!(result.server_info().is_none());
        assert!(result.instructions.is_none());
        assert_eq!(
            select_version(&result.supported_versions, MCP_MODERN_PROTOCOL_VERSION),
            VersionChoice::Modern(MCP_MODERN_PROTOCOL_VERSION.to_string())
        );
    }

    #[test]
    fn unknown_extra_fields_do_not_break_parsing() {
        // Later revisions will add members; a strict parser would misread the
        // era probe as a failure.
        let result = parse(json!({
            "supportedVersions": ["2026-07-28"],
            "somethingFromTheFuture": {"nested": true}
        }));

        assert_eq!(result.supported_versions, vec!["2026-07-28"]);
    }

    #[test]
    fn selects_the_preferred_version_when_offered() {
        let supported = vec!["2025-11-25".to_string(), "2026-07-28".to_string()];

        assert_eq!(
            select_version(&supported, MCP_MODERN_PROTOCOL_VERSION),
            VersionChoice::Modern(MCP_MODERN_PROTOCOL_VERSION.to_string())
        );
    }

    #[test]
    fn only_older_revisions_means_use_the_handshake() {
        let supported = vec!["2025-03-26".to_string(), "2025-11-25".to_string()];

        assert_eq!(
            select_version(&supported, MCP_MODERN_PROTOCOL_VERSION),
            VersionChoice::Legacy
        );
    }

    #[test]
    fn only_newer_revisions_is_a_real_incompatibility() {
        // Aleph cannot invent a revision it does not implement; saying so beats
        // silently speaking the wrong one.
        let supported = vec!["2027-01-01".to_string()];

        assert_eq!(
            select_version(&supported, MCP_MODERN_PROTOCOL_VERSION),
            VersionChoice::Incompatible(supported)
        );
    }

    #[test]
    fn server_info_is_ignored_when_malformed() {
        let result = parse(json!({
            "_meta": {"io.modelcontextprotocol/serverInfo": "not-an-object"}
        }));

        assert!(result.server_info().is_none());
    }
}
