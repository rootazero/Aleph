//! Plugin-marketplace `SourceProvider` over the existing `MarketplaceManager`
//! (Anthropic-screened marketplaces → `TrustTier::Verified`).

use crate::extension::marketplace::manifest::parse_marketplace_manifest;
use crate::extension::marketplace::types::MarketplacePluginEntry;
use crate::extension::marketplace::MarketplaceManager;
use crate::store::provider::{SourceError, SourceProvider, SyncCtx};
use crate::store::types::{
    ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier,
};

pub fn plugin_entry_to_extension(provider_id: &str, pe: &MarketplacePluginEntry) -> ExtensionEntry {
    ExtensionEntry {
        id: format!("{provider_id}:{}", pe.name),
        kind: ExtensionKind::Plugin,
        category: ExtensionCategory::Other,
        name: pe.name.clone(),
        description: pe.description.clone().unwrap_or_default(),
        author: None,
        icon: None,
        tags: vec!["plugin".into()],
        version: pe.version.clone(),
        source_id: provider_id.to_string(),
        repo_url: Some(pe.source.clone()),
        trust_tier: TrustTier::Verified, // Anthropic-screened marketplaces
        requires_config: false,
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
    }
}

pub struct MarketplaceProvider {
    pub manager: MarketplaceManager,
    pub provider_id: String,
}

#[async_trait::async_trait]
impl SourceProvider for MarketplaceProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }
    fn kinds(&self) -> &[ExtensionKind] {
        &[ExtensionKind::Plugin, ExtensionKind::Skill]
    }
    fn trust_tier(&self) -> TrustTier {
        TrustTier::Verified
    }

    async fn sync(&self, _ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError> {
        // MarketplaceManager is sync/blocking (git + fs); for v1 this runs inside
        // the background sync task. If it stalls a worker noticeably, wrap the
        // per-marketplace body in tokio::task::spawn_blocking.
        let manager_names: Vec<String> = self.manager.list().keys().cloned().collect();
        let mut out = Vec::new();
        for name in manager_names {
            let cache_dir = self.manager.update(&name).map_err(SourceError::Network)?;
            let manifest = parse_marketplace_manifest(&cache_dir).map_err(SourceError::Parse)?;
            out.extend(
                manifest
                    .plugins
                    .iter()
                    .map(|pe| plugin_entry_to_extension(&self.provider_id, pe)),
            );
        }
        Ok(out)
    }

    async fn resolve_install_spec(
        &self,
        entry: &ExtensionEntry,
    ) -> Result<InstallSpec, SourceError> {
        // Plugins install via the marketplace path; the InstallSpec carries the
        // git source as a routing hint (the actual SHA256-verified install in P2
        // goes through MarketplaceManager::install_to_scope).
        let repo = entry
            .repo_url
            .clone()
            .ok_or_else(|| SourceError::Other("missing repo_url".into()))?;
        Ok(InstallSpec::GitDir {
            git_url: repo,
            subdir: None,
            git_ref: None,
            sha256: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_plugin_entry() {
        let pe = MarketplacePluginEntry {
            name: "hello".into(),
            source: "acme/hello".into(),
            description: Some("Says hi".into()),
            version: Some("0.2.0".into()),
            sha256: Some("abc123".into()),
        };
        let e = plugin_entry_to_extension("cc-marketplace", &pe);
        assert_eq!(e.kind, ExtensionKind::Plugin);
        assert_eq!(e.id, "cc-marketplace:hello");
        assert_eq!(e.source_id, "cc-marketplace");
        assert_eq!(e.trust_tier, TrustTier::Verified);
        assert_eq!(e.version.as_deref(), Some("0.2.0"));
        assert!(e.tags.contains(&"plugin".to_string()));
    }
}
