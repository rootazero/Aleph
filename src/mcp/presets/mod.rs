//! Built-in curated MCP preset catalog.
//!
//! A vetted, in-binary list of recommended MCP servers. Pure data: the Hub
//! primer (`hub::official_mcp`) projects this catalog into the `aleph-hub`
//! cache slot, and install/installed-status flow through the Hub pipeline.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::mcp::manager::McpTransportType;

/// Curated MCP server the user can one-click enable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPreset {
    /// Stable slug, the unique install key (also used as the server id).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Audience bucket.
    pub category: PresetCategory,
    /// One-line description (Chinese, user-facing).
    pub description: String,
    /// Vendor name.
    pub vendor: String,
    /// First-party official server (vs community).
    pub official: bool,
    /// Launch options, ranked: remote first, stdio fallback.
    pub transports: Vec<PresetTransport>,
    /// Env vars the user must/should provide.
    #[serde(default)]
    pub required_env: Vec<PresetEnvVar>,
    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Post-install setup guidance shown to the user (Chinese, user-facing).
    /// For presets needing out-of-band setup (e.g. a local editor-embedded
    /// server). `None` = no extra steps. `serde(default)` keeps old catalog
    /// entries (without this key) parseable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_install: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetCategory {
    Developer,
    Daily,
}

/// One launch option for a preset. `kind` reuses the manager transport enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetTransport {
    pub kind: McpTransportType,
    /// stdio executable (e.g. "npx", "uvx").
    #[serde(default)]
    pub command: Option<String>,
    /// stdio args; may contain `<ENV_KEY>` placeholders.
    #[serde(default)]
    pub args: Vec<String>,
    /// remote url; may contain `<ENV_KEY>` placeholders.
    #[serde(default)]
    pub url: Option<String>,
    /// Runtime needed for stdio (e.g. "node", "python"); None = no runtime
    /// (remote endpoints).
    #[serde(default)]
    pub requires_runtime: Option<String>,
}

/// An env var the preset needs; drives the key-entry UI and the NeedsKey reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetEnvVar {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Secret-looking value: masked in UI, never echoed back.
    #[serde(default)]
    pub secret: bool,
    /// Must be present for install to proceed (unless `default` is set).
    #[serde(default)]
    pub required: bool,
    /// Fallback value when the user provides none.
    #[serde(default)]
    pub default: Option<String>,
    /// Where to obtain the value.
    #[serde(default)]
    pub how_to_get_url: Option<String>,
}

const CATALOG_JSON: &str = include_str!("catalog.json");

/// Parsed, in-binary preset catalog (single source of truth).
///
/// A malformed bundled catalog is logged at error level and degrades to an
/// empty slice rather than panicking at startup. The Hub primer that
/// projects this catalog into the `aleph-hub` cache slot will simply have
/// nothing to project; the rest of the daemon is unaffected.
pub fn catalog() -> &'static [McpPreset] {
    static CELL: OnceLock<Vec<McpPreset>> = OnceLock::new();
    CELL.get_or_init(|| match serde_json::from_str(CATALOG_JSON) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::error!(
                error = %e,
                "bundled MCP preset catalog.json is malformed; returning empty catalog"
            );
            Vec::new()
        }
    })
}

/// Look up a preset by id.
pub fn find(id: &str) -> Option<&'static McpPreset> {
    catalog().iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_has_first_batch() {
        let all = catalog();
        // All 7 built-in official preset ids must be present
        for id in [
            "context7",
            "zhipu-vision",
            "amap",
            "minimax",
            "volcengine-veimagex",
            "siliconflow",
            "t8star",
        ] {
            assert!(find(id).is_some(), "missing preset: {id}");
        }
        // ids are unique
        let mut ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(ids, dedup, "duplicate preset id in catalog.json");
        // every preset has at least one transport
        assert!(all.iter().all(|p| !p.transports.is_empty()));
    }

    #[test]
    fn amap_requires_secret_key_and_has_remote_first() {
        let amap = find("amap").expect("amap present");
        assert!(amap
            .required_env
            .iter()
            .any(|e| e.key == "AMAP_MAPS_API_KEY" && e.secret && e.required));
        // remote first: the first transport is http and its url contains a key placeholder
        let first = &amap.transports[0];
        assert_eq!(first.kind, McpTransportType::Http);
        assert!(first
            .url
            .as_deref()
            .unwrap()
            .contains("<AMAP_MAPS_API_KEY>"));
    }

    #[test]
    fn post_install_defaults_to_none_when_absent() {
        // Back-compat: a preset JSON without the post_install key still parses.
        let json = r#"{
            "id": "x", "name": "X", "category": "developer",
            "description": "d", "vendor": "V", "official": true,
            "transports": [{ "kind": "http", "url": "https://x/mcp" }]
        }"#;
        let p: McpPreset = serde_json::from_str(json).expect("parse");
        assert!(p.post_install.is_none());
    }

    #[test]
    fn unreal_engine_preset_is_local_http_with_guidance() {
        let ue = find("unreal-engine").expect("unreal-engine present");
        assert_eq!(ue.transports.len(), 1);
        let t = &ue.transports[0];
        assert_eq!(t.kind, McpTransportType::Http);
        assert_eq!(t.url.as_deref(), Some("http://127.0.0.1:8000/mcp"));
        assert!(ue.required_env.is_empty());
        assert!(ue.official);
        let pi = ue.post_install.as_deref().expect("post_install set");
        assert!(pi.contains("Unreal MCP"));
    }
}
