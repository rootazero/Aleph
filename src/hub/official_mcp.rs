//! Official MCP preset projection + legacy migration.
//!
//! Projects the in-binary `src/mcp/presets/catalog.json` into `ExtensionEntry`s
//! for the `aleph-hub` source slot (consumed by `hub::primer`). Also migrates
//! servers installed via the retired preset path. The cold-start gate that
//! writes the slot lives in `hub::primer`.

use crate::hub::catalog_client::ALEPH_HUB_ID;
use crate::hub::types::{
    EnvDecl, ExtensionCategory, ExtensionEntry, ExtensionKind, InstallSpec, McpTransport, TrustTier,
};
use crate::mcp::manager::{McpManagerConfig, McpManagerHandle, McpTransportType};
use crate::mcp::presets::{self, McpPreset, PresetCategory, PresetEnvVar, PresetTransport};

fn map_category(c: PresetCategory) -> ExtensionCategory {
    match c {
        PresetCategory::Developer => ExtensionCategory::Developer,
        PresetCategory::Daily => ExtensionCategory::Utilities,
    }
}

fn map_env(ev: &PresetEnvVar) -> EnvDecl {
    let description = if ev.description.is_empty() {
        ev.label.clone()
    } else {
        ev.description.clone()
    };
    EnvDecl {
        name: ev.key.clone(),
        description: Some(description),
        required: ev.required,
        secret: ev.secret,
        default: ev.default.clone(),
        placeholder: None,
        how_to_get_url: ev.how_to_get_url.clone(),
    }
}

/// A transport is projectable iff it carries no `<ENV_KEY>` placeholder — the
/// Hub install path injects keys via env/headers, never by string interpolation.
fn is_projectable(t: &PresetTransport) -> bool {
    let clean = |s: &str| !s.contains('<');
    match t.kind {
        McpTransportType::Stdio => {
            // The previous check covered `args` only; a stdio `command` with an
            // interpolation (`<ENV_KEY>`) would have slipped through and the
            // secrets resolver would have no real env var to substitute — the
            // install would silently drop the placeholder. (review/hub-statics)
            t.command.as_deref().map(clean).unwrap_or(false) && t.args.iter().all(|a| clean(a))
        }
        McpTransportType::Http | McpTransportType::Sse => {
            t.url.as_deref().map(clean).unwrap_or(false)
        }
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
        trust_tier: if p.official {
            TrustTier::Official
        } else {
            TrustTier::Community
        },
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

/// Setup guidance for a Hub entry, if its id maps to an in-binary MCP preset
/// that carries `post_install`. Keyed by the Hub entry id (`aleph-hub:<slug>`).
/// Returns `None` for non-preset ids, unknown slugs, or presets without
/// guidance.
pub fn post_install_for(entry_id: &str) -> Option<&'static str> {
    let slug = entry_id.strip_prefix(&format!("{ALEPH_HUB_ID}:"))?;
    presets::find(slug)?.post_install.as_deref()
}

/// True iff `cfg` was installed via the retired preset path: its id is a known
/// preset slug AND its launch shape matches that preset. New Hub installs use
/// `aleph-hub_<slug>` ids, so a raw-slug id never collides with a Hub install.
pub fn is_legacy_preset_server(cfg: &McpManagerConfig) -> bool {
    let Some(preset) = presets::find(&cfg.id) else {
        return false;
    };
    preset.transports.iter().any(|t| match t.kind {
        McpTransportType::Stdio => t.command == cfg.command,
        McpTransportType::Http | McpTransportType::Sse => cfg.command.is_none(),
    })
}

/// Boot migration (D9): remove servers installed via the retired preset path so
/// the user re-installs from the Hub. Warn-only; never aborts boot.
pub async fn migrate_legacy_preset_servers(mcp: &McpManagerHandle) {
    let configs = match mcp.list_server_configs().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "preset migration: list_server_configs failed");
            return;
        }
    };
    for cfg in configs {
        if is_legacy_preset_server(&cfg) {
            let id = cfg.id.clone();
            match mcp.remove_server(id.clone()).await {
                Ok(()) => {
                    tracing::info!(%id, "removed retired-preset MCP server; re-install from Aleph Hub")
                }
                Err(e) => tracing::warn!(%id, error = %e, "preset migration: remove_server failed"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_legacy_preset_server, primer_entries};
    use crate::hub::types::{ExtensionKind, InstallSpec, TrustTier};

    fn by_id(
        entries: &[crate::hub::types::ExtensionEntry],
        id: &str,
    ) -> crate::hub::types::ExtensionEntry {
        entries
            .iter()
            .find(|e| e.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("missing {id}"))
    }

    #[test]
    fn unreal_engine_projects_to_keyless_remote() {
        let e = primer_entries();
        let ue = by_id(&e, "aleph-hub:unreal-engine");
        assert_eq!(ue.kind, ExtensionKind::Mcp);
        assert_eq!(ue.trust_tier, TrustTier::Official);
        match ue.install_spec.unwrap() {
            InstallSpec::McpRemote { url, transport, .. } => {
                assert_eq!(url, "http://127.0.0.1:8000/mcp");
                assert!(matches!(
                    transport,
                    crate::hub::types::McpTransport::StreamableHttp
                ));
            }
            other => panic!("expected McpRemote, got {other:?}"),
        }
        assert!(!ue.requires_config);
    }

    /// `map_env` used to drop `how_to_get_url`, so the Configure step asked for
    /// a key with nowhere to get it. This is the first of the five hops between
    /// the catalog and the Panel form field — the one that was cut.
    #[test]
    fn projected_env_carries_how_to_get_url() {
        let e = primer_entries();
        let zhipu = by_id(&e, "aleph-hub:zhipu-vision");
        let InstallSpec::McpStdio { env, .. } = zhipu.install_spec.unwrap() else {
            panic!("expected McpStdio");
        };
        let key = env
            .iter()
            .find(|d| d.name == "Z_AI_API_KEY")
            .expect("Z_AI_API_KEY declared");
        assert_eq!(
            key.how_to_get_url.as_deref(),
            Some("https://bigmodel.cn/coding-plan/personal/overview")
        );
        // A preset that declares no source for a var leaves it unset.
        assert!(env
            .iter()
            .find(|d| d.name == "Z_AI_MODE")
            .expect("Z_AI_MODE declared")
            .how_to_get_url
            .is_none());
    }

    #[test]
    fn post_install_for_unreal_returns_guidance() {
        let g = super::post_install_for("aleph-hub:unreal-engine").expect("guidance");
        assert!(g.contains("Unreal MCP"));
    }

    #[test]
    fn post_install_for_preset_without_guidance_is_none() {
        assert!(super::post_install_for("aleph-hub:context7").is_none());
    }

    #[test]
    fn post_install_for_unprefixed_or_unknown_is_none() {
        assert!(super::post_install_for("unreal-engine").is_none()); // missing aleph-hub: prefix
        assert!(super::post_install_for("aleph-hub:nope").is_none()); // unknown slug
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
                assert!(env
                    .iter()
                    .any(|d| d.name == "AMAP_MAPS_API_KEY" && d.required && d.secret));
            }
            other => panic!("expected McpStdio (http url has <KEY>), got {other:?}"),
        }
        assert!(amap.requires_config);
    }

    #[test]
    fn zhipu_vision_projects_to_stdio_with_required_key() {
        let e = primer_entries();
        let z = by_id(&e, "aleph-hub:zhipu-vision");
        assert_eq!(z.trust_tier, TrustTier::Official);
        match z.install_spec.unwrap() {
            InstallSpec::McpStdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert_eq!(args, vec!["-y", "@z_ai/mcp-server"]);
                assert!(env
                    .iter()
                    .any(|d| d.name == "Z_AI_API_KEY" && d.required && d.secret));
            }
            other => panic!("expected McpStdio, got {other:?}"),
        }
        assert!(z.requires_config);
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

    #[test]
    fn legacy_detection_matches_old_slug_and_shape() {
        use crate::mcp::manager::McpManagerConfig;
        // minimax old install: raw slug id + matching stdio command.
        let minimax = McpManagerConfig::stdio("minimax", "MiniMax", "uvx");
        assert!(is_legacy_preset_server(&minimax));
        // amap old install: raw slug id + remote (no command) matches its http transport.
        let amap = McpManagerConfig::http("amap", "高德地图", "https://mcp.amap.com/mcp?key=k");
        assert!(is_legacy_preset_server(&amap));
        // New Hub install id is never a raw slug -> not legacy.
        let hub = McpManagerConfig::stdio("aleph-hub_minimax", "MiniMax", "uvx");
        assert!(!is_legacy_preset_server(&hub));
        // User custom server that merely shares a slug name but a different command.
        let custom = McpManagerConfig::stdio("minimax", "My MiniMax", "/opt/custom");
        assert!(!is_legacy_preset_server(&custom));
        // Unknown id -> not legacy.
        let other = McpManagerConfig::stdio("totally-custom", "X", "node");
        assert!(!is_legacy_preset_server(&other));
    }
}
