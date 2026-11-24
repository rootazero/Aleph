#![allow(dead_code)]

//! Source-owned, read-only capability views for bound channels.
//!
//! This module derives Generic Gateway capability values from the channel
//! registry's existing bound registrations. It never activates a provider or
//! creates another descriptor, ownership, or lifecycle authority.

use super::binding::{BoundChannelCapture, ChannelBindingError};
use super::{ChannelRegistration, ChannelRegistry};
use crate::capability::descriptor::{CapabilityDescriptor, CapabilityFacet, ProjectionMetadata};
use crate::capability::facade::{
    CapabilitySnapshot, Cursor, Projection, Reference, Scope, TransportTarget,
};
use crate::capability::ownership::BindingValidationError;
use crate::capability::provider::ProviderCallError;

const CHANNEL_NAMESPACE: &str = "aleph/gateway/channels";

struct BoundCapabilityCut {
    descriptors: Vec<CapabilityDescriptor>,
    source_revision: u64,
}

impl ChannelRegistry {
    /// Describe bound channels from one source publication cut.
    pub(crate) async fn describe_bound_capabilities(
        &self,
        scope: &Scope,
    ) -> Result<CapabilitySnapshot, ChannelBindingError> {
        let cut = self.bound_capability_cut(scope).await?;
        Ok(CapabilitySnapshot {
            capabilities: cut.descriptors,
            committed_cursor: Cursor(cut.source_revision),
        })
    }

    /// List the descriptors from one source publication cut.
    pub(crate) async fn list_bound_capabilities(
        &self,
        scope: &Scope,
    ) -> Result<Vec<CapabilityDescriptor>, ChannelBindingError> {
        Ok(self.bound_capability_cut(scope).await?.descriptors)
    }

    /// Project the descriptors from one source publication cut onto a transport.
    pub(crate) async fn project_bound_capabilities(
        &self,
        target: TransportTarget,
        scope: &Scope,
    ) -> Result<Projection, ChannelBindingError> {
        let cut = self.bound_capability_cut(scope).await?;
        Ok(Projection {
            target,
            metadata: ProjectionMetadata {
                surface: Some(
                    match target {
                        TransportTarget::Mcp => "mcp",
                        TransportTarget::AcpJson => "acp-json",
                        TransportTarget::StateDatabase => "state-database",
                    }
                    .into(),
                ),
            },
            capabilities: cut
                .descriptors
                .into_iter()
                .map(|descriptor| descriptor.id)
                .collect(),
        })
    }

    /// Resolve a bound channel into a source/provider/owner evidenced capture.
    ///
    /// This is a read lease, not execution admission: provider lifecycle state
    /// is observed for its generation only, so cold and disabled declarations
    /// remain resolvable without starting anything.
    pub(crate) async fn resolve_bound_capability(
        &self,
        reference: &Reference,
    ) -> Result<BoundChannelCapture, ChannelBindingError> {
        if reference.id.namespace != CHANNEL_NAMESPACE {
            return Err(ChannelBindingError::NotFound);
        }

        let (mut capture, ownership, owner, lifetime) = {
            let _lifecycle = self.lifecycle_sync.lock().await;
            let channels = self.channels.read().await;
            let entry = channels
                .get(&super::ChannelId::new(&reference.id.name))
                .cloned()
                .ok_or(ChannelBindingError::NotFound)?;
            let ChannelRegistration::Bound(bound) = &entry.registration else {
                return Err(ChannelBindingError::UnboundLegacy);
            };
            if bound.descriptor.id != reference.id {
                return Err(ChannelBindingError::NotFound);
            }
            if bound.descriptor.visibility != reference.visibility {
                return Err(ChannelBindingError::WrongVisibility);
            }

            (
                BoundChannelCapture {
                    source: bound.source.clone(),
                    entry: entry.clone(),
                    channel: entry.channel.clone(),
                    source_revision: self
                        .source_revision
                        .load(std::sync::atomic::Ordering::Relaxed),
                    entry_revision: bound.entry_revision,
                    descriptor_revision: bound.descriptor_revision,
                    descriptor: bound.descriptor.clone(),
                    descriptor_fingerprint: bound.descriptor_fingerprint,
                    provider: bound.provider.clone(),
                    provider_generation: 0,
                },
                bound.ownership.clone(),
                bound.contract.owner.clone(),
                bound.contract.lifetime,
            )
        };

        capture.provider_generation = capture.provider.snapshot().1;
        self.validate_bound_capture(&capture, &capture.entry)?;
        let (provider_state, provider_generation) = capture.provider.snapshot();
        if provider_generation != capture.provider_generation {
            return Err(ChannelBindingError::Provider(ProviderCallError {
                provider_id: capture
                    .provider
                    .with_snapshot(|provider| provider.provider_id().clone()),
                state: provider_state,
                expected_generation: capture.provider_generation,
                actual_generation: provider_generation,
            }));
        }
        if ownership.is_revoked(&capture.descriptor.id) {
            return Err(BindingValidationError::Revoked.into());
        }
        if ownership.is_disposed(lifetime) {
            return Err(BindingValidationError::Disposed.into());
        }
        let owner_generation = ownership
            .generation(&capture.descriptor.id, &reference.visibility)
            .ok_or(BindingValidationError::MissingBinding)?;
        ownership.with_current_binding(
            &capture.descriptor.id,
            &reference.visibility,
            owner_generation,
            &owner,
            &lifetime,
            || (),
        )?;

        let _lifecycle = self.lifecycle_sync.lock().await;
        let channels = self.channels.read().await;
        let current = channels
            .get(&super::ChannelId::new(&reference.id.name))
            .ok_or(ChannelBindingError::NotFound)?;
        if self
            .source_revision
            .load(std::sync::atomic::Ordering::Relaxed)
            != capture.source_revision
        {
            return Err(ChannelBindingError::StaleEntry);
        }
        self.validate_bound_capture(&capture, current)?;
        Ok(capture)
    }

    async fn bound_capability_cut(
        &self,
        scope: &Scope,
    ) -> Result<BoundCapabilityCut, ChannelBindingError> {
        let _lifecycle = self.lifecycle_sync.lock().await;
        let channels = self.channels.read().await;
        let mut descriptors: Vec<_> = channels
            .values()
            .filter_map(|entry| {
                let ChannelRegistration::Bound(bound) = &entry.registration else {
                    return None;
                };
                descriptor_in_scope(&bound.descriptor, scope).then(|| bound.descriptor.clone())
            })
            .collect();
        descriptors.sort_by(|left, right| {
            left.id
                .namespace
                .cmp(&right.id.namespace)
                .then_with(|| left.id.name.cmp(&right.id.name))
        });
        Ok(BoundCapabilityCut {
            descriptors,
            source_revision: self
                .source_revision
                .load(std::sync::atomic::Ordering::Relaxed),
        })
    }
}

fn descriptor_in_scope(descriptor: &CapabilityDescriptor, scope: &Scope) -> bool {
    scope.kind.is_none_or(|kind| kind == descriptor.kind)
        && scope
            .namespace
            .as_ref()
            .is_none_or(|namespace| namespace == &descriptor.id.namespace)
        && scope
            .facet
            .is_none_or(|facet: CapabilityFacet| descriptor.supports(facet))
        && descriptor.visibility == scope.visibility
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::descriptor::{CapabilityFacet, CapabilityKind};
    use crate::capability::facade::Reference;
    use crate::capability::ownership::{LifetimeScope, OwnerRef, OwnershipTree, VisibilityScope};
    use crate::gateway::channel_registry::binding::{
        ChannelInitialState, ChannelLifecycleContract, ChannelLifecycleDeclarationV1,
    };
    use crate::gateway::channel_registry::test_support::ResourceProbe;
    use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
    use crate::sync_primitives::Arc;

    fn contract(visibility: VisibilityScope) -> ChannelLifecycleContract {
        ChannelLifecycleContract::new(
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            visibility,
            ChannelLifecycleDeclarationV1::schema(),
            ProjectionMetadata::default(),
        )
        .unwrap()
    }

    async fn register(
        registry: &Arc<ChannelRegistry>,
        tree: &Arc<OwnershipTree>,
        id: &str,
        visibility: VisibilityScope,
        initial: ChannelInitialState,
        with_owner: bool,
        probe: ResourceProbe,
    ) -> Reference {
        let channel = Box::new(WebhookChannel::observed(
            id,
            WebhookChannelConfig::default(),
            probe,
        ));
        registry
            .register_bound_channel(channel, contract(visibility.clone()), tree.clone(), initial)
            .await
            .unwrap();
        let capture = registry
            .capture_bound_channel(&crate::gateway::channel::ChannelId::new(id))
            .await
            .unwrap();
        if with_owner {
            tree.register(
                capture.descriptor.id.clone(),
                capture.descriptor.owner.clone(),
                capture.descriptor.lifetime,
                capture.descriptor.visibility.clone(),
            )
            .unwrap();
        }
        Reference {
            id: capture.descriptor.id,
            visibility,
        }
    }

    #[tokio::test]
    async fn cold_and_disabled_reads_do_not_start_resources() {
        for initial in [ChannelInitialState::Cold, ChannelInitialState::Disabled] {
            let registry = Arc::new(ChannelRegistry::new());
            let tree = Arc::new(OwnershipTree::new());
            let probe = ResourceProbe::default();
            let reference = register(
                &registry,
                &tree,
                match initial {
                    ChannelInitialState::Cold => "cold-read",
                    ChannelInitialState::Disabled => "disabled-read",
                    ChannelInitialState::Discovered => unreachable!(),
                },
                VisibilityScope::default(),
                initial,
                true,
                probe.clone(),
            )
            .await;

            let snapshot = registry
                .describe_bound_capabilities(&Scope::default())
                .await
                .unwrap();
            assert_eq!(snapshot.capabilities.len(), 1);
            let capture = registry.resolve_bound_capability(&reference).await.unwrap();
            assert_eq!(
                capture.provider.state(),
                match initial {
                    ChannelInitialState::Cold => crate::capability::provider::ProviderState::Cold,
                    ChannelInitialState::Disabled =>
                        crate::capability::provider::ProviderState::Disabled,
                    ChannelInitialState::Discovered => unreachable!(),
                }
            );
            assert_eq!(probe.count("client_build_attempt"), 0);
            assert_eq!(probe.count("request_attempt"), 0);
        }
    }

    #[tokio::test]
    async fn scope_filters_kind_namespace_facet_and_visibility() {
        let registry = Arc::new(ChannelRegistry::new());
        let tree = Arc::new(OwnershipTree::new());
        register(
            &registry,
            &tree,
            "default-scope",
            VisibilityScope::default(),
            ChannelInitialState::Cold,
            true,
            ResourceProbe::default(),
        )
        .await;
        let private_visibility = VisibilityScope {
            principal: Some("alice".into()),
            ..VisibilityScope::default()
        };
        register(
            &registry,
            &tree,
            "private-scope",
            private_visibility.clone(),
            ChannelInitialState::Cold,
            true,
            ResourceProbe::default(),
        )
        .await;

        let default_scope = registry
            .list_bound_capabilities(&Scope::default())
            .await
            .unwrap();
        assert_eq!(
            default_scope
                .iter()
                .map(|d| d.id.name.as_str())
                .collect::<Vec<_>>(),
            ["default-scope"]
        );
        assert!(registry
            .list_bound_capabilities(&Scope {
                kind: Some(CapabilityKind::Tool),
                ..Scope::default()
            })
            .await
            .unwrap()
            .is_empty());
        assert!(registry
            .list_bound_capabilities(&Scope {
                namespace: Some("foreign".into()),
                ..Scope::default()
            })
            .await
            .unwrap()
            .is_empty());
        assert!(registry
            .list_bound_capabilities(&Scope {
                facet: Some(CapabilityFacet::Callable),
                ..Scope::default()
            })
            .await
            .unwrap()
            .is_empty());
        let private_scope = registry
            .list_bound_capabilities(&Scope {
                visibility: private_visibility,
                ..Scope::default()
            })
            .await
            .unwrap();
        assert_eq!(
            private_scope
                .iter()
                .map(|d| d.id.name.as_str())
                .collect::<Vec<_>>(),
            ["private-scope"]
        );
    }

    #[tokio::test]
    async fn projections_share_filtered_ids_and_name_each_target() {
        let registry = Arc::new(ChannelRegistry::new());
        let tree = Arc::new(OwnershipTree::new());
        register(
            &registry,
            &tree,
            "projected",
            VisibilityScope::default(),
            ChannelInitialState::Cold,
            true,
            ResourceProbe::default(),
        )
        .await;
        for (target, surface) in [
            (TransportTarget::Mcp, "mcp"),
            (TransportTarget::AcpJson, "acp-json"),
            (TransportTarget::StateDatabase, "state-database"),
        ] {
            let projection = registry
                .project_bound_capabilities(target, &Scope::default())
                .await
                .unwrap();
            assert_eq!(projection.target, target);
            assert_eq!(projection.metadata.surface.as_deref(), Some(surface));
            assert_eq!(projection.capabilities.len(), 1);
            assert_eq!(projection.capabilities[0].name, "projected");
        }
    }

    #[tokio::test]
    async fn resolve_fails_closed_for_unknown_foreign_visibility_and_owner_state() {
        let registry = Arc::new(ChannelRegistry::new());
        let tree = Arc::new(OwnershipTree::new());
        let reference = register(
            &registry,
            &tree,
            "resolve-state",
            VisibilityScope::default(),
            ChannelInitialState::Cold,
            true,
            ResourceProbe::default(),
        )
        .await;

        let unknown = Reference {
            id: crate::capability::descriptor::CapabilityId {
                namespace: CHANNEL_NAMESPACE.into(),
                name: "missing".into(),
            },
            visibility: VisibilityScope::default(),
        };
        assert!(matches!(
            registry.resolve_bound_capability(&unknown).await,
            Err(ChannelBindingError::NotFound)
        ));
        let foreign = Reference {
            id: crate::capability::descriptor::CapabilityId {
                namespace: "foreign".into(),
                name: reference.id.name.clone(),
            },
            visibility: VisibilityScope::default(),
        };
        assert!(matches!(
            registry.resolve_bound_capability(&foreign).await,
            Err(ChannelBindingError::NotFound)
        ));
        let wrong_visibility = Reference {
            visibility: VisibilityScope {
                principal: Some("bob".into()),
                ..VisibilityScope::default()
            },
            ..reference.clone()
        };
        assert!(matches!(
            registry.resolve_bound_capability(&wrong_visibility).await,
            Err(ChannelBindingError::WrongVisibility)
        ));

        tree.revoke(&reference.id);
        assert!(matches!(
            registry.resolve_bound_capability(&reference).await,
            Err(ChannelBindingError::Owner(BindingValidationError::Revoked))
        ));

        let disposed_registry = Arc::new(ChannelRegistry::new());
        let disposed_tree = Arc::new(OwnershipTree::new());
        let disposed_reference = register(
            &disposed_registry,
            &disposed_tree,
            "disposed-state",
            VisibilityScope::default(),
            ChannelInitialState::Cold,
            true,
            ResourceProbe::default(),
        )
        .await;
        disposed_tree.dispose(LifetimeScope::Runtime);
        assert!(matches!(
            disposed_registry
                .resolve_bound_capability(&disposed_reference)
                .await,
            Err(ChannelBindingError::Owner(BindingValidationError::Disposed))
        ));
    }

    #[tokio::test]
    async fn resolve_of_unregistered_source_rejects_old_reference() {
        let registry = Arc::new(ChannelRegistry::new());
        let tree = Arc::new(OwnershipTree::new());
        let reference = register(
            &registry,
            &tree,
            "stale-source",
            VisibilityScope::default(),
            ChannelInitialState::Cold,
            true,
            ResourceProbe::default(),
        )
        .await;
        registry
            .unregister_bound_channel(&crate::gateway::channel::ChannelId::new(&reference.id.name))
            .await
            .unwrap();
        assert!(matches!(
            registry.resolve_bound_capability(&reference).await,
            Err(ChannelBindingError::NotFound)
        ));
    }
}
