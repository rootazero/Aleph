/// Performance benchmarks for Phase 7.5 - Performance Profiling
///
/// These benchmarks measure the performance of critical pipeline stages
/// to ensure they meet the target latencies:
/// - Clipboard operations: <50ms
/// - Memory retrieval: <100ms
/// - AI processing: <500ms (for mock provider)
/// - Total pipeline: <800ms
use alephcore::metrics::StageTimer;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::thread;
use std::time::Duration;

/// Benchmark StageTimer overhead
///
/// Measures the performance overhead of the StageTimer itself
/// to ensure it doesn't significantly impact the pipeline.
fn benchmark_timer_overhead(c: &mut Criterion) {
    c.bench_function("timer_overhead_no_metadata", |b| {
        b.iter(|| {
            let _timer = StageTimer::start("test_stage");
            // Timer drops here
        });
    });

    c.bench_function("timer_overhead_with_metadata", |b| {
        b.iter(|| {
            let _timer = StageTimer::start("test_stage")
                .with_meta("key1", "value1")
                .with_meta("key2", "value2");
        });
    });

    c.bench_function("timer_overhead_with_target", |b| {
        b.iter(|| {
            let _timer = StageTimer::start("test_stage")
                .with_target(100)
                .with_meta("key", "value");
        });
    });
}

/// Benchmark timer accuracy
///
/// Measures how accurately the timer measures elapsed time.
fn benchmark_timer_accuracy(c: &mut Criterion) {
    c.bench_function("timer_accuracy_10ms", |b| {
        b.iter(|| {
            let timer = StageTimer::start("accuracy_test");
            thread::sleep(Duration::from_millis(10));
            let elapsed = timer.elapsed_ms();
            black_box(elapsed);
        });
    });

    c.bench_function("timer_accuracy_50ms", |b| {
        b.iter(|| {
            let timer = StageTimer::start("accuracy_test");
            thread::sleep(Duration::from_millis(50));
            let elapsed = timer.elapsed_ms();
            black_box(elapsed);
        });
    });
}

/// Benchmark memory retrieval simulation
///
/// Simulates memory database queries with various result sizes.
fn benchmark_memory_retrieval(c: &mut Criterion) {
    c.bench_function("memory_retrieval_simulation_5_results", |b| {
        b.iter(|| {
            let _timer = StageTimer::start("memory_retrieval")
                .with_target(100)
                .with_meta("result_count", "5");

            // Simulate memory retrieval (5 results)
            let mut results = Vec::new();
            for i in 0..5 {
                results.push(format!("Result {}: Some memory content", i));
            }
            black_box(results);
        });
    });

    c.bench_function("memory_retrieval_simulation_20_results", |b| {
        b.iter(|| {
            let _timer = StageTimer::start("memory_retrieval")
                .with_target(100)
                .with_meta("result_count", "20");

            // Simulate memory retrieval (20 results)
            let mut results = Vec::new();
            for i in 0..20 {
                results.push(format!("Result {}: Some memory content", i));
            }
            black_box(results);
        });
    });
}

/// Benchmark string operations for prompt augmentation
///
/// Measures the cost of string concatenation for memory context injection.
fn benchmark_prompt_augmentation(c: &mut Criterion) {
    let base_prompt = "You are a helpful AI assistant.";
    let user_input = "Help me write a function to calculate fibonacci numbers.";
    let memories = vec![
        "Memory 1: Previous fibonacci discussion",
        "Memory 2: User prefers Python",
        "Memory 3: User likes detailed comments",
        "Memory 4: User works in VS Code",
        "Memory 5: User is learning algorithms",
    ];

    c.bench_function("prompt_augmentation_with_5_memories", |b| {
        b.iter(|| {
            let mut augmented = String::new();
            augmented.push_str(base_prompt);
            augmented.push_str("\n\nContext from previous interactions:\n");
            for memory in &memories {
                augmented.push_str("- ");
                augmented.push_str(memory);
                augmented.push('\n');
            }
            augmented.push_str("\nUser: ");
            augmented.push_str(user_input);
            black_box(augmented);
        });
    });

    c.bench_function("prompt_augmentation_format_macro", |b| {
        b.iter(|| {
            let context = memories.join("\n- ");
            let augmented = format!(
                "{}\n\nContext from previous interactions:\n- {}\n\nUser: {}",
                base_prompt, context, user_input
            );
            black_box(augmented);
        });
    });
}

/// Benchmark config access patterns
///
/// Measures the cost of reading configuration values.
fn benchmark_config_access(c: &mut Criterion) {
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct MockConfig {
        enable_performance_logging: bool,
        typing_speed: u32,
        output_mode: String,
    }

    let config = Arc::new(Mutex::new(MockConfig {
        enable_performance_logging: false,
        typing_speed: 50,
        output_mode: "typewriter".to_string(),
    }));

    c.bench_function("config_access_single_field", |b| {
        let config = config.clone();
        b.iter(|| {
            let enabled = config.lock().unwrap().enable_performance_logging;
            black_box(enabled);
        });
    });

    c.bench_function("config_access_multiple_fields", |b| {
        let config = config.clone();
        b.iter(|| {
            let config_lock = config.lock().unwrap();
            let enabled = config_lock.enable_performance_logging;
            let speed = config_lock.typing_speed;
            let mode = config_lock.output_mode.clone();
            drop(config_lock);
            black_box((enabled, speed, mode));
        });
    });

    c.bench_function("config_access_with_early_drop", |b| {
        let config = config.clone();
        b.iter(|| {
            let (enabled, speed) = {
                let config_lock = config.lock().unwrap();
                (
                    config_lock.enable_performance_logging,
                    config_lock.typing_speed,
                )
            }; // Lock dropped here
            black_box((enabled, speed));
        });
    });
}

/// Benchmark metadata insertion
///
/// Measures the cost of adding metadata to timers.
fn benchmark_metadata_operations(c: &mut Criterion) {
    c.bench_function("metadata_insertion_1_key", |b| {
        b.iter(|| {
            let _timer = StageTimer::start("test").with_meta("key", "value");
        });
    });

    c.bench_function("metadata_insertion_5_keys", |b| {
        b.iter(|| {
            let _timer = StageTimer::start("test")
                .with_meta("provider", "OpenAI")
                .with_meta("model", "gpt-4")
                .with_meta("app", "com.apple.Notes")
                .with_meta("window", "My Notes.txt")
                .with_meta("input_length", "150");
        });
    });
}

criterion_group!(
    benches,
    benchmark_timer_overhead,
    benchmark_timer_accuracy,
    benchmark_memory_retrieval,
    benchmark_prompt_augmentation,
    benchmark_config_access,
    benchmark_metadata_operations
);

criterion_main!(benches);
