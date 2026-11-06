//! Capability discovery and resolution with visibility-scoped filtering.
//!
//! Metadata is demand-loaded and retained in one bounded cache. Live canonical
//! revisions establish freshness; optional invalidation events are only hints.

use crate::capability::cache_policy::{ResolverCache, ResolverConfig};
use crate::capability::descriptor::{CapabilityDescriptor, CapabilityId};
use crate::capability::facade::{BackendLease, Reference, ResolveError, Scope, Zahir};
use crate::capability::ownership::VisibilityScope;
use crate::sync_primitives::Arc;
use std::sync::Mutex;
use tokio::sync::Semaphore;

/// Capability resolver providing discovery and resolution with caching.
pub struct CapabilityResolver {
    zahir: Arc<dyn Zahir>,
    cache: Mutex<ResolverCache>,
    loads: Semaphore,
}

impl CapabilityResolver {
    /// Create a new resolver backed by the given Zahir facade.
    #[must_use]
    pub fn new(zahir: Arc<dyn Zahir>) -> Self {
        Self::with_config(zahir, ResolverConfig::default())
    }

    /// Configure entry retention and overlapping source work. Bounds belong
    /// to this resolver, not its provider. No waiting queue is allocated.
    #[must_use]
    pub fn with_config(zahir: Arc<dyn Zahir>, config: ResolverConfig) -> Self {
        Self {
            zahir,
            cache: Mutex::new(ResolverCache::new(config)),
            loads: Semaphore::new(usize::from(config.max_concurrent_loads)),
        }
    }

    /// Discover capabilities matching the given scope.
    ///
    /// Returns all capabilities visible under the scope's visibility envelope,
    /// filtered by kind, namespace, and facet if specified. Saturation returns
    /// an empty result without starting source work. Full snapshots are not cached.
    pub fn discover(&self, scope: Scope) -> Vec<CapabilityDescriptor> {
        let Ok(_permit) = self.loads.try_acquire() else {
            return Vec::new();
        };
        self.zahir.describe(scope).capabilities
    }

    /// Mint a fresh lease through Zahir, never through cached metadata.
    ///
    /// Saturation returns Unknown under the existing closed error contract.
    /// A successful lease may prime metadata retention, but callers must still
    /// validate the lease before use; generation/authorization are never cached.
    pub fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError> {
        let _permit = self
            .loads
            .try_acquire()
            .map_err(|_| ResolveError::Unknown)?;
        let epoch = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .epoch();
        let lease = self.zahir.resolve(reference)?;
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(lease.descriptor.clone(), &epoch);
        Ok(lease)
    }

    /// Validate a lease is still current.
    ///
    /// Checks the lease's owner generation against the current binding. Always
    /// delegates to Zahir; we never cache validation results because they must
    /// reflect live ownership state.
    pub fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError> {
        self.zahir.validate_lease(lease)
    }

    /// Invalidate cache entries for a specific capability ID.
    ///
    /// Call this after observing a capability change event for the ID. The next
    /// resolve will fetch fresh data from Zahir.
    pub fn invalidate(&self, id: &CapabilityId) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.invalidate_id(id);
    }

    /// Clear the entire cache.
    ///
    /// Call this after an ownership bump or other event that invalidates all
    /// cached descriptors.
    pub fn invalidate_all(&self) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.clear();
    }

    /// Look up a capability by ID without resolving a full lease.
    ///
    /// Reuses metadata only after a live source revision/visibility check.
    /// Otherwise loads the requested descriptor on demand, without activation.
    /// Saturation returns None. This method never grants invocation authority.
    pub fn lookup(
        &self,
        id: &CapabilityId,
        visibility: &VisibilityScope,
    ) -> Option<CapabilityDescriptor> {
        let _permit = self.loads.try_acquire().ok()?;
        crate::capability::lazy_load::lookup(
            self.zahir.as_ref(),
            &self.cache,
            &Reference {
                id: id.clone(),
                visibility: visibility.clone(),
            },
        )
    }
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod optimization_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::backend::TOOL_NAMESPACE;
    use crate::capability::descriptor::{CapabilityFacet, CapabilityKind};
    use crate::capability::ownership::{
        LifetimeScope, OwnerGeneration, OwnerRef, OwnershipTree, VisibilityScope,
    };
    use crate::capability::zahir_facade::ZahirFacade;
    use crate::session::events::ToolOutput;
    use crate::tools::descriptor::ToolCapabilityDescriptor;
    use crate::tools::handlers::ToolHandler;
    use crate::tools::registry::ToolHandlerRegistry;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::json;

    fn fake_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: format!("desc {name}"),
            input_schema: json!({"type": "object", "properties": {}}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    pub(super) struct FakeHandler {
        pub(super) definition: ToolDefinition,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({"tool": self.definition.name}),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            self.definition.clone()
        }
    }

    fn fake_handler(name: &str) -> FakeHandler {
        FakeHandler {
            definition: fake_def(name),
        }
    }

    fn tool_id(name: &str) -> CapabilityId {
        CapabilityId {
            namespace: TOOL_NAMESPACE.to_string(),
            name: name.to_string(),
        }
    }

    fn ref_for(id: &CapabilityId) -> Reference {
        Reference {
            id: id.clone(),
            visibility: VisibilityScope::default(),
        }
    }

    fn scope_all() -> Scope {
        Scope {
            kind: None,
            namespace: None,
            facet: None,
            visibility: VisibilityScope::default(),
        }
    }

    fn host_with_tools(tools: &[&str]) -> (Arc<ZahirFacade>, ToolHandlerRegistry) {
        let registry = ToolHandlerRegistry::new();
        for name in tools {
            registry
                .register(
                    ToolCapabilityDescriptor::from_definition(&fake_def(name), 0),
                    Arc::new(fake_handler(name)),
                )
                .expect("register test tool");
        }
        let tree = Arc::new(OwnershipTree::new());
        let facade = ZahirFacade::new(registry.clone(), tree);
        (facade, registry)
    }

    #[test]
    fn discover_returns_all_visible_capabilities_in_scope() {
        let (facade, _reg) = host_with_tools(&["discover_a", "discover_b"]);
        let resolver = CapabilityResolver::new(facade);

        let capabilities = resolver.discover(scope_all());

        assert_eq!(capabilities.len(), 2);
        assert!(capabilities.iter().any(|d| d.id.name == "discover_a"));
        assert!(capabilities.iter().any(|d| d.id.name == "discover_b"));
    }

    #[test]
    fn discover_filters_by_kind() {
        let (facade, _reg) = host_with_tools(&["kind_filter"]);
        let resolver = CapabilityResolver::new(facade);

        let tool_scope = Scope {
            kind: Some(CapabilityKind::Tool),
            ..scope_all()
        };
        let skill_scope = Scope {
            kind: Some(CapabilityKind::Skill),
            ..scope_all()
        };

        assert_eq!(resolver.discover(tool_scope).len(), 1);
        assert!(resolver.discover(skill_scope).is_empty());
    }

    #[test]
    fn discover_filters_by_namespace() {
        let (facade, _reg) = host_with_tools(&["ns_filter"]);
        let resolver = CapabilityResolver::new(facade);

        let tool_ns = Scope {
            namespace: Some(TOOL_NAMESPACE.to_string()),
            ..scope_all()
        };
        let foreign_ns = Scope {
            namespace: Some("aleph/skills".to_string()),
            ..scope_all()
        };

        assert_eq!(resolver.discover(tool_ns).len(), 1);
        assert!(resolver.discover(foreign_ns).is_empty());
    }

    #[test]
    fn discover_filters_by_facet() {
        let (facade, _reg) = host_with_tools(&["facet_filter"]);
        let resolver = CapabilityResolver::new(facade);

        let callable = Scope {
            facet: Some(CapabilityFacet::Callable),
            ..scope_all()
        };
        let events = Scope {
            facet: Some(CapabilityFacet::EventSource),
            ..scope_all()
        };

        assert_eq!(resolver.discover(callable).len(), 1);
        assert!(resolver.discover(events).is_empty());
    }

    #[test]
    fn resolve_returns_valid_lease_with_current_generation() {
        let (facade, _reg) = host_with_tools(&["resolve_me"]);
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("resolve_me");

        let lease = resolver.resolve(ref_for(&id)).expect("resolve succeeds");

        assert_eq!(lease.descriptor.id, id);
        assert_eq!(lease.owner_generation, OwnerGeneration(0));
        assert!(resolver.validate_lease(&lease).is_ok());
    }

    #[test]
    fn resolve_caches_descriptor_for_repeated_lookups() {
        let (facade, _reg) = host_with_tools(&["cached"]);
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("cached");

        // First resolve: cache miss, loads from Zahir.
        let lease1 = resolver.resolve(ref_for(&id)).expect("first resolve");

        // Second resolve: cache hit, but still gets fresh lease through Zahir.
        let lease2 = resolver.resolve(ref_for(&id)).expect("second resolve");

        // Both leases have the same descriptor but are independently resolved.
        assert_eq!(lease1.descriptor, lease2.descriptor);
        assert_eq!(lease1.owner_generation, lease2.owner_generation);
    }

    #[test]
    fn resolve_fails_for_unknown_id() {
        let (facade, _reg) = host_with_tools(&["exists"]);
        let resolver = CapabilityResolver::new(facade);
        let missing = tool_id("missing");

        assert_eq!(
            resolver.resolve(ref_for(&missing)),
            Err(ResolveError::Unknown)
        );
    }

    #[test]
    fn resolve_respects_visibility_scope() {
        let (facade, _reg) = host_with_tools(&["vis_check"]);
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("vis_check");

        // Default visibility succeeds.
        assert!(resolver.resolve(ref_for(&id)).is_ok());

        // Custom visibility with no binding fails.
        let custom_ref = Reference {
            id: id.clone(),
            visibility: VisibilityScope {
                workspace: Some("custom".to_string()),
                ..VisibilityScope::default()
            },
        };
        assert_eq!(resolver.resolve(custom_ref), Err(ResolveError::NotVisible));
    }

    #[test]
    fn validate_lease_detects_stale_generation() {
        let (_facade, _reg) = host_with_tools(&["stale"]);
        let tree = Arc::new(OwnershipTree::new());
        let facade = ZahirFacade::new(_reg, tree.clone());
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("stale");

        let lease = resolver.resolve(ref_for(&id)).expect("resolve succeeds");
        assert!(resolver.validate_lease(&lease).is_ok());

        // Bump ownership generation, making the lease stale.
        tree.bump(LifetimeScope::Runtime).expect("bump succeeds");

        assert_eq!(
            resolver.validate_lease(&lease),
            Err(ResolveError::StaleOwner)
        );
    }

    #[test]
    fn invalidate_clears_cached_descriptor_for_id() {
        let (facade, reg) = host_with_tools(&["invalidate_me"]);
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("invalidate_me");

        // Prime the cache.
        let _lease = resolver.resolve(ref_for(&id)).expect("resolve succeeds");

        // Verify cache hit by checking lookup returns immediately.
        let descriptor = resolver
            .lookup(&id, &VisibilityScope::default())
            .expect("cached lookup");
        assert_eq!(descriptor.id, id);

        // Invalidate the cached entry.
        resolver.invalidate(&id);

        // Replace the tool in the registry with a different schema.
        let mut new_def = fake_def("invalidate_me");
        new_def.input_schema = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        reg.replace(
            ToolCapabilityDescriptor::from_definition(&new_def, 0),
            Arc::new(FakeHandler {
                definition: new_def,
            }),
        )
        .expect("replace succeeds");

        // Next lookup should fetch the updated descriptor from Zahir.
        let fresh = resolver
            .lookup(&id, &VisibilityScope::default())
            .expect("fresh lookup");
        assert_ne!(descriptor.schema.fingerprint, fresh.schema.fingerprint);
    }

    #[test]
    fn lookup_refreshes_replaced_descriptor_without_notifications() {
        let (facade, registry) = host_with_tools(&["unobserved_replace"]);
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("unobserved_replace");
        let before = resolver.lookup(&id, &VisibilityScope::default()).unwrap();
        let mut definition = fake_def("unobserved_replace");
        definition.input_schema = json!({"type": "object", "required": ["changed"]});
        registry
            .replace(
                ToolCapabilityDescriptor::from_definition(&definition, 0),
                Arc::new(FakeHandler { definition }),
            )
            .unwrap();

        let after = resolver.lookup(&id, &VisibilityScope::default()).unwrap();
        assert_ne!(before.revision, after.revision);
        assert_ne!(before.schema.fingerprint, after.schema.fingerprint);
    }

    #[test]
    fn invalidate_all_clears_entire_cache() {
        let (facade, _reg) = host_with_tools(&["clear_a", "clear_b"]);
        let resolver = CapabilityResolver::new(facade);
        let id_a = tool_id("clear_a");
        let id_b = tool_id("clear_b");

        // Prime the cache with two entries.
        let _lease_a = resolver.resolve(ref_for(&id_a)).expect("resolve a");
        let _lease_b = resolver.resolve(ref_for(&id_b)).expect("resolve b");

        // Verify both are cached.
        assert!(resolver
            .lookup(&id_a, &VisibilityScope::default())
            .is_some());
        assert!(resolver
            .lookup(&id_b, &VisibilityScope::default())
            .is_some());

        // Clear the entire cache.
        resolver.invalidate_all();

        // Both entries should be gone (lookup will re-fetch from Zahir).
        let cache = resolver.cache.lock().unwrap();
        assert!(cache.descriptors.as_ref().unwrap().is_empty());
    }

    #[test]
    fn lookup_returns_descriptor_without_full_lease() {
        let (facade, _reg) = host_with_tools(&["lookup_only"]);
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("lookup_only");

        let descriptor = resolver
            .lookup(&id, &VisibilityScope::default())
            .expect("lookup succeeds");

        assert_eq!(descriptor.id, id);
        assert_eq!(descriptor.kind, CapabilityKind::Tool);
    }

    #[test]
    fn lookup_returns_none_for_unknown_id() {
        let (facade, _reg) = host_with_tools(&["exists"]);
        let resolver = CapabilityResolver::new(facade);
        let missing = tool_id("missing");

        assert!(resolver
            .lookup(&missing, &VisibilityScope::default())
            .is_none());
    }

    #[test]
    fn lookup_caches_discovered_descriptor() {
        let (facade, _reg) = host_with_tools(&["lookup_cached"]);
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("lookup_cached");

        // First lookup: cache miss, discovers through Zahir.
        let desc1 = resolver
            .lookup(&id, &VisibilityScope::default())
            .expect("first lookup");

        // Second lookup: cache hit.
        let desc2 = resolver
            .lookup(&id, &VisibilityScope::default())
            .expect("second lookup");

        assert_eq!(desc1, desc2);
    }

    #[test]
    fn cache_stores_multiple_visibility_scopes_per_id() {
        let (_facade, _reg) = host_with_tools(&["multi_vis"]);
        let tree = Arc::new(OwnershipTree::new());
        let facade = ZahirFacade::new(_reg, tree.clone());
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("multi_vis");

        let vis_a = VisibilityScope {
            workspace: Some("ws-a".to_string()),
            ..VisibilityScope::default()
        };
        let vis_b = VisibilityScope {
            workspace: Some("ws-b".to_string()),
            ..VisibilityScope::default()
        };

        // Register under both visibility scopes.
        tree.register(
            id.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis_a.clone(),
        )
        .expect("register vis_a");
        tree.register(
            id.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis_b.clone(),
        )
        .expect("register vis_b");

        // Lookup under vis_a.
        let desc_a = resolver.lookup(&id, &vis_a).expect("lookup vis_a");
        assert_eq!(desc_a.visibility, vis_a);

        // Lookup under vis_b.
        let desc_b = resolver.lookup(&id, &vis_b).expect("lookup vis_b");
        assert_eq!(desc_b.visibility, vis_b);

        // Both are cached independently.
        let cache = resolver.cache.lock().unwrap();
        let cached = cache.descriptors.as_ref().unwrap();
        assert_eq!(cached.iter().filter(|(key, _)| key.0 == id).count(), 2);
    }

    #[test]
    fn invalidate_removes_all_visibility_scopes_for_id() {
        let (_facade, _reg) = host_with_tools(&["inval_multi"]);
        let tree = Arc::new(OwnershipTree::new());
        let facade = ZahirFacade::new(_reg, tree.clone());
        let resolver = CapabilityResolver::new(facade);
        let id = tool_id("inval_multi");

        let vis_a = VisibilityScope {
            workspace: Some("ws-a".to_string()),
            ..VisibilityScope::default()
        };
        let vis_b = VisibilityScope {
            workspace: Some("ws-b".to_string()),
            ..VisibilityScope::default()
        };

        tree.register(
            id.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis_a.clone(),
        )
        .unwrap();
        tree.register(
            id.clone(),
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            vis_b.clone(),
        )
        .unwrap();

        // Prime cache with both scopes.
        let _desc_a = resolver.lookup(&id, &vis_a);
        let _desc_b = resolver.lookup(&id, &vis_b);

        // Invalidate the ID.
        resolver.invalidate(&id);

        // Both visibility entries should be gone.
        let cache = resolver.cache.lock().unwrap();
        assert!(!cache
            .descriptors
            .as_ref()
            .unwrap()
            .iter()
            .any(|(key, _)| key.0 == id));
    }
}
