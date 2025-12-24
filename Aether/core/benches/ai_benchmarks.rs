use aethecore::providers::mock::MockProvider;
use aethecore::providers::AiProvider;
/// Performance benchmarks for AI pipeline
///
/// Benchmarks:
/// - Routing performance (10 rules, 100 inputs)
/// - Memory retrieval + augmentation
/// - Full pipeline with mock provider
///
/// Run with: cargo bench --bench ai_benchmarks
use aethecore::{Config, ProviderConfig, Router, RoutingRuleConfig};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::time::Duration;

/// Create a config with multiple routing rules for testing
fn create_benchmark_config() -> Config {
    let mut config = Config::default();

    // Add mock provider
    config.providers.insert(
        "mock".to_string(),
        ProviderConfig {
            provider_type: Some("mock".to_string()),
            api_key: Some("test".to_string()),
            model: "test".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        },
    );

    // Add 10 routing rules to test matching performance
    for i in 0..10 {
        config.rules.push(RoutingRuleConfig {
            regex: format!("^/rule{}", i),
            provider: "mock".to_string(),
            system_prompt: Some(format!("System prompt {}", i)),
        });
    }

    // Add catch-all rule
    config.rules.push(RoutingRuleConfig {
        regex: ".*".to_string(),
        provider: "mock".to_string(),
        system_prompt: None,
    });

    config.general.default_provider = Some("mock".to_string());

    config
}

/// Benchmark routing performance with 10 rules
fn bench_routing_performance(c: &mut Criterion) {
    let config = create_benchmark_config();
    let router = Router::new(&config).unwrap();

    let mut group = c.benchmark_group("routing");

    // Benchmark first rule match (best case)
    group.bench_function("first_rule_match", |b| {
        b.iter(|| router.route(black_box("/rule0 test input")).unwrap());
    });

    // Benchmark middle rule match
    group.bench_function("middle_rule_match", |b| {
        b.iter(|| router.route(black_box("/rule5 test input")).unwrap());
    });

    // Benchmark last rule match (catch-all, worst case)
    group.bench_function("catch_all_match", |b| {
        b.iter(|| router.route(black_box("no match, use catch-all")).unwrap());
    });

    // Benchmark 100 diverse inputs
    group.bench_function("100_diverse_inputs", |b| {
        b.iter(|| {
            for i in 0..100 {
                let input = if i % 10 == 0 {
                    format!("/rule{} test", i / 10)
                } else {
                    format!("random input {}", i)
                };
                black_box(router.route(&input).unwrap());
            }
        });
    });

    group.finish();
}

/// Benchmark mock provider processing
fn bench_mock_provider(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let provider = MockProvider::new("Test response".to_string());

    let mut group = c.benchmark_group("mock_provider");

    group.bench_function("basic_processing", |b| {
        b.to_async(&runtime).iter(|| async {
            provider
                .process(black_box("Test input"), None)
                .await
                .unwrap()
        });
    });

    group.bench_function("with_system_prompt", |b| {
        b.to_async(&runtime).iter(|| async {
            provider
                .process(black_box("Test input"), Some("You are a helpful assistant"))
                .await
                .unwrap()
        });
    });

    group.finish();
}

/// Benchmark mock provider with delay (simulating network latency)
fn bench_mock_provider_with_delay(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("mock_provider_latency");
    group.measurement_time(Duration::from_secs(10)); // Longer measurement time for delays

    // Test different latencies
    for delay_ms in [10, 50, 100].iter() {
        let provider = MockProvider::new("Test response".to_string())
            .with_delay(Duration::from_millis(*delay_ms));

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}ms", delay_ms)),
            delay_ms,
            |b, _| {
                b.to_async(&runtime).iter(|| async {
                    provider
                        .process(black_box("Test input"), None)
                        .await
                        .unwrap()
                });
            },
        );
    }

    group.finish();
}

/// Benchmark config validation
fn bench_config_validation(c: &mut Criterion) {
    let config = create_benchmark_config();

    let mut group = c.benchmark_group("config");

    group.bench_function("validate", |b| {
        b.iter(|| black_box(config.validate().unwrap()));
    });

    group.bench_function("create_router", |b| {
        b.iter(|| black_box(Router::new(&config).unwrap()));
    });

    group.finish();
}

/// Benchmark full AI pipeline (routing + mock processing)
fn bench_full_pipeline(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let config = create_benchmark_config();
    let router = Router::new(&config).unwrap();

    let mut group = c.benchmark_group("full_pipeline");

    group.bench_function("route_and_process", |b| {
        b.to_async(&runtime).iter(|| async {
            let (provider, system_prompt) = router.route(black_box("Test input")).unwrap();
            provider
                .process(black_box("Test input"), system_prompt)
                .await
                .unwrap()
        });
    });

    group.bench_function("10_sequential_requests", |b| {
        b.to_async(&runtime).iter(|| async {
            for i in 0..10 {
                let input = format!("Request {}", i);
                let (provider, system_prompt) = router.route(&input).unwrap();
                black_box(provider.process(&input, system_prompt).await.unwrap());
            }
        });
    });

    group.finish();
}

/// Benchmark pattern matching performance
fn bench_regex_matching(c: &mut Criterion) {
    let config = create_benchmark_config();
    let router = Router::new(&config).unwrap();

    let mut group = c.benchmark_group("regex");

    // Test different input lengths
    for length in [10, 100, 1000].iter() {
        let input = "a".repeat(*length);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}chars", length)),
            length,
            |b, _| {
                b.iter(|| router.route(black_box(&input)).unwrap());
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_routing_performance,
    bench_mock_provider,
    bench_mock_provider_with_delay,
    bench_config_validation,
    bench_full_pipeline,
    bench_regex_matching
);

criterion_main!(benches);
