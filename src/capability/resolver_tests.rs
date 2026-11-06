//! Behavior tests for resolver retention, demand loading, and source freshness.

use super::*;
use crate::capability::cache_policy::{EvictionPolicy, ResolverConfig};
use crate::capability::descriptor::CapabilityRevision;
use crate::capability::facade::{
    CapabilityChangeStream, CapabilitySnapshot, Cursor, Projection, TransportTarget,
};
use crate::capability::ownership::{LifetimeScope, OwnerRef, OwnershipTree};
use crate::capability::provider_registry::ActivationError;
use crate::capability::zahir_facade::ZahirFacade;
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::service::ToolDefinition;
use std::cell::{Cell, RefCell};

// Delegate all answers to the real facade; counters observe materialization,
// not stand in for canonical registry/visibility/freshness behavior.
struct Observed {
    inner: Arc<ZahirFacade>,
    loads: Cell<usize>,
    descriptions: Cell<usize>,
    resolutions: Cell<usize>,
    before_load: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl Zahir for Observed {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot {
        self.descriptions.set(self.descriptions.get() + 1);
        self.inner.describe(scope)
    }
    fn lookup_descriptor(&self, reference: &Reference) -> Option<CapabilityDescriptor> {
        self.loads.set(self.loads.get() + 1);
        let callback = self.before_load.borrow_mut().take();
        if let Some(callback) = callback {
            callback();
        }
        self.inner.lookup_descriptor(reference)
    }
    fn descriptor_revision(&self, reference: &Reference) -> Option<CapabilityRevision> {
        self.inner.descriptor_revision(reference)
    }
    fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError> {
        self.resolutions.set(self.resolutions.get() + 1);
        self.inner.resolve(reference)
    }
    fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError> {
        self.inner.validate_lease(lease)
    }
    fn activate(&self, reference: Reference) -> Result<BackendLease, ActivationError> {
        self.inner.activate(reference)
    }
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream {
        self.inner.subscribe(scope, cursor)
    }
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection {
        self.inner.project(target, scope)
    }
}

// Zahir deliberately has no Send/Sync supertraits; reentrancy is tested locally.
#[allow(clippy::arc_with_non_send_sync)]
fn setup(
    config: ResolverConfig,
) -> (
    Arc<CapabilityResolver>,
    Arc<Observed>,
    ToolHandlerRegistry,
    Arc<OwnershipTree>,
) {
    let registry = ToolHandlerRegistry::new();
    for name in ["a", "b", "c"] {
        register(&registry, name, r#"{"type":"object"}"#);
    }
    let tree = Arc::new(OwnershipTree::new());
    let observed = Arc::new(Observed {
        inner: ZahirFacade::new(registry.clone(), tree.clone()),
        loads: Cell::new(0),
        descriptions: Cell::new(0),
        resolutions: Cell::new(0),
        before_load: RefCell::new(None),
    });
    (
        Arc::new(CapabilityResolver::with_config(observed.clone(), config)),
        observed,
        registry,
        tree,
    )
}

fn register(registry: &ToolHandlerRegistry, name: &str, schema: &str) {
    let definition = ToolDefinition {
        name: name.into(),
        description: name.into(),
        input_schema: serde_json::from_str(schema).unwrap(),
        source: crate::tools::service::ToolSource::Builtin,
        metadata: Default::default(),
    };
    registry
        .register(
            ToolCapabilityDescriptor::from_definition(&definition, 0),
            Arc::new(super::tests::FakeHandler { definition }),
        )
        .unwrap();
}

fn id(name: &str) -> CapabilityId {
    CapabilityId {
        namespace: "aleph/tools".into(),
        name: name.into(),
    }
}

fn lookup(resolver: &CapabilityResolver, name: &str) -> Option<CapabilityDescriptor> {
    resolver.lookup(&id(name), &VisibilityScope::default())
}

#[test]
fn cold_lookup_materializes_only_requested_descriptor_and_warm_reuses_it() {
    let (resolver, observed, _, tree) = setup(ResolverConfig::default());
    assert_eq!(observed.loads.get(), 0);
    assert_eq!(lookup(&resolver, "a").unwrap().id, id("a"));
    assert!(tree
        .generation(&id("a"), &VisibilityScope::default())
        .is_some());
    assert!(
        tree.generation(&id("b"), &VisibilityScope::default())
            .is_none(),
        "lazy lookup must not reconcile unrelated siblings"
    );
    assert_eq!(lookup(&resolver, "a").unwrap().id, id("a"));
    assert_eq!(observed.loads.get(), 1);
    assert_eq!(observed.descriptions.get(), 0, "no sibling enumeration");
    assert_eq!(
        observed.resolutions.get(),
        0,
        "metadata does not mint a lease"
    );
}

#[test]
fn lru_and_fifo_evict_their_real_victim_and_reload_it() {
    for (eviction, victim) in [(EvictionPolicy::Lru, "b"), (EvictionPolicy::Fifo, "a")] {
        let (resolver, observed, _, _) = setup(ResolverConfig {
            max_entries: 2,
            eviction,
            ..Default::default()
        });
        for name in ["a", "b", "a", "c"] {
            assert!(lookup(&resolver, name).is_some());
        }
        assert_eq!(observed.loads.get(), 3);
        assert!(lookup(&resolver, victim).is_some());
        assert_eq!(
            observed.loads.get(),
            4,
            "evicted victim was actually reloaded"
        );
    }
}

#[test]
fn zero_capacity_disables_retention_but_not_lookup() {
    let (resolver, observed, _, _) = setup(ResolverConfig {
        max_entries: 0,
        ..Default::default()
    });
    for _ in 0..3 {
        assert!(lookup(&resolver, "a").is_some());
    }
    assert_eq!(observed.loads.get(), 3);
}

#[test]
fn invalidation_forces_reload_and_does_not_retain_inflight_result() {
    for all in [false, true] {
        let (resolver, observed, _, _) = setup(ResolverConfig::default());
        let weak = Arc::downgrade(&resolver);
        *observed.before_load.borrow_mut() = Some(Box::new(move || {
            let resolver = weak.upgrade().unwrap();
            if all {
                resolver.invalidate_all();
            } else {
                resolver.invalidate(&id("a"));
            }
        }));
        assert!(lookup(&resolver, "a").is_some());
        assert!(lookup(&resolver, "a").is_some());
        assert_eq!(
            observed.loads.get(),
            2,
            "load crossing invalidation must not repopulate cache"
        );
        assert!(lookup(&resolver, "a").is_some());
        assert_eq!(
            observed.loads.get(),
            2,
            "post-invalidation reload is cacheable"
        );
    }
}

#[test]
fn visibility_entries_share_capacity_and_namespace_never_aliases() {
    let (resolver, observed, _, tree) = setup(ResolverConfig {
        max_entries: 1,
        ..Default::default()
    });
    let visibility = VisibilityScope {
        workspace: Some("private".into()),
        ..Default::default()
    };
    tree.register(
        id("a"),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        visibility.clone(),
    )
    .unwrap();
    assert!(lookup(&resolver, "a").is_some());
    assert_eq!(
        resolver.lookup(&id("a"), &visibility).unwrap().visibility,
        visibility
    );
    assert!(lookup(&resolver, "a").is_some());
    assert_eq!(
        observed.loads.get(),
        3,
        "visibility keys count against same capacity"
    );
    let foreign = CapabilityId {
        namespace: "foreign".into(),
        name: "a".into(),
    };
    assert!(resolver
        .lookup(&foreign, &VisibilityScope::default())
        .is_none());
    let absent = VisibilityScope {
        workspace: Some("absent".into()),
        ..Default::default()
    };
    assert!(resolver.lookup(&id("a"), &absent).is_none());
}

#[test]
fn cached_metadata_never_masks_revocation_generation_or_closure() {
    let (resolver, observed, registry, tree) = setup(ResolverConfig::default());
    let lease = resolver
        .resolve(Reference {
            id: id("a"),
            visibility: VisibilityScope::default(),
        })
        .unwrap();
    assert!(lookup(&resolver, "a").is_some());
    tree.bump(LifetimeScope::Runtime).unwrap();
    assert_eq!(
        resolver.validate_lease(&lease),
        Err(ResolveError::StaleOwner)
    );
    let fresh = resolver
        .resolve(Reference {
            id: id("a"),
            visibility: VisibilityScope::default(),
        })
        .unwrap();
    assert!(resolver.validate_lease(&fresh).is_ok());
    assert_ne!(lease.owner_generation, fresh.owner_generation);
    assert_eq!(observed.resolutions.get(), 2);
    assert!(tree.revoke(&id("a")));
    assert!(lookup(&resolver, "a").is_none());
    assert_eq!(
        resolver.resolve(Reference {
            id: id("a"),
            visibility: VisibilityScope::default()
        }),
        Err(ResolveError::Revoked)
    );
    assert!(lookup(&resolver, "b").is_some());
    registry.close();
    assert!(lookup(&resolver, "b").is_none());
}

#[test]
fn reentrant_loading_is_bounded_and_errors_release_capacity() {
    let (resolver, observed, _, _) = setup(ResolverConfig {
        max_concurrent_loads: 1,
        ..Default::default()
    });
    let weak = Arc::downgrade(&resolver);
    *observed.before_load.borrow_mut() = Some(Box::new(move || {
        let resolver = weak.upgrade().unwrap();
        assert!(lookup(&resolver, "b").is_none());
        assert!(resolver.discover(Scope::default()).is_empty());
        assert_eq!(
            resolver.resolve(Reference {
                id: id("b"),
                visibility: VisibilityScope::default()
            }),
            Err(ResolveError::Unknown)
        );
    }));
    assert!(lookup(&resolver, "a").is_some());
    assert_eq!(observed.loads.get(), 1, "saturation did not reach backend");
    assert!(lookup(&resolver, "b").is_some());
    assert!(lookup(&resolver, "missing").is_none());
    assert!(lookup(&resolver, "c").is_some(), "error released permit");
}

#[test]
fn unregister_and_reregister_cannot_reuse_old_metadata() {
    let (resolver, observed, registry, _) = setup(ResolverConfig::default());
    let before = lookup(&resolver, "a").unwrap();
    assert!(registry.unregister("a").is_some());
    assert!(lookup(&resolver, "a").is_none());
    register(&registry, "a", r#"{"type":"object","required":["new"]}"#);
    let after = lookup(&resolver, "a").unwrap();
    assert_ne!(before.revision, after.revision);
    assert_ne!(before.schema.fingerprint, after.schema.fingerprint);
    assert_eq!(observed.loads.get(), 3);
}

#[test]
fn repeated_resolve_replaces_one_cache_key_without_evicting_siblings() {
    let (resolver, observed, _, _) = setup(ResolverConfig {
        max_entries: 2,
        ..Default::default()
    });
    for name in ["a", "b", "a", "a"] {
        resolver
            .resolve(Reference {
                id: id(name),
                visibility: VisibilityScope::default(),
            })
            .unwrap();
    }
    assert!(lookup(&resolver, "b").is_some());
    assert_eq!(
        observed.loads.get(),
        0,
        "duplicate resolutions must not consume extra slots"
    );
    assert_eq!(
        observed.resolutions.get(),
        4,
        "every lease is resolved live"
    );
}

#[test]
fn unwinding_a_load_releases_its_work_permit() {
    let (resolver, observed, _, _) = setup(ResolverConfig {
        max_concurrent_loads: 1,
        ..Default::default()
    });
    *observed.before_load.borrow_mut() = Some(Box::new(|| panic!("controlled source panic")));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lookup(&resolver, "a")));
    assert!(result.is_err());
    assert!(
        lookup(&resolver, "b").is_some(),
        "unwind must not leak admission capacity"
    );
}

#[test]
fn stopped_tool_provider_hides_warm_metadata_without_notifications() {
    use crate::capability::provider::ProviderState;
    let (resolver, observed, _, _) = setup(ResolverConfig::default());
    assert!(lookup(&resolver, "a").is_some());
    let provider = observed.inner.provider();
    provider.transition(ProviderState::Stopping).unwrap();
    assert!(lookup(&resolver, "a").is_none());
    provider.transition(ProviderState::Stopped).unwrap();
    assert!(lookup(&resolver, "a").is_none());
    provider.transition(ProviderState::Starting).unwrap();
    provider.transition(ProviderState::Ready).unwrap();
    assert!(lookup(&resolver, "a").is_some());
    let fresh = resolver
        .resolve(Reference {
            id: id("a"),
            visibility: VisibilityScope::default(),
        })
        .unwrap();
    assert!(resolver.validate_lease(&fresh).is_ok());
}

#[test]
fn cold_generic_provider_metadata_does_not_activate_or_mint_a_lease() {
    use crate::capability::descriptor::{
        CapabilityFacets, CapabilityKind, ProjectionMetadata, SchemaRef,
    };
    use crate::capability::provider::{
        ProviderHandle, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
    };
    use crate::capability::provider_registry::{ProviderRegistration, ProviderRegistry};
    let provider = ProviderHandle::new(ProviderLifecycle::new(
        ProviderId::new("test/lazy").unwrap(),
        ProviderTransport::InProcess,
    ));
    provider.transition(ProviderState::Cold).unwrap();
    let capability = CapabilityId {
        namespace: "test/resources".into(),
        name: "cold".into(),
    };
    let providers = ProviderRegistry::new();
    providers
        .register(ProviderRegistration::new(
            CapabilityDescriptor {
                id: capability.clone(),
                kind: CapabilityKind::Resource,
                schema: SchemaRef {
                    version: 1,
                    fingerprint: [7; 32],
                },
                revision: CapabilityRevision(1),
                lifetime: LifetimeScope::Runtime,
                visibility: VisibilityScope::default(),
                owner: OwnerRef::Runtime,
                services: Vec::new(),
                metadata: ProjectionMetadata::default(),
                facets: CapabilityFacets::new(),
            },
            provider.clone(),
        ))
        .unwrap();
    let tree = Arc::new(OwnershipTree::new());
    let resolver = CapabilityResolver::new(ZahirFacade::new_with_providers(
        ToolHandlerRegistry::new(),
        tree.clone(),
        providers,
    ));
    for _ in 0..2 {
        assert_eq!(
            resolver
                .lookup(&capability, &VisibilityScope::default())
                .unwrap()
                .id,
            capability
        );
    }
    assert_eq!(provider.snapshot(), (ProviderState::Cold, 0));
    assert_eq!(
        resolver.resolve(Reference {
            id: capability.clone(),
            visibility: VisibilityScope::default()
        }),
        Err(ResolveError::NotVisible)
    );
    tree.register(
        capability.clone(),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        VisibilityScope::default(),
    )
    .unwrap();
    assert_eq!(
        resolver.resolve(Reference {
            id: capability,
            visibility: VisibilityScope::default()
        }),
        Err(ResolveError::StaleProvider)
    );
    assert_eq!(provider.snapshot(), (ProviderState::Cold, 0));
}

#[test]
fn zero_work_budget_never_starts_backend_work() {
    let (resolver, observed, _, _) = setup(ResolverConfig {
        max_concurrent_loads: 0,
        ..Default::default()
    });
    assert!(lookup(&resolver, "a").is_none());
    assert!(resolver.discover(Scope::default()).is_empty());
    assert_eq!(
        resolver.resolve(Reference {
            id: id("a"),
            visibility: VisibilityScope::default()
        }),
        Err(ResolveError::Unknown)
    );
    assert_eq!(
        (
            observed.loads.get(),
            observed.descriptions.get(),
            observed.resolutions.get()
        ),
        (0, 0, 0)
    );
}
