//! Source-private cold channel declarations; no executable provider admission.
#![allow(dead_code)] // Published contracts are consumed by later source lifecycle tasks.

use super::{ChannelHandle, ChannelRegistrationEntry};
use crate::acp::manager::SessionKey;
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityKind, ProjectionMetadata, SchemaRef,
    ServiceRelation,
};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use crate::capability::provider::{ProviderCallError, ProviderHandle, ProviderLifecycleError};
use crate::sync_primitives::Arc;
use crate::tools::descriptor::canonicalize_json;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) struct ChannelLifecycleDeclarationV1;

impl ChannelLifecycleDeclarationV1 {
    pub(crate) fn schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "aleph:gateway:channel-lifecycle-declaration:v1",
            "type": "object",
            "additionalProperties": false,
            "required": ["declaration_kind", "channel_id", "provider_transport", "lifecycle_model_version"],
            "properties": {
                "declaration_kind": {"const": "channel-provider-lifecycle"},
                "channel_id": {"type": "string", "minLength": 1},
                "provider_transport": {"const": "in_process"},
                "lifecycle_model_version": {"const": 1}
            }
        })
    }
}

pub(crate) struct ChannelLifecycleContract {
    pub(super) owner: OwnerRef,
    pub(super) lifetime: LifetimeScope,
    pub(super) visibility: VisibilityScope,
    pub(super) lifecycle_schema: Value,
    pub(super) schema: SchemaRef,
    pub(super) metadata: ProjectionMetadata,
}

impl ChannelLifecycleContract {
    pub(crate) fn new(
        owner: OwnerRef,
        lifetime: LifetimeScope,
        visibility: VisibilityScope,
        lifecycle_schema: Value,
        metadata: ProjectionMetadata,
    ) -> Result<Self, ChannelBindingError> {
        let mut canonical = Vec::new();
        canonicalize_json(&lifecycle_schema, &mut canonical);
        let mut expected = Vec::new();
        canonicalize_json(&ChannelLifecycleDeclarationV1::schema(), &mut expected);
        if canonical != expected {
            return Err(ChannelBindingError::InvalidContract);
        }
        Ok(Self {
            owner,
            lifetime,
            visibility,
            schema: SchemaRef {
                version: 1,
                fingerprint: canonical_schema_fingerprint(&lifecycle_schema),
            },
            lifecycle_schema,
            metadata,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ChannelInitialState {
    Discovered,
    Cold,
    Disabled,
}

#[derive(Clone)]
pub(crate) struct BoundChannelCapture {
    pub(super) source: Arc<()>,
    pub(super) entry: Arc<ChannelRegistrationEntry>,
    pub(super) channel: ChannelHandle,
    pub(super) source_revision: u64,
    pub(super) entry_revision: u64,
    pub(super) descriptor: CapabilityDescriptor,
    pub(super) descriptor_fingerprint: [u8; 32],
    pub(super) provider: ProviderHandle,
    pub(super) provider_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelRuntimePhase {
    Start,
    Stop,
}

/// Diagnostic only, never authority to admit work or dispose a transport.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChannelCleanupResidual {
    pub(super) active_work: usize,
    pub(super) unjoined_forwarder: bool,
    pub(super) transport_may_be_live: bool,
    pub(super) mount_retained: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ChannelBindingError {
    #[error("channel not found")]
    NotFound,
    #[error("legacy channel has no provider binding")]
    UnboundLegacy,
    #[error("unsupported channel lifecycle declaration")]
    InvalidContract,
    #[error("wrong channel source")]
    WrongSource,
    #[error("wrong channel provider")]
    WrongProvider,
    #[error("wrong channel visibility")]
    WrongVisibility,
    #[error("stale channel entry")]
    StaleEntry,
    #[error("stale channel descriptor")]
    StaleDescriptor,
    #[error("channel is retiring")]
    Retiring,
    #[error("channel publication is busy")]
    Busy,
    #[error("channel source publication revision exhausted")]
    PublicationExhausted,
    #[error(transparent)]
    Provider(#[from] ProviderCallError),
    #[error(transparent)]
    ProviderLifecycle(#[from] ProviderLifecycleError),
    #[error("channel runtime {phase:?}: {error}; residual: {residual:?}")]
    Runtime {
        phase: ChannelRuntimePhase,
        error: String,
        residual: ChannelCleanupResidual,
    },
}

pub(crate) fn canonical_schema_fingerprint(schema: &Value) -> [u8; 32] {
    let mut bytes = Vec::new();
    canonicalize_json(schema, &mut bytes);
    Sha256::digest(bytes).into()
}

/// Explicit wire tags, arrays and nulls preserve all descriptor distinctions.
pub(crate) fn canonical_descriptor_fingerprint(descriptor: &CapabilityDescriptor) -> [u8; 32] {
    let kind = match descriptor.kind {
        CapabilityKind::Tool => "tool",
        CapabilityKind::Skill => "skill",
        CapabilityKind::Agent => "agent",
        CapabilityKind::Task => "task",
        CapabilityKind::Resource => "resource",
        CapabilityKind::EventSource => "event_source",
        CapabilityKind::Subscription => "subscription",
        CapabilityKind::Plugin => "plugin",
        CapabilityKind::Hook => "hook",
    };
    let lifetime = match descriptor.lifetime {
        LifetimeScope::Runtime => "runtime",
        LifetimeScope::Session => "session",
        LifetimeScope::Run => "run",
        LifetimeScope::Task => "task",
        LifetimeScope::External => "external",
    };
    let owner = match &descriptor.owner {
        OwnerRef::Runtime => json!(["runtime"]),
        OwnerRef::Session(session) => json!(["session", session_value(session)]),
        OwnerRef::Run(run) => json!(["run", run.0]),
        OwnerRef::Task(task) => json!(["task", task.0]),
    };
    let services: Vec<_> = descriptor.services.iter().map(|service| {
        let relation = match service.relation {
            ServiceRelation::Requires => "requires",
            ServiceRelation::Provides => "provides",
            ServiceRelation::Conflicts => "conflicts",
        };
        json!({"relation": relation, "target": {"namespace": service.target.namespace, "name": service.target.name}})
    }).collect();
    let facets: Vec<_> = [
        (CapabilityFacet::Callable, "callable"),
        (CapabilityFacet::EventSource, "event_source"),
        (CapabilityFacet::Subscription, "subscription"),
        (CapabilityFacet::ProviderLifecycle, "provider_lifecycle"),
        (CapabilityFacet::ExternalEffect, "external_effect"),
    ]
    .into_iter()
    .filter_map(|(facet, tag)| descriptor.supports(facet).then_some(tag))
    .collect();
    canonical_schema_fingerprint(&json!({
        "id": {"namespace": descriptor.id.namespace, "name": descriptor.id.name},
        "kind": kind,
        "schema": {"version": descriptor.schema.version, "fingerprint": descriptor.schema.fingerprint},
        "revision": descriptor.revision.0,
        "owner": owner,
        "lifetime": lifetime,
        "visibility": {
            "principal": descriptor.visibility.principal,
            "workspace": descriptor.visibility.workspace,
            "session": descriptor.visibility.session.as_ref().map(session_value),
            "allowed_kinds": descriptor.visibility.allowed_kinds,
            "allowed_ids": descriptor.visibility.allowed_ids,
            "approval": descriptor.visibility.approval,
        },
        "services": services,
        "metadata": {"surface": descriptor.metadata.surface},
        "facets": facets,
    }))
}

fn session_value(session: &SessionKey) -> Value {
    // Preserve path identity without lossy display strings or filesystem I/O.
    json!({"harness_id": session.harness_id(), "cwd": session.cwd().as_os_str().as_encoded_bytes(), "name": session.name()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::manager::SessionKey;
    use crate::capability::descriptor::{
        CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
        CapabilityRevision, ProjectionMetadata, SchemaRef, Service, ServiceRelation,
    };
    use crate::capability::ownership::{LifetimeScope, OwnerRef, RunId, TaskId, VisibilityScope};
    use serde_json::json;

    fn contract(
        schema: serde_json::Value,
    ) -> Result<ChannelLifecycleContract, ChannelBindingError> {
        ChannelLifecycleContract::new(
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            VisibilityScope::default(),
            schema,
            ProjectionMetadata::default(),
        )
    }

    #[test]
    fn lifecycle_contract_rejects_placeholder_and_operation_schema() {
        for schema in [
            json!({}),
            json!(true),
            json!({"type":"object"}),
            json!({"type":"object", "properties":{"message":{"type":"string"}}, "required":["message"]}),
            json!({"input":{"type":"object"},"output":{"type":"string"}}),
        ] {
            assert!(matches!(
                contract(schema),
                Err(ChannelBindingError::InvalidContract)
            ));
        }
        let valid = ChannelLifecycleDeclarationV1::schema();
        for field in [
            "$schema",
            "$id",
            "additionalProperties",
            "required",
            "properties",
        ] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(matches!(
                contract(missing),
                Err(ChannelBindingError::InvalidContract)
            ));
        }
        for field in [
            "declaration_kind",
            "channel_id",
            "provider_transport",
            "lifecycle_model_version",
        ] {
            let mut missing = valid.clone();
            missing["properties"].as_object_mut().unwrap().remove(field);
            assert!(matches!(
                contract(missing),
                Err(ChannelBindingError::InvalidContract)
            ));
        }
        let mut empty = valid.clone();
        empty["required"] = json!([]);
        assert!(matches!(
            contract(empty),
            Err(ChannelBindingError::InvalidContract)
        ));
        assert!(contract(valid).is_ok());
    }

    #[test]
    fn lifecycle_schema_fingerprint_is_canonical() {
        let ordered_a: serde_json::Value =
            serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":["x","y"]}"#).unwrap();
        let ordered_b: serde_json::Value =
            serde_json::from_str(r#"{"a":["x","y"],"z":{"a":1,"b":2}}"#).unwrap();
        assert_eq!(
            canonical_schema_fingerprint(&ordered_a),
            canonical_schema_fingerprint(&ordered_b)
        );
        let mut changed = ordered_a.clone();
        changed["a"] = json!(["y", "x"]);
        assert_ne!(
            canonical_schema_fingerprint(&ordered_a),
            canonical_schema_fingerprint(&changed)
        );
        changed["z"]["a"] = json!(3);
        assert_ne!(
            canonical_schema_fingerprint(&ordered_a),
            canonical_schema_fingerprint(&changed)
        );
        assert_ne!(
            canonical_schema_fingerprint(&ChannelLifecycleDeclarationV1::schema()),
            [0; 32]
        );
        let reordered = serde_json::from_str(r#"{
            "properties": {
                "lifecycle_model_version":{"const":1}, "provider_transport":{"const":"in_process"},
                "channel_id":{"minLength":1,"type":"string"}, "declaration_kind":{"const":"channel-provider-lifecycle"}
            },
            "required":["declaration_kind","channel_id","provider_transport","lifecycle_model_version"],
            "additionalProperties":false,"type":"object",
            "$id":"aleph:gateway:channel-lifecycle-declaration:v1",
            "$schema":"https://json-schema.org/draft/2020-12/schema"
        }"#).unwrap();
        assert!(contract(reordered).is_ok());
    }

    fn descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor {
            id: CapabilityId {
                namespace: "aleph/gateway/channels".into(),
                name: "team/a/b".into(),
            },
            kind: CapabilityKind::Resource,
            schema: SchemaRef {
                version: 1,
                fingerprint: canonical_schema_fingerprint(&ChannelLifecycleDeclarationV1::schema()),
            },
            revision: CapabilityRevision(1),
            lifetime: LifetimeScope::Runtime,
            visibility: VisibilityScope::default(),
            owner: OwnerRef::Runtime,
            services: vec![],
            metadata: ProjectionMetadata::default(),
            facets: CapabilityFacets::new().with(CapabilityFacet::ProviderLifecycle),
        }
    }

    #[test]
    fn descriptor_only_change_changes_full_fingerprint() {
        let before = descriptor();
        let session = SessionKey::with_name("test", "/tmp/channel-binding", Some("named"));
        let mutations: Vec<Box<dyn Fn(&mut CapabilityDescriptor)>> = vec![
            Box::new(|d| d.id.namespace.push('x')),
            Box::new(|d| d.id.name.push('x')),
            Box::new(|d| d.kind = CapabilityKind::Tool),
            Box::new(|d| d.revision.0 += 1),
            Box::new(|d| d.owner = OwnerRef::Run(RunId("run".into()))),
            Box::new(|d| d.owner = OwnerRef::Task(TaskId("task".into()))),
            Box::new({
                let session = session.clone();
                move |d| d.owner = OwnerRef::Session(session.clone())
            }),
            Box::new(|d| d.lifetime = LifetimeScope::External),
            Box::new(|d| d.visibility.principal = Some("principal".into())),
            Box::new(|d| d.visibility.workspace = Some("workspace".into())),
            Box::new(move |d| d.visibility.session = Some(session.clone())),
            Box::new(|d| d.visibility.allowed_kinds = Some(vec!["Resource".into()])),
            Box::new(|d| d.visibility.allowed_ids = Some(vec!["team/a/b".into()])),
            Box::new(|d| d.visibility.approval = Some("approval".into())),
            Box::new(|d| d.metadata.surface = Some("internal".into())),
            Box::new(|d| d.facets = d.facets.with(CapabilityFacet::Callable)),
            Box::new(|d| {
                d.services.push(Service {
                    relation: ServiceRelation::Requires,
                    target: CapabilityId {
                        namespace: "dep".into(),
                        name: "one".into(),
                    },
                })
            }),
        ];
        for mutate in mutations {
            let mut changed = before.clone();
            mutate(&mut changed);
            assert_ne!(
                canonical_descriptor_fingerprint(&before),
                canonical_descriptor_fingerprint(&changed)
            );
            assert_eq!(before.schema.fingerprint, changed.schema.fingerprint);
        }
        for change_version in [false, true] {
            let mut changed = before.clone();
            if change_version {
                changed.schema.version += 1;
            } else {
                changed.schema.fingerprint[0] ^= 1;
            }
            assert_ne!(
                canonical_descriptor_fingerprint(&before),
                canonical_descriptor_fingerprint(&changed)
            );
        }
        let mut services = before.clone();
        services.services = vec![
            Service {
                relation: ServiceRelation::Requires,
                target: CapabilityId {
                    namespace: "dep".into(),
                    name: "one".into(),
                },
            },
            Service {
                relation: ServiceRelation::Provides,
                target: CapabilityId {
                    namespace: "dep".into(),
                    name: "two".into(),
                },
            },
        ];
        let mut changed = services.clone();
        changed.services.reverse();
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = services.clone();
        changed.services[0].relation = ServiceRelation::Conflicts;
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = services.clone();
        changed.services[0].target.name.push('x');
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = services.clone();
        changed.services[0].target.namespace.push('x');
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        let mut scope = before.clone();
        scope.visibility.allowed_ids = Some(vec!["a".into(), "b".into()]);
        changed = scope.clone();
        changed.visibility.allowed_ids.as_mut().unwrap().reverse();
        assert_ne!(
            canonical_descriptor_fingerprint(&scope),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = before.clone();
        changed.visibility.allowed_ids = Some(vec![]);
        assert_ne!(
            canonical_descriptor_fingerprint(&before),
            canonical_descriptor_fingerprint(&changed)
        );
    }
}
