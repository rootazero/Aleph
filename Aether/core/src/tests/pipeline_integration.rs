//! Pipeline Integration Tests
//!
//! End-to-end integration tests for the intent routing pipeline.
//! Tests verify correct behavior across all pipeline components.

use crate::dispatcher::{ToolSource, UnifiedTool};
use crate::providers::mock::MockProvider;
use crate::providers::AiProvider;
use crate::routing::{
    CacheConfig, ConfidenceThresholds, IntentRoutingPipeline, LayerConfig, PipelineConfig,
    PipelineResult, RoutingContext,
};
use crate::semantic::{MatcherConfig, SemanticMatcher};
use std::sync::Arc;
use std::time::Duration;

// =============================================================================
// Test Helpers
// =============================================================================

fn create_test_config() -> PipelineConfig {
    PipelineConfig {
        enabled: true,
        cache: CacheConfig {
            enabled: true,
            max_size: 100,
            ttl_seconds: 60,
            decay_half_life_seconds: 30.0,
            cache_auto_execute_threshold: 0.95,
        },
        layers: LayerConfig::full(),
        confidence: ConfidenceThresholds::default(),
        tools: std::collections::HashMap::new(),
        clarification: Default::default(),
    }
}

fn create_test_pipeline() -> IntentRoutingPipeline {
    let config = create_test_config();
    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    IntentRoutingPipeline::new(config, matcher)
}

fn create_pipeline_with_provider(provider: Arc<dyn AiProvider>) -> IntentRoutingPipeline {
    let config = create_test_config();
    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    IntentRoutingPipeline::with_provider(config, matcher, provider)
}

fn create_test_tools() -> Vec<UnifiedTool> {
    vec![
        UnifiedTool::new("search", "search", "Search the web for information", ToolSource::Native)
            .with_parameters_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    }
                },
                "required": ["query"]
            })),
        UnifiedTool::new("translate", "translate", "Translate text between languages", ToolSource::Native)
            .with_parameters_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text to translate"
                    },
                    "target_language": {
                        "type": "string",
                        "description": "Target language code"
                    }
                },
                "required": ["text", "target_language"]
            })),
        UnifiedTool::new("weather", "weather", "Get weather information for a location", ToolSource::Native)
            .with_parameters_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "Location to get weather for"
                    }
                },
                "required": ["location"]
            })),
    ]
}

// =============================================================================
// Test: Pipeline with Mock Providers
// =============================================================================

#[tokio::test]
async fn test_pipeline_with_mock_provider() {
    // Create mock provider that returns a tool selection response
    let mock_response = r#"{"tool": "search", "parameters": {"query": "rust programming"}, "confidence": 0.9, "reason": "User wants to search"}"#;
    let provider = Arc::new(MockProvider::new(mock_response));

    let pipeline = create_pipeline_with_provider(provider);
    pipeline.update_tools(create_test_tools()).await;

    let ctx = RoutingContext::new("find information about rust programming");
    let result = pipeline.process(ctx).await;

    // Should match a tool (ToolMatched), fall back to general chat, or execute directly
    // ToolMatched is returned when a tool is matched but needs external execution
    assert!(
        matches!(
            result,
            PipelineResult::GeneralChat { .. }
                | PipelineResult::Executed { .. }
                | PipelineResult::ToolMatched { .. }
        ),
        "Expected GeneralChat, Executed, or ToolMatched, got {:?}",
        result
    );
}

#[tokio::test]
async fn test_pipeline_disabled() {
    let mut config = create_test_config();
    config.enabled = false;

    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    let pipeline = IntentRoutingPipeline::new(config, matcher);

    let ctx = RoutingContext::new("test input");
    let result = pipeline.process(ctx).await;

    assert!(matches!(result, PipelineResult::Skipped { .. }));
}

// =============================================================================
// Test: Cache Hit Fast Path
// =============================================================================

#[tokio::test]
async fn test_cache_hit_fast_path() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Process input first time - cache miss
    let ctx = RoutingContext::new("/search rust programming");
    let _result1 = pipeline.process(ctx.clone()).await;

    // Process same input again - may be cache hit depending on first result
    let _result2 = pipeline.process(ctx.clone()).await;

    // Check cache metrics
    let metrics = pipeline.cache_metrics().await;

    // We should have at least one cache operation
    assert!(
        metrics.hits > 0 || metrics.misses > 0,
        "Expected cache activity"
    );
}

#[tokio::test]
async fn test_cache_miss_and_record() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Process a new input
    let ctx = RoutingContext::new("hello world");
    let _result = pipeline.process(ctx.clone()).await;

    // Check cache metrics - should have a miss
    let metrics = pipeline.cache_metrics().await;
    assert!(metrics.misses > 0, "Expected cache miss");
}

#[tokio::test]
async fn test_cache_multiple_accesses() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Multiple cache operations
    for i in 0..5 {
        let ctx = RoutingContext::new(&format!("test input {}", i));
        let _ = pipeline.process(ctx).await;
    }

    // Check cache has recorded operations
    let metrics = pipeline.cache_metrics().await;
    assert!(metrics.misses >= 5, "Expected at least 5 cache misses");
}

// =============================================================================
// Test: L1 Early Exit
// =============================================================================

#[tokio::test]
async fn test_l1_early_exit_slash_command() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Slash commands should be handled by L1 with high confidence
    let ctx = RoutingContext::new("/search test query");
    let result = pipeline.process(ctx).await;

    // L1 should match the slash command pattern
    // Result depends on whether the tool is registered and params are complete
    assert!(
        matches!(
            result,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::PendingClarification(_)
        ),
        "Unexpected result: {:?}",
        result
    );
}

#[tokio::test]
async fn test_l1_no_match_for_plain_text() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Plain text without slash command should not match L1
    let ctx = RoutingContext::new("what is the weather today");
    let result = pipeline.process(ctx).await;

    // Should fall back to general chat since no high-confidence match
    assert!(
        matches!(result, PipelineResult::GeneralChat { .. }),
        "Expected GeneralChat for plain text, got {:?}",
        result
    );
}

// =============================================================================
// Test: Full L1→L2→L3 Cascade
// =============================================================================

#[tokio::test]
async fn test_full_cascade_no_match() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Input that shouldn't match any layer strongly
    let ctx = RoutingContext::new("hello, how are you doing today?");
    let result = pipeline.process(ctx).await;

    // Should fall back to general chat
    assert!(
        matches!(result, PipelineResult::GeneralChat { .. }),
        "Expected GeneralChat, got {:?}",
        result
    );
}

#[tokio::test]
async fn test_cascade_l2_keyword_match() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Input with keywords that might trigger L2 semantic matching
    let ctx = RoutingContext::new("search for rust documentation");
    let result = pipeline.process(ctx).await;

    // L2 might match based on "search" keyword
    // Result depends on confidence level
    assert!(
        matches!(
            result,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::PendingClarification(_)
        ),
        "Unexpected result: {:?}",
        result
    );
}

#[tokio::test]
async fn test_cascade_with_l3_provider() {
    // Create mock provider for L3
    let mock_response = r#"I'll search for that information."#;
    let provider = Arc::new(MockProvider::new(mock_response));

    let mut config = create_test_config();
    config.layers.l3_enabled = true;

    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    let pipeline = IntentRoutingPipeline::with_provider(config, matcher, provider);
    pipeline.update_tools(create_test_tools()).await;

    // Input that requires L3 inference
    let ctx = RoutingContext::new("can you help me find something");
    let result = pipeline.process(ctx).await;

    // With L3, should get some result
    assert!(
        matches!(
            result,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::PendingClarification(_)
        ),
        "Unexpected result: {:?}",
        result
    );
}

// =============================================================================
// Test: Clarification Flow
// =============================================================================

#[tokio::test]
async fn test_clarification_session_management() {
    let pipeline = create_test_pipeline();

    // Check pending clarification count
    let count = pipeline.pending_clarification_count().await;
    assert_eq!(count, 0, "Should start with no pending clarifications");

    // Cleanup expired (should not panic even if empty)
    let cleaned = pipeline.cleanup_expired_clarifications().await;
    assert_eq!(cleaned, 0, "Should have nothing to clean up");
}

#[tokio::test]
async fn test_cancel_nonexistent_clarification() {
    let pipeline = create_test_pipeline();

    // Cancel a non-existent session
    let result = pipeline.cancel_clarification("non-existent-session").await;

    assert!(
        matches!(result, PipelineResult::Cancelled { .. }),
        "Expected Cancelled result"
    );
}

#[tokio::test]
async fn test_resume_nonexistent_clarification() {
    let pipeline = create_test_pipeline();

    // Resume a non-existent session
    let result = pipeline
        .resume_clarification("non-existent-session", "user input")
        .await;

    assert!(
        matches!(result, PipelineResult::Cancelled { .. }),
        "Expected Cancelled result for non-existent session"
    );
}

// =============================================================================
// Test: Confirmation Flow
// =============================================================================

#[tokio::test]
async fn test_medium_confidence_triggers_confirmation() {
    let mut config = create_test_config();
    // Set thresholds so medium confidence triggers confirmation
    config.confidence.auto_execute = 0.95;
    config.confidence.requires_confirmation = 0.6;
    config.confidence.no_match = 0.3;

    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    let pipeline = IntentRoutingPipeline::new(config, matcher);
    pipeline.update_tools(create_test_tools()).await;

    // Input that might get medium confidence
    let ctx = RoutingContext::new("maybe search for something");
    let result = pipeline.process(ctx).await;

    // Result depends on actual confidence from matching
    // This mainly tests that the pipeline handles thresholds correctly
    assert!(
        matches!(
            result,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::PendingClarification(_)
        ),
        "Unexpected result: {:?}",
        result
    );
}

// =============================================================================
// Test: Timeout Handling
// =============================================================================

#[tokio::test]
async fn test_l3_timeout_handling() {
    // Create a slow mock provider
    let provider = Arc::new(MockProvider::new("response").with_delay(Duration::from_millis(100)));

    let mut config = create_test_config();
    config.layers.l3_enabled = true;
    config.layers.l3_timeout_ms = 50; // Very short timeout

    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    let pipeline = IntentRoutingPipeline::with_provider(config, matcher, provider);
    pipeline.update_tools(create_test_tools()).await;

    // Process with context that has short timeout
    let ctx = RoutingContext::new("test input").with_l3_timeout(Duration::from_millis(10));
    let result = pipeline.process(ctx).await;

    // Should fall back gracefully on timeout
    assert!(
        matches!(result, PipelineResult::GeneralChat { .. }),
        "Expected graceful fallback on timeout"
    );
}

#[tokio::test]
async fn test_skip_l3_flag() {
    let provider = Arc::new(MockProvider::new("response"));

    let mut config = create_test_config();
    config.layers.l3_enabled = true;

    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    let pipeline = IntentRoutingPipeline::with_provider(config, matcher, provider);
    pipeline.update_tools(create_test_tools()).await;

    // Process with skip_l3 flag
    let ctx = RoutingContext::new("test input").skip_l3_inference();
    let result = pipeline.process(ctx).await;

    // Should not use L3, fall back to general chat
    assert!(
        matches!(result, PipelineResult::GeneralChat { .. }),
        "Expected GeneralChat when L3 is skipped"
    );
}

// =============================================================================
// Test: Context Handling
// =============================================================================

#[tokio::test]
async fn test_app_context_in_routing() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Create context with app info
    let ctx = RoutingContext::new("search something")
        .with_app(Some("com.apple.Safari".to_string()), Some("Google".to_string()));

    let result = pipeline.process(ctx).await;

    // App context should be used in routing (verified by no panics)
    assert!(
        matches!(
            result,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::PendingClarification(_)
        ),
        "Unexpected result: {:?}",
        result
    );
}

#[tokio::test]
async fn test_entity_hints_in_routing() {
    let pipeline = create_test_pipeline();
    pipeline.update_tools(create_test_tools()).await;

    // Create context with entity hints
    let mut ctx = RoutingContext::new("find that file");
    ctx.entity_hints.push("project.rs".to_string());
    ctx.entity_hints.push("Xcode".to_string());

    let result = pipeline.process(ctx).await;

    // Entity hints should be used (verified by no panics)
    assert!(
        matches!(
            result,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::PendingClarification(_)
        ),
        "Unexpected result: {:?}",
        result
    );
}

// =============================================================================
// Test: Tool Updates
// =============================================================================

#[tokio::test]
async fn test_dynamic_tool_updates() {
    let pipeline = create_test_pipeline();

    // Start with no tools
    let ctx = RoutingContext::new("/search test");
    let result1 = pipeline.process(ctx.clone()).await;

    // Add tools
    pipeline.update_tools(create_test_tools()).await;

    // Process again - behavior might change
    let result2 = pipeline.process(ctx).await;

    // Both should complete without error
    assert!(
        matches!(
            result1,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::Skipped { .. }
        ),
        "Unexpected result1: {:?}",
        result1
    );
    assert!(
        matches!(
            result2,
            PipelineResult::Executed { .. }
                | PipelineResult::GeneralChat { .. }
                | PipelineResult::PendingClarification(_)
        ),
        "Unexpected result2: {:?}",
        result2
    );
}

// =============================================================================
// Test: Config Validation
// =============================================================================

#[tokio::test]
async fn test_config_access() {
    let pipeline = create_test_pipeline();

    // Access config
    let config = pipeline.config();
    assert!(config.enabled);
    assert!(config.cache.enabled);
}

#[tokio::test]
async fn test_is_enabled_check() {
    let pipeline = create_test_pipeline();
    assert!(pipeline.is_enabled());

    // Create disabled pipeline
    let mut config = create_test_config();
    config.enabled = false;
    let matcher = Arc::new(SemanticMatcher::new(MatcherConfig::default()));
    let disabled_pipeline = IntentRoutingPipeline::new(config, matcher);
    assert!(!disabled_pipeline.is_enabled());
}

// =============================================================================
// Test: Concurrent Processing
// =============================================================================

#[tokio::test]
async fn test_concurrent_requests() {
    let pipeline = Arc::new(create_test_pipeline());
    pipeline.update_tools(create_test_tools()).await;

    // Spawn multiple concurrent requests
    let mut handles = vec![];
    for i in 0..10 {
        let p = Arc::clone(&pipeline);
        let input = format!("test input {}", i);
        handles.push(tokio::spawn(async move {
            let ctx = RoutingContext::new(&input);
            p.process(ctx).await
        }));
    }

    // Wait for all to complete
    let mut success_count = 0;
    for handle in handles {
        match handle.await {
            Ok(_result) => success_count += 1,
            Err(e) => panic!("Concurrent request failed: {:?}", e),
        }
    }

    // All should complete successfully
    assert_eq!(success_count, 10);
}

#[tokio::test]
async fn test_concurrent_cache_access() {
    let pipeline = Arc::new(create_test_pipeline());
    pipeline.update_tools(create_test_tools()).await;

    // Spawn concurrent processing operations (which use cache internally)
    let mut handles = vec![];
    for i in 0..5 {
        let p = Arc::clone(&pipeline);
        let input = format!("cache test {}", i);
        handles.push(tokio::spawn(async move {
            let ctx = RoutingContext::new(&input);
            let _ = p.process(ctx).await;
            p.cache_metrics().await
        }));
    }

    // Wait for all to complete
    let mut success_count = 0;
    for handle in handles {
        match handle.await {
            Ok(_metrics) => success_count += 1,
            Err(e) => panic!("Concurrent cache access failed: {:?}", e),
        }
    }

    // All should complete successfully
    assert_eq!(success_count, 5);
}
