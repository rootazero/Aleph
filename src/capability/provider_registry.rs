//! Core-owned registry for provider-shaped capabilities.
//!
//! This registry stores descriptors, lifecycle handles and owner-side hooks. It
//! never performs activation effects; owners implement `ProviderActivator` and
//! decide how to start their resources.

use crate::capability::descriptor::{CapabilityDescriptor, CapabilityId};
use crate::capability::ownership::{OwnerRef, VisibilityScope};
use crate::capability::provider::{ProviderHandle, ProviderId};
use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActivationError {
    #[error("provider owner rejected activation")]
    Rejected,
    #[error("provider activation failed")]
    Failed,
    #[error("provider not found")]
    Unknown,
    #[error("provider not visible in requested scope")]
    NotVisible,
}

/// Owner-performed activation boundary. Implementations perform effects; Core
/// only invokes the request and observes the provider handle afterward.
pub trait ProviderActivator: Send + Sync {
    fn activate(&self) -> Result<(), ActivationError>;
}

#[derive(Clone)]
pub struct ProviderRegistration {
    pub descriptor: CapabilityDescriptor,
    pub provider: ProviderHandle,
    pub owner: OwnerRef,
    pub visibility: VisibilityScope,
    pub activator: Option<Arc<dyn ProviderActivator>>,
}

impl ProviderRegistration {
    #[must_use]
    pub fn new(descriptor: CapabilityDescriptor, provider: ProviderHandle) -> Self {
        Self {
            owner: descriptor.owner.clone(),
            visibility: descriptor.visibility.clone(),
            descriptor,
            provider,
            activator: None,
        }
    }

    #[must_use]
    pub fn with_activator(mut self, activator: Arc<dyn ProviderActivator>) -> Self {
        self.activator = Some(activator);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderRegistryError {
    #[error("provider capability is already registered: {0:?}")]
    Duplicate(CapabilityId),
}

#[derive(Clone)]
pub(crate) struct ProviderEntry {
    pub(crate) descriptor: CapabilityDescriptor,
    pub(crate) provider: ProviderHandle,
    pub(crate) owner: OwnerRef,
    pub(crate) visibility: VisibilityScope,
    pub(crate) provider_id: ProviderId,
    pub(crate) activator: Option<Arc<dyn ProviderActivator>>,
}

#[derive(Default)]
struct ProviderRegistryState {
    entries: HashMap<CapabilityId, ProviderEntry>,
    revision: u64,
}

/// The one Core authority for provider-shaped capability registrations.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    state: Arc<RwLock<ProviderRegistryState>>,
}

impl ProviderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        registration: ProviderRegistration,
    ) -> Result<(), ProviderRegistryError> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.entries.contains_key(&registration.descriptor.id) {
            return Err(ProviderRegistryError::Duplicate(registration.descriptor.id));
        }
        let provider_id = registration.provider.provider_id();
        state.entries.insert(
            registration.descriptor.id.clone(),
            ProviderEntry {
                descriptor: registration.descriptor,
                provider: registration.provider,
                owner: registration.owner,
                visibility: registration.visibility,
                provider_id,
                activator: registration.activator,
            },
        );
        state.revision = state.revision.saturating_add(1);
        Ok(())
    }

    pub(crate) fn entry(&self, id: &CapabilityId) -> Option<ProviderEntry> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .get(id)
            .cloned()
    }

    pub(crate) fn entries(&self) -> Vec<ProviderEntry> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .values()
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::descriptor::{
        CapabilityFacet, CapabilityFacets, CapabilityKind, CapabilityRevision, ProjectionMetadata,
        SchemaRef,
    };
    use crate::capability::ownership::{LifetimeScope, OwnerRef};
    use crate::capability::provider::{ProviderLifecycle, ProviderTransport};

    #[test]
    fn registration_preserves_provider_identity_and_revision() {
        let provider = ProviderHandle::new(ProviderLifecycle::ready(
            ProviderId::new("test/provider").unwrap(),
            ProviderTransport::InProcess,
        ));
        let descriptor = CapabilityDescriptor {
            id: CapabilityId {
                namespace: "aleph/resources".into(),
                name: "test".into(),
            },
            kind: CapabilityKind::Resource,
            schema: SchemaRef {
                version: 1,
                fingerprint: [0; 32],
            },
            revision: CapabilityRevision(0),
            lifetime: LifetimeScope::Runtime,
            visibility: VisibilityScope::default(),
            owner: OwnerRef::Runtime,
            services: Vec::new(),
            metadata: ProjectionMetadata::default(),
            facets: CapabilityFacets::new().with(CapabilityFacet::ProviderLifecycle),
        };
        let registry = ProviderRegistry::new();
        registry
            .register(ProviderRegistration::new(
                descriptor.clone(),
                provider.clone(),
            ))
            .unwrap();
        let entry = registry.entry(&descriptor.id).unwrap();
        assert_eq!(entry.provider_id.as_str(), "test/provider");
        assert_eq!(entry.provider.snapshot().1, 1);
        assert_eq!(registry.revision(), 1);
    }
}
