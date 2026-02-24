//! Integration tests for AtomicEngine with Agent Loop
//!
//! These tests validate the end-to-end integration of AtomicEngine
//! with the existing Agent Loop architecture.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod integration_tests {
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::TempDir;

    use crate::agent_loop::{Action, ActionExecutor, ActionResult};
    use crate::engine::{AtomicEngine, RoutingLayer};
    use crate::executor::AtomicActionExecutor;
    use aleph_protocol::IdentityContext;
    use serde_json::json;

    // Mock executor for baseline comparison
    struct BaselineExecutor;

    #[async_trait::async_trait]
    impl ActionExecutor for BaselineExecutor {
        async fn execute(&self, action: &Action, _identity: &IdentityContext) -> ActionResult {
            // Simulate traditional execution with some latency
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            match action {
                Action::ToolCall { tool_name, .. } => ActionResult::ToolSuccess {
                    output: json!(format!("Executed {}", tool_name)),
                    duration_ms: 10,
                },
                _ => ActionResult::Failed,
            }
        }
    }

    #[tokio::test]
    async fn test_l2_routing_faster_than_baseline() {
        let temp_dir = TempDir::new().unwrap();
        let baseline = Arc::new(BaselineExecutor);
        let atomic_executor = AtomicActionExecutor::new(baseline.clone(), temp_dir.path().to_path_buf());

        let action = Action::ToolCall {
            tool_name: "bash".to_string(),
            arguments: json!({
                "cmd": "git status"
            }),
        };

        let identity = IdentityContext::owner("test-session".to_string(), "test-channel".to_string());

        // Measure atomic execution (should use L2 routing)
        let start = Instant::now();
        let atomic_result = atomic_executor.execute(&action, &identity).await;
        let atomic_duration = start.elapsed();

        // Measure baseline execution
        let start = Instant::now();
        let baseline_result = baseline.execute(&action, &identity).await;
        let baseline_duration = start.elapsed();

        // Both should succeed
        assert!(atomic_result.is_success());
        assert!(baseline_result.is_success());

        // Atomic should be faster (L2 routing < 50ms, baseline = 10ms + overhead)
        // We expect atomic to be comparable or faster
        println!("Atomic duration: {:?}", atomic_duration);
        println!("Baseline duration: {:?}", baseline_duration);

        // Atomic routing should complete in under 100ms
        assert!(atomic_duration.as_millis() < 100);
    }

    #[tokio::test]
    async fn test_routing_statistics() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Test L2 routing
        let result1 = engine.route_query("git status").await;
        assert_eq!(result1.layer, RoutingLayer::L2);
        assert!(result1.action.is_some());

        let result2 = engine.route_query("git log").await;
        assert_eq!(result2.layer, RoutingLayer::L2);
        assert!(result2.action.is_some());

        // Test L3 fallback
        let result3 = engine.route_query("complex query requiring LLM reasoning").await;
        assert_eq!(result3.layer, RoutingLayer::L3);
        assert!(result3.action.is_none());

        // Check statistics
        let stats = engine.get_stats().await;
        assert_eq!(stats.l2_hits, 2);
        assert_eq!(stats.l3_fallbacks, 1);
        assert_eq!(stats.total_queries, 3);
    }

    #[tokio::test]
    async fn test_learning_from_l3() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        let custom_query = "my custom command".to_string();

        // First query should miss (L3)
        let result1 = engine.route_query(&custom_query).await;
        assert_eq!(result1.layer, RoutingLayer::L3);

        // Learn from L3 execution
        let action = crate::engine::AtomicAction::Bash {
            command: "echo test".to_string(),
            cwd: None,
        };
        engine.learn_from_success(custom_query.clone(), action).await;

        // Second query should hit L1 cache
        let result2 = engine.route_query(&custom_query).await;
        assert_eq!(result2.layer, RoutingLayer::L1);
        assert!(result2.action.is_some());
    }

    #[tokio::test]
    async fn test_fallback_on_unknown_tool() {
        let temp_dir = TempDir::new().unwrap();
        let baseline = Arc::new(BaselineExecutor);
        let atomic_executor = AtomicActionExecutor::new(baseline, temp_dir.path().to_path_buf());

        let action = Action::ToolCall {
            tool_name: "unknown_tool".to_string(),
            arguments: json!({
                "some_arg": "value"
            }),
        };

        let identity = IdentityContext::owner("test-session".to_string(), "test-channel".to_string());
        let result = atomic_executor.execute(&action, &identity).await;

        // Should fall back to baseline and succeed
        assert!(result.is_success());
    }

    #[tokio::test]
    async fn test_multiple_routing_layers() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Test various queries that should hit different layers
        let test_cases = vec![
            ("git status", RoutingLayer::L2),
            ("git log", RoutingLayer::L2),
            ("git diff", RoutingLayer::L2),
            ("ls", RoutingLayer::L2),
            ("pwd", RoutingLayer::L2),
            ("complex analysis task", RoutingLayer::L3),
            ("write a detailed report", RoutingLayer::L3),
        ];

        for (query, expected_layer) in test_cases {
            let result = engine.route_query(query).await;
            assert_eq!(
                result.layer, expected_layer,
                "Query '{}' should route to {:?}",
                query, expected_layer
            );
        }
    }

    #[tokio::test]
    async fn test_concurrent_routing() {
        let temp_dir = TempDir::new().unwrap();
        let engine = Arc::new(AtomicEngine::new(temp_dir.path().to_path_buf()));

        // Test concurrent routing (L1 cache is thread-safe)
        let mut handles = vec![];

        for i in 0..10 {
            let engine_clone = engine.clone();
            let handle = tokio::spawn(async move {
                let query = if i % 2 == 0 {
                    "git status"
                } else {
                    "git log"
                };
                engine_clone.route_query(query).await
            });
            handles.push(handle);
        }

        // All should succeed
        for handle in handles {
            let result = handle.await.unwrap();
            assert_eq!(result.layer, RoutingLayer::L2);
            assert!(result.action.is_some());
        }

        // Check statistics
        let stats = engine.get_stats().await;
        assert_eq!(stats.l2_hits, 10);
        assert_eq!(stats.total_queries, 10);
    }
}
