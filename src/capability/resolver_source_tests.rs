//! Deterministic source-provenance transitions through the real resolver.

use super::*;

#[test]
#[allow(clippy::arc_with_non_send_sync)] // Same local reentrancy contract as setup.
fn generic_fallback_cannot_poison_tool_metadata_after_provider_recovery() {
    for kind in [CapabilityKind::Resource, CapabilityKind::Tool] {
        for restore_inside_load in [false, true] {
            let registry = ToolHandlerRegistry::new();
            register(&registry, "shared", r#"{"type":"object"}"#);
            let providers = ProviderRegistry::new();
            let facade = ZahirFacade::new_with_providers(
                registry,
                Arc::new(OwnershipTree::new()),
                providers.clone(),
            );
            let reference = Reference {
                id: id("shared"),
                visibility: VisibilityScope::default(),
            };
            let canonical = facade.lookup_descriptor(&reference).unwrap();
            assert_eq!(canonical.kind, CapabilityKind::Tool);
            let (generic_provider, generic) = register_cold_descriptor(
                &providers,
                reference.id.clone(),
                kind,
                canonical.revision,
            );
            assert_ne!(canonical.schema.fingerprint, generic.schema.fingerprint);
            let old_lease = facade.resolve(reference.clone()).unwrap();
            let observed = Arc::new(Observed {
                inner: facade.clone(),
                loads: Cell::new(0),
                descriptions: Cell::new(0),
                resolutions: Cell::new(0),
                before_load: RefCell::new(None),
                after_load: RefCell::new(None),
            });
            let resolver = CapabilityResolver::new(observed.clone());
            let stopping = facade.provider();
            let during_stop = facade.clone();
            let stopped_reference = reference.clone();
            *observed.before_load.borrow_mut() = Some(Box::new(move || {
                stopping.transition(ProviderState::Stopping).unwrap();
                assert_eq!(
                    during_stop.resolve(stopped_reference),
                    Err(ResolveError::Unknown)
                );
            }));
            if restore_inside_load {
                let restoring = facade.provider();
                *observed.after_load.borrow_mut() = Some(Box::new(move || {
                    for state in [
                        ProviderState::Stopped,
                        ProviderState::Starting,
                        ProviderState::Ready,
                    ] {
                        restoring.transition(state).unwrap();
                    }
                }));
            }
            assert_eq!(
                facade.descriptor_revision(&reference),
                Some(canonical.revision)
            );
            assert_eq!(
                resolver.lookup(&reference.id, &reference.visibility),
                Some(generic)
            );
            assert_eq!(observed.loads.get(), 1);
            if !restore_inside_load {
                for state in [
                    ProviderState::Stopped,
                    ProviderState::Starting,
                    ProviderState::Ready,
                ] {
                    facade.provider().transition(state).unwrap();
                }
            }
            // No stopped-state lookup or invalidation may erase the poisoned key.
            assert_eq!(
                facade.descriptor_revision(&reference),
                Some(canonical.revision)
            );
            assert_eq!(
                facade.lookup_descriptor(&reference),
                Some(canonical.clone())
            );
            for _ in 0..2 {
                assert_eq!(resolver.lookup(&reference.id, &reference.visibility), Some(canonical.clone()),
                    "generic {kind:?}, restore_inside_load={restore_inside_load} must not shadow restored Tool");
            }
            assert_eq!(
                observed.loads.get(),
                2,
                "reload canonical once, then reuse it warm"
            );
            assert_eq!(generic_provider.snapshot(), (ProviderState::Cold, 0));
            assert_eq!(
                facade.validate_lease(&old_lease),
                Err(ResolveError::StaleProvider)
            );
            let fresh = resolver.resolve(reference).unwrap();
            assert_eq!(resolver.validate_lease(&fresh), Ok(()));
        }
    }
}
