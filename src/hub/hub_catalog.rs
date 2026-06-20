//! Wire format for a versioned static Hub catalog artifact (the contract
//! produced by Aleph-Hub and consumed by `StaticHubProvider`). It is the
//! objective subset of `ExtensionEntry` — no per-user state ever crosses the
//! wire; `installed`/`enabled` are stamped locally.
//!
//! See docs/superpowers/specs/2026-06-20-extension-hub-federation-design.md §4.

use serde::Deserialize;

use crate::hub::types::{
    ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier,
};

/// Current artifact schema version this client understands.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct HubCatalogArtifact {
    pub manifest: HubCatalogManifest,
    #[serde(default)]
    pub entries: Vec<HubCatalogEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubCatalogManifest {
    pub schema_version: u32,
    pub hub_id: String,
    pub name: String,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub entry_count: Option<u64>,
    #[serde(default)]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HubCatalogEntry {
    pub id: String,
    pub kind: ExtensionKind,
    pub category: ExtensionCategory,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Upstream author repo (open-source attribution). Required by contract;
    /// `Option` only to tolerate a malformed artifact without a hard parse error.
    pub repo_url: Option<String>,
    pub trust_tier: TrustTier,
    #[serde(default)]
    pub requires_config: bool,
    #[serde(default)]
    pub config_schema: Option<serde_json::Value>,
    pub install_spec: InstallSpec,
}

impl HubCatalogEntry {
    /// Project the objective wire record into the cache's `ExtensionEntry`,
    /// stamping the source id and zeroing per-user state.
    #[must_use]
    pub fn into_entry(&self, hub_id: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: self.id.clone(),
            kind: self.kind,
            category: self.category,
            name: self.name.clone(),
            description: self.description.clone(),
            author: self.author.clone(),
            icon: self.icon.clone(),
            tags: self.tags.clone(),
            version: self.version.clone(),
            source_id: hub_id.to_string(),
            repo_url: self.repo_url.clone(),
            trust_tier: self.trust_tier,
            requires_config: self.requires_config,
            config_schema: self.config_schema.clone(),
            installed: false,
            enabled: false,
            update_available: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "manifest": {"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub","generated_at":"2026-06-20T00:00:00Z","entry_count":1},
      "entries": [{
        "id":"aleph-hub:acme/foo","kind":"mcp","category":"developer","name":"Acme Foo",
        "description":"d","repo_url":"https://github.com/acme/foo","trust_tier":"verified",
        "requires_config":false,
        "install_spec":{"type":"mcp_stdio","command":"npx","args":["@acme/foo"]}
      }]
    }"#;

    #[test]
    fn parses_and_normalizes() {
        let art: HubCatalogArtifact = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(art.manifest.schema_version, 1);
        assert_eq!(art.manifest.hub_id, "aleph-hub");
        let e = art.entries[0].into_entry(&art.manifest.hub_id);
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.trust_tier, TrustTier::Verified);
        assert_eq!(e.repo_url.as_deref(), Some("https://github.com/acme/foo"));
        // Per-user fields are stamped locally, never from the wire.
        assert!(!e.installed && !e.enabled && !e.update_available);
    }
}
