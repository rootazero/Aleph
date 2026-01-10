/// Performance benchmarks for Intent Routing Pipeline
///
/// Benchmarks:
/// - Cache lookup time (<50ms target)
/// - L1 regex matching time (<100ms target)
/// - Full pipeline latency (<500ms target for cache miss)
///
/// Run with: cargo bench --bench pipeline_bench
use aethecore::dispatcher::{ToolSource, UnifiedTool};
use aethecore::routing::{
    CacheConfig, ClarificationConfig, ConfidenceThresholds, IntentAction, IntentCache,
    IntentRoutingPipeline, IntentSignal, L1RegexMatcher, LayerConfig, PipelineConfig,
    RoutingContext, RoutingLayerType,
};
use aethecore::semantic::{MatcherConfig, SemanticMatcher};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::Arc;
use std::time::Duration;

/// Create a test pipeline config
fn create_benchmark_config() -> PipelineConfig {
    PipelineConfig {
        enabled: true,
        cache: CacheConfig {
            enabled: true,
            max_size: 1000,
            ttl_seconds: 3600,
            decay_half_life_seconds: 600.0,
            cache_auto_execute_threshold: 0.85,
        },
        layers: LayerConfig::full(),
        confidence: ConfidenceThresholds::default(),
        tools: std::collections::HashMap::new(),
        clarification: ClarificationConfig::default(),
    }
}

/// Create a test pipeline
fn create_benchmark_pipeline() -> IntentRoutingPipeline {
    let config = create_benchmark_config();
    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    IntentRoutingPipeline::new(config, matcher)
}

/// Create test tools
fn create_test_tools() -> Vec<UnifiedTool> {
    vec![
        UnifiedTool::new("search", "search", "Search the web", ToolSource::Native),
        UnifiedTool::new("translate", "translate", "Translate text", ToolSource::Native),
        UnifiedTool::new("weather", "weather", "Get weather info", ToolSource::Native),
    ]
}

/// Benchmark IntentCache operations
fn bench_cache_operations(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("intent_cache");

    // Create cache
    let cache = IntentCache::new(CacheConfig {
        enabled: true,
        max_size: 1000,
        ttl_seconds: 3600,
        decay_half_life_seconds: 600.0,
        cache_auto_execute_threshold: 0.85,
    });

    // Benchmark cache lookup (miss)
    group.bench_function("cache_miss", |b| {
        b.to_async(&runtime).iter(|| async {
            cache.get(black_box("test input")).await
        });
    });

    // Populate cache for hit test
    runtime.block_on(async {
        cache
            .put(
                "cached input",
                "search",
                serde_json::json!({"query": "test"}),
                0.95,
                IntentAction::Execute,
            )
            .await;
    });

    // Benchmark cache lookup (hit)
    group.bench_function("cache_hit", |b| {
        b.to_async(&runtime).iter(|| async {
            cache.get(black_box("cached input")).await
        });
    });

    // Benchmark cache put
    group.bench_function("cache_put", |b| {
        b.to_async(&runtime).iter(|| async {
            cache
                .put(
                    black_box("new input"),
                    "tool",
                    serde_json::json!({}),
                    0.8,
                    IntentAction::Execute,
                )
                .await
        });
    });

    // Benchmark cache metrics
    group.bench_function("cache_metrics", |b| {
        b.to_async(&runtime).iter(|| async { cache.metrics().await });
    });

    group.finish();
}

/// Benchmark L1 regex matching
fn bench_l1_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("l1_regex");

    // Create L1 matcher with semantic matcher
    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    let l1_matcher = L1RegexMatcher::new(matcher);

    // Benchmark slash command match (best case)
    group.bench_function("slash_command_match", |b| {
        b.iter(|| {
            let ctx = RoutingContext::new("/search weather in Beijing");
            let _ = l1_matcher.match_input(black_box(&ctx));
        });
    });

    // Benchmark no match (plain text)
    group.bench_function("no_match", |b| {
        b.iter(|| {
            let ctx = RoutingContext::new("What is the weather today?");
            let _ = l1_matcher.match_input(black_box(&ctx));
        });
    });

    // Test with varying input lengths
    for length in [50, 200, 1000].iter() {
        let input = format!("/search {}", "a".repeat(*length));

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}chars", length)),
            length,
            |b, _| {
                b.iter(|| {
                    let ctx = RoutingContext::new(&input);
                    let _ = l1_matcher.match_input(black_box(&ctx));
                });
            },
        );
    }

    group.finish();
}

/// Benchmark full pipeline processing
fn bench_full_pipeline(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("pipeline");
    group.measurement_time(Duration::from_secs(10));

    // Create pipeline
    let pipeline = runtime.block_on(async {
        let p = create_benchmark_pipeline();
        p.update_tools(create_test_tools()).await;
        p
    });
    let pipeline = Arc::new(pipeline);

    // Benchmark L1 early exit (slash command)
    let p1 = Arc::clone(&pipeline);
    group.bench_function("l1_early_exit", |b| {
        b.to_async(&runtime).iter(|| async {
            let ctx = RoutingContext::new("/search weather");
            p1.process(black_box(ctx)).await
        });
    });

    // Benchmark full cascade (no L1 match, skip L3)
    let p2 = Arc::clone(&pipeline);
    group.bench_function("full_cascade_no_l3", |b| {
        b.to_async(&runtime).iter(|| async {
            let mut ctx = RoutingContext::new("What is the weather today?");
            ctx.skip_l3 = true; // Skip L3 for benchmark consistency
            p2.process(black_box(ctx)).await
        });
    });

    // Benchmark cache hit path
    let p3 = Arc::clone(&pipeline);
    runtime.block_on(async {
        // Warm up cache
        let ctx = RoutingContext::new("/search cached query");
        let _ = p3.process(ctx).await;
    });

    group.bench_function("cache_hit_path", |b| {
        b.to_async(&runtime).iter(|| async {
            let ctx = RoutingContext::new("/search cached query");
            p3.process(black_box(ctx)).await
        });
    });

    group.finish();
}

/// Benchmark concurrent pipeline access
fn bench_concurrent_access(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("pipeline_concurrent");
    group.measurement_time(Duration::from_secs(10));

    let pipeline = runtime.block_on(async {
        let p = create_benchmark_pipeline();
        p.update_tools(create_test_tools()).await;
        p
    });
    let pipeline = Arc::new(pipeline);

    // Benchmark concurrent requests
    for concurrency in [5, 10, 20].iter() {
        let p = Arc::clone(&pipeline);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}_concurrent", concurrency)),
            concurrency,
            |b, &n| {
                b.to_async(&runtime).iter(|| async {
                    let mut handles = vec![];
                    for i in 0..n {
                        let p_clone = Arc::clone(&p);
                        let input = format!("/search request {}", i);
                        handles.push(tokio::spawn(async move {
                            let mut ctx = RoutingContext::new(&input);
                            ctx.skip_l3 = true;
                            p_clone.process(ctx).await
                        }));
                    }
                    for handle in handles {
                        let _ = handle.await;
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark IntentSignal creation and manipulation
fn bench_intent_signal(c: &mut Criterion) {
    let mut group = c.benchmark_group("intent_signal");

    // Benchmark signal creation (no tool)
    group.bench_function("create_no_tool", |b| {
        b.iter(|| {
            IntentSignal::new(black_box(RoutingLayerType::L1Regex), 0.0)
        });
    });

    // Benchmark signal creation (with tool)
    group.bench_function("create_with_tool", |b| {
        let tool = UnifiedTool::new("search", "search", "Search", ToolSource::Native);

        b.iter(|| {
            IntentSignal::with_tool(black_box(RoutingLayerType::L1Regex), tool.clone(), 0.95)
        });
    });

    // Benchmark signal with parameters
    group.bench_function("with_parameters", |b| {
        let tool = UnifiedTool::new("search", "search", "Search", ToolSource::Native);
        let params = serde_json::json!({
            "query": "weather in Beijing",
            "location": "Beijing",
            "count": 10
        });

        b.iter(|| {
            IntentSignal::with_tool(RoutingLayerType::L1Regex, tool.clone(), 0.95)
                .with_parameters(black_box(params.clone()))
        });
    });

    group.finish();
}

/// Benchmark RoutingContext creation
fn bench_routing_context(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing_context");

    // Simple context
    group.bench_function("simple", |b| {
        b.iter(|| RoutingContext::new(black_box("test input")));
    });

    // Context with skip_l3
    group.bench_function("with_skip_l3", |b| {
        b.iter(|| {
            let mut ctx = RoutingContext::new(black_box("test input"));
            ctx.skip_l3 = true;
            ctx
        });
    });

    group.finish();
}

/// Benchmark confidence thresholds
fn bench_thresholds(c: &mut Criterion) {
    let mut group = c.benchmark_group("thresholds");

    let thresholds = ConfidenceThresholds::default();

    // Benchmark threshold checks
    group.bench_function("check_auto_execute", |b| {
        b.iter(|| black_box(0.95) >= thresholds.auto_execute);
    });

    group.bench_function("check_confirmation", |b| {
        b.iter(|| {
            let confidence = black_box(0.75);
            confidence >= thresholds.requires_confirmation && confidence < thresholds.auto_execute
        });
    });

    group.finish();
}

/// Benchmark cache metrics aggregation
fn bench_cache_metrics(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("cache_metrics_agg");

    // Create cache with entries
    let cache = IntentCache::new(CacheConfig {
        enabled: true,
        max_size: 1000,
        ttl_seconds: 3600,
        decay_half_life_seconds: 600.0,
        cache_auto_execute_threshold: 0.85,
    });

    // Populate cache
    runtime.block_on(async {
        for i in 0..100 {
            let input = format!("query {}", i);
            cache
                .put(&input, "tool", serde_json::json!({}), 0.8, IntentAction::Execute)
                .await;
        }
    });

    // Simulate some hits and misses
    runtime.block_on(async {
        for i in 0..50 {
            let input = format!("query {}", i);
            let _ = cache.get(&input).await;
        }
        for i in 100..150 {
            let input = format!("query {}", i);
            let _ = cache.get(&input).await;
        }
    });

    // Benchmark metrics retrieval
    group.bench_function("get_metrics", |b| {
        b.to_async(&runtime).iter(|| async { cache.metrics().await });
    });

    // Benchmark hit rate calculation
    group.bench_function("calculate_hit_rate", |b| {
        b.to_async(&runtime).iter(|| async {
            let metrics = cache.metrics().await;
            let total = metrics.hits + metrics.misses;
            if total > 0 {
                metrics.hits as f64 / total as f64
            } else {
                0.0
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_cache_operations,
    bench_l1_matching,
    bench_full_pipeline,
    bench_concurrent_access,
    bench_intent_signal,
    bench_routing_context,
    bench_thresholds,
    bench_cache_metrics,
);

criterion_main!(benches);
