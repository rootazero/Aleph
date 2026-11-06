//! Audit decoration must preserve the producing source's cache eligibility.
use super::*;

#[test]
#[allow(clippy::arc_with_non_send_sync)]
fn audited_resolver_preserves_source_coupled_cacheability_during_recovery() {
    use crate::capability::events::{AuditedZahir, CapabilityEventEmitter, InMemoryEventStream};
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
    let (_, generic) = register_cold_descriptor(
        &providers,
        reference.id.clone(),
        CapabilityKind::Tool,
        canonical.revision,
    );
    let observed = Arc::new(Observed {
        inner: facade.clone(),
        loads: Cell::new(0),
        descriptions: Cell::new(0),
        resolutions: Cell::new(0),
        before_load: RefCell::new(None),
        after_load: RefCell::new(None),
    });
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let audited = Arc::new(AuditedZahir::new(
        observed.clone(),
        OwnerRef::Runtime,
        CapabilityEventEmitter::new(stream),
    ));
    assert_eq!(
        audited.descriptor_revision(&reference),
        Some(canonical.revision)
    );
    assert_eq!(
        audited.lookup_descriptor(&reference),
        Some(canonical.clone())
    );
    assert_eq!(
        audited.lookup_descriptor_for_cache(&reference),
        Some((canonical.clone(), true))
    );
    observed.loads.set(0);
    let stopping = facade.provider();
    *observed.before_load.borrow_mut() = Some(Box::new(move || {
        stopping.transition(ProviderState::Stopping).unwrap();
    }));
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
    let resolver = CapabilityResolver::new(audited);
    assert_eq!(
        resolver.lookup(&reference.id, &reference.visibility),
        Some(generic)
    );
    assert_eq!(
        resolver.lookup(&reference.id, &reference.visibility),
        Some(canonical.clone()),
        "the fallback must not borrow the pre-load canonical revision's cacheability"
    );
    assert_eq!(
        resolver.lookup(&reference.id, &reference.visibility),
        Some(canonical)
    );
    assert_eq!(
        observed.loads.get(),
        2,
        "recover once, then reuse the canonical entry"
    );
    assert_eq!(observed.descriptions.get(), 0);
}
