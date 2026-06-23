//! Cold-start primer + legacy migration for official MCP presets.
//!
//! Projects the in-binary `src/mcp/presets/catalog.json` into `ExtensionEntry`s
//! under the `aleph-hub` source slot so official MCP is browsable/installable
//! offline and before the remote catalog is first fetched. The remote fetch
//! later overwrites the slot wholesale (no peer source, no local dedup).

use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, McpTransport, TrustTier,
};
use crate::mcp::manager::McpTransportType;
use crate::mcp::presets::{self, McpPreset, PresetCategory, PresetEnvVar, PresetTransport};

fn map_category(c: PresetCategory) -> ExtensionCategory {
    match c {
        PresetCategory::Developer => ExtensionCategory::Developer,
        PresetCategory::Daily => ExtensionCategory::Utilities,
        PresetCategory::ModelProvider => ExtensionCategory::Design,
    }
}

fn map_env(ev: &PresetEnvVar) -> EnvDecl {
    let description = if ev.description.is_empty() { ev.label.clone() } else { ev.description.clone() };
    EnvDecl {
        name: ev.key.clone(),
        description: Some(description),
        required: ev.required,
        secret: ev.secret,
        default: ev.default.clone(),
        placeholder: None,
    }
}

/// A transport is projectable iff it carries no `<ENV_KEY>` placeholder — the
/// Hub install path injects keys via env/headers, never by string interpolation.
fn is_projectable(t: &PresetTransport) -> bool {
    let clean = |s: &str| !s.contains('<');
    match t.kind {
        McpTransportType::Stdio => t.args.iter().all(|a| clean(a)),
        McpTransportType::Http | McpTransportType::Sse => t.url.as_deref().map(clean).unwrap_or(false),
    }
}

fn map_install_spec(p: &McpPreset) -> Option<InstallSpec> {
    let t = p.transports.iter().find(|t| is_projectable(t))?;
    let env: Vec<EnvDecl> = p.required_env.iter().map(map_env).collect();
    Some(match t.kind {
        McpTransportType::Stdio => InstallSpec::McpStdio {
            command: t.command.clone().unwrap_or_default(),
            args: t.args.clone(),
            env,
        },
        McpTransportType::Http => InstallSpec::McpRemote {
            url: t.url.clone().unwrap_or_default(),
            transport: McpTransport::StreamableHttp,
            headers: vec![],
        },
        McpTransportType::Sse => InstallSpec::McpRemote {
            url: t.url.clone().unwrap_or_default(),
            transport: McpTransport::Sse,
            headers: vec![],
        },
    })
}

fn map_entry(p: &McpPreset) -> Option<ExtensionEntry> {
    let spec = map_install_spec(p)?;
    Some(ExtensionEntry {
        id: format!("{ALEPH_HUB_ID}:{}", p.id),
        kind: ExtensionKind::Mcp,
        category: map_category(p.category),
        name: p.name.clone(),
        description: p.description.clone(),
        author: Some(p.vendor.clone()),
        icon: None,
        tags: p.tags.clone(),
        version: None,
        source_id: ALEPH_HUB_ID.to_string(),
        repo_url: None,
        trust_tier: if p.official { TrustTier::Official } else { TrustTier::Community },
        requires_config: spec.requires_config(),
        config_schema: None,
        installed: false,
        enabled: false,
        update_available: false,
        via: Some(ALEPH_HUB_ID.to_string()),
        install_spec: Some(spec),
    })
}

/// Project the in-binary official MCP preset catalog into Hub catalog entries.
pub fn primer_entries() -> Vec<ExtensionEntry> {
    presets::catalog().iter().filter_map(map_entry).collect()
}

#[cfg(test)]
mod tests {
    use crate::hub::types::{ExtensionKind, InstallSpec, TrustTier};
    use super::primer_entries;

    fn by_id(entries: &[crate::hub::types::ExtensionEntry], id: &str) -> crate::hub::types::ExtensionEntry {
        entries.iter().find(|e| e.id == id).cloned().unwrap_or_else(|| panic!("missing {id}"))
    }

    #[test]
    fn primer_ids_are_aleph_hub_prefixed_and_official() {
        let e = primer_entries();
        let ctx = by_id(&e, "aleph-hub:context7");
        assert_eq!(ctx.kind, ExtensionKind::Mcp);
        assert_eq!(ctx.trust_tier, TrustTier::Official);
        assert_eq!(ctx.source_id, "aleph-hub");
    }

    #[test]
    fn context7_projects_to_keyless_remote() {
        let e = primer_entries();
        let ctx = by_id(&e, "aleph-hub:context7");
        match ctx.install_spec.unwrap() {
            InstallSpec::McpRemote { url, .. } => assert_eq!(url, "https://mcp.context7.com/mcp"),
            other => panic!("expected McpRemote, got {other:?}"),
        }
        assert!(!ctx.requires_config);
    }

    #[test]
    fn amap_skips_key_interpolated_http_for_stdio_env() {
        let e = primer_entries();
        let amap = by_id(&e, "aleph-hub:amap");
        match amap.install_spec.unwrap() {
            InstallSpec::McpStdio { command, env, .. } => {
                assert_eq!(command, "npx");
                assert!(env.iter().any(|d| d.name == "AMAP_MAPS_API_KEY" && d.required && d.secret));
            }
            other => panic!("expected McpStdio (http url has <KEY>), got {other:?}"),
        }
        assert!(amap.requires_config);
    }

    #[test]
    fn veimagex_carries_all_four_env_decls() {
        let e = primer_entries();
        let v = by_id(&e, "aleph-hub:volcengine-veimagex");
        match v.install_spec.unwrap() {
            InstallSpec::McpStdio { command, env, .. } => {
                assert_eq!(command, "uvx");
                assert_eq!(env.len(), 4);
            }
            other => panic!("expected McpStdio, got {other:?}"),
        }
    }
}
