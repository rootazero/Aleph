//! Lightweight UI capability primitives for the Core layer.
//!
//! This module owns the typed UI capability contract: the namespace, the
//! schema, and the descriptor that the gateway publishes. The in-memory
//! display/input stub and the synchronous legacy handler that used to sit
//! beside them were removed by the 2026-11-20 severed-wire audit: no
//! production path ever registered that handler, so the whole "legacy UI
//! handler" arm was a severed wire (see
//! `review-results/sev-wire-2026-11-20/cap-ui-REPORT.md`, CUI-02). The live
//! UI path is [`crate::capability::ui_surface`] plus the gateway's
//! `capability_ui` face, which owns rendering and input transport.

use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const UI_NAMESPACE: &str = "aleph/ui";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UiError {
    #[error("UI capability is closed")]
    Closed,
    #[error("UI input is invalid: {0}")]
    InvalidInput(String),
    #[error("UI state lock is poisoned")]
    Poisoned,
    #[error("UI surface is unavailable")]
    Unavailable,
    #[error("UI surface attachment is stale")]
    StaleAttachment,
    #[error("UI input queue is full")]
    QueueFull,
    #[error("UI surface already has a pending display")]
    Busy,
    #[error("UI render acknowledgement reported failure")]
    RenderFailed,
    #[error("UI render acknowledgement timed out")]
    Timeout,
    #[error("UI display was cancelled")]
    Cancelled,
}

pub(crate) fn schema() -> Value {
    json!({
        "oneOf":[
            {
                "type":"object",
                "properties":{
                    "operation":{"const":"display"},
                    "text":{"type":"string"}
                },
                "required":["operation","text"],
                "additionalProperties":false
            },
            {
                "type":"object",
                "properties":{
                    "operation":{"const":"input"}
                },
                "required":["operation"],
                "additionalProperties":false
            }
        ]
    })
}

pub(crate) fn descriptor() -> CapabilityDescriptor {
    let schema = schema();
    let mut bytes = Vec::new();
    crate::tools::descriptor::canonicalize_json(&schema, &mut bytes);
    CapabilityDescriptor {
        id: CapabilityId {
            namespace: UI_NAMESPACE.into(),
            name: "surface".into(),
        },
        kind: CapabilityKind::Ui,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_descriptor_is_addressed_by_its_own_namespace() {
        let descriptor = descriptor();
        assert_eq!(descriptor.id.namespace, UI_NAMESPACE);
        assert_eq!(descriptor.id.name, "surface");
        assert_eq!(descriptor.kind, CapabilityKind::Ui);
        assert!(descriptor.supports(CapabilityFacet::Callable));
    }

    #[test]
    fn ui_schema_covers_the_display_and_input_operations() {
        let schema = schema();
        let variants = schema
            .get("oneOf")
            .and_then(Value::as_array)
            .expect("oneOf variants");
        assert_eq!(variants.len(), 2);
        assert!(variants.iter().all(|variant| {
            variant
                .get("properties")
                .and_then(|properties| properties.get("operation"))
                .is_some()
        }));
    }
}
