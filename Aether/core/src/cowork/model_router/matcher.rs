//! Model Matcher Implementation
//!
//! This module provides intelligent routing of tasks to optimal AI models
//! based on task characteristics, model capabilities, and cost preferences.

use super::{Capability, CostStrategy, CostTier, LatencyTier, ModelProfile, ModelRoutingRules};
use crate::cowork::types::{Task, TaskType};
use std::collections::HashMap;

/// Error type for model routing operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum RoutingError {
    #[error("No model available for task: {task_type}")]
    NoModelAvailable { task_type: String },

    #[error("Model profile not found: {profile_id}")]
    ProfileNotFound { profile_id: String },

    #[error("No model with capability: {capability:?}")]
    NoCapabilityMatch { capability: Capability },
}

/// Trait for routing tasks to AI models
pub trait ModelRouter: Send + Sync {
    /// Route a task to the optimal model profile
    fn route(&self, task: &Task) -> Result<ModelProfile, RoutingError>;

    /// Get a model profile by ID
    fn get_profile(&self, id: &str) -> Option<&ModelProfile>;

    /// Get all available model profiles
    fn profiles(&self) -> &[ModelProfile];

    /// Check if a profile supports a specific capability
    fn supports_capability(&self, profile_id: &str, capability: &Capability) -> bool;

    /// Find the best model for a specific capability
    fn find_best_for(&self, capability: Capability) -> Option<ModelProfile>;

    /// Find a balanced model (medium cost, medium latency)
    fn find_balanced(&self) -> Option<ModelProfile>;

    /// Find the cheapest model with a specific capability
    fn find_cheapest_with(&self, capability: Capability) -> Option<ModelProfile>;
}

/// Model matcher that routes tasks to optimal AI models
pub struct ModelMatcher {
    /// Model profiles indexed by ID
    profiles_by_id: HashMap<String, ModelProfile>,

    /// Model profiles as a vector (for iteration)
    profiles_vec: Vec<ModelProfile>,

    /// Routing rules configuration
    rules: ModelRoutingRules,

    /// Profiles indexed by capability for fast lookup
    capability_index: HashMap<Capability, Vec<String>>,
}

impl ModelMatcher {
    /// Create a new ModelMatcher with profiles and routing rules
    pub fn new(profiles: Vec<ModelProfile>, rules: ModelRoutingRules) -> Self {
        let mut profiles_by_id = HashMap::new();
        let mut capability_index: HashMap<Capability, Vec<String>> = HashMap::new();

        // Index profiles by ID and capability
        for profile in &profiles {
            profiles_by_id.insert(profile.id.clone(), profile.clone());

            for capability in &profile.capabilities {
                capability_index
                    .entry(*capability)
                    .or_default()
                    .push(profile.id.clone());
            }
        }

        Self {
            profiles_by_id,
            profiles_vec: profiles,
            rules,
            capability_index,
        }
    }

    /// Get the routing rules
    pub fn rules(&self) -> &ModelRoutingRules {
        &self.rules
    }

    /// Extract routing hints from a task
    fn extract_task_hints(&self, task: &Task) -> TaskHints {
        let mut hints = TaskHints::default();

        // Check for explicit model preference
        hints.model_preference = task.model_preference.clone();

        // Extract hints from task type
        match &task.task_type {
            TaskType::AiInference(ai_task) => {
                hints.requires_privacy = ai_task.requires_privacy;
                hints.has_images = ai_task.has_images;

                // Check prompt length for long context hint
                if ai_task.prompt.len() > 50_000 {
                    hints.needs_long_context = true;
                }
            }
            TaskType::CodeExecution(_) => {
                hints.task_type_hint = Some("code_generation".to_string());
            }
            TaskType::DocumentGeneration(_) => {
                hints.task_type_hint = Some("long_document".to_string());
            }
            _ => {}
        }

        // Check parameters for additional hints
        if let Some(obj) = task.parameters.as_object() {
            if let Some(ctx_len) = obj.get("context_length").and_then(|v| v.as_u64()) {
                if ctx_len > 100_000 {
                    hints.needs_long_context = true;
                }
            }
        }

        hints
    }

    /// Route based on task type mapping
    fn route_by_task_type(&self, task_type: &str) -> Option<ModelProfile> {
        self.rules
            .get_for_task_type(task_type)
            .and_then(|id| self.profiles_by_id.get(id))
            .cloned()
    }

    /// Route based on capability requirements
    fn route_by_capability(&self, capability: Capability) -> Option<ModelProfile> {
        // First check if there's a specific mapping in rules
        if let Some(id) = self.rules.get_for_capability(capability) {
            if let Some(profile) = self.profiles_by_id.get(id) {
                return Some(profile.clone());
            }
        }

        // Fall back to finding best profile with this capability
        self.find_best_for(capability)
    }

    /// Apply cost strategy to select from multiple candidates
    fn apply_cost_strategy(&self, candidates: &[&ModelProfile]) -> Option<ModelProfile> {
        if candidates.is_empty() {
            return None;
        }

        match self.rules.cost_strategy {
            CostStrategy::Cheapest => {
                candidates
                    .iter()
                    .min_by_key(|p| p.cost_tier)
                    .map(|p| (*p).clone())
            }
            CostStrategy::BestQuality => {
                // Higher cost = better quality (in general)
                candidates
                    .iter()
                    .max_by_key(|p| p.cost_tier)
                    .map(|p| (*p).clone())
            }
            CostStrategy::Balanced => {
                // Prefer medium cost, then look at latency
                candidates
                    .iter()
                    .min_by(|a, b| {
                        // Score: prefer Medium cost, then Fast latency
                        let score_a = cost_balance_score(a);
                        let score_b = cost_balance_score(b);
                        score_a.cmp(&score_b)
                    })
                    .map(|p| (*p).clone())
            }
        }
    }

    /// Get default model profile
    fn get_default_profile(&self) -> Option<ModelProfile> {
        self.rules
            .get_default()
            .and_then(|id| self.profiles_by_id.get(id))
            .cloned()
    }
}

/// Compute a balance score for a profile (lower is better for balanced strategy)
fn cost_balance_score(profile: &ModelProfile) -> u32 {
    let cost_score = match profile.cost_tier {
        CostTier::Free => 2,   // Slightly penalize free (may have limitations)
        CostTier::Low => 1,
        CostTier::Medium => 0, // Preferred
        CostTier::High => 2,
    };

    let latency_score = match profile.latency_tier {
        LatencyTier::Fast => 0,
        LatencyTier::Medium => 1,
        LatencyTier::Slow => 2,
    };

    cost_score + latency_score
}

/// Routing hints extracted from a task
#[derive(Debug, Default)]
struct TaskHints {
    /// Explicit model preference from task
    model_preference: Option<String>,
    /// Task requires privacy (should use local model)
    requires_privacy: bool,
    /// Task involves images
    has_images: bool,
    /// Task needs long context
    needs_long_context: bool,
    /// Task type hint (e.g., "code_generation")
    task_type_hint: Option<String>,
}

impl ModelRouter for ModelMatcher {
    fn route(&self, task: &Task) -> Result<ModelProfile, RoutingError> {
        let hints = self.extract_task_hints(task);

        // 1. Check for explicit model preference
        if let Some(ref pref) = hints.model_preference {
            if let Some(profile) = self.profiles_by_id.get(pref) {
                return Ok(profile.clone());
            }
            // Log warning but continue with automatic routing
            tracing::warn!(
                preferred_model = pref,
                "Preferred model not found, falling back to automatic routing"
            );
        }

        // 2. Handle privacy requirement
        if hints.requires_privacy {
            if let Some(profile) = self.route_by_capability(Capability::LocalPrivacy) {
                return Ok(profile);
            }
            // Try to find any local model
            if let Some(profile) = self.profiles_vec.iter().find(|p| p.local) {
                return Ok(profile.clone());
            }
        }

        // 3. Handle image tasks
        if hints.has_images {
            if let Some(profile) = self.route_by_capability(Capability::ImageUnderstanding) {
                return Ok(profile);
            }
        }

        // 4. Handle long context tasks
        if hints.needs_long_context {
            if let Some(profile) = self.route_by_capability(Capability::LongContext) {
                return Ok(profile);
            }
        }

        // 5. Route by task type hint
        if let Some(ref type_hint) = hints.task_type_hint {
            if let Some(profile) = self.route_by_task_type(type_hint) {
                return Ok(profile);
            }
        }

        // 6. Route by task category
        let category = task.task_type.category();
        if let Some(profile) = self.route_by_task_type(category) {
            return Ok(profile);
        }

        // 7. Try to find a balanced model
        if let Some(profile) = self.find_balanced() {
            return Ok(profile);
        }

        // 8. Use default model
        if let Some(profile) = self.get_default_profile() {
            return Ok(profile);
        }

        // 9. Return first available profile
        if let Some(profile) = self.profiles_vec.first() {
            return Ok(profile.clone());
        }

        Err(RoutingError::NoModelAvailable {
            task_type: category.to_string(),
        })
    }

    fn get_profile(&self, id: &str) -> Option<&ModelProfile> {
        self.profiles_by_id.get(id)
    }

    fn profiles(&self) -> &[ModelProfile] {
        &self.profiles_vec
    }

    fn supports_capability(&self, profile_id: &str, capability: &Capability) -> bool {
        self.profiles_by_id
            .get(profile_id)
            .map(|p| p.has_capability(*capability))
            .unwrap_or(false)
    }

    fn find_best_for(&self, capability: Capability) -> Option<ModelProfile> {
        // Get all profiles with this capability
        let profile_ids = self.capability_index.get(&capability)?;

        let candidates: Vec<&ModelProfile> = profile_ids
            .iter()
            .filter_map(|id| self.profiles_by_id.get(id))
            .collect();

        self.apply_cost_strategy(&candidates)
    }

    fn find_balanced(&self) -> Option<ModelProfile> {
        if self.profiles_vec.is_empty() {
            return None;
        }

        let candidates: Vec<&ModelProfile> = self.profiles_vec.iter().collect();
        self.apply_cost_strategy(&candidates)
    }

    fn find_cheapest_with(&self, capability: Capability) -> Option<ModelProfile> {
        let profile_ids = self.capability_index.get(&capability)?;

        profile_ids
            .iter()
            .filter_map(|id| self.profiles_by_id.get(id))
            .min_by_key(|p| p.cost_tier)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::types::{AiTask, CodeExec, FileOp, Language};
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
                || profile.max_context.map_or(false, |c| c >= 100_000)
        );
    }

    // =========================================================================
    // Capability-Based Routing Tests
    // =========================================================================

    #[test]
    fn test_find_best_for_capability() {
        let matcher = create_matcher();

        // Find best for ImageUnderstanding
        let profile = matcher.find_best_for(Capability::ImageUnderstanding).unwrap();
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
        let profile = matcher.find_cheapest_with(Capability::CodeGeneration).unwrap();
        // claude-sonnet (Medium) is cheaper than claude-opus (High)
        assert_eq!(profile.id, "claude-sonnet");

        // Find cheapest with LocalPrivacy
        let profile = matcher.find_cheapest_with(Capability::LocalPrivacy).unwrap();
        assert_eq!(profile.id, "ollama-llama");
        assert_eq!(profile.cost_tier, CostTier::Free);
    }

    #[test]
    fn test_find_balanced() {
        let matcher = create_matcher();

        let profile = matcher.find_balanced().unwrap();
        // Should prefer Medium cost and Fast/Medium latency
        assert!(profile.cost_tier <= CostTier::Medium || profile.latency_tier <= LatencyTier::Medium);
    }

    // =========================================================================
    // Cost Strategy Tests
    // =========================================================================

    #[test]
    fn test_cost_strategy_cheapest() {
        let profiles = create_test_profiles();
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_cost_strategy(CostStrategy::Cheapest);
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
        let rules = ModelRoutingRules::new("claude-sonnet")
            .with_cost_strategy(CostStrategy::BestQuality);
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
        let tasks = vec![
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
}
