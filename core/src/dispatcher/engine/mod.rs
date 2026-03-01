//! AgentEngine - unified API for task orchestration
//!
//! Renamed from CoworkEngine to reflect the agent-centric architecture.

mod config;
mod constants;
mod core;
mod routing;

// Re-export all public items
pub use config::{AgentConfig, ExecutionState};
pub use constants::{
    DEFAULT_ALLOW_NETWORK, DEFAULT_CODE_EXEC_ENABLED, DEFAULT_CODE_EXEC_RUNTIME,
    DEFAULT_CODE_EXEC_TIMEOUT, DEFAULT_CONFIRMATION_TIMEOUT_SECS, DEFAULT_CONNECTION_TIMEOUT_SECS,
    DEFAULT_FILE_OPS_ENABLED, DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_RETRIES, DEFAULT_MAX_TOKENS,
    DEFAULT_PASS_ENV, DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE,
    DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE, DEFAULT_SANDBOX_ENABLED, MAX_PARALLELISM,
    MAX_STDERR_SIZE, MAX_STDOUT_SIZE, MAX_TASK_RETRIES, REQUIRE_CONFIRMATION,
};
pub use core::AgentEngine;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{AiTask, FileOp, Task, TaskGraph, TaskType};
    use crate::dispatcher::model_router::{
        Capability, CostTier, LatencyTier, ModelProfile, ModelRoutingRules,
    };
    use crate::dispatcher::planner::TaskPlanner;
    use crate::error::Result;
    use std::path::PathBuf;
    use crate::sync_primitives::{AtomicUsize, Ordering};
    use crate::sync_primitives::Arc;

    fn create_test_graph() -> TaskGraph {
        let mut graph = TaskGraph::new("test_graph", "Test Graph");

        graph.add_task(Task::new(
            "task_1",
            "Task 1",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        ));

        graph.add_task(Task::new(
            "task_2",
            "Task 2",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        ));

        graph.add_dependency("task_1", "task_2");

        graph
    }

    // Mock planner for testing
    struct MockPlanner;

    #[async_trait::async_trait]
    impl TaskPlanner for MockPlanner {
        async fn plan(&self, _request: &str) -> Result<TaskGraph> {
            Ok(create_test_graph())
        }

        fn name(&self) -> &str {
            "MockPlanner"
        }
    }

    #[tokio::test]
    async fn test_engine_execute() {
        // Create a mock provider (we won't use it for execution)
        let config = AgentConfig::default();

        // Create engine with mock planner
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let graph = create_test_graph();
        let summary = engine.execute(graph).await.unwrap();

        assert_eq!(summary.total_tasks, 2);
        assert_eq!(summary.completed_tasks, 2);
        assert_eq!(summary.failed_tasks, 0);
    }

    #[tokio::test]
    async fn test_engine_progress_events() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let event_count = Arc::new(AtomicUsize::new(0));
        let count_clone = event_count.clone();

        engine.subscribe(Arc::new(
            crate::dispatcher::monitor::CallbackSubscriber::new(move |_| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            }),
        ));

        let graph = create_test_graph();
        engine.execute(graph).await.unwrap();

        // Should have received multiple events (start, complete for each task, graph complete, progress updates)
        assert!(event_count.load(Ordering::SeqCst) >= 4);
    }

    #[tokio::test]
    async fn test_engine_cancel() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test cancel mechanism
        assert!(!engine.is_cancelled());
        engine.cancel();
        assert!(engine.is_cancelled());

        // Execute with pre-cancelled state (state gets reset in execute)
        let graph = create_test_graph();
        let summary = engine.execute(graph).await.unwrap();

        // execute() resets state, so tasks complete normally
        // This verifies the engine can be reused after cancel
        assert_eq!(summary.total_tasks, 2);
    }

    #[tokio::test]
    async fn test_engine_pause_resume() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test pause/resume mechanism
        assert!(!engine.is_paused());
        engine.pause();
        assert!(engine.is_paused());
        engine.resume();
        assert!(!engine.is_paused());
    }

    // =========================================================================
    // Model Routing Tests
    // =========================================================================

    fn create_test_profiles() -> Vec<ModelProfile> {
        vec![
            ModelProfile::new("claude-opus", "anthropic", "claude-opus-4")
                .with_capabilities(vec![Capability::Reasoning, Capability::CodeGeneration])
                .with_cost_tier(CostTier::High)
                .with_latency_tier(LatencyTier::Slow),
            ModelProfile::new("claude-sonnet", "anthropic", "claude-sonnet-4")
                .with_capabilities(vec![Capability::TextAnalysis, Capability::CodeGeneration])
                .with_cost_tier(CostTier::Medium)
                .with_latency_tier(LatencyTier::Medium),
            ModelProfile::new("claude-haiku", "anthropic", "claude-haiku-3")
                .with_capabilities(vec![Capability::TextAnalysis])
                .with_cost_tier(CostTier::Low)
                .with_latency_tier(LatencyTier::Fast),
        ]
    }

    fn create_routing_config() -> AgentConfig {
        let profiles = create_test_profiles();
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_task_type("code_generation", "claude-opus")
            .with_task_type("quick_tasks", "claude-haiku");

        AgentConfig::default().with_model_routing(profiles, rules)
    }

    #[test]
    fn test_engine_model_routing_enabled() {
        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        assert!(engine.has_model_routing());
        assert!(engine.model_matcher().is_some());
    }

    #[test]
    fn test_engine_model_routing_disabled() {
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        assert!(!engine.has_model_routing());
        assert!(engine.model_matcher().is_none());
    }

    #[test]
    fn test_engine_route_task() {
        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let task = Task::new(
            "t1",
            "Test Task",
            TaskType::AiInference(AiTask {
                prompt: "Test prompt".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let profile = engine.route_task(&task).unwrap();
        // Should route to default model (claude-sonnet)
        assert_eq!(profile.id, "claude-sonnet");
    }

    #[test]
    fn test_engine_model_profiles() {
        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let profiles = engine.model_profiles();
        assert_eq!(profiles.len(), 3);
    }

    #[tokio::test]
    async fn test_engine_execute_with_routing_fallback() {
        // When model routing is disabled, execute_with_routing should fall back to execute
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let graph = create_test_graph();
        let summary = engine.execute_with_routing(graph).await.unwrap();

        assert_eq!(summary.total_tasks, 2);
        assert_eq!(summary.completed_tasks, 2);
    }

    #[test]
    fn test_engine_route_by_intent() {
        use crate::dispatcher::model_router::TaskIntent;

        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test routing by intent
        let profile = engine
            .route_by_intent(&TaskIntent::CodeGeneration, None)
            .unwrap();
        assert_eq!(profile.id, "claude-opus");

        // Test routing with preferred model override
        let profile = engine
            .route_by_intent(&TaskIntent::CodeGeneration, Some("claude-haiku"))
            .unwrap();
        assert_eq!(profile.id, "claude-haiku");
    }

    #[test]
    fn test_engine_route_from_rule() {
        use crate::config::RoutingRuleConfig;

        let config = create_routing_config();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        // Test routing from a rule with intent_type
        let rule = RoutingRuleConfig::command("^/code", "anthropic", None)
            .with_intent_type("code_generation");
        let profile = engine.route_from_rule(&rule).unwrap();
        assert_eq!(profile.id, "claude-opus");

        // Test routing from a rule with preferred_model
        let rule = RoutingRuleConfig::command("^/quick", "anthropic", None)
            .with_intent_type("code_generation")
            .with_preferred_model("claude-haiku");
        let profile = engine.route_from_rule(&rule).unwrap();
        assert_eq!(profile.id, "claude-haiku");
    }

    #[test]
    fn test_engine_route_by_intent_disabled() {
        use crate::dispatcher::model_router::TaskIntent;

        // When model routing is disabled, should return error
        let config = AgentConfig::default();
        let engine = AgentEngine::with_planner(config, Arc::new(MockPlanner));

        let result = engine.route_by_intent(&TaskIntent::CodeGeneration, None);
        assert!(result.is_err());
    }
}
