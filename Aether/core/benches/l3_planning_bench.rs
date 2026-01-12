/// Performance benchmarks for L3 Task Planning
///
/// Benchmarks:
/// - QuickHeuristics detection (<10ms target)
/// - Multi-step pattern detection
///
/// Run with: cargo bench --bench l3_planning_bench
use aethecore::routing::QuickHeuristics;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::time::Duration;

/// Test inputs for heuristics benchmarks
const TEST_INPUTS: &[&str] = &[
    // Multi-step Chinese
    "搜索最新的AI新闻，然后总结主要内容",
    "先翻译这段文字，再发送给同事",
    "搜索天气预报，然后生成周报",
    // Multi-step English
    "Search for news and summarize it",
    "Find information then translate to Chinese",
    "Download the file and extract contents",
    // Single-step Chinese
    "搜索今天的天气",
    "翻译这段话",
    "帮我查一下",
    // Single-step English
    "What is the weather?",
    "Translate this",
    "Search for cats",
    // Conversational (no tools)
    "你好，今天怎么样？",
    "Hello, how are you?",
    "谢谢你的帮助",
];

/// Benchmark QuickHeuristics::analyze for various inputs
fn bench_heuristics_analyze(c: &mut Criterion) {
    let mut group = c.benchmark_group("quick_heuristics");
    group.measurement_time(Duration::from_secs(5));

    // Benchmark individual inputs
    for input in TEST_INPUTS {
        let id = if input.len() > 20 {
            format!("{}...", &input.chars().take(20).collect::<String>())
        } else {
            input.to_string()
        };
        group.bench_with_input(BenchmarkId::new("analyze", &id), input, |b, input| {
            b.iter(|| QuickHeuristics::analyze(black_box(input)));
        });
    }

    group.finish();
}

/// Benchmark QuickHeuristics::is_likely_multi_step for quick check
fn bench_heuristics_quick_check(c: &mut Criterion) {
    let mut group = c.benchmark_group("quick_heuristics");
    group.measurement_time(Duration::from_secs(5));

    // Multi-step input
    group.bench_function("is_multi_step_chinese", |b| {
        b.iter(|| {
            QuickHeuristics::is_likely_multi_step(black_box("搜索新闻，然后总结"))
        });
    });

    group.bench_function("is_multi_step_english", |b| {
        b.iter(|| {
            QuickHeuristics::is_likely_multi_step(black_box("Search and summarize news"))
        });
    });

    // Single-step input
    group.bench_function("is_single_step_chinese", |b| {
        b.iter(|| QuickHeuristics::is_likely_multi_step(black_box("搜索天气")));
    });

    group.bench_function("is_single_step_english", |b| {
        b.iter(|| QuickHeuristics::is_likely_multi_step(black_box("Search weather")));
    });

    group.finish();
}

/// Benchmark batch processing of multiple inputs
fn bench_heuristics_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("quick_heuristics_batch");

    group.bench_function("batch_15_inputs", |b| {
        b.iter(|| {
            for input in TEST_INPUTS {
                let _ = QuickHeuristics::analyze(black_box(input));
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_heuristics_analyze,
    bench_heuristics_quick_check,
    bench_heuristics_batch,
);
criterion_main!(benches);
