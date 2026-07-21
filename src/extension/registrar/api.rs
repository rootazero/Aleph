//! `CapabilityApi` — unified registration surface for plugins
//!
//! Takes a mutable borrow of `PluginRegistry` and dispatches
//! `CapabilityDeclaration` variants to the appropriate `register_*` method,
//! with tiered permission checking.

use anyhow::{anyhow, Result};

use crate::extension::capability::{CapabilityDeclaration, Tier};
use crate::extension::manifest::PluginPermission;
use crate::extension::registry::PluginRegistry;
use crate::extension::types::PluginRecord;

/// Unified registration API for writing capabilities into `PluginRegistry`.
///
/// This struct borrows the registry mutably and provides permission-checked
/// capability registration. It is the single entry point that all registrars
/// (MCP, WASM, manifest adapters) use to write into the registry.
pub struct CapabilityApi<'a> {
    /// The plugin registry to write into
    registry: &'a mut PluginRegistry,
    /// ID of the plugin performing registration
    plugin_id: String,
    /// Permissions granted to this plugin
    permissions: Vec<PluginPermission>,
}

impl<'a> CapabilityApi<'a> {
    /// Create a new `CapabilityApi` for a specific plugin.
    pub const fn new(
        registry: &'a mut PluginRegistry,
        plugin_id: String,
        permissions: Vec<PluginPermission>,
    ) -> Self {
        Self {
            registry,
            plugin_id,
            permissions,
        }
    }

    /// Register a capability declaration with tiered permission checking.
    ///
    /// - P0 (Core) and P1 (Important): no permission check
    /// - P2 (Pluggable): permission check if required by the capability
    pub fn register_capability(&mut self, decl: CapabilityDeclaration) -> Result<()> {
        self.validate_owner(&decl)?;
        let tier = decl.tier();

        match tier {
            Tier::Core | Tier::Important => {
                // No permission check needed
            }
            Tier::Pluggable => {
                if let Some(perm) = decl.required_permission() {
                    self.require_permission(&perm)?;
                }
            }
        }

        self.dispatch(decl)
    }

    /// Dispatch a capability declaration to the appropriate registry method.
    fn dispatch(&mut self, decl: CapabilityDeclaration) -> Result<()> {
        match decl {
            CapabilityDeclaration::Tool(tool) => {
                self.registry.register_tool(tool);
            }
            CapabilityDeclaration::Hook(hook) => {
                self.registry.register_hook(hook);
            }
            CapabilityDeclaration::Service(service) => {
                self.registry.register_service(service);
            }
            CapabilityDeclaration::Skill(skill) => {
                // Plugin `commands/` markdown arrives here too, as a
                // `SkillRegistration` tagged `skill_type = Command`.
                self.registry.register_skill(skill);
            }
            CapabilityDeclaration::Agent(agent) => {
                self.registry.register_agent(agent);
            }
            CapabilityDeclaration::McpServer(_) => {
                // No-op: MCP servers are handled by the loader, not the registry
            }
        }
        Ok(())
    }

    /// Reload a plugin: unregister all existing capabilities and re-register
    /// with a new record and capability set.
    pub fn reload(
        &mut self,
        record: PluginRecord,
        new_caps: Vec<CapabilityDeclaration>,
    ) -> Result<()> {
        for cap in &new_caps {
            self.validate_owner(cap)?;
            if let Some(perm) = cap.required_permission() {
                self.require_permission(&perm)?;
            }
        }

        self.registry.unregister_plugin(&self.plugin_id);
        self.registry.register_plugin(record);

        for cap in new_caps {
            self.dispatch(cap)?;
        }

        Ok(())
    }

    fn validate_owner(&self, decl: &CapabilityDeclaration) -> Result<()> {
        let owner = match decl {
            CapabilityDeclaration::Tool(value) => Some(value.plugin_id.as_str()),
            CapabilityDeclaration::Hook(value) => Some(value.plugin_id.as_str()),
            CapabilityDeclaration::Service(value) => Some(value.plugin_id.as_str()),
            CapabilityDeclaration::Skill(value) => Some(value.plugin_id.as_str()),
            CapabilityDeclaration::Agent(value) => Some(value.plugin_id.as_str()),
            CapabilityDeclaration::McpServer(_) => None,
        };
        if owner.is_some_and(|owner| owner != self.plugin_id.as_str()) {
            return Err(anyhow!(
                "Capability owner does not match plugin '{}': {:?}",
                self.plugin_id,
                owner
            ));
        }
        Ok(())
    }

    /// Check that a required permission is present in this plugin's grants.
    fn require_permission(&self, perm: &PluginPermission) -> Result<()> {
        if self.permissions.contains(perm) {
            Ok(())
        } else {
            Err(anyhow!(
                "Plugin '{}' lacks required permission '{}' for this capability",
                self.plugin_id,
                perm
            ))
        }
    }

    /// Get a reference to the underlying registry (for inspection in tests).
    #[must_use]
    pub const fn registry(&self) -> &PluginRegistry {
        self.registry
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::registry::{
        AgentRegistration, HookRegistration, ServiceRegistration, SkillRegistration,
        ToolRegistration,
    };
    use crate::extension::types::HookEvent;

    fn make_registry_and_plugin() -> PluginRegistry {
        let mut registry = PluginRegistry::new();
        let record = PluginRecord::new(
            "test-plugin".to_string(),
            "Test Plugin".to_string(),
            crate::extension::types::PluginKind::Static,
            crate::extension::types::PluginOrigin::Global,
        );
        registry.register_plugin(record);
        registry
    }

    fn make_tool() -> CapabilityDeclaration {
        CapabilityDeclaration::Tool(ToolRegistration {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handle".to_string(),
            plugin_id: "test-plugin".to_string(),
        })
    }

    fn make_service() -> CapabilityDeclaration {
        CapabilityDeclaration::Service(ServiceRegistration {
            id: "test-service".to_string(),
            name: "Test Service".to_string(),
            start_handler: "start".to_string(),
            stop_handler: "stop".to_string(),
            plugin_id: "test-plugin".to_string(),
            auto_start: true,
        })
    }

    fn make_skill() -> CapabilityDeclaration {
        CapabilityDeclaration::Skill(SkillRegistration {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            content: "You are a test skill.".to_string(),
            triggers: vec!["test".to_string()],
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        })
    }

    fn make_hook() -> CapabilityDeclaration {
        CapabilityDeclaration::Hook(HookRegistration {
            event: HookEvent::BeforeToolCall,
            priority: 0,
            handler: "on_tool".to_string(),
            name: None,
            description: None,
            plugin_id: "test-plugin".to_string(),
            kind: None,
            matcher: None,
            actions: Vec::new(),
            plugin_root: None,
            timeout_secs: None,
        })
    }

    // ── P0 registration succeeds without permissions ─────────────────────

    #[test]
    fn test_p0_registration_no_permissions_required() {
        let mut registry = make_registry_and_plugin();
        let mut api = CapabilityApi::new(
            &mut registry,
            "test-plugin".to_string(),
            vec![], // no permissions
        );

        // Tool is P0 (Core) — should succeed
        assert!(api.register_capability(make_tool()).is_ok());
        assert!(api.registry().get_tool("my_tool").is_some());

        // Hook is P0 (Core) — should succeed
        assert!(api.register_capability(make_hook()).is_ok());
        assert_eq!(api.registry().list_hooks().len(), 1);
    }

    #[test]
    fn test_p2_service_fails_without_background_permission() {
        let mut registry = make_registry_and_plugin();
        let mut api = CapabilityApi::new(
            &mut registry,
            "test-plugin".to_string(),
            vec![], // no permissions
        );

        let result = api.register_capability(make_service());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("background"));
    }

    // ── P2 Service succeeds with Background permission ───────────────────

    #[test]
    fn test_p2_service_succeeds_with_background_permission() {
        let mut registry = make_registry_and_plugin();
        let mut api = CapabilityApi::new(
            &mut registry,
            "test-plugin".to_string(),
            vec![PluginPermission::Background],
        );

        assert!(api.register_capability(make_service()).is_ok());
        assert!(api.registry().get_service("test-service").is_some());
    }

    // ── Reload clears and re-registers ───────────────────────────────────

    #[test]
    fn test_reload_clears_and_reregisters() {
        let mut registry = make_registry_and_plugin();

        // First registration
        {
            let mut api = CapabilityApi::new(&mut registry, "test-plugin".to_string(), vec![]);
            api.register_capability(make_tool()).unwrap();
            assert!(api.registry().get_tool("my_tool").is_some());
        }

        // Reload with different capabilities
        {
            let new_record = PluginRecord::new(
                "test-plugin".to_string(),
                "Test Plugin v2".to_string(),
                crate::extension::types::PluginKind::Static,
                crate::extension::types::PluginOrigin::Global,
            );

            let mut api = CapabilityApi::new(&mut registry, "test-plugin".to_string(), vec![]);

            api.reload(new_record, vec![make_skill()]).unwrap();

            // Old tool should be gone
            assert!(api.registry().get_tool("my_tool").is_none());
            // New skill should be present
            assert!(api.registry().get_skill("test-skill").is_some());
            // Plugin record updated
            assert_eq!(
                api.registry().get_plugin("test-plugin").unwrap().name,
                "Test Plugin v2"
            );
        }
    }

    // ── Dispatch routes to correct registry collections ──────────────────

    #[test]
    fn test_dispatch_routes_correctly() {
        let mut registry = make_registry_and_plugin();
        let mut api = CapabilityApi::new(
            &mut registry,
            "test-plugin".to_string(),
            vec![PluginPermission::Background],
        );

        // Register one of each type
        api.register_capability(make_tool()).unwrap();
        api.register_capability(make_hook()).unwrap();
        api.register_capability(make_service()).unwrap();
        api.register_capability(make_skill()).unwrap();

        api.register_capability(CapabilityDeclaration::Agent(AgentRegistration {
            name: "test-agent".to_string(),
            description: Some("d".to_string()),
            content: "prompt".to_string(),
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        }))
        .unwrap();

        // McpServer is a no-op
        api.register_capability(CapabilityDeclaration::McpServer(
            crate::extension::types::McpServerConfig {
                command: "npx".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
            },
        ))
        .unwrap();

        // Verify each collection got the registration
        let reg = api.registry();
        assert!(reg.get_tool("my_tool").is_some());
        assert_eq!(reg.list_hooks().len(), 1);
        assert!(reg.get_service("test-service").is_some());
        assert!(reg.get_skill("test-skill").is_some());
        assert!(reg.get_agent("test-agent").is_some());
    }

    // ── Skill (P2, no permission required) registers without permissions ─

    #[test]
    fn test_p2_skill_no_permission_needed() {
        let mut registry = make_registry_and_plugin();
        let mut api = CapabilityApi::new(
            &mut registry,
            "test-plugin".to_string(),
            vec![], // no permissions
        );

        // Skill is P2 but required_permission() returns None
        assert!(api.register_capability(make_skill()).is_ok());
        assert!(api.registry().get_skill("test-skill").is_some());
    }
}
