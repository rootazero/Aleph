//! Cold-start projection of bundled official plugins into Hub catalog entries.
//!
//! Projects the bundled `aleph-official` marketplace manifest (embedded via
//! `BUNDLED_PLUGINS`) into `ExtensionEntry`s for the `aleph-hub` source slot
//! (consumed by `hub::primer`) so official plugins are browsable/installable
//! offline and before the remote catalog is fetched. The remote fetch later
//! overwrites the slot wholesale (no peer source, no dedup).

use crate::bundled::{BUNDLED_PLUGINS, OFFICIAL_PLUGINS_REPO};
use crate::extension::marketplace::manifest::parse_marketplace_toml_content;
use crate::extension::marketplace::MarketplacePluginEntry;
use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, TrustTier};

/// Relative path of the marketplace manifest inside the bundled plugins tree.
const MARKETPLACE_TOML: &str = ".claude-plugin/marketplace.toml";

/// Project one marketplace plugin entry into a Hub catalog entry.
///
/// `source_id` is `aleph-hub` (slot correctness — the remote fetch refreshes the
/// slot by this key). The `GitDir` spec is a *routing marker* (makes `run_install`
/// take the plugin branch) plus provenance; the plugin install path reads the
/// local marketplace cache by `name`, so `git_url`/`subdir` are NOT consumed there.
fn project_plugin(entry: &MarketplacePluginEntry) -> ExtensionEntry {
    // The marketplace `source` is a "./<dir>" path relative to the marketplace
    // root; keep only the leaf for provenance.
    // Only the path form names a directory inside the repo; an external
    // source (github/npm/...) has no subdir here, and provenance falls back to
    // the repo root rather than inventing one.
    let subdir = entry.source.as_relative_path().map(|source| {
        source
            .strip_prefix("./")
            .or_else(|| source.strip_prefix('.'))
            .unwrap_or(source)
            .to_string()
    });
    let spec = InstallSpec::GitDir {
        git_url: OFFICIAL_PLUGINS_REPO.to_string(),
        subdir,
        git_ref: None,
        sha256: None,
    };
    ExtensionEntry {
        id: format!("{ALEPH_HUB_ID}:{}", entry.name),
        kind: ExtensionKind::Plugin,
        category: ExtensionCategory::Other,
        name: entry.name.clone(),
        description: entry.description.clone().unwrap_or_default(),
        author: None,
        icon: None,
        tags: vec![ExtensionKind::Plugin.as_str().to_string()],
        version: entry.version.clone(),
        source_id: ALEPH_HUB_ID.to_string(),
        repo_url: Some(OFFICIAL_PLUGINS_REPO.to_string()),
        trust_tier: TrustTier::Official,
        requires_config: spec.requires_config(),
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
        via: Some(ALEPH_HUB_ID.to_string()),
        install_spec: Some(spec),
    }
}

/// Project the in-binary bundled official marketplace's plugins into Hub entries.
/// Returns `[]` (logged) when the `plugins/` submodule was absent at build time
/// or the bundled manifest is missing/unparseable.
pub fn primer_entries() -> Vec<ExtensionEntry> {
    let Some(content) = BUNDLED_PLUGINS
        .get_file(MARKETPLACE_TOML)
        .and_then(|f| f.contents_utf8())
    else {
        tracing::info!(
            "official plugins primer: bundled marketplace manifest absent (submodule absent at build) — no plugin entries"
        );
        return Vec::new();
    };
    match parse_marketplace_toml_content(content) {
        Ok(manifest) => manifest.plugins.iter().map(project_plugin).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "official plugins primer: failed to parse bundled marketplace manifest");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MANIFEST: &str = r##"
name = "aleph-official"

[[plugins]]
name = "diagnostics"
source = "./diagnostics"
description = "System health monitoring"
version = "0.1.0"

[[plugins]]
name = "diff-viewer"
source = "./diff-viewer"
"##;

    #[test]
    fn project_plugin_yields_official_aleph_hub_plugin_entry() {
        let manifest = parse_marketplace_toml_content(SAMPLE_MANIFEST).unwrap();
        let e = project_plugin(&manifest.plugins[0]);
        assert_eq!(e.id, "aleph-hub:diagnostics");
        assert_eq!(e.kind, ExtensionKind::Plugin);
        assert_eq!(e.category, ExtensionCategory::Other);
        assert_eq!(e.trust_tier, TrustTier::Official);
        assert_eq!(e.source_id, "aleph-hub");
        assert_eq!(e.via.as_deref(), Some("aleph-hub"));
        assert_eq!(e.name, "diagnostics");
        assert_eq!(e.description, "System health monitoring");
        assert_eq!(e.version.as_deref(), Some("0.1.0"));
        assert!(!e.installed);
        match e.install_spec.unwrap() {
            InstallSpec::GitDir {
                git_url,
                subdir,
                git_ref,
                sha256,
            } => {
                assert_eq!(git_url, OFFICIAL_PLUGINS_REPO);
                assert_eq!(subdir.as_deref(), Some("diagnostics"));
                assert!(git_ref.is_none() && sha256.is_none());
            }
            other => panic!("expected GitDir, got {other:?}"),
        }
        // GitDir requires no env config → plugin cards install with no config gate.
        assert!(!e.requires_config);
    }

    #[test]
    fn project_plugin_defaults_absent_description_and_version() {
        let manifest = parse_marketplace_toml_content(SAMPLE_MANIFEST).unwrap();
        // diff-viewer entry omits description and version.
        let e = project_plugin(&manifest.plugins[1]);
        assert_eq!(e.id, "aleph-hub:diff-viewer");
        assert_eq!(e.name, "diff-viewer");
        assert_eq!(e.description, "");
        assert!(e.version.is_none());
    }

    #[test]
    fn primer_entries_tolerates_absent_bundle() {
        // The plugins submodule may be empty in dev/CI; primer_entries must not
        // panic, and whatever it returns must be well-formed official plugins
        // anchored in the aleph-hub slot.
        let entries = primer_entries();
        for e in &entries {
            assert_eq!(e.kind, ExtensionKind::Plugin);
            assert_eq!(e.trust_tier, TrustTier::Official);
            assert_eq!(e.source_id, ALEPH_HUB_ID);
            assert!(e.id.starts_with("aleph-hub:"));
        }
    }
}
