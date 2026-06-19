//! Built-in curated MCP preset catalog.
//!
//! A vetted, in-binary list of recommended MCP servers users can enable with
//! one click (or via natural language). Pure data + pure decision logic; the
//! gateway handler is a thin adapter that drives `McpManagerHandle`.

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
    /// Mainland-China reachability hint (display only).
    pub reachability: Reachability,
    /// Launch options, ranked: remote first, stdio fallback.
    pub transports: Vec<PresetTransport>,
    /// Env vars the user must/should provide.
    #[serde(default)]
    pub required_env: Vec<PresetEnvVar>,
    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetCategory {
    Developer,
    Daily,
    ModelProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reachability {
    /// Built for mainland China, fully reachable.
    CnNative,
    /// Global service, mainland reachability not guaranteed.
    Global,
    /// Reported unreliable behind the GFW.
    CnUnreliable,
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
pub fn catalog() -> &'static [McpPreset] {
    static CELL: OnceLock<Vec<McpPreset>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON)
            .expect("bundled MCP preset catalog.json must be valid")
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
        // 首批 4 个 id 必须都在
        for id in ["context7", "amap", "minimax", "volcengine-veimagex"] {
            assert!(find(id).is_some(), "missing preset: {id}");
        }
        // id 唯一
        let mut ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(ids, dedup, "duplicate preset id in catalog.json");
        // 每个 preset 至少一个 transport
        assert!(all.iter().all(|p| !p.transports.is_empty()));
    }

    #[test]
    fn amap_requires_secret_key_and_has_remote_first() {
        let amap = find("amap").expect("amap present");
        assert!(amap
            .required_env
            .iter()
            .any(|e| e.key == "AMAP_MAPS_API_KEY" && e.secret && e.required));
        // 远程优先：第一个 transport 是 http 且 url 含 key 占位
        let first = &amap.transports[0];
        assert_eq!(first.kind, McpTransportType::Http);
        assert!(first.url.as_deref().unwrap().contains("<AMAP_MAPS_API_KEY>"));
    }
}
