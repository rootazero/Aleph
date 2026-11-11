//! Immutable ambient authority snapshots and their callable handlers.

use crate::capability::backend::{
    LegacyCapabilityDispatchError, LegacyCapabilityHandler, LegacyCapabilityHandlerRegistry,
    LegacyCapabilityRegistryError,
};
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::sync_primitives::Arc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::env;

pub const AMBIENT_NAMESPACE: &str = "aleph/ambient";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AmbientError {
    #[error("ambient identity must not be empty")]
    InvalidIdentity,
    #[error("environment variable name is invalid")]
    InvalidEnvironmentVariableName,
    #[error("environment variable is missing: {0}")]
    MissingEnvironmentVariable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AmbientRegistrationError {
    #[error(transparent)]
    Ambient(#[from] AmbientError),
    #[error(transparent)]
    Registry(#[from] LegacyCapabilityRegistryError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmbientSnapshot {
    ProcessId(u32),
    UserId(String),
    EnvironmentVariable { name: String, value: String },
}

impl AmbientSnapshot {
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::ProcessId(id) => json!({"kind": "process_id", "value": id}),
            Self::UserId(id) => json!({"kind": "user_id", "value": id}),
            Self::EnvironmentVariable { name, value } => {
                json!({"kind": "environment_variable", "name": name, "value": value})
            }
        }
    }
}

/// A read-only ambient authority source.
pub trait AmbientCapability: Send + Sync {
    fn snapshot(&self) -> Result<AmbientSnapshot, AmbientError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(u32);

impl ProcessId {
    #[must_use]
    pub fn current() -> Self {
        Self(std::process::id())
    }

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }

    pub fn snapshot(&self) -> Result<AmbientSnapshot, AmbientError> {
        Ok(AmbientSnapshot::ProcessId(self.0))
    }
}

impl AmbientCapability for ProcessId {
    fn snapshot(&self) -> Result<AmbientSnapshot, AmbientError> {
        Self::snapshot(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserId(String);

impl UserId {
    pub fn new(value: impl Into<String>) -> Result<Self, AmbientError> {
        let value = value.into();
        (!value.trim().is_empty())
            .then_some(Self(value))
            .ok_or(AmbientError::InvalidIdentity)
    }

    pub fn current() -> Result<Self, AmbientError> {
        let value = env::var("USER")
            .or_else(|_| env::var("USERNAME"))
            .map_err(|_| AmbientError::MissingEnvironmentVariable("USER".into()))?;
        Self::new(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn snapshot(&self) -> Result<AmbientSnapshot, AmbientError> {
        Ok(AmbientSnapshot::UserId(self.0.clone()))
    }
}

impl AmbientCapability for UserId {
    fn snapshot(&self) -> Result<AmbientSnapshot, AmbientError> {
        Self::snapshot(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentVariable {
    name: String,
    value_override: Option<String>,
}

impl EnvironmentVariable {
    pub fn new(name: impl Into<String>) -> Result<Self, AmbientError> {
        let name = name.into();
        if name.is_empty() || name.contains('=') || name.contains('\0') {
            return Err(AmbientError::InvalidEnvironmentVariableName);
        }
        Ok(Self {
            name,
            value_override: None,
        })
    }

    /// Construct a deterministic source for tests and in-memory bridges.
    pub fn from_value(
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, AmbientError> {
        Ok(Self {
            value_override: Some(value.into()),
            ..Self::new(name)?
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn snapshot(&self) -> Result<AmbientSnapshot, AmbientError> {
        let value = self
            .value_override
            .clone()
            .or_else(|| env::var(&self.name).ok())
            .ok_or_else(|| AmbientError::MissingEnvironmentVariable(self.name.clone()))?;
        Ok(AmbientSnapshot::EnvironmentVariable {
            name: self.name.clone(),
            value,
        })
    }
}

impl AmbientCapability for EnvironmentVariable {
    fn snapshot(&self) -> Result<AmbientSnapshot, AmbientError> {
        Self::snapshot(self)
    }
}

pub struct AmbientHandler {
    capability: Arc<dyn AmbientCapability>,
}

impl AmbientHandler {
    #[must_use]
    pub fn new(capability: Arc<dyn AmbientCapability>) -> Self {
        Self { capability }
    }
}

impl LegacyCapabilityHandler for AmbientHandler {
    fn invoke(&self, input: Value) -> Result<Value, LegacyCapabilityDispatchError> {
        if !input.is_object() {
            return Err(LegacyCapabilityDispatchError::InvalidInput(
                "ambient handlers accept an object input".into(),
            ));
        }
        self.capability
            .snapshot()
            .map(|snapshot| snapshot.to_json())
            .map_err(|error| LegacyCapabilityDispatchError::Execution(error.to_string()))
    }
}

fn descriptor(name: &str) -> CapabilityDescriptor {
    let schema = json!({"type":"object","additionalProperties":false});
    let mut bytes = Vec::new();
    crate::tools::descriptor::canonicalize_json(&schema, &mut bytes);
    CapabilityDescriptor {
        id: CapabilityId {
            namespace: AMBIENT_NAMESPACE.into(),
            name: name.into(),
        },
        kind: CapabilityKind::Ambient,
        schema: SchemaRef {
            version: 1,
            fingerprint: Sha256::digest(bytes).into(),
        },
        revision: CapabilityRevision(0),
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        owner: OwnerRef::Runtime,
        services: Vec::new(),
        metadata: ProjectionMetadata::default(),
        facets: CapabilityFacets::new().with(CapabilityFacet::Callable),
    }
}

pub fn register_process_handler(
    registry: &LegacyCapabilityHandlerRegistry,
) -> Result<CapabilityId, LegacyCapabilityRegistryError> {
    registry.register(
        descriptor("process.id"),
        Arc::new(AmbientHandler::new(Arc::new(ProcessId::current()))),
    )
}

pub fn register_user_handler(
    registry: &LegacyCapabilityHandlerRegistry,
) -> Result<CapabilityId, AmbientRegistrationError> {
    let capability = UserId::current()?;
    Ok(registry.register(
        descriptor("user.id"),
        Arc::new(AmbientHandler::new(Arc::new(capability))),
    )?)
}

pub fn register_environment_handler(
    registry: &LegacyCapabilityHandlerRegistry,
    name: impl Into<String>,
) -> Result<CapabilityId, AmbientRegistrationError> {
    let capability = EnvironmentVariable::new(name)?;
    Ok(registry.register(
        descriptor(&format!("environment.{}", capability.name())),
        Arc::new(AmbientHandler::new(Arc::new(capability))),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::backend::{CapabilityBackend, LegacyCapabilityHandlerRegistry};
    use serde_json::json;

    #[test]
    fn process_id_snapshot_is_current_and_immutable() {
        let capability = ProcessId::current();
        let first = capability.snapshot().expect("process id is available");
        let second = capability.snapshot().expect("process id is available");

        assert!(matches!(&first, AmbientSnapshot::ProcessId(id) if *id > 0));
        assert_eq!(first, second);
    }

    #[test]
    fn user_id_snapshot_preserves_the_supplied_identity() {
        let capability = UserId::new("alice").expect("valid user id");

        assert_eq!(
            capability.snapshot().expect("user id is available"),
            AmbientSnapshot::UserId("alice".into())
        );
    }

    #[test]
    fn user_id_rejects_empty_identity() {
        assert!(matches!(
            UserId::new(""),
            Err(AmbientError::InvalidIdentity)
        ));
    }

    #[test]
    fn environment_snapshot_fails_closed_when_variable_is_missing() {
        let capability = EnvironmentVariable::new("ALEPH_TASK2_MISSING_ENV").unwrap();

        assert!(matches!(
            capability.snapshot(),
            Err(AmbientError::MissingEnvironmentVariable(name))
                if name == "ALEPH_TASK2_MISSING_ENV"
        ));
    }

    #[test]
    fn environment_snapshot_reads_a_present_variable() {
        let capability =
            EnvironmentVariable::from_value("ALEPH_TASK2_PRESENT_ENV", "value").unwrap();

        assert_eq!(
            capability.snapshot().unwrap(),
            AmbientSnapshot::EnvironmentVariable {
                name: "ALEPH_TASK2_PRESENT_ENV".into(),
                value: "value".into(),
            }
        );
    }

    #[test]
    fn ambient_process_handler_is_discoverable_and_dispatchable() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        let id = register_process_handler(&registry).expect("register process capability");

        let descriptor = registry.lookup(&id).expect("descriptor is discoverable");
        assert_eq!(descriptor.kind, CapabilityKind::Ambient);
        assert_eq!(registry.enumerate(&Default::default()).len(), 1);
        assert!(registry.dispatch(&id, json!({})).is_ok());
    }

    #[test]
    fn ambient_registry_fails_closed_for_an_unknown_id() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        let missing = CapabilityId {
            namespace: AMBIENT_NAMESPACE.into(),
            name: "missing".into(),
        };

        assert!(matches!(
            registry.dispatch(&missing, json!({})),
            Err(LegacyCapabilityDispatchError::NotFound(id)) if id == missing
        ));
    }

    #[test]
    fn ambient_handler_rejects_non_object_input() {
        let registry = LegacyCapabilityHandlerRegistry::new();
        let id = register_process_handler(&registry).expect("register process capability");

        assert!(matches!(
            registry.dispatch(&id, json!(null)),
            Err(LegacyCapabilityDispatchError::InvalidInput(_))
        ));
    }
}
