//! Model Router FFI Types
//!
//! Contains Model Router FFI types:
//! - ModelCapabilityFFI, ModelCostTierFFI, ModelLatencyTierFFI, ModelCostStrategyFFI
//! - ModelProfileFFI, TaskTypeMappingFFI, CapabilityMappingFFI
//! - ModelRoutingRulesFFI, StageResultFFI

use crate::dispatcher::model_router::{
    Capability, CostStrategy, CostTier, LatencyTier, ModelProfile, ModelRoutingRules, StageResult,
};

// ============================================================================
// Model Router FFI Enums
// ============================================================================

/// Model capability for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCapabilityFFI {
    CodeGeneration,
    CodeReview,
    TextAnalysis,
    ImageUnderstanding,
    VideoUnderstanding,
    LongContext,
    Reasoning,
    LocalPrivacy,
    FastResponse,
    SimpleTask,
    LongDocument,
}

impl From<Capability> for ModelCapabilityFFI {
    fn from(cap: Capability) -> Self {
        match cap {
            Capability::CodeGeneration => ModelCapabilityFFI::CodeGeneration,
            Capability::CodeReview => ModelCapabilityFFI::CodeReview,
            Capability::TextAnalysis => ModelCapabilityFFI::TextAnalysis,
            Capability::ImageUnderstanding => ModelCapabilityFFI::ImageUnderstanding,
            Capability::VideoUnderstanding => ModelCapabilityFFI::VideoUnderstanding,
            Capability::LongContext => ModelCapabilityFFI::LongContext,
            Capability::Reasoning => ModelCapabilityFFI::Reasoning,
            Capability::LocalPrivacy => ModelCapabilityFFI::LocalPrivacy,
            Capability::FastResponse => ModelCapabilityFFI::FastResponse,
            Capability::SimpleTask => ModelCapabilityFFI::SimpleTask,
            Capability::LongDocument => ModelCapabilityFFI::LongDocument,
        }
    }
}

impl From<ModelCapabilityFFI> for Capability {
    fn from(cap: ModelCapabilityFFI) -> Self {
        match cap {
            ModelCapabilityFFI::CodeGeneration => Capability::CodeGeneration,
            ModelCapabilityFFI::CodeReview => Capability::CodeReview,
            ModelCapabilityFFI::TextAnalysis => Capability::TextAnalysis,
            ModelCapabilityFFI::ImageUnderstanding => Capability::ImageUnderstanding,
            ModelCapabilityFFI::VideoUnderstanding => Capability::VideoUnderstanding,
            ModelCapabilityFFI::LongContext => Capability::LongContext,
            ModelCapabilityFFI::Reasoning => Capability::Reasoning,
            ModelCapabilityFFI::LocalPrivacy => Capability::LocalPrivacy,
            ModelCapabilityFFI::FastResponse => Capability::FastResponse,
            ModelCapabilityFFI::SimpleTask => Capability::SimpleTask,
            ModelCapabilityFFI::LongDocument => Capability::LongDocument,
        }
    }
}

/// Model cost tier for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCostTierFFI {
    Free,
    Low,
    Medium,
    High,
}

impl From<CostTier> for ModelCostTierFFI {
    fn from(tier: CostTier) -> Self {
        match tier {
            CostTier::Free => ModelCostTierFFI::Free,
            CostTier::Low => ModelCostTierFFI::Low,
            CostTier::Medium => ModelCostTierFFI::Medium,
            CostTier::High => ModelCostTierFFI::High,
        }
    }
}

impl From<ModelCostTierFFI> for CostTier {
    fn from(tier: ModelCostTierFFI) -> Self {
        match tier {
            ModelCostTierFFI::Free => CostTier::Free,
            ModelCostTierFFI::Low => CostTier::Low,
            ModelCostTierFFI::Medium => CostTier::Medium,
            ModelCostTierFFI::High => CostTier::High,
        }
    }
}

/// Model latency tier for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLatencyTierFFI {
    Fast,
    Medium,
    Slow,
}

impl From<LatencyTier> for ModelLatencyTierFFI {
    fn from(tier: LatencyTier) -> Self {
        match tier {
            LatencyTier::Fast => ModelLatencyTierFFI::Fast,
            LatencyTier::Medium => ModelLatencyTierFFI::Medium,
            LatencyTier::Slow => ModelLatencyTierFFI::Slow,
        }
    }
}

impl From<ModelLatencyTierFFI> for LatencyTier {
    fn from(tier: ModelLatencyTierFFI) -> Self {
        match tier {
            ModelLatencyTierFFI::Fast => LatencyTier::Fast,
            ModelLatencyTierFFI::Medium => LatencyTier::Medium,
            ModelLatencyTierFFI::Slow => LatencyTier::Slow,
        }
    }
}

/// Model cost strategy for FFI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCostStrategyFFI {
    Cheapest,
    Balanced,
    BestQuality,
}

impl From<CostStrategy> for ModelCostStrategyFFI {
    fn from(strategy: CostStrategy) -> Self {
        match strategy {
            CostStrategy::Cheapest => ModelCostStrategyFFI::Cheapest,
            CostStrategy::Balanced => ModelCostStrategyFFI::Balanced,
            CostStrategy::BestQuality => ModelCostStrategyFFI::BestQuality,
        }
    }
}

impl From<ModelCostStrategyFFI> for CostStrategy {
    fn from(strategy: ModelCostStrategyFFI) -> Self {
        match strategy {
            ModelCostStrategyFFI::Cheapest => CostStrategy::Cheapest,
            ModelCostStrategyFFI::Balanced => CostStrategy::Balanced,
            ModelCostStrategyFFI::BestQuality => CostStrategy::BestQuality,
        }
    }
}

// ============================================================================
// Model Router FFI Structs
// ============================================================================

/// Model profile for FFI
#[derive(Debug, Clone)]
pub struct ModelProfileFFI {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub capabilities: Vec<ModelCapabilityFFI>,
    pub cost_tier: ModelCostTierFFI,
    pub latency_tier: ModelLatencyTierFFI,
    pub max_context: Option<u32>,
    pub local: bool,
}

impl From<ModelProfile> for ModelProfileFFI {
    fn from(profile: ModelProfile) -> Self {
        Self {
            id: profile.id,
            provider: profile.provider,
            model: profile.model,
            capabilities: profile
                .capabilities
                .into_iter()
                .map(ModelCapabilityFFI::from)
                .collect(),
            cost_tier: ModelCostTierFFI::from(profile.cost_tier),
            latency_tier: ModelLatencyTierFFI::from(profile.latency_tier),
            max_context: profile.max_context,
            local: profile.local,
        }
    }
}

impl From<&ModelProfile> for ModelProfileFFI {
    fn from(profile: &ModelProfile) -> Self {
        Self {
            id: profile.id.clone(),
            provider: profile.provider.clone(),
            model: profile.model.clone(),
            capabilities: profile
                .capabilities
                .iter()
                .copied()
                .map(ModelCapabilityFFI::from)
                .collect(),
            cost_tier: ModelCostTierFFI::from(profile.cost_tier),
            latency_tier: ModelLatencyTierFFI::from(profile.latency_tier),
            max_context: profile.max_context,
            local: profile.local,
        }
    }
}

impl From<ModelProfileFFI> for ModelProfile {
    fn from(profile: ModelProfileFFI) -> Self {
        Self {
            id: profile.id,
            provider: profile.provider,
            model: profile.model,
            capabilities: profile
                .capabilities
                .into_iter()
                .map(Capability::from)
                .collect(),
            cost_tier: CostTier::from(profile.cost_tier),
            latency_tier: LatencyTier::from(profile.latency_tier),
            max_context: profile.max_context,
            local: profile.local,
            parameters: None,
        }
    }
}

/// Task type to model mapping entry for FFI
#[derive(Debug, Clone)]
pub struct TaskTypeMappingFFI {
    pub task_type: String,
    pub model_id: String,
}

/// Capability to model mapping entry for FFI
#[derive(Debug, Clone)]
pub struct CapabilityMappingFFI {
    pub capability: ModelCapabilityFFI,
    pub model_id: String,
}

/// Model routing rules for FFI
#[derive(Debug, Clone)]
pub struct ModelRoutingRulesFFI {
    pub task_type_mappings: Vec<TaskTypeMappingFFI>,
    pub capability_mappings: Vec<CapabilityMappingFFI>,
    pub cost_strategy: ModelCostStrategyFFI,
    pub default_model: Option<String>,
    pub enable_pipelines: bool,
}

impl From<ModelRoutingRules> for ModelRoutingRulesFFI {
    fn from(rules: ModelRoutingRules) -> Self {
        Self {
            task_type_mappings: rules
                .task_type_mappings
                .into_iter()
                .map(|(task_type, model_id)| TaskTypeMappingFFI {
                    task_type,
                    model_id,
                })
                .collect(),
            capability_mappings: rules
                .capability_mappings
                .into_iter()
                .map(|(cap, model_id)| CapabilityMappingFFI {
                    capability: ModelCapabilityFFI::from(cap),
                    model_id,
                })
                .collect(),
            cost_strategy: ModelCostStrategyFFI::from(rules.cost_strategy),
            default_model: rules.default_model,
            enable_pipelines: rules.enable_pipelines,
        }
    }
}

impl From<&ModelRoutingRules> for ModelRoutingRulesFFI {
    fn from(rules: &ModelRoutingRules) -> Self {
        Self {
            task_type_mappings: rules
                .task_type_mappings
                .iter()
                .map(|(task_type, model_id)| TaskTypeMappingFFI {
                    task_type: task_type.clone(),
                    model_id: model_id.clone(),
                })
                .collect(),
            capability_mappings: rules
                .capability_mappings
                .iter()
                .map(|(cap, model_id)| CapabilityMappingFFI {
                    capability: ModelCapabilityFFI::from(*cap),
                    model_id: model_id.clone(),
                })
                .collect(),
            cost_strategy: ModelCostStrategyFFI::from(rules.cost_strategy),
            default_model: rules.default_model.clone(),
            enable_pipelines: rules.enable_pipelines,
        }
    }
}

impl From<ModelRoutingRulesFFI> for ModelRoutingRules {
    fn from(rules: ModelRoutingRulesFFI) -> Self {
        let mut result = ModelRoutingRules::default();

        for mapping in rules.task_type_mappings {
            result
                .task_type_mappings
                .insert(mapping.task_type, mapping.model_id);
        }

        for mapping in rules.capability_mappings {
            result
                .capability_mappings
                .insert(Capability::from(mapping.capability), mapping.model_id);
        }

        result.cost_strategy = CostStrategy::from(rules.cost_strategy);
        result.default_model = rules.default_model;
        result.enable_pipelines = rules.enable_pipelines;

        result
    }
}

/// Stage result for FFI
#[derive(Debug, Clone)]
pub struct StageResultFFI {
    pub stage_id: String,
    pub model_used: String,
    pub provider: String,
    pub output_json: String,
    pub tokens_used: u32,
    pub duration_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

impl From<StageResult> for StageResultFFI {
    fn from(result: StageResult) -> Self {
        Self {
            stage_id: result.stage_id,
            model_used: result.model_used,
            provider: result.provider,
            output_json: result.output.to_string(),
            tokens_used: result.tokens_used,
            duration_ms: result.duration.as_millis() as u64,
            success: result.success,
            error: result.error,
        }
    }
}

impl From<&StageResult> for StageResultFFI {
    fn from(result: &StageResult) -> Self {
        Self {
            stage_id: result.stage_id.clone(),
            model_used: result.model_used.clone(),
            provider: result.provider.clone(),
            output_json: result.output.to_string(),
            tokens_used: result.tokens_used,
            duration_ms: result.duration.as_millis() as u64,
            success: result.success,
            error: result.error.clone(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_capability_ffi_conversion() {
        let capabilities = vec![
            (
                Capability::CodeGeneration,
                ModelCapabilityFFI::CodeGeneration,
            ),
            (Capability::CodeReview, ModelCapabilityFFI::CodeReview),
            (Capability::TextAnalysis, ModelCapabilityFFI::TextAnalysis),
            (
                Capability::ImageUnderstanding,
                ModelCapabilityFFI::ImageUnderstanding,
            ),
            (
                Capability::VideoUnderstanding,
                ModelCapabilityFFI::VideoUnderstanding,
            ),
            (Capability::LongContext, ModelCapabilityFFI::LongContext),
            (Capability::Reasoning, ModelCapabilityFFI::Reasoning),
            (Capability::LocalPrivacy, ModelCapabilityFFI::LocalPrivacy),
            (Capability::FastResponse, ModelCapabilityFFI::FastResponse),
            (Capability::SimpleTask, ModelCapabilityFFI::SimpleTask),
            (Capability::LongDocument, ModelCapabilityFFI::LongDocument),
        ];

        for (cap, expected_ffi) in capabilities {
            let ffi: ModelCapabilityFFI = cap.into();
            assert_eq!(ffi, expected_ffi);

            let back: Capability = ffi.into();
            assert_eq!(back, cap);
        }
    }

    #[test]
    fn test_model_cost_tier_ffi_conversion() {
        let tiers = vec![
            (CostTier::Free, ModelCostTierFFI::Free),
            (CostTier::Low, ModelCostTierFFI::Low),
            (CostTier::Medium, ModelCostTierFFI::Medium),
            (CostTier::High, ModelCostTierFFI::High),
        ];

        for (tier, expected_ffi) in tiers {
            let ffi: ModelCostTierFFI = tier.into();
            assert_eq!(ffi, expected_ffi);

            let back: CostTier = ffi.into();
            assert_eq!(back, tier);
        }
    }

    #[test]
    fn test_model_latency_tier_ffi_conversion() {
        let tiers = vec![
            (LatencyTier::Fast, ModelLatencyTierFFI::Fast),
            (LatencyTier::Medium, ModelLatencyTierFFI::Medium),
            (LatencyTier::Slow, ModelLatencyTierFFI::Slow),
        ];

        for (tier, expected_ffi) in tiers {
            let ffi: ModelLatencyTierFFI = tier.into();
            assert_eq!(ffi, expected_ffi);

            let back: LatencyTier = ffi.into();
            assert_eq!(back, tier);
        }
    }

    #[test]
    fn test_model_cost_strategy_ffi_conversion() {
        let strategies = vec![
            (CostStrategy::Cheapest, ModelCostStrategyFFI::Cheapest),
            (CostStrategy::Balanced, ModelCostStrategyFFI::Balanced),
            (CostStrategy::BestQuality, ModelCostStrategyFFI::BestQuality),
        ];

        for (strategy, expected_ffi) in strategies {
            let ffi: ModelCostStrategyFFI = strategy.into();
            assert_eq!(ffi, expected_ffi);

            let back: CostStrategy = ffi.into();
            assert_eq!(back, strategy);
        }
    }

    #[test]
    fn test_model_profile_ffi_conversion() {
        let profile = ModelProfile {
            id: "claude-opus".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![Capability::Reasoning, Capability::CodeGeneration],
            cost_tier: CostTier::High,
            latency_tier: LatencyTier::Slow,
            max_context: Some(200000),
            local: false,
            parameters: None,
        };

        let ffi: ModelProfileFFI = profile.clone().into();
        assert_eq!(ffi.id, "claude-opus");
        assert_eq!(ffi.provider, "anthropic");
        assert_eq!(ffi.model, "claude-opus-4");
        assert_eq!(ffi.capabilities.len(), 2);
        assert_eq!(ffi.cost_tier, ModelCostTierFFI::High);
        assert_eq!(ffi.latency_tier, ModelLatencyTierFFI::Slow);
        assert_eq!(ffi.max_context, Some(200000));
        assert!(!ffi.local);

        let back: ModelProfile = ffi.into();
        assert_eq!(back.id, profile.id);
        assert_eq!(back.provider, profile.provider);
        assert_eq!(back.model, profile.model);
        assert_eq!(back.capabilities.len(), profile.capabilities.len());
        assert_eq!(back.cost_tier, profile.cost_tier);
        assert_eq!(back.latency_tier, profile.latency_tier);
        assert_eq!(back.max_context, profile.max_context);
        assert_eq!(back.local, profile.local);
    }

    #[test]
    fn test_model_profile_ffi_from_ref() {
        let profile = ModelProfile {
            id: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            capabilities: vec![Capability::ImageUnderstanding],
            cost_tier: CostTier::Medium,
            latency_tier: LatencyTier::Medium,
            max_context: None,
            local: false,
            parameters: None,
        };

        let ffi: ModelProfileFFI = (&profile).into();
        assert_eq!(ffi.id, "gpt-4o");
        assert_eq!(ffi.provider, "openai");
        assert_eq!(ffi.capabilities.len(), 1);
        assert_eq!(ffi.capabilities[0], ModelCapabilityFFI::ImageUnderstanding);
    }

    #[test]
    fn test_model_profile_ffi_local_model() {
        let profile = ModelProfile {
            id: "ollama-llama".to_string(),
            provider: "ollama".to_string(),
            model: "llama3.2".to_string(),
            capabilities: vec![Capability::LocalPrivacy, Capability::FastResponse],
            cost_tier: CostTier::Free,
            latency_tier: LatencyTier::Fast,
            max_context: None,
            local: true,
            parameters: None,
        };

        let ffi: ModelProfileFFI = profile.into();
        assert!(ffi.local);
        assert_eq!(ffi.cost_tier, ModelCostTierFFI::Free);
        assert_eq!(ffi.latency_tier, ModelLatencyTierFFI::Fast);
    }

    #[test]
    fn test_task_type_mapping_ffi() {
        let mapping = TaskTypeMappingFFI {
            task_type: "code_generation".to_string(),
            model_id: "claude-opus".to_string(),
        };

        assert_eq!(mapping.task_type, "code_generation");
        assert_eq!(mapping.model_id, "claude-opus");
    }

    #[test]
    fn test_capability_mapping_ffi() {
        let mapping = CapabilityMappingFFI {
            capability: ModelCapabilityFFI::Reasoning,
            model_id: "claude-opus".to_string(),
        };

        assert_eq!(mapping.capability, ModelCapabilityFFI::Reasoning);
        assert_eq!(mapping.model_id, "claude-opus");
    }

    #[test]
    fn test_model_routing_rules_ffi_creation() {
        let rules = ModelRoutingRulesFFI {
            cost_strategy: ModelCostStrategyFFI::Balanced,
            default_model: Some("claude-sonnet".to_string()),
            enable_pipelines: true,
            task_type_mappings: vec![
                TaskTypeMappingFFI {
                    task_type: "code_generation".to_string(),
                    model_id: "claude-opus".to_string(),
                },
                TaskTypeMappingFFI {
                    task_type: "quick_tasks".to_string(),
                    model_id: "claude-haiku".to_string(),
                },
            ],
            capability_mappings: vec![CapabilityMappingFFI {
                capability: ModelCapabilityFFI::Reasoning,
                model_id: "claude-opus".to_string(),
            }],
        };

        assert_eq!(rules.cost_strategy, ModelCostStrategyFFI::Balanced);
        assert_eq!(rules.default_model, Some("claude-sonnet".to_string()));
        assert!(rules.enable_pipelines);
        assert_eq!(rules.task_type_mappings.len(), 2);
        assert_eq!(rules.capability_mappings.len(), 1);
    }

    #[test]
    fn test_stage_result_ffi() {
        let result = StageResultFFI {
            stage_id: "stage_1".to_string(),
            model_used: "claude-opus".to_string(),
            provider: "anthropic".to_string(),
            output_json: r#"{"result": "Generated code..."}"#.to_string(),
            tokens_used: 1500,
            duration_ms: 2500,
            success: true,
            error: None,
        };

        assert_eq!(result.stage_id, "stage_1");
        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.model_used, "claude-opus");
        assert_eq!(result.provider, "anthropic");
        assert_eq!(result.tokens_used, 1500);
        assert_eq!(result.duration_ms, 2500);

        let failed_result = StageResultFFI {
            stage_id: "stage_2".to_string(),
            model_used: String::new(),
            provider: String::new(),
            output_json: String::new(),
            tokens_used: 0,
            duration_ms: 100,
            success: false,
            error: Some("API error".to_string()),
        };

        assert!(!failed_result.success);
        assert_eq!(failed_result.error, Some("API error".to_string()));
    }
}
