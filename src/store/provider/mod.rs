//! Pluggable source-provider layer. Each `SourceProvider` normalizes a remote
//! catalog (plugin marketplace, MCP registry, Docker catalog) into P0
//! `ExtensionEntry`s and syncs its slice into the rusqlite cache. Providers are
//! the only network callers; `sync()` runs in the background.

pub mod docker_mcp;
pub mod marketplace;
pub mod mcp_registry;
pub mod registry_builder;

use crate::store::cache::CatalogCache;
use crate::store::types::{ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

/// Reserved sync context (cache dir, shared http client) — empty in v1.
pub struct SyncCtx;

pub struct Query {
    pub text: String,
}

#[derive(Debug)]
pub enum SourceError {
    Network(String),
    Parse(String),
    Other(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(s) => write!(f, "network: {s}"),
            Self::Parse(s) => write!(f, "parse: {s}"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

#[async_trait::async_trait]
pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kinds(&self) -> &[ExtensionKind];
    fn trust_tier(&self) -> TrustTier;
    async fn sync(&self, ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError>;
    async fn search(&self, _q: &Query) -> Option<Result<Vec<ExtensionEntry>, SourceError>> {
        None
    }
    async fn resolve_install_spec(&self, entry: &ExtensionEntry)
        -> Result<InstallSpec, SourceError>;
}

pub struct SyncReport {
    pub synced: Vec<(String, usize)>,
    pub failed: Vec<(String, String)>,
}

pub struct ProviderRegistry {
    providers: Vec<Box<dyn SourceProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, p: Box<dyn SourceProvider>) {
        self.providers.push(p);
    }

    pub fn get(&self, id: &str) -> Option<&dyn SourceProvider> {
        self.providers.iter().find(|p| p.id() == id).map(|b| b.as_ref())
    }

    /// Provider metadata for `extensions.sources.list` (id, trust tier, kinds).
    pub fn list_sources(&self) -> Vec<(String, TrustTier, Vec<ExtensionKind>)> {
        self.providers
            .iter()
            .map(|p| (p.id().to_string(), p.trust_tier(), p.kinds().to_vec()))
            .collect()
    }

    /// Sync every provider concurrently; each writes its own slice via
    /// `replace_source` only on a successful, non-empty fetch (keeps last-good
    /// cache on failure).
    pub async fn sync_all_into(&self, cache: &CatalogCache) -> SyncReport {
        let ctx = SyncCtx;
        let futures = self
            .providers
            .iter()
            .map(|p| async { (p.id().to_string(), p.sync(&ctx).await) });
        let results = futures::future::join_all(futures).await;
        let mut report = SyncReport {
            synced: vec![],
            failed: vec![],
        };
        for (id, res) in results {
            match res {
                Ok(mut entries) if !entries.is_empty() => {
                    for e in &mut entries {
                        if e.category == crate::store::types::ExtensionCategory::Other {
                            e.category = crate::store::categorize::categorize(&e.name, &e.description, &e.tags, None);
                        }
                    }
                    if let Err(e) = cache.replace_source(&id, &entries).await {
                        report.failed.push((id, e.to_string()));
                    } else {
                        report.synced.push((id, entries.len()));
                    }
                }
                Ok(_) => report
                    .failed
                    .push((id, "empty result; kept last-good cache".into())),
                Err(e) => report.failed.push((id, e.to_string())),
            }
        }
        report
    }

    /// Route an entry to its provider and resolve its install spec.
    pub async fn resolve_for_entry(
        &self,
        entry: &crate::store::types::ExtensionEntry,
    ) -> Result<crate::store::types::InstallSpec, SourceError> {
        let provider = self
            .get(&entry.source_id)
            .ok_or_else(|| SourceError::Other(format!("no provider for source '{}'", entry.source_id)))?;
        provider.resolve_install_spec(entry).await
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::cache::CatalogFilter;
    use crate::store::provider::registry_builder::build_default_registry;
    use crate::store::types::ExtensionCategory;

    struct FakeProvider {
        id: String,
        entries: Vec<ExtensionEntry>,
    }

    #[async_trait::async_trait]
    impl SourceProvider for FakeProvider {
        fn id(&self) -> &str {
            &self.id
        }
        fn kinds(&self) -> &[ExtensionKind] {
            &[ExtensionKind::Mcp]
        }
        fn trust_tier(&self) -> TrustTier {
            TrustTier::Community
        }
        async fn sync(&self, _c: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
            Ok(self.entries.clone())
        }
        async fn resolve_install_spec(
            &self,
            _e: &ExtensionEntry,
        ) -> Result<InstallSpec, SourceError> {
            Ok(InstallSpec::OciImage { image: "x".into() })
        }
    }

    fn entry(id: &str, src: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind: ExtensionKind::Mcp,
            category: ExtensionCategory::Other,
            name: id.into(),
            description: String::new(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: src.into(),
            repo_url: None,
            trust_tier: TrustTier::Community,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
        }
    }

    #[tokio::test]
    async fn sync_all_writes_each_provider_slice() {
        let cache = CatalogCache::open_in_memory().unwrap();
        let mut reg = ProviderRegistry::new();
        reg.register(Box::new(FakeProvider {
            id: "p1".into(),
            entries: vec![entry("p1:a", "p1")],
        }));
        reg.register(Box::new(FakeProvider {
            id: "p2".into(),
            entries: vec![entry("p2:a", "p2"), entry("p2:b", "p2")],
        }));

        let report = reg.sync_all_into(&cache).await;
        assert_eq!(report.failed.len(), 0);
        let all = cache.query(&CatalogFilter::default()).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn resolve_for_entry_routes_by_source_id() {
        let reg = build_default_registry(Default::default());
        let mut e = entry("test:foo", "p1");
        e.source_id = "local".into();
        // "local" has no registered provider → Err, not panic
        assert!(reg.resolve_for_entry(&e).await.is_err());
    }
}
