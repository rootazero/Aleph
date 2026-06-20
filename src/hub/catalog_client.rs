//! Standalone `AlephHubCatalog` client: HTTP fetch → schema-version check →
//! injection scan → `into_entry` normalization → cache sync. No SourceProvider
//! trait, no in-memory spec map — install resolution is a pure cache lookup of
//! `ExtensionEntry.install_spec`.

use std::fmt;

use crate::hub::cache::CatalogCache;
use crate::hub::hub_catalog::{HubCatalogArtifact, SUPPORTED_SCHEMA_VERSION};
use crate::hub::trust::scan_for_injection;
use crate::hub::types::{ExtensionEntry, TrustTier};

/// Built-in official Aleph Hub source.
pub const ALEPH_HUB_ID: &str = "aleph-hub";
pub const ALEPH_HUB_NAME: &str = "Aleph Hub";
pub const ALEPH_HUB_URL: &str = "https://hub.heyaleph.com/catalog.json";

#[derive(Debug, Clone)]
pub enum CatalogError {
    Network(String),
    Parse(String),
    Schema(String),
    Other(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::Network(s) => write!(f, "network: {s}"),
            CatalogError::Parse(s) => write!(f, "parse: {s}"),
            CatalogError::Schema(s) => write!(f, "schema: {s}"),
            CatalogError::Other(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for CatalogError {}

/// Result of one sync into the cache.
#[derive(Debug, Clone)]
pub struct SyncReport {
    pub synced: usize,
    pub failed: Vec<String>,
}

/// Thin, stateless client for the single published Aleph Hub catalog artifact.
#[derive(Clone)]
pub struct AlephHubCatalog {
    id: String,
    #[allow(dead_code)]
    name: String,
    artifact_url: String,
    #[allow(dead_code)]
    trust_tier: TrustTier,
    http: reqwest::Client,
}

impl AlephHubCatalog {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        artifact_url: impl Into<String>,
        trust_tier: TrustTier,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            artifact_url: artifact_url.into(),
            trust_tier,
            http: reqwest::Client::new(),
        }
    }

    /// Parse + normalize an artifact body (no network) — schema check, injection
    /// scan (warn-only), then `into_entry`.
    fn ingest(&self, body: &str) -> Result<Vec<ExtensionEntry>, CatalogError> {
        let art: HubCatalogArtifact =
            serde_json::from_str(body).map_err(|e| CatalogError::Parse(e.to_string()))?;
        if art.manifest.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(CatalogError::Schema(format!(
                "artifact schema_version {} > supported {}",
                art.manifest.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }
        let mut out = Vec::with_capacity(art.entries.len());
        for he in &art.entries {
            let findings = scan_for_injection(&format!("{} {}", he.name, he.description));
            if !findings.is_empty() {
                tracing::warn!(hub = %self.id, id = %he.id, ?findings, "hub entry injection findings");
            }
            out.push(he.into_entry(&art.manifest.hub_id));
        }
        Ok(out)
    }

    /// Fetch the artifact over HTTP and normalize it.
    pub async fn fetch(&self) -> Result<Vec<ExtensionEntry>, CatalogError> {
        let resp = self
            .http
            .get(&self.artifact_url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| CatalogError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(CatalogError::Network(format!("HTTP {}", resp.status())));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| CatalogError::Network(e.to_string()))?;
        self.ingest(&body)
    }

    /// Fetch + atomically replace this source's cache slice. Never errors out:
    /// a fetch/cache failure yields `synced: 0` and keeps the last-good cache.
    pub async fn sync_into(&self, cache: &CatalogCache) -> SyncReport {
        match self.fetch().await {
            Ok(entries) => {
                let synced = entries.len();
                match cache.replace_source(&self.id, &entries).await {
                    Ok(()) => SyncReport { synced, failed: Vec::new() },
                    Err(e) => SyncReport { synced: 0, failed: vec![format!("cache write: {e}")] },
                }
            }
            Err(e) => SyncReport { synced: 0, failed: vec![e.to_string()] },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, ExtensionKind, InstallSpec};

    const FIXTURE: &str = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
      "entries":[{"id":"aleph-hub:acme/foo","kind":"mcp","category":"developer","name":"Foo",
      "description":"d","repo_url":"https://github.com/acme/foo","trust_tier":"verified",
      "install_spec":{"type":"mcp_stdio","command":"npx","args":["@acme/foo"],"env":[]},
      "via":"clawhub"}]}"#;

    #[test]
    fn ingest_populates_via_and_install_spec() {
        let c = AlephHubCatalog::new(ALEPH_HUB_ID, ALEPH_HUB_NAME, "http://unused", TrustTier::Verified);
        let entries = c.ingest(FIXTURE).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.category, ExtensionCategory::Developer);
        assert_eq!(e.via.as_deref(), Some("clawhub")); // wire `via` wins
        assert!(matches!(e.install_spec, Some(InstallSpec::McpStdio { .. })));
    }

    #[test]
    fn ingest_falls_back_to_hub_id_when_via_absent() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
          "entries":[{"id":"aleph-hub:x","kind":"mcp","category":"other","name":"X","description":"d",
          "repo_url":"https://github.com/x/x","trust_tier":"verified",
          "install_spec":{"type":"mcp_stdio","command":"c","args":[],"env":[]}}]}"#;
        let c = AlephHubCatalog::new(ALEPH_HUB_ID, ALEPH_HUB_NAME, "http://unused", TrustTier::Verified);
        let e = &c.ingest(body).unwrap()[0];
        assert_eq!(e.via.as_deref(), Some("aleph-hub")); // fallback to hub_id
    }

    #[test]
    fn ingest_rejects_future_schema() {
        let body = r#"{"manifest":{"schema_version":999,"hub_id":"h","name":"H"},"entries":[]}"#;
        let c = AlephHubCatalog::new("h", "H", "http://unused", TrustTier::Community);
        assert!(matches!(c.ingest(body), Err(CatalogError::Schema(_))));
    }

    #[test]
    fn constants_are_pinned() {
        assert_eq!(ALEPH_HUB_URL, "https://hub.heyaleph.com/catalog.json");
    }
}
