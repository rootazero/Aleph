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
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub synced: usize,
    pub failed: Vec<String>,
    /// The artifact's `generated_at`, when the fetch got far enough to parse a
    /// manifest — catalog freshness, so "0 synced" can be told apart from
    /// "synced a stale catalog".
    pub generated_at: Option<String>,
}

/// One normalized artifact: entries plus the manifest facts callers report on.
struct Ingested {
    entries: Vec<ExtensionEntry>,
    generated_at: Option<String>,
}

/// Thin, stateless client for the single published Aleph Hub catalog artifact.
#[derive(Clone)]
pub struct AlephHubCatalog {
    id: String,
    /// Human display name of this source. Becomes an entry's `via` (→ the
    /// Panel's `source_label`) when the wire declares no upstream provenance.
    name: String,
    artifact_url: String,
    /// Trust ceiling for this source: every entry's wire-declared `trust_tier`
    /// is clamped to it, so `official` is a property of the source rather than
    /// an entry's self-assertion.
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

    /// Parse + normalize an artifact body (no network) — schema check,
    /// structural integrity gate, injection scan (warn-only), `into_entry`, then
    /// the per-source trust clamp.
    fn ingest(&self, body: &str) -> Result<Ingested, CatalogError> {
        let art: HubCatalogArtifact =
            serde_json::from_str(body).map_err(|e| CatalogError::Parse(e.to_string()))?;
        if art.manifest.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(CatalogError::Schema(format!(
                "artifact schema_version {} > supported {}",
                art.manifest.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }
        // Reject before anything reaches the cache: the slot is replace-based, so
        // a partial artifact would wipe the last-good slice.
        art.validate().map_err(CatalogError::Schema)?;
        let mut out = Vec::with_capacity(art.entries.len());
        for he in &art.entries {
            let findings = scan_for_injection(&format!("{} {}", he.name, he.description));
            if !findings.is_empty() {
                tracing::warn!(hub = %self.id, id = %he.id, ?findings, "hub entry injection findings");
            }
            let mut entry = he.into_entry(&art.manifest.hub_id);
            entry.trust_tier = entry.trust_tier.clamped_to(self.trust_tier);
            // No wire-declared upstream provenance → label the entry with this
            // source's human name (the Panel renders `via` as `source_label`).
            if he.via.is_none() {
                entry.via = Some(self.name.clone());
            }
            out.push(entry);
        }
        Ok(Ingested {
            entries: out,
            generated_at: art.manifest.generated_at,
        })
    }

    /// Fetch the artifact over HTTP and normalize it.
    pub async fn fetch(&self) -> Result<Vec<ExtensionEntry>, CatalogError> {
        self.fetch_ingested().await.map(|i| i.entries)
    }

    async fn fetch_ingested(&self) -> Result<Ingested, CatalogError> {
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
    /// An empty-but-valid response (transient publish glitch) is treated as a
    /// non-fatal no-op — the last-good cache is preserved rather than wiped.
    pub async fn sync_into(&self, cache: &CatalogCache) -> SyncReport {
        match self.fetch_ingested().await {
            Ok(ing) if !ing.entries.is_empty() => {
                let synced = ing.entries.len();
                match cache.replace_source(&self.id, &ing.entries).await {
                    Ok(()) => SyncReport {
                        synced,
                        failed: Vec::new(),
                        generated_at: ing.generated_at,
                    },
                    Err(e) => SyncReport {
                        synced: 0,
                        failed: vec![format!("cache write: {e}")],
                        generated_at: ing.generated_at,
                    },
                }
            }
            Ok(ing) => SyncReport {
                synced: 0,
                failed: vec!["empty catalog; kept last-good cache".into()],
                generated_at: ing.generated_at,
            },
            Err(e) => SyncReport {
                synced: 0,
                failed: vec![e.to_string()],
                generated_at: None,
            },
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

    fn client() -> AlephHubCatalog {
        AlephHubCatalog::new(
            ALEPH_HUB_ID,
            ALEPH_HUB_NAME,
            "http://unused",
            TrustTier::Verified,
        )
    }

    #[test]
    fn ingest_populates_via_and_install_spec() {
        let entries = client().ingest(FIXTURE).unwrap().entries;
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert_eq!(e.category, ExtensionCategory::Developer);
        assert_eq!(e.via.as_deref(), Some("clawhub")); // wire `via` wins
        assert!(matches!(e.install_spec, Some(InstallSpec::McpStdio { .. })));
    }

    #[test]
    fn ingest_falls_back_to_source_display_name_when_via_absent() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
          "entries":[{"id":"aleph-hub:x","kind":"mcp","category":"other","name":"X","description":"d",
          "repo_url":"https://github.com/x/x","trust_tier":"verified",
          "install_spec":{"type":"mcp_stdio","command":"c","args":[],"env":[]}}]}"#;
        let e = &client().ingest(body).unwrap().entries[0];
        // The Panel renders `via` as `source_label`, a human label — so the
        // fallback is the source's display name, not its machine id.
        assert_eq!(e.via.as_deref(), Some(ALEPH_HUB_NAME));
    }

    #[test]
    fn ingest_clamps_entry_trust_to_source_ceiling() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
          "entries":[{"id":"aleph-hub:x","kind":"mcp","category":"other","name":"X","description":"d",
          "repo_url":null,"trust_tier":"official",
          "install_spec":{"type":"mcp_stdio","command":"c","args":[],"env":[]}}]}"#;
        // Source ceiling is Verified, so a self-declared `official` entry lands Verified.
        let e = &client().ingest(body).unwrap().entries[0];
        assert_eq!(e.trust_tier, TrustTier::Verified);
        // An Official source may publish Official entries.
        let official = AlephHubCatalog::new("h", "H", "http://unused", TrustTier::Official);
        assert_eq!(
            official.ingest(body).unwrap().entries[0].trust_tier,
            TrustTier::Official
        );
    }

    #[test]
    fn ingest_rejects_truncated_artifact_before_the_cache() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub","entry_count":9},
          "entries":[{"id":"aleph-hub:x","kind":"mcp","category":"other","name":"X","description":"d",
          "repo_url":null,"trust_tier":"verified",
          "install_spec":{"type":"mcp_stdio","command":"c","args":[],"env":[]}}]}"#;
        assert!(matches!(
            client().ingest(body),
            Err(CatalogError::Schema(_))
        ));
    }

    #[test]
    fn ingest_surfaces_generated_at_as_freshness() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub","generated_at":"2026-07-30T00:00:00Z"},
          "entries":[]}"#;
        assert_eq!(
            client().ingest(body).unwrap().generated_at.as_deref(),
            Some("2026-07-30T00:00:00Z")
        );
    }

    #[test]
    fn ingest_rejects_future_schema() {
        let body = r#"{"manifest":{"schema_version":999,"hub_id":"h","name":"H"},"entries":[]}"#;
        let c = AlephHubCatalog::new("h", "H", "http://unused", TrustTier::Community);
        assert!(matches!(c.ingest(body), Err(CatalogError::Schema(_))));
    }

    /// A wire entry claiming the `local:` installed-id namespace is rejected —
    /// those ids address real backend objects through `extensions.toggle` /
    /// `.uninstall`.
    #[test]
    fn ingest_rejects_reserved_local_id() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},
          "entries":[{"id":"local:mcp:github","kind":"mcp","category":"other","name":"X","description":"d",
          "repo_url":null,"trust_tier":"verified",
          "install_spec":{"type":"mcp_stdio","command":"c","args":[],"env":[]}}]}"#;
        assert!(matches!(
            client().ingest(body),
            Err(CatalogError::Schema(_))
        ));
    }

    #[test]
    fn constants_are_pinned() {
        assert_eq!(ALEPH_HUB_URL, "https://hub.heyaleph.com/catalog.json");
    }

    // --- Tests establishing the two facts the `sync_into` empty-guard depends on ---

    /// Fact 1: `ingest` of a syntactically valid artifact with an empty `entries`
    /// array returns `Ok(vec![])` — proving the empty-success trigger is reachable.
    #[test]
    fn ingest_empty_entries_returns_ok_empty_vec() {
        let body = r#"{"manifest":{"schema_version":1,"hub_id":"aleph-hub","name":"Aleph Hub"},"entries":[]}"#;
        let entries = client().ingest(body).unwrap().entries;
        assert!(
            entries.is_empty(),
            "expected empty vec from empty entries array"
        );
    }

    /// Fact 2: `CatalogCache::replace_source` with an empty slice is destructive —
    /// it wipes existing rows. This proves WHY the guard in `sync_into` is necessary:
    /// without it, an `Ok(vec![])` response would blank the entire cached catalog slice.
    #[tokio::test]
    async fn replace_source_with_empty_slice_wipes_existing_rows() {
        use crate::hub::cache::{CatalogCache, CatalogFilter};
        use crate::hub::types::{
            ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier as TT,
        };

        let cache = CatalogCache::open_in_memory().unwrap();
        let entry = ExtensionEntry {
            id: "aleph-hub:test/entry".into(),
            kind: ExtensionKind::Mcp,
            category: ExtensionCategory::Developer,
            name: "Test".into(),
            description: "d".into(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: ALEPH_HUB_ID.into(),
            repo_url: None,
            trust_tier: TT::Verified,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: None,
            install_spec: None,
        };
        cache.upsert_many(&[entry]).await.unwrap();

        // Confirm the row is present before the wipe.
        let before = cache
            .query(&CatalogFilter {
                source_id: Some(ALEPH_HUB_ID.into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(before.len(), 1, "seed row must be present");

        // An empty replace_source is destructive — rows are gone.
        cache.replace_source(ALEPH_HUB_ID, &[]).await.unwrap();
        let after = cache
            .query(&CatalogFilter {
                source_id: Some(ALEPH_HUB_ID.into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            after.is_empty(),
            "replace_source with empty slice must wipe existing rows"
        );
    }
}
