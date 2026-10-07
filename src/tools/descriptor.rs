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

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    #[default]
    Tool,
}

/// Whether a call whose result was lost (persisted but unanswered) may be
/// replayed automatically during recovery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayPolicy {
    /// Do not auto-replay: recovery must surface an unknown-result /
    /// verification prompt through the existing recovery path.
    #[default]
    Unsafe,
    /// Auto-replay allowed only when both the call-time descriptor and the
    /// currently-resolved descriptor explicitly permit `Safe`.
    Safe,
}

/// Length of a [`ReplayContractFingerprint`] digest (SHA-256 output).
pub const REPLAY_CONTRACT_DIGEST_LEN: usize = 32;

/// Version of the canonical replay-contract encoding. Bump whenever the
/// preimage shape changes; an old-version fingerprint then no longer matches a
/// newly computed one.
pub const REPLAY_CONTRACT_FINGERPRINT_VERSION: u8 = 1;

/// The audited implementation contract supplied by a Safe tool's registration
/// owner.
///
/// This is NOT a hash of handler machine code and it is not derived from the
/// descriptor: the owner attests that `id`/`version` names a specific
/// externally observable implementation whose replay contract they have
/// reviewed. It must be bumped whenever that behavior changes. The fingerprint
/// commits to it, so a descriptor-only hash cannot accidentally reuse an old
/// token after a handler change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplementationContract {
    pub id: String,
    pub version: String,
}

impl ImplementationContract {
    /// A contract is usable only when both id and version are non-empty.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.id.trim().is_empty() && !self.version.trim().is_empty()
    }
}

/// A versioned, fixed-size SHA-256 fingerprint of a Safe tool's replay
/// contract.
///
/// Serialized on the wire as a single lowercase string `v{version}:{hex}`.
/// The version prefixes both the preimage and the stored digest, so changing
/// the algorithm/encoding changes the wire form and the recomputed digest in
/// the same commit. A malformed or unknown-versioned value fails to
/// deserialize, which the event-layer decoder maps to a missing identity
/// (fail-closed) rather than a valid fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReplayContractFingerprint {
    version: u8,
    digest: [u8; REPLAY_CONTRACT_DIGEST_LEN],
}

impl ReplayContractFingerprint {
    #[must_use]
    pub const fn version(self) -> u8 {
        self.version
    }

    #[must_use]
    pub const fn digest(self) -> [u8; REPLAY_CONTRACT_DIGEST_LEN] {
        self.digest
    }

    /// The canonical wire spelling `v{version}:{64 lowercase hex}`.
    #[must_use]
    pub fn to_wire_string(self) -> String {
        format!("v{}:{}", self.version, hex::encode(self.digest))
    }

    /// Strict parser for the canonical wire form. Anything else is an error so
    /// a malformed or unknown token can never be read back as a valid
    /// fingerprint.
    pub fn parse(s: &str) -> Result<Self, String> {
        let Some(rest) = s.strip_prefix('v') else {
            return Err("fingerprint must start with `v`".to_string());
        };
        let Some((version, hex_digest)) = rest.split_once(':') else {
            return Err("fingerprint must be `v{n}:{hex}`".to_string());
        };
        let version: u8 = version
            .parse()
            .map_err(|_| "invalid fingerprint version".to_string())?;
        if version != REPLAY_CONTRACT_FINGERPRINT_VERSION {
            return Err(format!("unsupported fingerprint version {version}"));
        }
        if hex_digest.len() != REPLAY_CONTRACT_DIGEST_LEN * 2 {
            return Err(format!(
                "fingerprint digest must be {} hex chars",
                REPLAY_CONTRACT_DIGEST_LEN * 2
            ));
        }
        if !hex_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("fingerprint digest must use lowercase hex".to_string());
        }
        let bytes =
            hex::decode(hex_digest).map_err(|_| "fingerprint digest is not hex".to_string())?;
        let mut digest = [0u8; REPLAY_CONTRACT_DIGEST_LEN];
        digest.copy_from_slice(&bytes);
        Ok(Self { version, digest })
    }
}

impl Serialize for ReplayContractFingerprint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_wire_string())
    }
}

impl<'de> Deserialize<'de> for ReplayContractFingerprint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FingerprintVisitor;
        impl<'de> serde::de::Visitor<'de> for FingerprintVisitor {
            type Value = ReplayContractFingerprint;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a replay contract fingerprint string `v{n}:{64 hex}`")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                ReplayContractFingerprint::parse(value).map_err(E::custom)
            }
        }
        deserializer.deserialize_str(FingerprintVisitor)
    }
}

/// The durable, call-time identity of a Tool capability.
///
/// Captured when a `ToolCallRequested` event is emitted and persisted with it,
/// so recovery can prove a crash-interrupted call targeted the same callable
/// contract it now resolves — without persisting handler code, plugin paths, or
/// mutable runtime state. These three fields are the minimum needed to
/// re-evaluate replay eligibility; the event's existing `name` and `call_id`
/// remain the call's outer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallIdentity {
    pub schema_version: u32,
    pub revision: u64,
    pub replay_policy: ReplayPolicy,
    /// Present for a newly emitted Safe identity; `None` for legacy or Unsafe
    /// identities, which are never 2B-replayable. Omitted from the wire when
    /// `None` so legacy rows stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_contract_fingerprint: Option<ReplayContractFingerprint>,
}

impl ToolCallIdentity {
    /// Copy the durable identity fields from one descriptor.
    ///
    /// The descriptor's explicit `replay_policy` is carried verbatim; `Safe` is
    /// never inferred from `idempotent`, `concurrent_safe`, source, or name.
    /// The fingerprint is computed from the descriptor and is `Some` only when
    /// the descriptor is `Safe` and carries a valid audited implementation
    /// contract.
    #[must_use]
    pub fn from_descriptor(descriptor: &ToolCapabilityDescriptor) -> Self {
        Self {
            schema_version: descriptor.schema_version,
            revision: descriptor.revision,
            replay_policy: descriptor.replay_policy,
            replay_contract_fingerprint: descriptor.replay_contract_fingerprint(),
        }
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

pub trait ReplayPolicyLookup: Send + Sync {
    fn replay_policy(&self, name: &str) -> Option<ReplayPolicy>;
}

/// Read-only lookup of a Tool's durable call-time identity.
///
/// Implementations read the *current* descriptor for `name` and copy only
/// [`ToolCallIdentity`]'s durable fields. This is a same-generation snapshot
/// read, not a replay authorization API — the recovery predicate lives in
/// `session::boundary_repair`.
pub trait ToolDescriptorLookup: Send + Sync {
    fn tool_call_identity(&self, name: &str) -> Option<ToolCallIdentity>;
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

    #[error(
        "tool capability {name:?} declares Safe replay but has no valid audited \
         implementation contract (id and version are required)"
    )]
    SafeReplayRequiresImplementationContract { name: String },
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
    /// The audited implementation contract for a Safe tool. Required (with a
    /// non-empty id and version) whenever `replay_policy` is `Safe`; `None` is
    /// correct for `Unsafe` descriptors. Owner-supplied audit metadata, not
    /// derived from the handler definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation_contract: Option<ImplementationContract>,
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
            implementation_contract: None,
        }
    }

    /// Project the callable contract to the provider-facing metadata shape.
    /// UI category is intentionally a consumer classification; the full source
    /// identity remains on the descriptor.
    #[must_use]
    pub fn to_metadata_definition(&self) -> crate::tool_metadata::ToolDefinition {
        let category = match &self.source {
            ToolSource::Builtin => crate::tool_metadata::ToolCategory::Builtin,
            ToolSource::Mcp { .. } => crate::tool_metadata::ToolCategory::Mcp,
            ToolSource::Extension { .. } => crate::tool_metadata::ToolCategory::Custom,
        };
        crate::tool_metadata::ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.input_schema.clone(),
            requires_confirmation: self.requires_confirmation,
            category,
            strict: false,
        }
    }

    /// Project descriptor-owned fields to the command/UI catalog. The catalog
    /// still owns routing, visibility, conflict resolution, and presentation
    /// metadata.
    #[must_use]
    pub fn to_unified_tool(&self, id: String) -> crate::tool_metadata::UnifiedTool {
        let source = match &self.source {
            ToolSource::Builtin => crate::tool_metadata::ToolSource::Builtin,
            ToolSource::Mcp { server_id } => crate::tool_metadata::ToolSource::Mcp {
                server: server_id.clone(),
            },
            ToolSource::Extension { plugin_id } => crate::tool_metadata::ToolSource::Plugin {
                plugin_id: plugin_id.clone(),
            },
        };
        crate::tool_metadata::UnifiedTool::new(
            id,
            self.name.clone(),
            self.description.clone(),
            source,
        )
        .with_parameters_schema(self.input_schema.clone())
        .populate_safety_profile(self.idempotent, self.requires_confirmation)
    }

    /// Compute the stable replay contract fingerprint for this descriptor.
    ///
    /// An explicit Safe descriptor without a valid audited implementation
    /// contract has no fingerprint and is rejected by [`Self::validate`].
    #[must_use]
    pub fn replay_contract_fingerprint(&self) -> Option<ReplayContractFingerprint> {
        if self.replay_policy != ReplayPolicy::Safe {
            return None;
        }
        let contract = self.implementation_contract.as_ref()?;
        if !contract.is_valid() {
            return None;
        }
        Some(compute_replay_contract_fingerprint(self, contract))
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
        if self.replay_policy == ReplayPolicy::Safe
            && self
                .implementation_contract
                .as_ref()
                .is_none_or(|contract| !contract.is_valid())
        {
            return Err(DescriptorError::SafeReplayRequiresImplementationContract {
                name: self.name.clone(),
            });
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

fn compute_replay_contract_fingerprint(
    descriptor: &ToolCapabilityDescriptor,
    contract: &ImplementationContract,
) -> ReplayContractFingerprint {
    let preimage = canonical_replay_contract_preimage(descriptor, contract);
    let digest = Sha256::digest(preimage);
    let mut bytes = [0u8; REPLAY_CONTRACT_DIGEST_LEN];
    bytes.copy_from_slice(&digest);
    ReplayContractFingerprint {
        version: REPLAY_CONTRACT_FINGERPRINT_VERSION,
        digest: bytes,
    }
}

fn canonical_replay_contract_preimage(
    descriptor: &ToolCapabilityDescriptor,
    contract: &ImplementationContract,
) -> Vec<u8> {
    let mut out = Vec::new();
    push_len_prefixed(&mut out, b"aleph-replay-contract");
    out.push(REPLAY_CONTRACT_FINGERPRINT_VERSION);
    push_len_prefixed(&mut out, descriptor.name.as_bytes());
    push_len_prefixed(
        &mut out,
        match descriptor.kind {
            ToolKind::Tool => b"tool".as_slice(),
        },
    );
    out.extend_from_slice(&descriptor.schema_version.to_be_bytes());
    push_len_prefixed(&mut out, descriptor.description.as_bytes());
    let mut schema = Vec::new();
    canonicalize_json(&descriptor.input_schema, &mut schema);
    push_len_prefixed(&mut out, &schema);
    match &descriptor.source {
        ToolSource::Builtin => push_len_prefixed(&mut out, b"builtin"),
        ToolSource::Mcp { server_id } => {
            push_len_prefixed(&mut out, b"mcp");
            push_len_prefixed(&mut out, server_id.as_bytes());
        }
        ToolSource::Extension { plugin_id } => {
            push_len_prefixed(&mut out, b"extension");
            push_len_prefixed(&mut out, plugin_id.as_bytes());
        }
    }
    push_len_prefixed(
        &mut out,
        match descriptor.replay_policy {
            ReplayPolicy::Unsafe => b"unsafe".as_slice(),
            ReplayPolicy::Safe => b"safe".as_slice(),
        },
    );
    out.push(u8::from(descriptor.requires_confirmation));
    out.push(u8::from(descriptor.idempotent));
    out.push(u8::from(descriptor.concurrent_safe));
    match descriptor.max_duration_ms {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_be_bytes());
        }
        None => out.push(0),
    }
    push_len_prefixed(&mut out, contract.id.as_bytes());
    push_len_prefixed(&mut out, contract.version.as_bytes());
    out
}

fn push_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn canonicalize_json(value: &serde_json::Value, out: &mut Vec<u8>) {
    match value {
        serde_json::Value::Null => out.extend_from_slice(b"null"),
        serde_json::Value::Bool(value) => {
            out.extend_from_slice(if *value { b"true" } else { b"false" })
        }
        serde_json::Value::Number(value) => out.extend_from_slice(value.to_string().as_bytes()),
        serde_json::Value::String(value) => {
            out.extend_from_slice(&serde_json::to_vec(value).expect("JSON strings serialize"))
        }
        serde_json::Value::Array(values) => {
            out.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                canonicalize_json(value, out);
            }
            out.push(b']');
        }
        serde_json::Value::Object(values) => {
            out.push(b'{');
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(&serde_json::to_vec(key).expect("JSON keys serialize"));
                out.push(b':');
                canonicalize_json(&values[key], out);
            }
            out.push(b'}');
        }
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
            implementation_contract: None,
        }
    }

    fn descriptor_with_source(source: ToolSource) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            source,
            ..descriptor_with("test_tool", json!({"type": "object"}))
        }
    }

    #[test]
    fn descriptor_projects_source_schema_and_safety_to_metadata_and_catalog() {
        let mut descriptor = descriptor_with_source(ToolSource::Mcp {
            server_id: "search".into(),
        });
        descriptor.requires_confirmation = true;
        descriptor.idempotent = true;
        descriptor.concurrent_safe = true;
        descriptor.max_duration_ms = Some(1_234);

        let metadata = descriptor.to_metadata_definition();
        assert_eq!(metadata.category, crate::tool_metadata::ToolCategory::Mcp);
        assert!(metadata.requires_confirmation);
        assert_eq!(metadata.parameters, descriptor.input_schema);

        let catalog = descriptor.to_unified_tool("mcp:search:test_tool".into());
        assert_eq!(catalog.id, "mcp:search:test_tool");
        assert_eq!(catalog.name, "test_tool");
        assert_eq!(catalog.description, descriptor.description);
        assert_eq!(catalog.parameters_schema, Some(descriptor.input_schema));
        assert!(catalog.requires_confirmation);
        assert!(matches!(
            catalog.source,
            crate::tool_metadata::ToolSource::Mcp { ref server } if server == "search"
        ));
    }

    #[test]
    fn safe_replay_requires_an_audited_implementation_contract() {
        let mut descriptor = descriptor_with("safe", json!({"type": "object"}));
        descriptor.replay_policy = ReplayPolicy::Safe;
        assert!(matches!(
            descriptor.validate(),
            Err(DescriptorError::SafeReplayRequiresImplementationContract { .. })
        ));
        assert_eq!(descriptor.replay_contract_fingerprint(), None);
    }

    #[test]
    fn safe_replay_fingerprint_is_stable_and_canonicalizes_object_order() {
        let mut left = descriptor_with(
            "safe",
            serde_json::from_str(r#"{"b":2,"a":{"y":true,"x":1}}"#).unwrap(),
        );
        left.replay_policy = ReplayPolicy::Safe;
        left.implementation_contract = Some(ImplementationContract {
            id: "builtin:safe".into(),
            version: "7".into(),
        });
        let mut right = descriptor_with(
            "safe",
            serde_json::from_str(r#"{"a":{"x":1,"y":true},"b":2}"#).unwrap(),
        );
        right.replay_policy = ReplayPolicy::Safe;
        right.implementation_contract = left.implementation_contract.clone();
        assert_eq!(
            left.replay_contract_fingerprint(),
            right.replay_contract_fingerprint()
        );
        assert!(left.validate().is_ok());
        let fingerprint = left.replay_contract_fingerprint().unwrap();
        let encoded = serde_json::to_string(&fingerprint).unwrap();
        assert_eq!(
            ReplayContractFingerprint::parse(&serde_json::from_str::<String>(&encoded).unwrap())
                .unwrap(),
            fingerprint
        );
    }

    #[test]
    fn fingerprint_parser_rejects_unknown_version_and_noncanonical_hex() {
        let digest = "0a".repeat(REPLAY_CONTRACT_DIGEST_LEN * 2 / 2);
        assert!(ReplayContractFingerprint::parse(&format!("v2:{digest}")).is_err());
        assert!(
            ReplayContractFingerprint::parse(&format!("v1:{}", digest.to_uppercase())).is_err()
        );
    }

    #[test]
    fn default_replay_policy_is_unsafe() {
        assert_eq!(ReplayPolicy::default(), ReplayPolicy::Unsafe);
    }
    #[test]
    fn tool_call_identity_copies_descriptor_fields_verbatim() {
        let mut descriptor = descriptor_with("t", json!({"type": "object"}));
        descriptor.revision = 9;
        descriptor.replay_policy = ReplayPolicy::Safe;
        descriptor.implementation_contract = Some(ImplementationContract {
            id: "builtin:test".into(),
            version: "1".into(),
        });

        let identity = ToolCallIdentity::from_descriptor(&descriptor);
        assert_eq!(
            identity,
            ToolCallIdentity {
                schema_version: SCHEMA_VERSION,
                revision: 9,
                replay_policy: ReplayPolicy::Safe,
                replay_contract_fingerprint: descriptor.replay_contract_fingerprint(),
            }
        );
        assert_eq!(identity.replay_policy, descriptor.replay_policy);
    }

    #[test]
    fn tool_call_identity_defaults_to_unsafe() {
        let descriptor = descriptor_with("t", json!({"type": "object"}));
        assert_eq!(
            ToolCallIdentity::from_descriptor(&descriptor).replay_policy,
            ReplayPolicy::Unsafe
        );
    }

    #[test]
    fn tool_call_identity_round_trips() {
        let identity = ToolCallIdentity {
            schema_version: SCHEMA_VERSION,
            revision: 42,
            replay_policy: ReplayPolicy::Unsafe,
            replay_contract_fingerprint: None,
        };
        let encoded = serde_json::to_string(&identity).unwrap();
        let decoded: ToolCallIdentity = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, identity);
    }

    /// Fail-closed at the enum layer: an unknown/future policy name must not
    /// deserialize into *any* variant, so it can never be read as `Safe`. (The
    /// event-level decoder that maps this error to `identity: None` is a later
    /// session task.)
    #[test]
    fn unknown_replay_policy_never_decodes_to_safe() {
        let decoded: Result<ReplayPolicy, _> = serde_json::from_str("\"FuturePolicy\"");
        assert!(decoded.is_err());
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
        assert!(matches!(
            d.validate(),
            Err(DescriptorError::NameTooLong { .. })
        ));
    }

    #[test]
    fn validate_accepts_well_formed_descriptor() {
        let d = descriptor_with("ok", json!({"type": "object"}));
        assert!(d.validate().is_ok());
    }
}
