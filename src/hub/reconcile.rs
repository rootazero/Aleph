use crate::extension::{PluginRecord, PluginStatus};
use crate::hub::types::{ExtensionCategory, ExtensionEntry, ExtensionKind, TrustTier};
use crate::mcp::manager::{HealthStatus, McpServerInfo};
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
}
