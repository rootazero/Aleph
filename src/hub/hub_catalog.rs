//! Wire format for a versioned static Hub catalog artifact (the contract
//! produced by Aleph-Hub and consumed by `AlephHubCatalog`). It is the
//! objective subset of `ExtensionEntry` — no per-user state ever crosses the
//! wire; `installed`/`enabled` are stamped locally.
//!
//! See
//! docs/superpowers/specs/2026-06-20-aleph-hub-single-source-design.md

use serde::Deserialize;

use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

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
    /// Publish timestamp, surfaced as catalog freshness in the sync report.
    #[serde(default)]
    pub generated_at: Option<String>,
    /// Declared entry count — verified against `entries.len()` by
    /// [`HubCatalogArtifact::validate`] so a truncated artifact cannot silently
    /// replace a good cache slice with a partial one.
    #[serde(default)]
    pub entry_count: Option<u64>,
}

/// Ids beginning with this prefix name *locally installed* extensions
/// (`local:{kind}:{backend_id}`, see `gateway::handlers::extensions::lifecycle`).
/// The wire must never mint one: `extensions.toggle` / `.uninstall` route on that
/// prefix, so a catalog entry wearing it would address a real backend object.
const RESERVED_LOCAL_ID_PREFIX: &str = "local:";

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
    /// Upstream provenance label set by the publishing hub. Additive/back-compat.
    #[serde(default)]
    pub via: Option<String>,
}

impl HubCatalogArtifact {
    /// Structural integrity gate, run before any entry reaches the cache.
    ///
    /// The catalog slot is *replace*-based, so an artifact that parses but is
    /// wrong wipes the last-good slice. Three invariants are cheap and local:
    /// the declared `entry_count` must match (catches a truncated or
    /// partially-published artifact), ids must be unique (a duplicate silently
    /// shadows the earlier row through `upsert`), and no id may claim the
    /// reserved `local:` installed-id namespace.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(declared) = self.manifest.entry_count {
            let actual = self.entries.len() as u64;
            if declared != actual {
                return Err(format!(
                    "manifest entry_count {declared} != {actual} entries (truncated or partial artifact)"
                ));
            }
        }
        let mut seen = std::collections::HashSet::with_capacity(self.entries.len());
        for e in &self.entries {
            if e.id.trim().is_empty() {
                return Err("entry with empty id".into());
            }
            if e.name.trim().is_empty() {
                return Err(format!("entry '{}' has an empty name", e.id));
            }
            if e.id.starts_with(RESERVED_LOCAL_ID_PREFIX) {
                return Err(format!(
                    "entry '{}' claims the reserved '{RESERVED_LOCAL_ID_PREFIX}' installed-id namespace",
                    e.id
                ));
            }
            if !seen.insert(e.id.as_str()) {
                return Err(format!("duplicate entry id '{}'", e.id));
            }
        }
        Ok(())
    }
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
            // `requires_config` is denormalized on the entry for fast UI rendering;
            // always recompute from the authoritative spec rather than trusting the
            // wire value, so a hostile publisher cannot lie about whether a key
            // prompt is needed. See review/hub-statics.
            requires_config: self.install_spec.requires_config(),
            config_schema: self.config_schema.clone(),
            installed: false,
            enabled: false,
            update_available: false,
            via: self.via.clone().or_else(|| Some(hub_id.to_string())),
            install_spec: Some(self.install_spec.clone()),
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

    fn artifact(entries_json: &str, extra_manifest: &str) -> HubCatalogArtifact {
        let body = format!(
            r#"{{"manifest":{{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"{extra_manifest}}},"entries":{entries_json}}}"#
        );
        serde_json::from_str(&body).expect("fixture parses")
    }

    const ONE_ENTRY: &str = r#"[{"id":"aleph-hub:a","kind":"mcp","category":"other","name":"A",
        "description":"d","repo_url":null,"trust_tier":"verified",
        "install_spec":{"type":"mcp_stdio","command":"c","args":[]}}]"#;

    #[test]
    fn validate_accepts_matching_entry_count() {
        assert!(artifact(ONE_ENTRY, r#","entry_count":1"#)
            .validate()
            .is_ok());
        // absent entry_count is tolerated (field is optional by contract)
        assert!(artifact(ONE_ENTRY, "").validate().is_ok());
    }

    #[test]
    fn validate_rejects_truncated_artifact() {
        let err = artifact(ONE_ENTRY, r#","entry_count":7"#)
            .validate()
            .unwrap_err();
        assert!(err.contains("entry_count"), "{err}");
    }

    #[test]
    fn validate_rejects_reserved_local_id_namespace() {
        let entries = r#"[{"id":"local:mcp:github","kind":"mcp","category":"other","name":"X",
            "description":"d","repo_url":null,"trust_tier":"official",
            "install_spec":{"type":"mcp_stdio","command":"c","args":[]}}]"#;
        let err = artifact(entries, "").validate().unwrap_err();
        assert!(err.contains("reserved"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_ids_and_blank_fields() {
        let dup = r#"[{"id":"x","kind":"mcp","category":"other","name":"A","description":"d",
            "repo_url":null,"trust_tier":"official","install_spec":{"type":"mcp_stdio","command":"c","args":[]}},
            {"id":"x","kind":"mcp","category":"other","name":"B","description":"d",
            "repo_url":null,"trust_tier":"official","install_spec":{"type":"mcp_stdio","command":"c","args":[]}}]"#;
        assert!(artifact(dup, "")
            .validate()
            .unwrap_err()
            .contains("duplicate"));

        let blank = r#"[{"id":"  ","kind":"mcp","category":"other","name":"A","description":"d",
            "repo_url":null,"trust_tier":"official","install_spec":{"type":"mcp_stdio","command":"c","args":[]}}]"#;
        assert!(artifact(blank, "")
            .validate()
            .unwrap_err()
            .contains("empty id"));
    }

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
