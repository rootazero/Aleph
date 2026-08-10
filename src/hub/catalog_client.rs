//! Standalone `AlephHubCatalog` client: HTTP fetch → schema-version check →
//! injection scan → `into_entry` normalization → cache sync. No SourceProvider
//! trait, no in-memory spec map — install resolution is a pure cache lookup of
//! `ExtensionEntry.install_spec`.

use std::fmt;

use crate::hub::cache::CatalogCache;
use crate::hub::hub_catalog::{HubCatalogArtifact, SUPPORTED_SCHEMA_VERSION};
use crate::hub::trust::scan_for_injection;
use crate::hub::types::{ExtensionEntry, TrustTier};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};

/// Hard ceiling on the artifact body size.
///
/// The published Aleph Hub catalog is single-digit MiB today. A 32 MiB cap is
/// generous and bounds the worst case (a hostile or buggy upstream serving a
/// multi-GB response that would otherwise OOM the daemon before the JSON
/// parse even starts). Mirrors `DOC_FETCH_CEILING` in `fetch_docs.rs`.
pub const CATALOG_MAX_BYTES: usize = 32 * 1024 * 1024;

/// Fetch timeout for the catalog artifact.
const CATALOG_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Validate the publisher's `generated_at` so downstream consumers see a
/// bounded, RFC3339-shaped value. Anything else silently drops the field —
/// the sync still succeeds (the catalog itself was fine), only its freshness
/// signal degrades to "unknown".
fn sanitize_generated_at(raw: &str) -> Option<String> {
    // Length cap so a hostile publisher can't OOM the tool response.
    if raw.len() > 64 {
        return None;
    }
    let bytes = raw.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    // YYYY-MM-DDTHH:MM:SS [.<frac>] [Z|+HH:MM|-HH:MM]
    // Cheap structural check; doesn't parse the whole grammar.
    // Slot-by-slot positions where a non-digit character is required.
    const DELIMITER_BYTES: &[u8] = b"-+:.T";
    for (i, b) in bytes.iter().enumerate() {
        let is_date_or_time_field = i < 19;
        let is_separator = matches!(i, 4 | 7 | 10 | 13 | 16);
        let is_tz_open = i == 19;
        let is_accepted_elsewhere = DELIMITER_BYTES.contains(b)
            || b.is_ascii_digit()
            || (is_tz_open && matches!(b, b'.' | b'+' | b'-' | b'Z'));
        // Whitespace anywhere is fatal — the publisher's payload should be
        // tight, and trimming opens the door to look-alike padding attacks.
        if b.is_ascii_whitespace() {
            return None;
        }
        // Inside the date/time fields, allow only digits and separators at the
        // exact slot positions; no other characters.
        if is_date_or_time_field {
            let allowed = if is_separator {
                DELIMITER_BYTES.contains(b) && *b != b'.'
            } else {
                b.is_ascii_digit()
            };
            if !allowed {
                return None;
            }
        }
        // Past the 20th byte we relax the constraint for fractional seconds /
        // timezone, gated only by `is_accepted_elsewhere` so a stray `[` or
        // `;` cannot slip past.
        if !is_date_or_time_field && !is_accepted_elsewhere {
            return None;
        }
    }
    // Day/month digit sanity on the fixed-width slots so the parser never
    // hands a downstream consumer bytes it can re-parse. Full date validation
    // would need `chrono` so we keep it cheap.
    for slot in [&bytes[0..4], &bytes[5..7], &bytes[8..10]] {
        if !slot.iter().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some(raw.to_string())
}

#[cfg(test)]
mod sanitize_tests {
    use super::sanitize_generated_at;

    #[test]
    fn accepts_rfc3339_utc() {
        assert!(sanitize_generated_at("2026-07-30T00:00:00Z").is_some());
    }

    #[test]
    fn accepts_rfc3339_with_offset() {
        assert!(sanitize_generated_at("2026-07-30T12:34:56+02:00").is_some());
    }

    #[test]
    fn rejects_html() {
        assert!(sanitize_generated_at("<script>alert(1)</script>").is_none());
    }

    #[test]
    fn rejects_oversize() {
        assert!(sanitize_generated_at(&"a".repeat(65)).is_none());
    }

    #[test]
    fn rejects_short_or_malformed() {
        assert!(sanitize_generated_at("2026/07/30").is_none());
        assert!(sanitize_generated_at("not a date").is_none());
    }
}

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
        }
    }

    /// Parse + normalize an artifact body (no network) — schema check,
    /// structural integrity gate, injection scan (warn-only), `into_entry`, then
    /// the per-source trust clamp.
    fn ingest(&self, body: &str) -> Result<Ingested, CatalogError> {
        let art: HubCatalogArtifact =
            serde_json::from_str(body).map_err(|e| CatalogError::Parse(e.to_string()))?;
        // Schema-version: exact match. The wire is `u32`, so anything less is a
        // stale publisher miss, anything greater is a contract we haven't
        // learned to read. Forward compatibility is the publisher's problem
        // to negotiate (bump SUPPORTED_SCHEMA_VERSION here when ready).
        if art.manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(CatalogError::Schema(format!(
                "artifact schema_version {} != supported {}",
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
            // Validate `generated_at` shape so a hostile publisher cannot ship
            // an XSS payload or a multi-MiB string here. An invalid value
            // drops the field rather than rejects the whole sync — a stale
            // freshness signal is less harmful than refusing a valid catalog.
            generated_at: art.manifest.generated_at.as_deref().and_then(sanitize_generated_at),
        })
    }

    /// Fetch the artifact over HTTP and normalize it.
    pub async fn fetch(&self) -> Result<Vec<ExtensionEntry>, CatalogError> {
        self.fetch_ingested().await.map(|i| i.entries)
    }

    async fn fetch_ingested(&self) -> Result<Ingested, CatalogError> {
        // Use the project's `safe_fetch` so the same defenses every other
        // module applies (URL allow-list, DNS pinning, no silent redirect to
        // 10.0.0.x, cross-origin header strip) apply to the catalog too.
        // The `with_max_body_bytes` cap is the one the engineering audit
        // named explicitly: a hostile or buggy upstream cannot OOM the
        // daemon even before the JSON parse runs.
        let resp = safe_fetch(
            &self.artifact_url,
            &SsrfPolicy::default(),
            SafeFetchRequest::get(CATALOG_FETCH_TIMEOUT).with_max_body_bytes(CATALOG_MAX_BYTES),
        )
        .await
        .map_err(|e| CatalogError::Network(e.to_string()))?;
        if !resp.status.is_success() {
            return Err(CatalogError::Network(format!("HTTP {}", resp.status)));
        }
        let body = std::str::from_utf8(&resp.body)
            .map_err(|e| CatalogError::Parse(format!("non-utf8 body: {e}")))?;
        self.ingest(body)
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
