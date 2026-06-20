//! `StaticHubProvider` — consumes a versioned static Hub catalog artifact (the
//! Aleph-Hub contract) over HTTP and normalizes it into `ExtensionEntry`s. One
//! instance per federated hub (Aleph Hub, and later third-party hubs via their
//! own adapters). Browse is served from the local cache; this only fetches.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::hub::hub_catalog::{HubCatalogArtifact, SUPPORTED_SCHEMA_VERSION};
use crate::hub::provider::{Query, SourceError, SourceProvider, SyncCtx};
use crate::hub::trust::scan_for_injection;
use crate::hub::types::{ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

pub struct StaticHubProvider {
    id: String,
    name: String,
    artifact_url: String,
    trust_tier: TrustTier,
    http: reqwest::Client,
    /// install_spec by entry id, captured at sync for `resolve_install_spec`.
    specs: Mutex<HashMap<String, InstallSpec>>,
}

impl StaticHubProvider {
    #[must_use]
    pub fn new(id: String, name: String, artifact_url: String, trust_tier: TrustTier) -> Self {
        Self {
            id,
            name,
            artifact_url,
            trust_tier,
            http: reqwest::Client::new(),
            specs: Mutex::new(HashMap::new()),
        }
    }

    /// Parse + normalize an artifact body. Split out so tests need no network.
    fn ingest(&self, body: &str) -> Result<Vec<ExtensionEntry>, SourceError> {
        let art: HubCatalogArtifact =
            serde_json::from_str(body).map_err(|e| SourceError::Parse(e.to_string()))?;
        if art.manifest.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(SourceError::Other(format!(
                "hub '{}' artifact schema_version {} > supported {}",
                self.id, art.manifest.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }
        let mut specs = self.specs.lock().unwrap_or_else(|e| e.into_inner());
        specs.clear();
        let mut out = Vec::with_capacity(art.entries.len());
        for he in &art.entries {
            // Defense in depth: scan curated text for hidden-instruction attacks
            // even though the hub already curates it.
            let findings = scan_for_injection(&format!("{} {}", he.name, he.description));
            if !findings.is_empty() {
                tracing::warn!(hub = %self.id, id = %he.id, ?findings, "hub entry injection findings");
            }
            specs.insert(he.id.clone(), he.install_spec.clone());
            out.push(he.into_entry(&art.manifest.hub_id));
        }
        Ok(out)
    }
}

#[async_trait::async_trait]
impl SourceProvider for StaticHubProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn kinds(&self) -> &[ExtensionKind] {
        &[ExtensionKind::Skill, ExtensionKind::Plugin, ExtensionKind::Mcp]
    }
    fn trust_tier(&self) -> TrustTier {
        self.trust_tier
    }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        let resp = self
            .http
            .get(&self.artifact_url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SourceError::Network(format!("HTTP {}", resp.status())));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        self.ingest(&body)
    }

    async fn search(&self, _q: &Query) -> Option<Result<Vec<ExtensionEntry>, SourceError>> {
        None // browse is served from the local cache
    }

    async fn resolve_install_spec(
        &self,
        entry: &ExtensionEntry,
    ) -> Result<InstallSpec, SourceError> {
        let specs = self.specs.lock().unwrap_or_else(|e| e.into_inner());
        specs
            .get(&entry.id)
            .cloned()
            .ok_or_else(|| SourceError::Other(format!("no install spec cached for '{}'", entry.id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
      "entries":[{"id":"aleph-hub:acme/foo","kind":"mcp","category":"developer","name":"Foo",
      "description":"d","repo_url":"https://github.com/acme/foo","trust_tier":"verified",
      "install_spec":{"type":"mcp_stdio","command":"npx","args":["@acme/foo"]}}]}"#;

    #[test]
    fn ingest_normalizes_and_caches_spec() {
        let p = StaticHubProvider::new(
            "aleph-hub".into(),
            "Aleph Hub".into(),
            "http://unused".into(),
            TrustTier::Verified,
        );
        let entries = p.ingest(BODY).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "aleph-hub");
        assert_eq!(p.display_name(), "Aleph Hub");
        assert!(p
            .specs
            .lock()
            .unwrap()
            .contains_key("aleph-hub:acme/foo"));
    }

    #[test]
    fn ingest_rejects_future_schema() {
        let p = StaticHubProvider::new("h".into(), "H".into(), "x".into(), TrustTier::Community);
        let body = r#"{"manifest":{"schema_version":999,"hub_id":"h","name":"H"},"entries":[]}"#;
        assert!(matches!(p.ingest(body), Err(SourceError::Other(_))));
    }
}
