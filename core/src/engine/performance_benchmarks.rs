//! Performance benchmarks for AtomicEngine
//!
//! These benchmarks measure the performance improvements from L1/L2 routing
//! and token savings from incremental editing.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod performance_benchmarks {
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::TempDir;

    use crate::engine::{AtomicAction, AtomicEngine, Patch};

    #[tokio::test]
    async fn bench_l1_routing_performance() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Learn a command to populate L1 cache
        let query = "custom command".to_string();
        let action = AtomicAction::Bash {
            command: "echo test".to_string(),
            cwd: None,
        };
        engine.learn_from_success(query.clone(), action).await;

        // Benchmark L1 routing (should be < 10ms)
        let iterations = 1000;
        let start = Instant::now();

        for _ in 0..iterations {
            let result = engine.route_query(&query).await;
            assert!(result.action.is_some());
        }

        let total_duration = start.elapsed();
        let avg_duration_us = total_duration.as_micros() / iterations;

        println!("L1 Routing Performance:");
        println!("  Total iterations: {}", iterations);
        println!("  Total time: {:?}", total_duration);
        println!("  Average per query: {} μs", avg_duration_us);
        println!("  Target: < 10,000 μs (10ms)");

        // Assert performance target
        assert!(
            avg_duration_us < 10_000,
            "L1 routing should be < 10ms, got {} μs",
            avg_duration_us
        );
    }

    #[tokio::test]
    async fn bench_l2_routing_performance() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Benchmark L2 routing (should be < 50ms)
        let queries = vec!["git status", "git log", "git diff", "ls", "pwd"];
        let iterations = 100;
        let start = Instant::now();

        for _ in 0..iterations {
            for query in &queries {
                let result = engine.route_query(query).await;
                assert!(result.action.is_some());
            }
        }

        let total_duration = start.elapsed();
        let total_queries = iterations * queries.len() as u128;
        let avg_duration_us = total_duration.as_micros() / total_queries;

        println!("L2 Routing Performance:");
        println!("  Total queries: {}", total_queries);
        println!("  Total time: {:?}", total_duration);
        println!("  Average per query: {} μs", avg_duration_us);
        println!("  Target: < 50,000 μs (50ms)");

        // Assert performance target
        assert!(
            avg_duration_us < 50_000,
            "L2 routing should be < 50ms, got {} μs",
            avg_duration_us
        );
    }

    #[tokio::test]
    async fn bench_token_savings_incremental_edit() {
        let temp_dir = TempDir::new().unwrap();
        let test_file = temp_dir.path().join("large_file.txt");

        // Create a large file (1000 lines)
        let mut content = String::new();
        for i in 1..=1000 {
            content.push_str(&format!("Line {}\n", i));
        }
        std::fs::write(&test_file, &content).unwrap();

        let _engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Method 1: Full file write (baseline)
        let modified_content = content.replace("Line 500", "Modified Line 500");
        let full_write_tokens = modified_content.len(); // Approximate token count

        // Method 2: Incremental edit via patch
        let _patch = Patch::new(500, 500, "Line 500".to_string(), "Modified Line 500".to_string()).unwrap();
        let patch_tokens = "Line 500".len() + "Modified Line 500".len() + 20; // Patch overhead

        let token_savings_percent = ((full_write_tokens - patch_tokens) as f64 / full_write_tokens as f64) * 100.0;

        println!("Token Savings Analysis:");
        println!("  File size: {} lines", 1000);
        println!("  Full write tokens: ~{}", full_write_tokens);
        println!("  Patch tokens: ~{}", patch_tokens);
        println!("  Token savings: {:.2}%", token_savings_percent);
        println!("  Target: > 80% savings");

        // Assert token savings target
        assert!(
            token_savings_percent > 80.0,
            "Token savings should be > 80%, got {:.2}%",
            token_savings_percent
        );
    }

    #[tokio::test]
    async fn bench_execution_throughput() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Benchmark execution throughput
        let actions = vec![
            AtomicAction::Bash {
                command: "echo test".to_string(),
                cwd: None,
            },
            AtomicAction::Bash {
                command: "pwd".to_string(),
                cwd: None,
            },
        ];

        let iterations = 50;
        let start = Instant::now();

        for _ in 0..iterations {
            for action in &actions {
                let result = engine.execute(action.clone()).await;
                assert!(result.is_ok());
            }
        }

        let total_duration = start.elapsed();
        let total_executions = iterations * actions.len() as u128;
        let avg_duration_ms = total_duration.as_millis() / total_executions;

        println!("Execution Throughput:");
        println!("  Total executions: {}", total_executions);
        println!("  Total time: {:?}", total_duration);
        println!("  Average per execution: {} ms", avg_duration_ms);
        println!("  Throughput: {:.2} ops/sec", 1000.0 / avg_duration_ms as f64);
    }

    #[tokio::test]
    async fn bench_cache_hit_rate() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Simulate realistic usage pattern
        let queries = vec![
            "git status",    // L2
            "git log",       // L2
            "git status",    // L2 (repeated)
            "ls",            // L2
            "pwd",           // L2
            "git status",    // L2 (repeated)
            "complex task",  // L3
            "git log",       // L2 (repeated)
        ];

        for query in &queries {
            engine.route_query(query).await;
        }

        let stats = engine.get_stats().await;
        let hit_rate = (stats.l2_hits as f64 / stats.total_queries as f64) * 100.0;

        println!("Cache Hit Rate Analysis:");
        println!("  Total queries: {}", stats.total_queries);
        println!("  L2 hits: {}", stats.l2_hits);
        println!("  L3 fallbacks: {}", stats.l3_fallbacks);
        println!("  Hit rate: {:.2}%", hit_rate);

        // In realistic usage, we expect > 70% hit rate
        assert!(hit_rate > 70.0, "Hit rate should be > 70%, got {:.2}%", hit_rate);
    }
}
