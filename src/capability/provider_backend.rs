//! Capability backend adapter for the Core provider registry.

use crate::capability::backend::CapabilityBackend;
use crate::capability::descriptor::{CapabilityDescriptor, CapabilityId};
use crate::capability::facade::{CapabilitySnapshot, Cursor, Scope};
use crate::capability::provider_registry::{ProviderEntry, ProviderRegistry};

#[derive(Clone)]
pub struct ProviderBackendAdapter {
    pub registry: ProviderRegistry,
}

impl ProviderBackendAdapter {
    #[must_use]
    pub fn new(registry: ProviderRegistry) -> Self {
        Self { registry }
    }

    pub(crate) fn entry(&self, id: &CapabilityId) -> Option<ProviderEntry> {
        self.registry.entry(id)
    }

    pub(crate) fn entries(&self) -> Vec<ProviderEntry> {
        self.registry.entries()
    }

    fn descriptor_in_scope(descriptor: &CapabilityDescriptor, scope: &Scope) -> bool {
        scope.kind.is_none_or(|kind| descriptor.kind == kind)
            && scope
                .namespace
                .as_ref()
                .is_none_or(|namespace| descriptor.id.namespace == *namespace)
            && scope.facet.is_none_or(|facet| descriptor.supports(facet))
    }

    fn descriptors(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        self.entries()
            .into_iter()
            .map(|entry| {
                let mut descriptor = entry.descriptor;
                descriptor.visibility = scope.visibility.clone();
                descriptor
            })
            .filter(|descriptor| Self::descriptor_in_scope(descriptor, scope))
            .collect()
    }
}

impl CapabilityBackend for ProviderBackendAdapter {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor> {
        self.entry(id).map(|entry| entry.descriptor)
    }

    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        self.descriptors(scope)
    }

    fn snapshot(&self, scope: &Scope) -> CapabilitySnapshot {
        CapabilitySnapshot {
            capabilities: self.descriptors(scope),
            committed_cursor: Cursor(self.registry.revision()),
        }
    }
}
