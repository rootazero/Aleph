//! Integration tests for AtomicEngine
//!
//! These tests validate the AtomicEngine routing logic.

#[cfg(test)]
#[allow(clippy::module_inception)]
mod integration_tests {
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    use crate::engine::{AtomicEngine, RoutingLayer};
    use serde_json::json;

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
