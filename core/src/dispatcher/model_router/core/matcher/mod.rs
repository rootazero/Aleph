//! Model Matcher Implementation
//!
//! This module provides intelligent routing of tasks to optimal AI models
//! based on task characteristics, model capabilities, and cost preferences.

mod core;
mod routing;
mod types;

// Re-export all public types for backward compatibility
pub use self::core::ModelMatcher;
pub use routing::ModelRouter;
pub use types::{FallbackProvider, RoutingError};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{AiTask, CodeExec, FileOp, Language, Task, TaskType};
    use crate::dispatcher::model_router::core::{
        Capability, CostStrategy, CostTier, LatencyTier, ModelProfile, ModelRoutingRules,
        TaskIntent,
    };
    use std::path::PathBuf;

    fn create_test_profiles() -> Vec<ModelProfile> {
        vec![
            ModelProfile::new("claude-opus", "anthropic", "claude-opus-4")
                .with_capabilities(vec![
                    Capability::Reasoning,
                    Capability::CodeGeneration,
                    Capability::LongContext,
                ])
                .with_cost_tier(CostTier::High)
                .with_latency_tier(LatencyTier::Slow)
                .with_max_context(200_000),
            ModelProfile::new("claude-sonnet", "anthropic", "claude-sonnet-4")
                .with_capabilities(vec![
                    Capability::CodeGeneration,
                    Capability::CodeReview,
                    Capability::TextAnalysis,
                ])
                .with_cost_tier(CostTier::Medium)
                .with_latency_tier(LatencyTier::Medium),
            ModelProfile::new("claude-haiku", "anthropic", "claude-haiku")
                .with_capabilities(vec![Capability::FastResponse, Capability::SimpleTask])
                .with_cost_tier(CostTier::Low)
                .with_latency_tier(LatencyTier::Fast),
            ModelProfile::new("gpt-4o", "openai", "gpt-4o")
                .with_capabilities(vec![
                    Capability::ImageUnderstanding,
                    Capability::CodeGeneration,
                ])
                .with_cost_tier(CostTier::Medium)
                .with_latency_tier(LatencyTier::Medium),
            ModelProfile::new("ollama-llama", "ollama", "llama3.2")
                .with_capabilities(vec![Capability::LocalPrivacy, Capability::FastResponse])
                .with_cost_tier(CostTier::Free)
                .with_latency_tier(LatencyTier::Fast)
                .as_local(),
        ]
    }

    fn create_test_rules() -> ModelRoutingRules {
        ModelRoutingRules::new("claude-sonnet")
            .with_task_type("code_generation", "claude-opus")
            .with_task_type("code_review", "claude-sonnet")
            .with_task_type("image_analysis", "gpt-4o")
            .with_task_type("quick_tasks", "claude-haiku")
            .with_task_type("privacy_sensitive", "ollama-llama")
            .with_capability(Capability::ImageUnderstanding, "gpt-4o")
            .with_capability(Capability::LocalPrivacy, "ollama-llama")
            .with_cost_strategy(CostStrategy::Balanced)
    }

    fn create_matcher() -> ModelMatcher {
        ModelMatcher::new(create_test_profiles(), create_test_rules())
    }

    // =========================================================================
    // Basic Functionality Tests
    // =========================================================================

    #[test]
    fn test_matcher_creation() {
        let matcher = create_matcher();
        assert_eq!(matcher.profiles().len(), 5);
        assert!(matcher.get_profile("claude-opus").is_some());
        assert!(matcher.get_profile("nonexistent").is_none());
    }

    #[test]
    fn test_supports_capability() {
        let matcher = create_matcher();

        assert!(matcher.supports_capability("claude-opus", &Capability::Reasoning));
        assert!(matcher.supports_capability("gpt-4o", &Capability::ImageUnderstanding));
        assert!(!matcher.supports_capability("claude-haiku", &Capability::Reasoning));
        assert!(!matcher.supports_capability("nonexistent", &Capability::Reasoning));
    }

    // =========================================================================
    // Task Type Routing Tests
    // =========================================================================

    #[test]
    fn test_route_by_task_type_code_generation() {
        let matcher = create_matcher();

        let task = Task::new(
            "task_1",
            "Generate code",
            TaskType::CodeExecution(CodeExec::Script {
                code: "print('hello')".to_string(),
                language: Language::Python,
            }),
        );

        let profile = matcher.route(&task).unwrap();
        // CodeExecution maps to code_generation which maps to claude-opus
        assert_eq!(profile.id, "claude-opus");
    }

    #[test]
    fn test_route_ai_inference_default() {
        let matcher = create_matcher();

        let task = Task::new(
            "task_2",
            "AI task",
            TaskType::AiInference(AiTask {
                prompt: "Hello".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let profile = matcher.route(&task).unwrap();
        // Should use balanced/default model
        assert!(!profile.id.is_empty());
    }

    // =========================================================================
    // Model Preference Override Tests
    // =========================================================================

    #[test]
    fn test_explicit_model_preference() {
        let matcher = create_matcher();

        let task = Task::new(
            "task_3",
            "AI task",
            TaskType::AiInference(AiTask {
                prompt: "Hello".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_model("claude-haiku");

        let profile = matcher.route(&task).unwrap();
        assert_eq!(profile.id, "claude-haiku");
    }

    #[test]
    fn test_invalid_model_preference_fallback() {
        let matcher = create_matcher();

        let task = Task::new(
            "task_4",
            "AI task",
            TaskType::AiInference(AiTask {
                prompt: "Hello".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
        .with_model("nonexistent-model");

        // Should fall back to automatic routing
        let profile = matcher.route(&task).unwrap();
        assert!(!profile.id.is_empty());
    }

    // =========================================================================
    // Privacy Routing Tests
    // =========================================================================

    #[test]
    fn test_route_privacy_sensitive() {
        let matcher = create_matcher();

        let task = Task::new(
            "task_5",
            "Private task",
            TaskType::AiInference(AiTask {
                prompt: "Process my private data".to_string(),
                requires_privacy: true,
                has_images: false,
                output_format: None,
            }),
        );

        let profile = matcher.route(&task).unwrap();
        assert_eq!(profile.id, "ollama-llama");
        assert!(profile.local);
    }

    // =========================================================================
    // Image Routing Tests
    // =========================================================================

    #[test]
    fn test_route_image_task() {
        let matcher = create_matcher();

        let task = Task::new(
            "task_6",
            "Image analysis",
            TaskType::AiInference(AiTask {
                prompt: "Describe this image".to_string(),
                requires_privacy: false,
                has_images: true,
                output_format: None,
            }),
        );

        let profile = matcher.route(&task).unwrap();
        assert_eq!(profile.id, "gpt-4o");
        assert!(profile.has_capability(Capability::ImageUnderstanding));
    }

    // =========================================================================
    // Long Context Routing Tests
    // =========================================================================

    #[test]
    fn test_route_long_context() {
        let matcher = create_matcher();

        // Create a task with very long prompt
        let long_prompt = "x".repeat(60_000);
        let task = Task::new(
            "task_7",
            "Long document",
            TaskType::AiInference(AiTask {
                prompt: long_prompt,
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let profile = matcher.route(&task).unwrap();
        // Should route to a model with LongContext capability
        assert!(
            profile.has_capability(Capability::LongContext)
                || profile.max_context.is_some_and(|c| c >= 100_000)
        );
    }

    // =========================================================================
    // Capability-Based Routing Tests
    // =========================================================================

    #[test]
    fn test_find_best_for_capability() {
        let matcher = create_matcher();

        // Find best for ImageUnderstanding
        let profile = matcher
            .find_best_for(Capability::ImageUnderstanding)
            .unwrap();
        assert_eq!(profile.id, "gpt-4o");

        // Find best for LocalPrivacy
        let profile = matcher.find_best_for(Capability::LocalPrivacy).unwrap();
        assert_eq!(profile.id, "ollama-llama");

        // Find best for nonexistent capability mapping
        let profile = matcher.find_best_for(Capability::VideoUnderstanding);
        assert!(profile.is_none());
    }

    #[test]
    fn test_find_cheapest_with_capability() {
        let matcher = create_matcher();

        // Find cheapest with CodeGeneration
        let profile = matcher
            .find_cheapest_with(Capability::CodeGeneration)
            .unwrap();
        // claude-sonnet (Medium) is cheaper than claude-opus (High)
        assert_eq!(profile.id, "claude-sonnet");

        // Find cheapest with LocalPrivacy
        let profile = matcher
            .find_cheapest_with(Capability::LocalPrivacy)
            .unwrap();
        assert_eq!(profile.id, "ollama-llama");
        assert_eq!(profile.cost_tier, CostTier::Free);
    }

    #[test]
    fn test_find_balanced() {
        let matcher = create_matcher();

        let profile = matcher.find_balanced().unwrap();
        // Should prefer Medium cost and Fast/Medium latency
        assert!(
            profile.cost_tier <= CostTier::Medium || profile.latency_tier <= LatencyTier::Medium
        );
    }

    // =========================================================================
    // Cost Strategy Tests
    // =========================================================================

    #[test]
    fn test_cost_strategy_cheapest() {
        let profiles = create_test_profiles();
        let rules =
            ModelRoutingRules::new("claude-sonnet").with_cost_strategy(CostStrategy::Cheapest);
        let matcher = ModelMatcher::new(profiles, rules);

        let task = Task::new(
            "task_8",
            "Simple task",
            TaskType::AiInference(AiTask {
                prompt: "Hello".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let profile = matcher.route(&task).unwrap();
        // Should prefer cheapest model
        assert!(profile.cost_tier <= CostTier::Low);
    }

    #[test]
    fn test_cost_strategy_best_quality() {
        let profiles = create_test_profiles();
        let rules =
            ModelRoutingRules::new("claude-sonnet").with_cost_strategy(CostStrategy::BestQuality);
        let matcher = ModelMatcher::new(profiles, rules);

        let profile = matcher.find_balanced().unwrap();
        // Should prefer highest quality (highest cost)
        assert_eq!(profile.cost_tier, CostTier::High);
    }

    // =========================================================================
    // Default Model Fallback Tests
    // =========================================================================

    #[test]
    fn test_default_model_fallback() {
        let profiles = create_test_profiles();
        let rules = ModelRoutingRules::new("claude-haiku"); // Set haiku as default
        let matcher = ModelMatcher::new(profiles, rules);

        let task = Task::new(
            "task_9",
            "Generic task",
            TaskType::FileOperation(FileOp::List {
                path: PathBuf::from("/tmp"),
            }),
        );

        let profile = matcher.route(&task).unwrap();
        // Should use balanced or default
        assert!(!profile.id.is_empty());
    }

    // =========================================================================
    // Empty/Edge Case Tests
    // =========================================================================

    #[test]
    fn test_empty_profiles() {
        let rules = ModelRoutingRules::default();
        let matcher = ModelMatcher::new(vec![], rules);

        let task = Task::new(
            "task_10",
            "Test",
            TaskType::AiInference(AiTask {
                prompt: "Hello".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let result = matcher.route(&task);
        assert!(result.is_err());
    }

    #[test]
    fn test_single_profile() {
        let profiles = vec![ModelProfile::new("only-model", "test", "test-model")
            .with_capabilities(vec![Capability::TextAnalysis])
            .with_cost_tier(CostTier::Medium)];
        let rules = ModelRoutingRules::new("only-model");
        let matcher = ModelMatcher::new(profiles, rules);

        let task = Task::new(
            "task_11",
            "Test",
            TaskType::AiInference(AiTask {
                prompt: "Hello".to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        );

        let profile = matcher.route(&task).unwrap();
        assert_eq!(profile.id, "only-model");
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[test]
    fn test_full_routing_flow() {
        let matcher = create_matcher();

        // Test various task types
        let tasks = [
            // Code task -> claude-opus
            Task::new(
                "t1",
                "Code",
                TaskType::CodeExecution(CodeExec::Script {
                    code: "print(1)".to_string(),
                    language: Language::Python,
                }),
            ),
            // Privacy task -> ollama-llama
            Task::new(
                "t2",
                "Private",
                TaskType::AiInference(AiTask {
                    prompt: "secret".to_string(),
                    requires_privacy: true,
                    has_images: false,
                    output_format: None,
                }),
            ),
            // Image task -> gpt-4o
            Task::new(
                "t3",
                "Image",
                TaskType::AiInference(AiTask {
                    prompt: "describe".to_string(),
                    requires_privacy: false,
                    has_images: true,
                    output_format: None,
                }),
            ),
        ];

        let expected_models = ["claude-opus", "ollama-llama", "gpt-4o"];

        for (task, expected) in tasks.iter().zip(expected_models.iter()) {
            let profile = matcher.route(task).unwrap();
            assert_eq!(profile.id, *expected, "Task {} routed incorrectly", task.id);
        }
    }

    // =========================================================================
    // TaskIntent-Based Routing Tests
    // =========================================================================

    #[test]
    fn test_route_by_intent_code_generation() {
        let matcher = create_matcher();

        // CodeGeneration intent should route to claude-opus (per task_type mapping)
        let profile = matcher
            .route_by_intent(&TaskIntent::CodeGeneration)
            .unwrap();
        assert_eq!(profile.id, "claude-opus");
    }

    #[test]
    fn test_route_by_intent_code_review() {
        let matcher = create_matcher();

        // CodeReview intent should route to claude-sonnet (per task_type mapping)
        let profile = matcher.route_by_intent(&TaskIntent::CodeReview).unwrap();
        assert_eq!(profile.id, "claude-sonnet");
    }

    #[test]
    fn test_route_by_intent_image_analysis() {
        let matcher = create_matcher();

        // ImageAnalysis intent should route to gpt-4o (per task_type mapping)
        let profile = matcher.route_by_intent(&TaskIntent::ImageAnalysis).unwrap();
        assert_eq!(profile.id, "gpt-4o");
    }

    #[test]
    fn test_route_by_intent_quick_task() {
        let matcher = create_matcher();

        // QuickTask intent should route to claude-haiku (per task_type mapping)
        let profile = matcher.route_by_intent(&TaskIntent::QuickTask).unwrap();
        assert_eq!(profile.id, "claude-haiku");
    }

    #[test]
    fn test_route_by_intent_privacy_sensitive() {
        let matcher = create_matcher();

        // PrivacySensitive intent should route to ollama-llama (per task_type mapping)
        let profile = matcher
            .route_by_intent(&TaskIntent::PrivacySensitive)
            .unwrap();
        assert_eq!(profile.id, "ollama-llama");
    }

    #[test]
    fn test_route_by_intent_general_chat() {
        let matcher = create_matcher();

        // GeneralChat intent should fall back to default model
        let profile = matcher.route_by_intent(&TaskIntent::GeneralChat).unwrap();
        // Default is claude-sonnet in our test setup
        assert_eq!(profile.id, "claude-sonnet");
    }

    #[test]
    fn test_route_by_intent_capability_fallback() {
        // Test that when no task_type mapping exists, it falls back to capability
        let profiles = create_test_profiles();
        // Create rules without task_type mappings, only capability mappings
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_capability(Capability::ImageUnderstanding, "gpt-4o")
            .with_capability(Capability::LocalPrivacy, "ollama-llama");
        let matcher = ModelMatcher::new(profiles, rules);

        // ImageAnalysis requires ImageUnderstanding capability
        let profile = matcher.route_by_intent(&TaskIntent::ImageAnalysis).unwrap();
        assert_eq!(profile.id, "gpt-4o");

        // PrivacySensitive requires LocalPrivacy capability
        let profile = matcher
            .route_by_intent(&TaskIntent::PrivacySensitive)
            .unwrap();
        assert_eq!(profile.id, "ollama-llama");
    }

    #[test]
    fn test_route_by_intent_with_preference_override() {
        let matcher = create_matcher();

        // Even though CodeGeneration maps to claude-opus, explicit preference should win
        let profile = matcher
            .route_by_intent_with_preference(&TaskIntent::CodeGeneration, Some("claude-haiku"))
            .unwrap();
        assert_eq!(profile.id, "claude-haiku");
    }

    #[test]
    fn test_route_by_intent_with_preference_invalid_fallback() {
        let matcher = create_matcher();

        // Invalid preference should fall back to intent-based routing
        let profile = matcher
            .route_by_intent_with_preference(&TaskIntent::CodeGeneration, Some("nonexistent-model"))
            .unwrap();
        // Falls back to task_type mapping (claude-opus)
        assert_eq!(profile.id, "claude-opus");
    }

    #[test]
    fn test_route_by_intent_with_preference_none() {
        let matcher = create_matcher();

        // None preference should behave like route_by_intent
        let profile = matcher
            .route_by_intent_with_preference(&TaskIntent::ImageAnalysis, None)
            .unwrap();
        assert_eq!(profile.id, "gpt-4o");
    }

    #[test]
    fn test_route_by_intent_skills() {
        let matcher = create_matcher();

        // Skills intent has no required capability, should fall back to default
        let profile = matcher
            .route_by_intent(&TaskIntent::Skills("pdf".to_string()))
            .unwrap();
        assert_eq!(profile.id, "claude-sonnet"); // default
    }

    #[test]
    fn test_route_by_intent_custom() {
        let matcher = create_matcher();

        // Custom intent has no required capability, should fall back to default
        let profile = matcher
            .route_by_intent(&TaskIntent::Custom("my_workflow".to_string()))
            .unwrap();
        assert_eq!(profile.id, "claude-sonnet"); // default
    }

    // =========================================================================
    // Provider/Model Lookup Tests
    // =========================================================================

    #[test]
    fn test_find_by_provider_model_exact() {
        let matcher = create_matcher();

        // Find by exact provider and model
        let profile = matcher
            .find_by_provider_model("anthropic", Some("claude-opus-4"))
            .unwrap();
        assert_eq!(profile.id, "claude-opus");
    }

    #[test]
    fn test_find_by_provider_model_case_insensitive() {
        let matcher = create_matcher();

        // Case insensitive matching
        let profile = matcher
            .find_by_provider_model("ANTHROPIC", Some("CLAUDE-OPUS-4"))
            .unwrap();
        assert_eq!(profile.id, "claude-opus");
    }

    #[test]
    fn test_find_by_provider_only() {
        let matcher = create_matcher();

        // Find by provider only (returns first match)
        let profile = matcher.find_by_provider_model("openai", None).unwrap();
        assert_eq!(profile.id, "gpt-4o");
    }

    #[test]
    fn test_find_by_provider_model_not_found() {
        let matcher = create_matcher();

        // Nonexistent provider
        let profile = matcher.find_by_provider_model("nonexistent", None);
        assert!(profile.is_none());

        // Existing provider but wrong model
        let profile = matcher.find_by_provider_model("anthropic", Some("wrong-model"));
        assert!(profile.is_none());
    }

    // =========================================================================
    // Fallback Provider Tests
    // =========================================================================

    #[test]
    fn test_fallback_provider_new() {
        let fallback = FallbackProvider::new("openai");
        assert_eq!(fallback.provider, "openai");
        assert!(fallback.model.is_none());
    }

    #[test]
    fn test_fallback_provider_with_model() {
        let fallback = FallbackProvider::new("anthropic").with_model("claude-opus-4");
        assert_eq!(fallback.provider, "anthropic");
        assert_eq!(fallback.model.as_deref(), Some("claude-opus-4"));
    }

    #[test]
    fn test_fallback_provider_to_model_profile_openai() {
        let fallback = FallbackProvider::new("openai");
        let profile = fallback.to_model_profile();

        assert_eq!(profile.id, "fallback-openai");
        assert_eq!(profile.provider, "openai");
        assert_eq!(profile.model, "gpt-4o"); // default model
        assert_eq!(profile.cost_tier, CostTier::Medium);
        assert_eq!(profile.latency_tier, LatencyTier::Medium);
    }

    #[test]
    fn test_fallback_provider_to_model_profile_anthropic() {
        let fallback = FallbackProvider::new("anthropic");
        let profile = fallback.to_model_profile();

        assert_eq!(profile.id, "fallback-anthropic");
        assert_eq!(profile.provider, "anthropic");
        assert_eq!(profile.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_fallback_provider_to_model_profile_with_custom_model() {
        let fallback = FallbackProvider::new("google").with_model("gemini-2.0-pro");
        let profile = fallback.to_model_profile();

        assert_eq!(profile.id, "fallback-google");
        assert_eq!(profile.provider, "google");
        assert_eq!(profile.model, "gemini-2.0-pro"); // custom model
    }

    #[test]
    fn test_fallback_provider_to_model_profile_unknown_provider() {
        let fallback = FallbackProvider::new("custom-provider");
        let profile = fallback.to_model_profile();

        assert_eq!(profile.id, "fallback-custom-provider");
        assert_eq!(profile.provider, "custom-provider");
        assert_eq!(profile.model, "default"); // unknown provider default
    }

    #[test]
    fn test_matcher_with_fallback_provider() {
        let rules = ModelRoutingRules::default();
        let matcher = ModelMatcher::new(vec![], rules).with_fallback_provider("openai");

        assert!(matcher.has_fallback());
    }

    #[test]
    fn test_matcher_with_fallback_provider_and_model() {
        let rules = ModelRoutingRules::default();
        let matcher = ModelMatcher::new(vec![], rules)
            .with_fallback_provider_and_model("anthropic", "claude-opus-4");

        assert!(matcher.has_fallback());
    }

    #[test]
    fn test_matcher_without_fallback_provider() {
        let matcher = create_matcher();
        assert!(!matcher.has_fallback());
    }

    #[test]
    fn test_route_by_intent_uses_fallback_when_no_profiles() {
        let rules = ModelRoutingRules::default();
        let matcher = ModelMatcher::new(vec![], rules).with_fallback_provider("openai");

        // With no profiles configured, should use fallback
        let profile = matcher
            .route_by_intent(&TaskIntent::CodeGeneration)
            .unwrap();
        assert_eq!(profile.id, "fallback-openai");
        assert_eq!(profile.provider, "openai");
        assert_eq!(profile.model, "gpt-4o");
    }

    #[test]
    fn test_route_by_intent_uses_cost_strategy_before_fallback() {
        // Create matcher with only a haiku profile (no code generation capability)
        let haiku = ModelProfile::new("claude-haiku", "anthropic", "claude-haiku-3.5")
            .with_capabilities(vec![Capability::FastResponse, Capability::SimpleTask]);

        let rules = ModelRoutingRules::default(); // no task type mappings
        let matcher = ModelMatcher::new(vec![haiku], rules).with_fallback_provider("openai");

        // Even though CodeGeneration requires that capability and haiku doesn't have it,
        // cost_strategy will select haiku as a fallback before using fallback_provider.
        // This is correct behavior - configured models are preferred over fallback_provider.
        let profile = matcher
            .route_by_intent(&TaskIntent::CodeGeneration)
            .unwrap();
        assert_eq!(profile.id, "claude-haiku"); // haiku selected via cost strategy
    }

    #[test]
    fn test_route_by_intent_uses_fallback_only_when_no_profiles() {
        // When there are no profiles at all, fallback_provider is used
        let rules = ModelRoutingRules::default();
        let matcher = ModelMatcher::new(vec![], rules).with_fallback_provider("openai");

        let profile = matcher
            .route_by_intent(&TaskIntent::CodeGeneration)
            .unwrap();
        assert_eq!(profile.id, "fallback-openai");
    }

    #[test]
    fn test_route_by_intent_prefers_configured_model_over_fallback() {
        // Create matcher with proper code generation model
        let opus = ModelProfile::new("claude-opus", "anthropic", "claude-opus-4")
            .with_capabilities(vec![Capability::CodeGeneration, Capability::Reasoning]);

        let rules = ModelRoutingRules::default();
        let matcher = ModelMatcher::new(vec![opus], rules).with_fallback_provider("openai");

        // Should use configured model, not fallback
        let profile = matcher
            .route_by_intent(&TaskIntent::CodeGeneration)
            .unwrap();
        assert_eq!(profile.id, "claude-opus");
        assert_ne!(profile.id, "fallback-openai");
    }

    #[test]
    fn test_route_by_intent_returns_none_without_fallback() {
        let rules = ModelRoutingRules::default();
        let matcher = ModelMatcher::new(vec![], rules); // no profiles, no fallback

        // Should return None when no profiles and no fallback
        let profile = matcher.route_by_intent(&TaskIntent::CodeGeneration);
        assert!(profile.is_none());
    }

    #[test]
    fn test_set_fallback_provider() {
        let rules = ModelRoutingRules::default();
        let mut matcher = ModelMatcher::new(vec![], rules);

        assert!(!matcher.has_fallback());

        matcher.set_fallback_provider(Some(FallbackProvider::new("google")));
        assert!(matcher.has_fallback());

        // Test routing uses fallback
        let profile = matcher.route_by_intent(&TaskIntent::GeneralChat).unwrap();
        assert_eq!(profile.id, "fallback-google");
    }

    #[test]
    fn test_fallback_provider_claude_alias() {
        // "claude" should be treated same as "anthropic"
        let fallback = FallbackProvider::new("claude");
        let profile = fallback.to_model_profile();

        assert_eq!(profile.id, "fallback-claude");
        assert_eq!(profile.provider, "claude");
        assert_eq!(profile.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_fallback_provider_gemini_alias() {
        // "gemini" should be treated same as "google"
        let fallback = FallbackProvider::new("gemini");
        let profile = fallback.to_model_profile();

        assert_eq!(profile.id, "fallback-gemini");
        assert_eq!(profile.provider, "gemini");
        assert_eq!(profile.model, "gemini-1.5-flash");
    }
}
