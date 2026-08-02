//! Stage 2 acceptance tests — Tools Surface Unification.
//!
//! Covers: ≥2 integration (tool invocation end-to-end through harness),
//! ≥1 perf assertion (cache hit count), ≥1 property test (to_metadata_form
//! consistency with field-by-field manual conversion).

use crate::sync_primitives::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use proptest::prelude::*;
use serde_json::{json, Value};

use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tool_metadata::{ToolCategory, ToolDefinition as MetadataToolDefinition};
use crate::tools::service::{
    to_metadata_form, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};

// ===== Test scaffolding ============================================================

/// A `ToolService` impl that counts how many times `to_metadata_form`-equivalent
/// work was performed (i.e., cache misses), used by the perf assertion test.
struct CountingToolService {
    defs: Vec<ToolDefinition>,
    schema: arc_swap::ArcSwap<Option<Arc<[MetadataToolDefinition]>>>,
    miss_count: AtomicUsize,
}

impl CountingToolService {
    fn new(names: &[&str]) -> Self {
        let defs = names
            .iter()
            .map(|n| ToolDefinition {
                name: n.to_string(),
                description: format!("desc {n}"),
                input_schema: json!({"type": "object"}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            })
            .collect();
        Self {
            defs,
            schema: arc_swap::ArcSwap::from_pointee(None),
            miss_count: AtomicUsize::new(0),
        }
    }

    fn miss_count(&self) -> usize {
        self.miss_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ToolService for CountingToolService {
    async fn execute(&self, name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: json!({"echoed": name}),
            metadata: ToolOutputMetadata::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.defs.clone()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.defs.iter().find(|d| d.name == name).cloned()
    }

    fn metadata_schema(&self) -> Arc<[MetadataToolDefinition]> {
        if let Some(ref cached) = **self.schema.load() {
            return Arc::clone(cached);
        }
        self.miss_count.fetch_add(1, Ordering::SeqCst);
        let schema = to_metadata_form(&self.defs);
        self.schema.store(Arc::new(Some(Arc::clone(&schema))));
        schema
    }
}

// ===== Test 1: integration — first call populates schema =============================

#[test]
fn tool_service_first_metadata_schema_call_populates_arc() {
    let svc = CountingToolService::new(&["alpha", "beta"]);
    assert_eq!(svc.miss_count(), 0);
    let schema = svc.metadata_schema();
    assert_eq!(schema.len(), 2);
    assert_eq!(schema[0].name, "alpha");
    assert_eq!(svc.miss_count(), 1, "first call must be a cache miss");
}

// ===== Test 2: integration — repeat calls hit cache ==================================

#[test]
fn tool_service_repeat_metadata_schema_calls_share_arc() {
    let svc = CountingToolService::new(&["alpha", "beta"]);
    let s1 = svc.metadata_schema();
    let s2 = svc.metadata_schema();
    let s3 = svc.metadata_schema();
    assert!(Arc::ptr_eq(&s1, &s2));
    assert!(Arc::ptr_eq(&s2, &s3));
    assert_eq!(svc.miss_count(), 1, "only the first call should miss");
}

// ===== Test 3: perf assertion — 10 turns over stable registry == 1 conversion ========

#[test]
fn ten_turns_over_stable_registry_yield_exactly_one_cache_miss() {
    let svc = CountingToolService::new(&["a", "b", "c"]);
    let mut clones: Vec<Arc<[MetadataToolDefinition]>> = Vec::with_capacity(10);
    for _ in 0..10 {
        clones.push(svc.metadata_schema());
    }
    assert_eq!(
        svc.miss_count(),
        1,
        "Stage 2 acceptance: 10 turns with stable schema must yield 1 conversion, not 10"
    );
    // All 10 clones share the same underlying Arc.
    for c in &clones[1..] {
        assert!(Arc::ptr_eq(&clones[0], c));
    }
}

// ===== Test 4: property — to_metadata_form equals manual field-by-field map ========

prop_compose! {
    fn arb_loop_def()(
        name in "[a-z][a-z0-9_]{0,15}",
        desc in ".{0,40}",
    ) -> ToolDefinition {
        ToolDefinition {
            name,
            description: desc,
            input_schema: json!({"type": "object"}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn to_metadata_form_is_consistent_with_manual_conversion(
        defs in prop::collection::vec(arb_loop_def(), 0..8),
    ) {
        let auto = to_metadata_form(&defs);
        let manual: Vec<MetadataToolDefinition> = defs
            .iter()
            .map(|d| MetadataToolDefinition {
                name: d.name.clone(),
                description: d.description.clone(),
                parameters: d.input_schema.clone(),
                requires_confirmation: false,
                category: ToolCategory::Builtin,
                strict: false,
            })
            .collect();
        prop_assert_eq!(auto.len(), manual.len());
        for (a, m) in auto.iter().zip(manual.iter()) {
            prop_assert_eq!(&a.name, &m.name);
            prop_assert_eq!(&a.description, &m.description);
            prop_assert_eq!(&a.parameters, &m.parameters);
            prop_assert_eq!(a.requires_confirmation, m.requires_confirmation);
            prop_assert_eq!(a.strict, m.strict);
            prop_assert_eq!(matches!(a.category, ToolCategory::Builtin), true);
        }
    }
}
