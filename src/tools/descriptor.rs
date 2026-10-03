//! Tool capability descriptor — the single source of truth for a Tool's
//! capability contract.
//!
//! This module owns [`ToolSource`] (re-exported through `tools::service` for
//! existing caller path compatibility) and the [`ToolCapabilityDescriptor`]
//! contract that the capability registry keys on. The model-visible
//! `ToolDefinition` is a *projection* of the descriptor, not a second source
//! of truth — see [`ToolCapabilityDescriptor::from_definition`] and
//! [`ToolCapabilityDescriptor::matches_definition`].
//!
//! See: docs/superpowers/specs/2026-10-03-capability-tool-descriptor-design.md

use serde::{Deserialize, Serialize};

use crate::tools::service::ToolDefinition;

/// Current version of the descriptor contract. Bumped on any breaking change
/// to the descriptor fields or validation semantics.
pub const SCHEMA_VERSION: u32 = 1;

/// The provider-facing name limit for tool names. OpenAI-compatible function
/// names are restricted to 64 characters (`tools::handlers::mcp::sanitize_tool_name`),
/// and Anthropic/Gemini accept the same alphabet. A descriptor name longer
/// than this cannot be projected to every provider, so it is rejected.
pub const MAX_NAME_LEN: usize = 64;

/// The capability kind. Only `Tool` is modelled this iteration; Skill /
/// Plugin / MCP / ACP registries are deliberately out of scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    Tool,
}

impl Default for ToolKind {
    fn default() -> Self {
        Self::Tool
    }
}

/// Whether a call whose result was lost (persisted but unanswered) may be
/// replayed automatically during recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayPolicy {
    /// Do not auto-replay: recovery must surface an unknown-result /
    /// verification prompt through the existing recovery path.
    Unsafe,
    /// Auto-replay allowed only when both the call-time descriptor and the
    /// currently-resolved descriptor explicitly permit `Safe`.
    Safe,
}

impl Default for ReplayPolicy {
    fn default() -> Self {
        Self::Unsafe
    }
}

/// Where a tool capability originates. Kept here (rather than in `service`) as
/// the single definition; `tools::service` re-exports it for existing callers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSource {
    Builtin,
    Mcp { server_id: String },
    Extension { plugin_id: String },
}

/// Why a [`ToolCapabilityDescriptor`] failed [`ToolCapabilityDescriptor::validate`].
#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    #[error("tool capability name is empty")]
    EmptyName,

    #[error("tool capability name {name:?} exceeds {max} characters")]
    NameTooLong { name: String, max: usize },

    #[error("tool capability input schema must be a JSON object")]
    NonObjectInputSchema,

    #[error("tool capability schema_version must be positive, got {0}")]
    InvalidSchemaVersion(u32),

    #[error("tool capability revision must be positive, got {0}")]
    InvalidRevision(u64),
}

/// The frozen capability contract for one Tool.
///
/// This is the single source of truth the capability registry keys on. The
/// model-visible `ToolDefinition` is a projection of it — see
/// [`ToolCapabilityDescriptor::from_definition`] — not a second source of
/// truth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCapabilityDescriptor {
    pub name: String,
    #[serde(default)]
    pub kind: ToolKind,
    pub schema_version: u32,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub source: ToolSource,
    #[serde(default)]
    pub replay_policy: ReplayPolicy,
    #[serde(default)]
    pub requires_confirmation: bool,
    #[serde(default)]
    pub idempotent: bool,
    #[serde(default)]
    pub concurrent_safe: bool,
    #[serde(default)]
    pub max_duration_ms: Option<u64>,
    pub revision: u64,
}

impl ToolCapabilityDescriptor {
    /// Build a descriptor from a loop-side [`ToolDefinition`].
    ///
    /// `replay_policy` is fixed to [`ReplayPolicy::Unsafe`] — never inferred
    /// from `idempotent` or an external (MCP/ACP) source. Replay safety is a
    /// recovery decision that must come from explicit protocol metadata or a
    /// later audit, not from tool identity.
    #[must_use]
    pub fn from_definition(definition: &ToolDefinition, revision: u64) -> Self {
        Self {
            name: definition.name.clone(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: definition.description.clone(),
            input_schema: definition.input_schema.clone(),
            source: definition.source.clone(),
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: definition.metadata.requires_approval,
            idempotent: definition.metadata.idempotent,
            concurrent_safe: definition.metadata.concurrent_safe,
            max_duration_ms: definition.metadata.max_duration_ms,
            revision,
        }
    }

    /// Validate the descriptor's invariants.
    ///
    /// Rejects an empty name, a name longer than the provider name limit, a
    /// non-object input schema, and a non-positive `schema_version` or
    /// `revision`. Does **not** infer replay policy from the tool name.
    pub fn validate(&self) -> Result<(), DescriptorError> {
        if self.name.is_empty() {
            return Err(DescriptorError::EmptyName);
        }
        if self.name.chars().count() > MAX_NAME_LEN {
            return Err(DescriptorError::NameTooLong {
                name: self.name.clone(),
                max: MAX_NAME_LEN,
            });
        }
        if !self.input_schema.is_object() {
            return Err(DescriptorError::NonObjectInputSchema);
        }
        if self.schema_version == 0 {
            return Err(DescriptorError::InvalidSchemaVersion(self.schema_version));
        }
        if self.revision == 0 {
            return Err(DescriptorError::InvalidRevision(self.revision));
        }
        Ok(())
    }

    /// Whether `definition` is the same capability this descriptor was built
    /// from — the pairing check the registry runs before registering or
    /// replacing a handler.
    #[must_use]
    pub fn matches_definition(&self, definition: &ToolDefinition) -> bool {
        self.name == definition.name
            && self.source == definition.source
            && self.description == definition.description
            && self.input_schema == definition.input_schema
            && self.requires_confirmation == definition.metadata.requires_approval
            && self.idempotent == definition.metadata.idempotent
            && self.concurrent_safe == definition.metadata.concurrent_safe
            && self.max_duration_ms == definition.metadata.max_duration_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition(name: &str, source: ToolSource) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc {name}"),
            input_schema: json!({"type": "object", "properties": {}}),
            source,
            metadata: Default::default(),
        }
    }

    fn descriptor_with(name: &str, schema: serde_json::Value) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.to_string(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: "test".to_string(),
            input_schema: schema,
            source: ToolSource::Builtin,
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: false,
            idempotent: false,
            concurrent_safe: false,
            max_duration_ms: None,
            revision: 1,
        }
    }

    fn descriptor_with_source(source: ToolSource) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            source,
            ..descriptor_with("test_tool", json!({"type": "object"}))
        }
    }

    #[test]
    fn default_replay_policy_is_unsafe() {
        assert_eq!(ReplayPolicy::default(), ReplayPolicy::Unsafe);
    }

    #[test]
    fn descriptor_rejects_empty_name_and_non_object_schema() {
        let descriptor = descriptor_with("", serde_json::json!([]));
        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn descriptor_round_trip_preserves_source_and_safety_fields() {
        let descriptor = descriptor_with_source(ToolSource::Mcp {
            server_id: "srv".into(),
        });
        let encoded = serde_json::to_string(&descriptor).unwrap();
        let decoded: ToolCapabilityDescriptor = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn builtin_source_round_trips() {
        let descriptor = descriptor_with_source(ToolSource::Builtin);
        let encoded = serde_json::to_string(&descriptor).unwrap();
        let decoded: ToolCapabilityDescriptor = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.source, ToolSource::Builtin);
        assert_eq!(decoded, descriptor);
    }

    #[test]
    fn mcp_source_round_trips_with_server_id() {
        let descriptor = descriptor_with_source(ToolSource::Mcp {
            server_id: "github".into(),
        });
        let encoded = serde_json::to_string(&descriptor).unwrap();
        let decoded: ToolCapabilityDescriptor = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.source,
            ToolSource::Mcp {
                server_id: "github".into()
            }
        );
    }

    #[test]
    fn from_definition_copies_fields_and_fixes_replay_unsafe() {
        let mut def = definition("memory_search", ToolSource::Builtin);
        def.metadata.idempotent = true;
        def.metadata.concurrent_safe = true;
        def.metadata.max_duration_ms = Some(5_000);
        def.metadata.requires_approval = true;

        let d = ToolCapabilityDescriptor::from_definition(&def, 7);
        assert_eq!(d.name, "memory_search");
        assert_eq!(d.kind, ToolKind::Tool);
        assert_eq!(d.schema_version, SCHEMA_VERSION);
        assert_eq!(d.description, def.description);
        assert_eq!(d.input_schema, def.input_schema);
        assert_eq!(d.source, def.source);
        assert_eq!(d.replay_policy, ReplayPolicy::Unsafe);
        assert!(d.requires_confirmation);
        assert!(d.idempotent);
        assert!(d.concurrent_safe);
        assert_eq!(d.max_duration_ms, Some(5_000));
        assert_eq!(d.revision, 7);
    }

    #[test]
    fn from_definition_never_infers_safe_replay_from_idempotent() {
        let mut def = definition("read_only", ToolSource::Builtin);
        def.metadata.idempotent = true;
        let d = ToolCapabilityDescriptor::from_definition(&def, 1);
        assert_eq!(d.replay_policy, ReplayPolicy::Unsafe);
    }

    #[test]
    fn from_definition_never_infers_safe_replay_from_mcp_source() {
        let def = definition(
            "remote",
            ToolSource::Mcp {
                server_id: "srv".into(),
            },
        );
        let d = ToolCapabilityDescriptor::from_definition(&def, 1);
        assert_eq!(d.replay_policy, ReplayPolicy::Unsafe);
    }

    #[test]
    fn matches_definition_accepts_unchanged_definition() {
        let def = definition("memory_search", ToolSource::Builtin);
        let d = ToolCapabilityDescriptor::from_definition(&def, 3);
        assert!(d.matches_definition(&def));
    }

    #[test]
    fn matches_definition_rejects_name_drift() {
        let def = definition("memory_search", ToolSource::Builtin);
        let d = ToolCapabilityDescriptor::from_definition(&def, 3);
        let mut other = def.clone();
        other.name = "web_search".to_string();
        assert!(!d.matches_definition(&other));
    }

    #[test]
    fn matches_definition_rejects_source_drift() {
        let def = definition("tool", ToolSource::Builtin);
        let d = ToolCapabilityDescriptor::from_definition(&def, 3);
        let other = definition(
            "tool",
            ToolSource::Mcp {
                server_id: "srv".into(),
            },
        );
        assert!(!d.matches_definition(&other));
    }

    #[test]
    fn validate_rejects_non_object_input_schema() {
        let d = descriptor_with("ok", json!([]));
        assert!(matches!(
            d.validate(),
            Err(DescriptorError::NonObjectInputSchema)
        ));
    }

    #[test]
    fn validate_rejects_zero_schema_version_and_revision() {
        let mut d = descriptor_with("ok", json!({"type": "object"}));
        d.schema_version = 0;
        assert!(matches!(
            d.validate(),
            Err(DescriptorError::InvalidSchemaVersion(0))
        ));
        d.schema_version = SCHEMA_VERSION;
        d.revision = 0;
        assert!(matches!(
            d.validate(),
            Err(DescriptorError::InvalidRevision(0))
        ));
    }

    #[test]
    fn validate_rejects_name_longer_than_provider_limit() {
        let long = "a".repeat(MAX_NAME_LEN + 1);
        let d = descriptor_with(&long, json!({"type": "object"}));
        assert!(matches!(d.validate(), Err(DescriptorError::NameTooLong { .. })));
    }

    #[test]
    fn validate_accepts_well_formed_descriptor() {
        let d = descriptor_with("ok", json!({"type": "object"}));
        assert!(d.validate().is_ok());
    }
}
