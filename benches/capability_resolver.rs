//! Real resolver workloads; no wall-clock pass/fail thresholds.
//! Run: cargo bench -p alephcore --bench capability_resolver -- --noplot
//! Fast development measurement: add `--profile dev` before `--`.

use alephcore::capability::cache_policy::{EvictionPolicy, ResolverConfig};
use alephcore::capability::descriptor::{CapabilityDescriptor, CapabilityId, CapabilityRevision};
use alephcore::capability::facade::{
    BackendLease, CapabilityChangeStream, CapabilitySnapshot, Cursor, Projection, Reference,
    ResolveError, Scope, TransportTarget, Zahir,
};
use alephcore::capability::ownership::{OwnershipTree, VisibilityScope};
use alephcore::capability::provider_registry::ActivationError;
use alephcore::capability::resolver::CapabilityResolver;
use alephcore::capability::zahir_facade::ZahirFacade;
use alephcore::session::events::ToolOutput;
use alephcore::tools::descriptor::ToolCapabilityDescriptor;
use alephcore::tools::handlers::ToolHandler;
use alephcore::tools::registry::ToolHandlerRegistry;
use alephcore::tools::service::{ToolDefinition, ToolError, ToolSource};
use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::json;
use std::{
    hint::black_box,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Arc,
    },
    time::Duration,
};

struct Handler(ToolDefinition);
#[async_trait]
impl ToolHandler for Handler {
    async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: json!(null),
            metadata: Default::default(),
        })
    }
    fn definition(&self) -> ToolDefinition {
        self.0.clone()
    }
}

struct Measured {
    facade: Arc<ZahirFacade>,
    loads: AtomicU64,
}
impl Zahir for Measured {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot {
        self.facade.describe(scope)
    }
    fn lookup_descriptor(&self, reference: &Reference) -> Option<CapabilityDescriptor> {
        self.lookup_descriptor_for_cache(reference)
            .map(|(descriptor, _)| descriptor)
    }
    fn lookup_descriptor_for_cache(
        &self,
        reference: &Reference,
    ) -> Option<(CapabilityDescriptor, bool)> {
        self.loads.fetch_add(1, Relaxed);
        self.facade.lookup_descriptor_for_cache(reference)
    }
    fn descriptor_revision(&self, reference: &Reference) -> Option<CapabilityRevision> {
        self.facade.descriptor_revision(reference)
    }
    fn resolve(&self, reference: Reference) -> Result<BackendLease, ResolveError> {
        self.facade.resolve(reference)
    }
    fn validate_lease(&self, lease: &BackendLease) -> Result<(), ResolveError> {
        self.facade.validate_lease(lease)
    }
    fn activate(&self, reference: Reference) -> Result<BackendLease, ActivationError> {
        self.facade.activate(reference)
    }
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream {
        self.facade.subscribe(scope, cursor)
    }
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection {
        self.facade.project(target, scope)
    }
}

fn fixture() -> Arc<Measured> {
    let registry = ToolHandlerRegistry::new();
    let properties: serde_json::Map<String, serde_json::Value> = (0..32)
        .map(|i| {
            (
                format!("field_{i}"),
                json!({"type":"string", "description":"benchmark input field"}),
            )
        })
        .collect();
    for index in 0..1024 {
        let definition = ToolDefinition {
            name: format!("tool_{index}"),
            description: "resolver benchmark".into(),
            input_schema: json!({"type":"object", "properties":properties}),
            source: ToolSource::Builtin,
            metadata: Default::default(),
        };
        registry
            .register(
                ToolCapabilityDescriptor::from_definition(&definition, 0),
                Arc::new(Handler(definition)),
            )
            .unwrap();
    }
    Arc::new(Measured {
        facade: ZahirFacade::new(registry, Arc::new(OwnershipTree::new())),
        loads: AtomicU64::new(0),
    })
}

fn benchmarks(c: &mut Criterion) {
    let source = fixture();
    let visibility = VisibilityScope::default();
    let ids: Vec<_> = (0..128)
        .map(|i| CapabilityId {
            namespace: "aleph/tools".into(),
            name: format!("tool_{i}"),
        })
        .collect();
    let resolver = CapabilityResolver::new(source.clone());
    let mut group = c.benchmark_group("capability_resolver_1024_tools_32_fields");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2));

    // Print deterministic materialization counts separately from timing samples.
    let before = source.loads.load(Relaxed);
    for _ in 0..100 {
        black_box(resolver.lookup(&ids[0], &visibility).unwrap());
    }
    println!(
        "warm workload: operations=100 materializations={} hits=99 misses=1",
        source.loads.load(Relaxed) - before
    );
    assert_eq!(source.loads.load(Relaxed) - before, 1);
    let before = source.loads.load(Relaxed);
    for _ in 0..100 {
        resolver.invalidate(&ids[0]);
        black_box(resolver.lookup(&ids[0], &visibility).unwrap());
    }
    println!(
        "cold workload: operations=100 materializations={} hits=0 misses=100",
        source.loads.load(Relaxed) - before
    );
    assert_eq!(source.loads.load(Relaxed) - before, 100);
    group.bench_function("warm_lookup", |b| {
        b.iter(|| black_box(resolver.lookup(black_box(&ids[0]), &visibility).unwrap()))
    });
    group.bench_function("invalidate_then_cold_lookup", |b| {
        b.iter(|| {
            resolver.invalidate(&ids[0]);
            black_box(resolver.lookup(&ids[0], &visibility).unwrap());
        })
    });
    group.bench_function("full_namespace_describe_reference", |b| {
        b.iter(|| {
            black_box(source.describe(Scope {
                namespace: Some("aleph/tools".into()),
                ..Default::default()
            }));
        })
    });
    group.bench_function("fresh_resolve", |b| {
        b.iter(|| {
            black_box(
                resolver
                    .resolve(Reference {
                        id: ids[0].clone(),
                        visibility: visibility.clone(),
                    })
                    .unwrap(),
            );
        })
    });
    for eviction in [EvictionPolicy::Lru, EvictionPolicy::Fifo] {
        let bounded = CapabilityResolver::with_config(
            source.clone(),
            ResolverConfig {
                max_entries: 64,
                eviction,
                ..Default::default()
            },
        );
        let before = source.loads.load(Relaxed);
        for index in 0..256 {
            black_box(bounded.lookup(&ids[index % 128], &visibility).unwrap());
        }
        let misses = source.loads.load(Relaxed) - before;
        println!("{eviction:?} thrash workload: capacity=64 working_set=128 operations=256 materializations={misses} hits={} misses={misses}", 256 - misses);
        assert_eq!(misses, 256);
        let mut index = 0;
        group.bench_function(format!("{eviction:?}_eviction_reload_128_over_64"), |b| {
            b.iter(|| {
                let id = &ids[index % 128];
                index += 1;
                black_box(bounded.lookup(id, &visibility).unwrap());
            })
        });
    }
    group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
