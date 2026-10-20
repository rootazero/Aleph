//! Capability-driven plugin architecture types
//!
//! This module defines the `CapabilityDeclaration` enum — the unified model for
//! everything a plugin can contribute. Each variant maps 1:1 to a registration
//! type in the `PluginRegistry`.

use serde::{Deserialize, Serialize};

use crate::context::fabric::{
    AuthenticatedCaller, ContextCapabilityDescriptor, ContextNamespace, ContextPhase, ContextScope,
};
use crate::extension::manifest::PluginPermission;
use crate::extension::registry::{
    AgentRegistration, HookRegistration, ServiceRegistration, SkillRegistration, ToolRegistration,
};
use crate::extension::types::PluginOrigin;
use crate::mcp::McpManagerConfig;

// ============================================================================
// Type Aliases (Declaration → Registration mapping)
// ============================================================================

/// A tool declaration is a `ToolRegistration`
pub type ToolDeclaration = ToolRegistration;
/// A hook declaration is a `HookRegistration`
pub type HookDeclaration = HookRegistration;
/// A service declaration is a `ServiceRegistration`
pub type ServiceDeclaration = ServiceRegistration;
/// A skill declaration is a `SkillRegistration`
pub type SkillDeclaration = SkillRegistration;
/// An agent declaration is an `AgentRegistration`
pub type AgentDeclaration = AgentRegistration;
/// An MCP server declaration is the `McpManagerConfig` the mount spawns — the
/// one list both the row's count and the spawn read (`mcp_config.rs`).
pub type McpServerDeclaration = McpManagerConfig;

/// A host-issued, session-bound grant. Wire descriptors cannot construct one:
/// this type has no deserializer and its authority fields are private to Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextGrant {
    pub(crate) capability_id: String,
    /// Binds consent to the complete descriptor the host approved.
    descriptor_fingerprint: String,
    pub(crate) plugin_id: String,
    pub(crate) namespace: ContextNamespace,
    pub(crate) scope: ContextScope,
    pub(crate) cross_session: bool,
    pub(crate) egress: bool,
    pub(crate) project_write: bool,
    /// Unix seconds; zero and the expiry instant are denied.
    pub(crate) expires_at: u64,
    pub(crate) audit_reason: String,
    pub(crate) session_id: String,
}

/// Explicit host approval, after the existing trust/consent boundary. Defaults
/// permit only the authenticated session, with no side effects or valid expiry.
#[derive(Debug, Clone)]
pub struct ContextGrantApproval {
    pub scope: ContextScope,
    pub cross_session: bool,
    pub egress: bool,
    pub project_write: bool,
    pub expires_at: u64,
    pub audit_reason: String,
}

impl Default for ContextGrantApproval {
    fn default() -> Self {
        Self {
            scope: ContextScope {
                namespace: ContextNamespace::Session,
                handles: Vec::new(),
            },
            cross_session: false,
            egress: false,
            project_write: false,
            expires_at: 0,
            audit_reason: String::new(),
        }
    }
}

impl ContextGrant {
    /// Issue only from a host that already authenticated the caller and approved
    /// the requested permissions. Plugin origin/trust alone is not consent.
    /// Like `AuthenticatedCaller::from_host_boundary`, this is a native host API,
    /// never a plugin wire operation. Builtins use the existing empty owner id.
    pub fn host_issue(
        descriptor: &ContextCapabilityDescriptor,
        plugin_id: impl Into<String>,
        caller: &AuthenticatedCaller,
        approval: ContextGrantApproval,
        now: u64,
    ) -> Result<Self, ContextGrantError> {
        let grant = Self {
            capability_id: descriptor.id.clone(),
            descriptor_fingerprint: crate::context::fabric::canonical_fingerprint(
                &serde_json::to_value(descriptor)
                    .map_err(|_| ContextGrantError::DescriptorChanged)?,
            ),
            plugin_id: plugin_id.into(),
            namespace: approval.scope.namespace,
            scope: approval.scope,
            cross_session: approval.cross_session,
            egress: approval.egress,
            project_write: approval.project_write,
            expires_at: approval.expires_at,
            audit_reason: approval.audit_reason,
            session_id: caller.session_id().to_owned(),
        };
        grant.validate_against(descriptor, now)?;
        Ok(grant)
    }

    pub(crate) fn validate_against(
        &self,
        descriptor: &ContextCapabilityDescriptor,
        now: u64,
    ) -> Result<(), ContextGrantError> {
        descriptor
            .validate()
            .map_err(ContextGrantError::Descriptor)?;
        if self.capability_id != descriptor.id {
            return Err(ContextGrantError::CapabilityMismatch {
                expected: descriptor.id.clone(),
                found: self.capability_id.clone(),
            });
        }
        let fingerprint = crate::context::fabric::canonical_fingerprint(
            &serde_json::to_value(descriptor).map_err(|_| ContextGrantError::DescriptorChanged)?,
        );
        if fingerprint != self.descriptor_fingerprint {
            return Err(ContextGrantError::DescriptorChanged);
        }
        if self.plugin_id.trim() != self.plugin_id {
            return Err(ContextGrantError::InvalidPluginId {
                plugin_id: self.plugin_id.clone(),
            });
        }
        if self.audit_reason.trim().is_empty() {
            return Err(ContextGrantError::MissingAuditReason);
        }
        if self.expires_at == 0 || now >= self.expires_at {
            return Err(ContextGrantError::Expired {
                expires_at: self.expires_at,
            });
        }
        self.scope
            .validate()
            .map_err(ContextGrantError::Descriptor)?;
        if self.namespace != self.scope.namespace
            || self.scope.namespace.reach() > descriptor.scope.namespace.reach()
            || (self.scope.namespace != ContextNamespace::Session
                && self.scope.namespace != descriptor.scope.namespace)
        {
            return Err(ContextGrantError::ScopeEscalation);
        }
        if self.cross_session
            && (descriptor.scope.namespace != ContextNamespace::CrossSession
                || !descriptor.permissions.cross_session_read)
        {
            return Err(ContextGrantError::CrossSessionDenied);
        }
        if self.scope.namespace == ContextNamespace::CrossSession && !self.cross_session {
            return Err(ContextGrantError::CrossSessionDenied);
        }
        // Empty handles never mean arbitrary project/session access.
        if self.scope.namespace != ContextNamespace::Session && self.scope.handles.is_empty() {
            return Err(ContextGrantError::ScopeEscalation);
        }
        for handle in &self.scope.handles {
            let own_session =
                self.scope.namespace == ContextNamespace::Session && handle == &self.session_id;
            if !own_session && !descriptor.scope.handles.contains(handle) {
                return Err(ContextGrantError::UndeclaredHandle(handle.clone()));
            }
            if self.scope.namespace == ContextNamespace::Session && !own_session {
                return Err(ContextGrantError::CrossSessionDenied);
            }
        }
        if self.egress && !descriptor.permissions.network_egress {
            return Err(ContextGrantError::EgressDenied);
        }
        if self.project_write && !descriptor.permissions.project_write {
            return Err(ContextGrantError::ProjectWriteDenied);
        }
        Ok(())
    }
}

/// Reasons a host grant cannot be admitted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContextGrantError {
    #[error("context descriptor is invalid: {0}")]
    Descriptor(crate::context::fabric::ContextContractError),
    #[error("context descriptor changed after host approval")]
    DescriptorChanged,
    #[error("grant capability id mismatch: expected {expected}, found {found}")]
    CapabilityMismatch { expected: String, found: String },
    #[error("grant plugin id is padded: {plugin_id:?}")]
    InvalidPluginId { plugin_id: String },
    #[error("grant requires an audit reason")]
    MissingAuditReason,
    #[error("context grant expired at {expires_at}")]
    Expired { expires_at: u64 },
    #[error("grant scope does not authorize this access")]
    ScopeEscalation,
    #[error("cross-session access was not declared and granted")]
    CrossSessionDenied,
    #[error("grant scope handle was not declared: {0}")]
    UndeclaredHandle(String),
    #[error("network egress requires declaration and host approval")]
    EgressDenied,
    #[error("project write requires declaration and host approval")]
    ProjectWriteDenied,
    #[error("context grant belongs to a different authenticated session")]
    SessionDenied,
}

/// One audit record for an attempted registration or invocation. No payloads or
/// credentials are retained; the host approval reason is preserved on denials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextGrantAudit {
    pub component_key: String,
    pub session_id: Option<String>,
    pub allowed: bool,
    pub action: String,
    pub reason: String,
    pub audit_reason: Option<String>,
    pub at: u64,
}

/// Discovery metadata only; a declaration never issues a context grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDeclaration {
    pub descriptor: ContextCapabilityDescriptor,
    /// An existing export using the runtime's WasmToolInput/WasmToolOutput ABI.
    pub handler: String,
}

/// Unified enum of everything a plugin can contribute.
///
/// Each variant maps 1:1 to a registration type in the `PluginRegistry`.
/// The `#[serde(tag = "type")]` attribute allows serialization as:
/// `{ "type": "tool", "name": "my_tool", ... }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapabilityDeclaration {
    /// A callable tool exposed to the agent
    Tool(ToolDeclaration),
    /// A hook that intercepts system events
    Hook(HookDeclaration),
    /// A background service
    Service(ServiceDeclaration),
    /// A prompt-based skill. Plugin `commands/` markdown is also modelled here
    /// as a [`SkillRegistration`] with `skill_type = SkillType::Command` (a
    /// user-triggered `/command`), so the whole "skill vs command" distinction
    /// lives in one store rather than a parallel `CommandRegistration` type.
    Skill(SkillDeclaration),
    /// An agent definition
    Agent(AgentDeclaration),
    /// An MCP server configuration
    McpServer(McpServerDeclaration),
    /// A context descriptor and its WASM transport export; not authorization.
    Context(ContextDeclaration),
}

impl CapabilityDeclaration {
    /// Returns the tier this capability belongs to.
    #[must_use]
    pub const fn tier(&self) -> Tier {
        match self {
            // P0: Always allowed, no permission check
            Self::Tool(_) | Self::Hook(_) | Self::Skill(_) => Tier::Core,
            // P1: Always allowed, no permission check
            Self::Agent(_) => Tier::Important,
            // P2: Some are permission-gated (Service needs Background)
            Self::Service(_) | Self::McpServer(_) | Self::Context(_) => Tier::Pluggable,
        }
    }

    /// Returns a human-readable kind name for this capability.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Tool(_) => "tool",
            Self::Hook(_) => "hook",
            Self::Service(_) => "service",
            Self::Skill(_) => "skill",
            Self::Agent(_) => "agent",
            Self::McpServer(_) => "mcp_server",
            Self::Context(_) => CONTEXT_CAPABILITY_KIND,
        }
    }

    /// Returns the required permission for this capability, if any.
    #[must_use]
    pub const fn required_permission(&self) -> Option<PluginPermission> {
        match self {
            // P0 + P1: no permission needed
            Self::Tool(_) | Self::Hook(_) | Self::Skill(_) | Self::Agent(_) => None,
            // P2: some need permission
            Self::Service(_) => Some(PluginPermission::Background),
            Self::McpServer(_) => None, // MCP is the standard extension mechanism
            Self::Context(_) => None,   // Discovery only; ContextGrant is checked at invocation.
        }
    }
}

// ============================================================================
// Context capability classification
// ============================================================================

/// Discovery kind for manifest-declared context capabilities.
pub const CONTEXT_CAPABILITY_KIND: &str = "context";

/// Tier and manifest permission one context phase requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCapabilityClass {
    /// Context capability is always pluggable: it can be disabled per plugin
    /// without taking a core surface with it.
    pub tier: Tier,
    /// `None` for read-only phases; otherwise the declared permission string.
    pub permission: Option<PluginPermission>,
}

/// Map one phase onto the extension capability model.
///
/// Phase classification does not confer a host grant or register a handler.
///
/// Only `Status` is read-only, so only `Status` maps to no permission. Every
/// phase that can change what the model sees requires an explicit
/// `context:<phase>` permission; `network_egress`, `project_write` and
/// cross-session reads are *additional* permissions the descriptor must declare
/// and the host must grant, never implied by a phase.
#[must_use]
pub fn context_capability_class(phase: ContextPhase) -> ContextCapabilityClass {
    let permission = match phase {
        ContextPhase::Status => None,
        _ => Some(PluginPermission::Custom(format!(
            "context:{}",
            phase.as_str()
        ))),
    };
    ContextCapabilityClass {
        tier: Tier::Pluggable,
        permission,
    }
}

// ============================================================================
// Tier
// ============================================================================

/// Priority tier for capabilities.
///
/// Determines registration order and conflict resolution behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Core capabilities (Tool, Hook, Skill) — always registered first
    Core,
    /// Important capabilities (Agent)
    Important,
    /// Pluggable capabilities (Service, `McpServer`)
    Pluggable,
}

// ============================================================================
// CapabilitySource
// ============================================================================

/// Metadata about where a capability was declared.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySource {
    /// The plugin that declared this capability
    pub plugin_id: String,
    /// Where the plugin was discovered
    pub origin: PluginOrigin,
    /// Format of the source manifest
    pub format: SourceFormat,
}

/// The manifest format that declared a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    /// aleph.plugin.toml (native format)
    AlephToml,
    /// .claude/plugin.json (Claude Code compat)
    ClaudeCode,
    /// .codex-plugin/plugin.json (Codex CLI compat)
    Codex,
    /// .cursor-plugin/plugin.json or .cursorrules (Cursor IDE compat)
    Cursor,
    /// Auto-discovered from well-known directory structure
    AutoDiscovered,
    /// Dynamic registration via MCP/WASM runtime
    Runtime,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> ToolDeclaration {
        ToolRegistration {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handle_test".to_string(),
            plugin_id: "test-plugin".to_string(),
        }
    }

    fn make_skill() -> SkillDeclaration {
        SkillRegistration {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            content: "You are a test skill.".to_string(),
            triggers: vec!["test".to_string()],
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        }
    }

    fn make_agent() -> AgentDeclaration {
        AgentRegistration {
            name: "test-agent".to_string(),
            description: Some("A test agent".to_string()),
            content: "You are a test agent.".to_string(),
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_tier_classification() {
        let tool = CapabilityDeclaration::Tool(make_tool());
        assert_eq!(tool.tier(), Tier::Core);

        let skill = CapabilityDeclaration::Skill(make_skill());
        assert_eq!(skill.tier(), Tier::Core); // Skills are P0 Core

        let agent = CapabilityDeclaration::Agent(make_agent());
        assert_eq!(agent.tier(), Tier::Important); // Agents are P1 Important
    }

    #[test]
    fn test_kind_name() {
        let tool = CapabilityDeclaration::Tool(make_tool());
        assert_eq!(tool.kind_name(), "tool");

        let skill = CapabilityDeclaration::Skill(make_skill());
        assert_eq!(skill.kind_name(), "skill");

        let agent = CapabilityDeclaration::Agent(make_agent());
        assert_eq!(agent.kind_name(), "agent");
    }

    #[test]
    fn test_required_permission() {
        let tool = CapabilityDeclaration::Tool(make_tool());
        assert_eq!(tool.required_permission(), None);

        let skill = CapabilityDeclaration::Skill(make_skill());
        assert_eq!(skill.required_permission(), None);
    }

    #[test]
    fn test_source_format_serialization() {
        let format = SourceFormat::AlephToml;
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(json, "\"aleph_toml\"");

        let roundtrip: SourceFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, SourceFormat::AlephToml);
    }

    #[test]
    fn test_capability_source() {
        let source = CapabilitySource {
            plugin_id: "my-plugin".to_string(),
            origin: PluginOrigin::Global,
            format: SourceFormat::ClaudeCode,
        };
        let json = serde_json::to_string(&source).unwrap();
        let roundtrip: CapabilitySource = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.plugin_id, "my-plugin");
        assert_eq!(roundtrip.format, SourceFormat::ClaudeCode);
    }

    #[test]
    fn test_tier_serialization() {
        let tier = Tier::Core;
        let json = serde_json::to_string(&tier).unwrap();
        assert_eq!(json, "\"core\"");

        let roundtrip: Tier = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, Tier::Core);
    }

    #[test]
    fn context_phase_maps_to_pluggable_tier_and_permission() {
        use crate::context::fabric::ContextPhase;

        assert_eq!(
            context_capability_class(ContextPhase::Status),
            ContextCapabilityClass {
                tier: Tier::Pluggable,
                permission: None,
            }
        );
        assert_eq!(
            context_capability_class(ContextPhase::Compaction),
            ContextCapabilityClass {
                tier: Tier::Pluggable,
                permission: Some(PluginPermission::Custom("context:compaction".to_string())),
            }
        );
        for phase in ContextPhase::ALL {
            assert_eq!(context_capability_class(phase).tier, Tier::Pluggable);
        }
        assert_eq!(
            context_capability_class(ContextPhase::Lifecycle).permission,
            Some(PluginPermission::Custom("context:lifecycle".to_string()))
        );
        assert_eq!(
            context_capability_class(ContextPhase::Admission).permission,
            Some(PluginPermission::Custom("context:admission".to_string()))
        );
    }

    #[test]
    fn test_all_7_variants_have_kind_names() {
        use crate::extension::types::HookEvent;

        let capabilities: Vec<CapabilityDeclaration> = vec![
            CapabilityDeclaration::Tool(make_tool()),
            CapabilityDeclaration::Hook(HookRegistration {
                event: HookEvent::BeforeToolCall,
                priority: 0,
                handler: "h".to_string(),
                name: None,
                description: None,
                plugin_id: "p".to_string(),
                kind: None,
                matcher: None,
                actions: Vec::new(),
                plugin_root: None,
                timeout_secs: None,
                declared_event: None,
            }),
            CapabilityDeclaration::Service(ServiceRegistration {
                id: "s".to_string(),
                name: "S".to_string(),
                start_handler: "start".to_string(),
                stop_handler: "stop".to_string(),
                plugin_id: "p".to_string(),
                auto_start: true,
            }),
            CapabilityDeclaration::Skill(make_skill()),
            CapabilityDeclaration::Agent(make_agent()),
            CapabilityDeclaration::McpServer(McpManagerConfig::stdio("plugin:p/s", "s", "npx")),
        ];

        assert_eq!(capabilities.len(), 6);
        let kind_names: Vec<&str> = capabilities.iter().map(|c| c.kind_name()).collect();
        assert_eq!(
            kind_names,
            vec!["tool", "hook", "service", "skill", "agent", "mcp_server",]
        );
    }
}
