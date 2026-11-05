//! Session-scoped capability registration.
//!
//! A routing session is a `Resource` capability.  This module owns the small
//! adapter between the session lifecycle and the existing provider/ownership
//! authorities; it does not introduce a second descriptor or handler map.

use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision, ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{
    LifetimeScope, OwnerRef, OwnershipTree, RegisterError, VisibilityScope,
};
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
};
use crate::capability::provider_registry::{
    ProviderRegistration, ProviderRegistry, ProviderRegistryError, ProviderRemovalError,
};
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Namespace for capabilities representing application sessions.
pub const SESSION_NAMESPACE: &str = "aleph/sessions";

/// Stable metadata projected with a session capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCapabilityMetadata {
    pub session_id: SessionKey,
    pub created_at: i64,
    pub state: String,
}

impl SessionCapabilityMetadata {
    #[must_use]
    pub fn new(session_id: SessionKey, created_at: i64, state: impl Into<String>) -> Self {
        Self {
            session_id,
            created_at,
            state: state.into(),
        }
    }

    fn attributes(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("id".to_string(), self.session_id.to_key_string()),
            ("created_at".to_string(), self.created_at.to_string()),
            ("state".to_string(), self.state.clone()),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionCapabilityError {
    #[error("session capability ownership registration failed: {0:?}")]
    Ownership(RegisterError),
    #[error("session capability provider registration failed: {0}")]
    Provider(#[from] ProviderRegistryError),
    #[error("session capability provider removal failed: {0}")]
    Removal(#[from] ProviderRemovalError),
}

/// Lifecycle sink used by session stores without coupling them to Zahir's
/// concrete facade.  Calls are synchronous and happen only after the session
/// row mutation has committed; registry broadcasts are best-effort notifications.
pub trait SessionCapabilitySink: Send + Sync {
    fn register_session(
        &self,
        metadata: SessionCapabilityMetadata,
    ) -> Result<(), SessionCapabilityError>;

    fn unregister_session(&self, session_id: &SessionKey) -> Result<(), SessionCapabilityError>;
}

/// The provider/ownership-backed session capability sink.
#[derive(Clone)]
pub struct SessionCapabilityRegistry {
    providers: ProviderRegistry,
    ownership: Arc<OwnershipTree>,
}

impl SessionCapabilityRegistry {
    #[must_use]
    pub fn new(providers: ProviderRegistry, ownership: Arc<OwnershipTree>) -> Self {
        Self {
            providers,
            ownership,
        }
    }

    #[must_use]
    pub fn capability_id(session_id: &SessionKey) -> CapabilityId {
        CapabilityId {
            namespace: SESSION_NAMESPACE.to_string(),
            name: session_id.to_key_string(),
        }
    }

    fn descriptor(metadata: &SessionCapabilityMetadata) -> CapabilityDescriptor {
        let id = Self::capability_id(&metadata.session_id);
        let attributes = metadata.attributes();
        let mut canonical = Vec::new();
        canonical.extend_from_slice(b"aleph/session-resource/v1\0");
        canonical.extend_from_slice(id.name.as_bytes());
        canonical.extend_from_slice(
            &serde_json::to_vec(&attributes).expect("BTreeMap<String, String> serializes"),
        );
        let digest = Sha256::digest(canonical);
        let mut fingerprint = [0_u8; 32];
        fingerprint.copy_from_slice(&digest);

        CapabilityDescriptor {
            id,
            kind: CapabilityKind::Resource,
            schema: SchemaRef {
                version: 1,
                fingerprint,
            },
            revision: CapabilityRevision(1),
            lifetime: LifetimeScope::Session,
            visibility: VisibilityScope {
                session: Some(metadata.session_id.clone()),
                ..VisibilityScope::default()
            },
            owner: OwnerRef::Session(metadata.session_id.clone()),
            services: Vec::new(),
            metadata: ProjectionMetadata {
                surface: Some("session".to_string()),
                attributes,
            },
            facets: CapabilityFacets::new().with(CapabilityFacet::EventSource),
        }
    }

    fn provider(descriptor: &CapabilityDescriptor) -> ProviderRegistration {
        let provider_id = ProviderId::new(format!("{}/{}", SESSION_NAMESPACE, descriptor.id.name))
            .expect("session capability provider id is non-empty");
        ProviderRegistration::new(
            descriptor.clone(),
            ProviderHandle::new(ProviderLifecycle::ready(
                provider_id,
                ProviderTransport::InProcess,
            )),
        )
    }
}

impl SessionCapabilitySink for SessionCapabilityRegistry {
    fn register_session(
        &self,
        metadata: SessionCapabilityMetadata,
    ) -> Result<(), SessionCapabilityError> {
        let descriptor = Self::descriptor(&metadata);
        let id = descriptor.id.clone();
        let visibility = descriptor.visibility.clone();
        self.ownership
            .register(
                id.clone(),
                descriptor.owner.clone(),
                descriptor.lifetime,
                visibility,
            )
            .map_err(SessionCapabilityError::Ownership)?;

        match self.providers.register(Self::provider(&descriptor)) {
            Ok(()) => Ok(()),
            Err(ProviderRegistryError::Duplicate(existing)) => {
                // Concurrent callers may race after the ownership insert. A
                // duplicate is idempotent only when the exact capability now
                // exists; never turn an unrelated registration into success.
                if existing == id {
                    Ok(())
                } else {
                    Err(ProviderRegistryError::Duplicate(existing).into())
                }
            }
        }
    }

    fn unregister_session(&self, session_id: &SessionKey) -> Result<(), SessionCapabilityError> {
        let id = Self::capability_id(session_id);
        let removed = self.providers.unregister_id(&id);
        let revoked = self.ownership.revoke(&id);
        match removed {
            Ok(_) => Ok(()),
            Err(ProviderRemovalError::NotFound(_)) if revoked || self.ownership.is_revoked(&id) => {
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::facade::{Scope, Zahir};
    use crate::capability::ownership::{OwnerRef, VisibilityScope};
    use crate::capability::zahir_facade::ZahirFacade;
    use crate::tools::registry::ToolHandlerRegistry;

    fn registry() -> (SessionCapabilityRegistry, Arc<ZahirFacade>) {
        let providers = ProviderRegistry::new();
        let ownership = Arc::new(OwnershipTree::new());
        let sink = SessionCapabilityRegistry::new(providers.clone(), ownership.clone());
        let facade =
            ZahirFacade::new_with_providers(ToolHandlerRegistry::new(), ownership, providers);
        (sink, facade)
    }

    #[test]
    fn registration_is_discoverable_with_routing_owner_and_metadata() {
        let (sink, facade) = registry();
        let session = SessionKey::main("capability-test");
        sink.register_session(SessionCapabilityMetadata::new(
            session.clone(),
            1_725_000_000,
            "created",
        ))
        .unwrap();

        let snapshot = facade.describe(Scope {
            kind: Some(CapabilityKind::Resource),
            namespace: Some(SESSION_NAMESPACE.to_string()),
            visibility: VisibilityScope {
                session: Some(session.clone()),
                ..VisibilityScope::default()
            },
            ..Scope::default()
        });
        assert_eq!(snapshot.capabilities.len(), 1);
        let descriptor = &snapshot.capabilities[0];
        assert!(facade
            .describe(Scope {
                kind: Some(CapabilityKind::Resource),
                namespace: Some(SESSION_NAMESPACE.to_string()),
                ..Scope::default()
            })
            .capabilities
            .is_empty());
        assert_eq!(descriptor.owner, OwnerRef::Session(session.clone()));
        assert_eq!(descriptor.lifetime, LifetimeScope::Session);
        assert_eq!(
            descriptor.metadata.attributes["id"],
            session.to_key_string()
        );
        assert_eq!(descriptor.metadata.attributes["created_at"], "1725000000");
        assert_eq!(descriptor.metadata.attributes["state"], "created");
    }

    #[test]
    fn subscription_receives_registration_and_unregistration_without_data_loss() {
        let (sink, _facade) = registry();
        let mut events = sink.providers.subscribe();
        let session = SessionKey::main("events");
        sink.register_session(SessionCapabilityMetadata::new(
            session.clone(),
            1,
            "created",
        ))
        .unwrap();
        let registered = events.try_recv().unwrap();
        assert!(
            matches!(registered, crate::capability::provider_registry::ProviderRegistryChange::Registered { id, .. } if id == SessionCapabilityRegistry::capability_id(&session))
        );

        sink.unregister_session(&session).unwrap();
        let unregistered = events.try_recv().unwrap();
        assert!(
            matches!(unregistered, crate::capability::provider_registry::ProviderRegistryChange::Unregistered { id, .. } if id == SessionCapabilityRegistry::capability_id(&session))
        );
    }

    #[test]
    fn unregister_removes_only_the_target_session_and_is_idempotent() {
        let (sink, facade) = registry();
        let first = SessionKey::main("first");
        let second = SessionKey::main("second");
        for session in [&first, &second] {
            sink.register_session(SessionCapabilityMetadata::new(
                session.clone(),
                1,
                "created",
            ))
            .unwrap();
        }

        sink.unregister_session(&first).unwrap();
        sink.unregister_session(&first).unwrap();
        let snapshot = facade.describe(Scope {
            namespace: Some(SESSION_NAMESPACE.to_string()),
            visibility: VisibilityScope {
                session: Some(second.clone()),
                ..VisibilityScope::default()
            },
            ..Scope::default()
        });
        assert_eq!(snapshot.capabilities.len(), 1);
        assert_eq!(
            snapshot.capabilities[0].id,
            SessionCapabilityRegistry::capability_id(&second)
        );
    }
}
