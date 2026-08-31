//! Live-backend reconciliation: project what is actually installed (MCP servers,
//! plugins, skills) into the one `ExtensionEntry` shape, then stamp that state
//! onto catalog entries.
//!
//! This is Hub domain logic, not interface I/O (R4): the `extensions.*` RPC
//! handlers and the `hub_catalog_search` tool are two callers of the same
//! reconciliation, and neither owns it.

use std::collections::HashMap;

use crate::extension::{PluginRecord, PluginStatus};
use crate::hub::install::mcp_server_id;
use crate::hub::origin::{local_ref_addresses, InstallOrigin};
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};
use crate::mcp::manager::{HealthStatus, McpManagerHandle, McpServerInfo};
use crate::skill::status::SkillStatusEntry;

fn base_entry(kind: ExtensionKind, local_id: &str, name: String) -> ExtensionEntry {
    ExtensionEntry {
        id: format!("local:{}:{}", kind.as_str(), local_id),
        kind,
        category: ExtensionCategory::Other,
        name,
        description: String::new(),
        author: None,
        icon: None,
        tags: vec![kind.as_str().to_string()],
        version: None,
        source_id: "local".into(),
        repo_url: None,
        trust_tier: TrustTier::Unverified,
        requires_config: false,
        config_schema: None,
        installed: true,
        enabled: true,
        update_available: false,
        via: None,
        install_spec: None,
    }
}

pub fn mcp_to_entry(info: &McpServerInfo) -> ExtensionEntry {
    let mut e = base_entry(ExtensionKind::Mcp, &info.id, info.name.clone());
    e.enabled = !matches!(info.health, HealthStatus::Stopped | HealthStatus::Dead);
    e
}

pub fn plugin_to_entry(p: &PluginRecord) -> ExtensionEntry {
    let mut e = base_entry(ExtensionKind::Plugin, &p.id, p.name.clone());
    e.description = p.description.clone().unwrap_or_default();
    e.version = p.version.clone();
    e.enabled = matches!(p.status, PluginStatus::Loaded);
    e
}

pub fn skill_to_entry(s: &SkillStatusEntry) -> ExtensionEntry {
    let mut e = base_entry(ExtensionKind::Skill, s.id.as_str(), s.name.clone());
    e.enabled = !s.disabled;
    e
}

/// Live-reconciled installed extensions across MCP / plugins / skills.
///
/// Best-effort: a failing or empty backend is logged and skipped — it never
/// aborts, so a flaky MCP actor cannot blank the catalog or installed views.
/// All calls are local (no network), so callers stay offline-capable.
pub async fn collect_installed(mcp: Option<McpManagerHandle>) -> Vec<ExtensionEntry> {
    let mut out = Vec::new();

    if let Some(mcp) = &mcp {
        match mcp.list_servers().await {
            Ok(servers) => out.extend(servers.iter().map(mcp_to_entry)),
            Err(e) => tracing::warn!("collect_installed: mcp list failed: {e}"),
        }
    }

    if let Some(mgr) = crate::extension::try_extension_manager() {
        if let Err(e) = mgr.ensure_loaded().await {
            tracing::warn!("collect_installed: failed to load plugins: {e}");
        }
        out.extend(mgr.list_plugin_records().await.iter().map(plugin_to_entry));
    }

    crate::skill::ensure_shared_skill_system_initialized().await;
    out.extend(
        crate::skill::shared_skill_system()
            .full_status()
            .await
            .iter()
            .map(skill_to_entry),
    );

    out
}

/// Stamp `installed` / `enabled` / `update_available` onto each catalog entry.
///
/// `installed` / `enabled` come from the **live** backends: MCP matches exactly
/// by its deterministic derived id (`local:mcp:{mcp_server_id(entry.id)}`);
/// Plugin / Skill match by case-insensitive `name` within the same `kind`,
/// BUT only when the name is unambiguous within the installed set — when two
/// distinct installed entries share a name we can no longer tell which
/// catalog entry owns it, so we leave BOTH catalog entries unmarked and emit
/// a warning (see H6 in review/hub-statics). The previously-silent collision
/// made the UI claim two catalog entries were installed when only one was.
///
/// `update_available` comes from the install provenance ledger and is only ever
/// claimed for an entry the live set already reports installed: the ledger says
/// what version/spec *we* installed, the catalog says what is offered now. No
/// ledger row → no claim.
pub fn mark_installed_state(
    catalog: &mut [ExtensionEntry],
    installed: &[ExtensionEntry],
    origins: &[InstallOrigin],
) {
    // (kind, lowercased name) -> set of installed entries with that name.
    // Using a Vec per key lets us detect collisions: `by_name[k].len() > 1`
    // means we can't safely attribute the installed state to any single
    // catalog entry.
    let mut by_name: HashMap<(String, String), Vec<&ExtensionEntry>> = HashMap::new();
    // Parallel MCP facade-id index: the MCP branch below would otherwise do
    // an O(installed) linear scan per catalog entry, turning this loop into
    // O(catalog × installed). The façade id is unique per install, so a
    // single HashMap lookup suffices.
    let mut by_mcp_facade: HashMap<String, &ExtensionEntry> = HashMap::new();
    for e in installed {
        by_name
            .entry((e.kind.as_str().to_string(), e.name.trim().to_lowercase()))
            .or_default()
            .push(e);
        if e.kind == ExtensionKind::Mcp {
            by_mcp_facade.insert(e.id.clone(), e);
        }
    }

    for e in catalog.iter_mut() {
        let enabled = if e.kind == ExtensionKind::Mcp {
            let expected = format!("local:mcp:{}", mcp_server_id(&e.id));
            by_mcp_facade.get(&expected).map(|ie| ie.enabled)
        } else {
            let key = (e.kind.as_str().to_string(), e.name.trim().to_lowercase());
            match by_name.get(&key) {
                Some(candidates) if candidates.len() == 1 => Some(candidates[0].enabled),
                Some(candidates) => {
                    // Ambiguous: two installed entries share this name. Prefer
                    // the ledger to disambiguate — the install_origin row ties
                    // an entry_id to a backend, so if exactly one candidate has
                    // a ledger row matching our catalog id we can claim it.
                    let ours = candidates.iter().find(|c| {
                        origins.iter().any(|o| o.entry_id == e.id)
                            && local_ref_addresses(&ledger_local_ref(origins, &e.id), &c.id)
                    });
                    match ours {
                        Some(c) => Some(c.enabled),
                        None => {
                            tracing::warn!(
                                kind = e.kind.as_str(),
                                name = %e.name,
                                candidates = candidates.len(),
                                "reconcile: name collision in installed set; skipping stamp"
                            );
                            None
                        }
                    }
                }
                None => None,
            }
        };
        if let Some(en) = enabled {
            e.installed = true;
            e.enabled = en;
            if let Some(origin) = origins.iter().find(|o| o.entry_id == e.id) {
                e.update_available = crate::hub::origin::update_available(origin, e);
            }
        }
    }
}

/// Look up the ledger `local_ref` for an entry id (used to disambiguate
/// installed-set collisions). Returns `""` when the entry has no ledger row,
/// which makes `local_ref_addresses("", _)` false and lets the caller fall
/// through to the warning path.
fn ledger_local_ref(origins: &[InstallOrigin], entry_id: &str) -> String {
    origins
        .iter()
        .find(|o| o.entry_id == entry_id)
        .map(|o| o.local_ref.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::manager::McpTransportType;

    #[test]
    fn mcp_server_becomes_installed_entry() {
        let info = McpServerInfo {
            id: "github".into(),
            name: "GitHub".into(),
            transport: McpTransportType::Stdio,
            tool_count: 12,
            resource_count: 0,
            resource_template_count: 0,
            prompt_count: 0,
            health: HealthStatus::Healthy,
        };
        let e = mcp_to_entry(&info);
        assert_eq!(e.kind, ExtensionKind::Mcp);
        assert!(e.installed);
        assert!(e.enabled); // Healthy => enabled
        assert_eq!(e.id, "local:mcp:github");
        assert_eq!(e.source_id, "local");
        assert_eq!(e.trust_tier, TrustTier::Unverified);
    }

    #[test]
    fn stopped_mcp_is_disabled() {
        let info = McpServerInfo {
            id: "x".into(),
            name: "X".into(),
            transport: McpTransportType::Stdio,
            tool_count: 0,
            resource_count: 0,
            resource_template_count: 0,
            prompt_count: 0,
            health: HealthStatus::Stopped,
        };
        assert!(!mcp_to_entry(&info).enabled);
    }
    fn catalog_entry(id: &str, kind: ExtensionKind, name: &str) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind,
            category: ExtensionCategory::Other,
            name: name.into(),
            description: String::new(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: "aleph-hub".into(),
            repo_url: None,
            trust_tier: TrustTier::Unverified,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: Some("Aleph Hub".into()),
            install_spec: None,
        }
    }

    fn installed_entry(id: &str, kind: ExtensionKind, name: &str, enabled: bool) -> ExtensionEntry {
        let mut e = catalog_entry(id, kind, name);
        e.installed = true;
        e.enabled = enabled;
        e.source_id = "local".into();
        e.via = None;
        e
    }

    #[test]
    fn mcp_entry_marked_installed_by_derived_id() {
        // catalog id "aleph-hub:github" -> install id "aleph-hub_github"
        // -> reconciled installed id "local:mcp:aleph-hub_github"
        let mut catalog = vec![catalog_entry(
            "aleph-hub:github",
            ExtensionKind::Mcp,
            "GitHub",
        )];
        let installed = vec![installed_entry(
            "local:mcp:aleph-hub_github",
            ExtensionKind::Mcp,
            "GitHub",
            true,
        )];
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(catalog[0].installed);
        assert!(catalog[0].enabled);
    }

    #[test]
    fn mcp_entry_not_installed_when_no_match() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:absent",
            ExtensionKind::Mcp,
            "Nope",
        )];
        let installed = vec![installed_entry(
            "local:mcp:something-else",
            ExtensionKind::Mcp,
            "Other",
            true,
        )];
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(!catalog[0].installed);
    }

    #[test]
    fn plugin_entry_marked_installed_by_name_case_insensitive() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:cool-plugin",
            ExtensionKind::Plugin,
            "Cool Plugin",
        )];
        // discovered plugin id differs; matched by name; enabled=false propagates
        let installed = vec![installed_entry(
            "local:plugin:whatever",
            ExtensionKind::Plugin,
            "cool plugin",
            false,
        )];
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(catalog[0].installed);
        assert!(!catalog[0].enabled);
    }

    #[test]
    fn name_match_does_not_cross_kinds() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:x",
            ExtensionKind::Skill,
            "Shared Name",
        )];
        let installed = vec![installed_entry(
            "local:plugin:x",
            ExtensionKind::Plugin,
            "Shared Name",
            true,
        )];
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(!catalog[0].installed);
    }

    #[test]
    fn official_primer_slug_reconciles_against_live_server() {
        // primer id "aleph-hub:volcengine-veimagex" -> server id "aleph-hub_volcengine-veimagex"
        let mut catalog = vec![catalog_entry(
            "aleph-hub:volcengine-veimagex",
            ExtensionKind::Mcp,
            "veImageX",
        )];
        let installed = vec![installed_entry(
            "local:mcp:aleph-hub_volcengine-veimagex",
            ExtensionKind::Mcp,
            "veImageX",
            true,
        )];
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(catalog[0].installed);
    }

    #[test]
    fn skill_entry_marked_installed_by_name_case_insensitive() {
        // The primer's "aleph-hub:pdf-tools" Skill entry collapses against a live
        // local:skill entry of the same name — this is why official skills show
        // installed with NO reconcile change (the convergence's load-bearing fact).
        let mut catalog = vec![catalog_entry(
            "aleph-hub:pdf-tools",
            ExtensionKind::Skill,
            "PDF Tools",
        )];
        let installed = vec![installed_entry(
            "local:skill:pdf-tools",
            ExtensionKind::Skill,
            "pdf tools",
            true,
        )];
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(catalog[0].installed);
        assert!(catalog[0].enabled);
    }

    // --- update badge, driven by the install provenance ledger ---------------

    fn mcp_pair(version: Option<&str>) -> (Vec<ExtensionEntry>, Vec<ExtensionEntry>) {
        let mut c = catalog_entry("aleph-hub:github", ExtensionKind::Mcp, "GitHub");
        c.version = version.map(str::to_owned);
        c.install_spec = Some(crate::hub::types::InstallSpec::McpStdio {
            command: "npx".into(),
            args: vec!["@gh/mcp".into()],
            env: vec![],
        });
        let installed = vec![installed_entry(
            "local:mcp:aleph-hub_github",
            ExtensionKind::Mcp,
            "GitHub",
            true,
        )];
        (vec![c], installed)
    }

    #[test]
    fn update_badge_lights_when_catalog_version_moved_past_the_install() {
        let (mut catalog, installed) = mcp_pair(Some("1.0.0"));
        // Ledger recorded the 1.0.0 install; the catalog now offers 2.0.0.
        let mut at_install = catalog[0].clone();
        at_install.version = Some("1.0.0".into());
        let origin = crate::hub::origin::InstallOrigin::record(
            &at_install,
            at_install.install_spec.as_ref().unwrap(),
            "aleph-hub_github",
            0,
        );
        catalog[0].version = Some("2.0.0".into());
        mark_installed_state(&mut catalog, &installed, &[origin]);
        assert!(catalog[0].installed);
        assert!(catalog[0].update_available);
    }

    #[test]
    fn update_badge_stays_dark_at_the_same_version_and_spec() {
        let (mut catalog, installed) = mcp_pair(Some("1.0.0"));
        let origin = crate::hub::origin::InstallOrigin::record(
            &catalog[0].clone(),
            catalog[0].install_spec.as_ref().unwrap(),
            "aleph-hub_github",
            0,
        );
        mark_installed_state(&mut catalog, &installed, &[origin]);
        assert!(catalog[0].installed);
        assert!(!catalog[0].update_available);
    }

    /// A ledger row for something the live backends do not report must not light
    /// the badge — the badge is only meaningful next to an installed entry.
    #[test]
    fn update_badge_never_claims_for_a_non_installed_entry() {
        let (mut catalog, _) = mcp_pair(Some("1.0.0"));
        let mut at_install = catalog[0].clone();
        at_install.version = Some("1.0.0".into());
        let origin = crate::hub::origin::InstallOrigin::record(
            &at_install,
            at_install.install_spec.as_ref().unwrap(),
            "aleph-hub_github",
            0,
        );
        catalog[0].version = Some("2.0.0".into());
        // Empty live installed set.
        mark_installed_state(&mut catalog, &[], &[origin]);
        assert!(!catalog[0].installed);
        assert!(!catalog[0].update_available);
    }

    /// Pre-ledger installs (no row) keep working: installed still resolves from
    /// the live backends, the badge simply makes no claim.
    #[test]
    fn entry_without_a_ledger_row_is_installed_but_makes_no_update_claim() {
        let (mut catalog, installed) = mcp_pair(Some("1.0.0"));
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(catalog[0].installed);
        assert!(!catalog[0].update_available);
    }

    // --- the installed panel's badge, walked façade id → ledger → catalog -----
    #[test]
    fn skill_entry_not_installed_when_name_differs() {
        let mut catalog = vec![catalog_entry(
            "aleph-hub:pdf-tools",
            ExtensionKind::Skill,
            "PDF Tools",
        )];
        let installed = vec![installed_entry(
            "local:skill:other",
            ExtensionKind::Skill,
            "Other Skill",
            true,
        )];
        mark_installed_state(&mut catalog, &installed, &[]);
        assert!(!catalog[0].installed);
    }
}
