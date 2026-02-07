//! Performance benchmarks for reflex layer
//!
//! Validates that:
//! - L1 exact match: < 10ms
//! - L2 keyword routing: < 50ms

#[cfg(test)]
mod benches {
    use super::super::*;
    use std::time::Instant;

    #[test]
    fn bench_l1_exact_match() {
        let reflex = ReflexLayer::new();

        // Populate L1 cache with 100 entries
        for i in 0..100 {
            let input = format!("command_{}", i);
            let action = AtomicAction::Bash {
                command: input.clone(),
                cwd: None,
            };
            reflex.learn_from_success(&input, action);
        }

        // Benchmark L1 lookup
        let test_input = "command_50";
        let iterations = 1000;

        let start = Instant::now();
        for _ in 0..iterations {
            let _ = reflex.try_reflex(test_input);
        }
        let elapsed = start.elapsed();

        let avg_time_us = elapsed.as_micros() / iterations;
        let avg_time_ms = avg_time_us as f64 / 1000.0;

        println!("L1 exact match: {} iterations in {:?}", iterations, elapsed);
        println!("Average time per lookup: {:.3}ms ({} μs)", avg_time_ms, avg_time_us);

        // Verify < 10ms (actually should be < 1ms)
        assert!(
            avg_time_ms < 10.0,
            "L1 lookup too slow: {:.3}ms (target: < 10ms)",
            avg_time_ms
        );

        // In practice, should be much faster (< 0.1ms)
        assert!(
            avg_time_ms < 1.0,
            "L1 lookup slower than expected: {:.3}ms (expected: < 1ms)",
            avg_time_ms
        );
    }

    #[test]
    fn bench_l2_keyword_routing() {
        let reflex = ReflexLayer::with_default_rules();

        // Benchmark L2 keyword routing
        let test_inputs = vec![
            "read src/main.rs",
            "git status",
            "ls src/",
            "pwd",
            "cat config.toml",
        ];

        for test_input in &test_inputs {
            let iterations = 1000;

            let start = Instant::now();
            for _ in 0..iterations {
                let _ = reflex.try_reflex(test_input);
            }
            let elapsed = start.elapsed();

            let avg_time_us = elapsed.as_micros() / iterations;
            let avg_time_ms = avg_time_us as f64 / 1000.0;

            println!(
                "L2 routing for '{}': {} iterations in {:?}",
                test_input, iterations, elapsed
            );
            println!("Average time per lookup: {:.3}ms ({} μs)", avg_time_ms, avg_time_us);

            // Verify < 50ms
            assert!(
                avg_time_ms < 50.0,
                "L2 routing too slow for '{}': {:.3}ms (target: < 50ms)",
                test_input,
                avg_time_ms
            );

            // In practice, should be much faster (< 5ms)
            assert!(
                avg_time_ms < 5.0,
                "L2 routing slower than expected for '{}': {:.3}ms (expected: < 5ms)",
                test_input,
                avg_time_ms
            );
        }
    }

    #[test]
    fn bench_l3_fallback_detection() {
        let reflex = ReflexLayer::with_default_rules();

        // Benchmark L3 fallback detection (should be fast since it's just checking)
        let test_input = "analyze the codebase and find all bugs";
        let iterations = 1000;

        let start = Instant::now();
        for _ in 0..iterations {
            let _ = reflex.try_reflex(test_input);
        }
        let elapsed = start.elapsed();

        let avg_time_us = elapsed.as_micros() / iterations;
        let avg_time_ms = avg_time_us as f64 / 1000.0;

        println!("L3 fallback detection: {} iterations in {:?}", iterations, elapsed);
        println!("Average time per check: {:.3}ms ({} μs)", avg_time_ms, avg_time_us);

        // Fallback detection should be very fast (< 10ms)
        assert!(
            avg_time_ms < 10.0,
            "L3 fallback detection too slow: {:.3}ms (target: < 10ms)",
            avg_time_ms
        );
    }

    #[test]
    fn bench_learning() {
        let reflex = ReflexLayer::new();

        // Benchmark learning new patterns
        let iterations = 1000;

        let start = Instant::now();
        for i in 0..iterations {
            let input = format!("command_{}", i);
            let action = AtomicAction::Bash {
                command: input.clone(),
                cwd: None,
            };
            reflex.learn_from_success(&input, action);
        }
        let elapsed = start.elapsed();

        let avg_time_us = elapsed.as_micros() / iterations;
        let avg_time_ms = avg_time_us as f64 / 1000.0;

        println!("Learning: {} patterns in {:?}", iterations, elapsed);
        println!("Average time per learn: {:.3}ms ({} μs)", avg_time_ms, avg_time_us);

        // Learning should be fast (< 1ms)
        assert!(
            avg_time_ms < 1.0,
            "Learning too slow: {:.3}ms (target: < 1ms)",
            avg_time_ms
        );
    }

    #[test]
    fn bench_cache_size_impact() {
        // Test performance with different cache sizes
        let cache_sizes = vec![10, 100, 1000, 10000];

        for size in cache_sizes {
            let reflex = ReflexLayer::new();

            // Populate cache
            for i in 0..size {
                let input = format!("command_{}", i);
                let action = AtomicAction::Bash {
                    command: input.clone(),
                    cwd: None,
                };
                reflex.learn_from_success(&input, action);
            }

            // Benchmark lookup (worst case: last item)
            let test_input = format!("command_{}", size - 1);
            let iterations = 1000;

            let start = Instant::now();
            for _ in 0..iterations {
                let _ = reflex.try_reflex(&test_input);
            }
            let elapsed = start.elapsed();

            let avg_time_us = elapsed.as_micros() / iterations;
            let avg_time_ms = avg_time_us as f64 / 1000.0;

            println!(
                "Cache size {}: {} iterations in {:?}",
                size, iterations, elapsed
            );
            println!("Average time per lookup: {:.3}ms ({} μs)", avg_time_ms, avg_time_us);

            // Should still be fast even with large cache (DashMap is concurrent)
            assert!(
                avg_time_ms < 10.0,
                "Lookup too slow with cache size {}: {:.3}ms",
                size,
                avg_time_ms
            );
        }
    }
}
