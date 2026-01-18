//! Model Profile Definitions
//!
//! This module defines data structures for describing AI model capabilities,
//! cost characteristics, and performance tiers.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Model capability tags for characterizing model strengths
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Code creation and completion
    CodeGeneration,
    /// Code analysis and review
    CodeReview,
    /// Text understanding and summarization
    TextAnalysis,
    /// Vision/image analysis
    ImageUnderstanding,
    /// Video analysis
    VideoUnderstanding,
    /// Large context window support (>100k tokens)
    LongContext,
    /// Complex reasoning tasks
    Reasoning,
    /// Local execution for privacy-sensitive data
    LocalPrivacy,
    /// Low latency response (<2s)
    FastResponse,
    /// Simple task handling (Q&A, classification)
    SimpleTask,
    /// Long document processing
    LongDocument,
}

impl Capability {
    /// Get all available capabilities
    pub fn all() -> &'static [Capability] {
        &[
            Capability::CodeGeneration,
            Capability::CodeReview,
            Capability::TextAnalysis,
            Capability::ImageUnderstanding,
            Capability::VideoUnderstanding,
            Capability::LongContext,
            Capability::Reasoning,
            Capability::LocalPrivacy,
            Capability::FastResponse,
            Capability::SimpleTask,
            Capability::LongDocument,
        ]
    }

    /// Get human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Capability::CodeGeneration => "Code Generation",
            Capability::CodeReview => "Code Review",
            Capability::TextAnalysis => "Text Analysis",
            Capability::ImageUnderstanding => "Image Understanding",
            Capability::VideoUnderstanding => "Video Understanding",
            Capability::LongContext => "Long Context",
            Capability::Reasoning => "Reasoning",
            Capability::LocalPrivacy => "Local Privacy",
            Capability::FastResponse => "Fast Response",
            Capability::SimpleTask => "Simple Task",
            Capability::LongDocument => "Long Document",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Cost tier for cost-aware routing decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostTier {
    /// Free tier (local models)
    Free,
    /// Low cost tier (~$0.25/1M tokens)
    Low,
    /// Medium cost tier (~$3/1M tokens)
    Medium,
    /// High cost tier (~$15/1M tokens)
    High,
}

impl CostTier {
    /// Get relative cost multiplier for comparison
    pub fn cost_multiplier(&self) -> f64 {
        match self {
            CostTier::Free => 0.0,
            CostTier::Low => 1.0,
            CostTier::Medium => 10.0,
            CostTier::High => 50.0,
        }
    }
}

impl Default for CostTier {
    fn default() -> Self {
        CostTier::Medium
    }
}

impl std::fmt::Display for CostTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CostTier::Free => write!(f, "Free"),
            CostTier::Low => write!(f, "Low"),
            CostTier::Medium => write!(f, "Medium"),
            CostTier::High => write!(f, "High"),
        }
    }
}

/// Latency tier for latency-sensitive task routing
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatencyTier {
    /// Fast response (<2s typical)
    Fast,
    /// Medium latency (2-10s typical)
    Medium,
    /// Slow response (>10s typical)
    Slow,
}

impl Default for LatencyTier {
    fn default() -> Self {
        LatencyTier::Medium
    }
}

impl std::fmt::Display for LatencyTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LatencyTier::Fast => write!(f, "Fast"),
            LatencyTier::Medium => write!(f, "Medium"),
            LatencyTier::Slow => write!(f, "Slow"),
        }
    }
}

/// Model profile describing an AI model's characteristics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Unique identifier (e.g., "claude-opus", "gpt-4o")
    pub id: String,

    /// Provider name (anthropic, openai, google, ollama)
    pub provider: String,

    /// Model name for API calls
    pub model: String,

    /// Capability tags - first in list indicates primary strength
    #[serde(default)]
    pub capabilities: Vec<Capability>,

    /// Cost tier for cost-aware routing
    #[serde(default)]
    pub cost_tier: CostTier,

    /// Latency tier for latency-sensitive tasks
    #[serde(default)]
    pub latency_tier: LatencyTier,

    /// Maximum context window in tokens
    #[serde(default)]
    pub max_context: Option<u32>,

    /// Whether this is a local model (no network calls)
    #[serde(default)]
    pub local: bool,

    /// Custom parameters for provider-specific settings
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

impl ModelProfile {
    /// Create a new model profile with required fields
    pub fn new(
        id: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            model: model.into(),
            capabilities: Vec::new(),
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: None,
            local: false,
            parameters: None,
        }
    }

    /// Builder method to add capabilities
    pub fn with_capabilities(mut self, capabilities: Vec<Capability>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Builder method to set cost tier
    pub fn with_cost_tier(mut self, tier: CostTier) -> Self {
        self.cost_tier = tier;
        self
    }

    /// Builder method to set latency tier
    pub fn with_latency_tier(mut self, tier: LatencyTier) -> Self {
        self.latency_tier = tier;
        self
    }

    /// Builder method to set max context
    pub fn with_max_context(mut self, max_context: u32) -> Self {
        self.max_context = Some(max_context);
        self
    }

    /// Builder method to mark as local model
    pub fn as_local(mut self) -> Self {
        self.local = true;
        self
    }

    /// Check if model has a specific capability
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Get capabilities as a HashSet for fast lookup
    pub fn capability_set(&self) -> HashSet<Capability> {
        self.capabilities.iter().copied().collect()
    }

    /// Get primary capability (first in list)
    pub fn primary_capability(&self) -> Option<Capability> {
        self.capabilities.first().copied()
    }

    /// Check if model supports long context (>100k tokens)
    pub fn supports_long_context(&self) -> bool {
        self.max_context.map_or(false, |ctx| ctx >= 100_000)
            || self.has_capability(Capability::LongContext)
    }

    /// Check if model is suitable for privacy-sensitive tasks
    pub fn is_privacy_safe(&self) -> bool {
        self.local || self.has_capability(Capability::LocalPrivacy)
    }
}

impl std::fmt::Display for ModelProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}/{}) [{}]",
            self.id, self.provider, self.model, self.cost_tier
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_display() {
        assert_eq!(Capability::CodeGeneration.display_name(), "Code Generation");
        assert_eq!(Capability::LocalPrivacy.display_name(), "Local Privacy");
    }

    #[test]
    fn test_capability_all() {
        let all = Capability::all();
        assert_eq!(all.len(), 11);
        assert!(all.contains(&Capability::CodeGeneration));
        assert!(all.contains(&Capability::LocalPrivacy));
    }

    #[test]
    fn test_cost_tier_ordering() {
        assert!(CostTier::Free < CostTier::Low);
        assert!(CostTier::Low < CostTier::Medium);
        assert!(CostTier::Medium < CostTier::High);
    }

    #[test]
    fn test_cost_tier_multiplier() {
        assert_eq!(CostTier::Free.cost_multiplier(), 0.0);
        assert!(CostTier::High.cost_multiplier() > CostTier::Medium.cost_multiplier());
    }

    #[test]
    fn test_latency_tier_ordering() {
        assert!(LatencyTier::Fast < LatencyTier::Medium);
        assert!(LatencyTier::Medium < LatencyTier::Slow);
    }

    #[test]
    fn test_model_profile_builder() {
        let profile = ModelProfile::new("claude-opus", "anthropic", "claude-opus-4")
            .with_capabilities(vec![Capability::Reasoning, Capability::CodeGeneration])
            .with_cost_tier(CostTier::High)
            .with_latency_tier(LatencyTier::Slow)
            .with_max_context(200_000);

        assert_eq!(profile.id, "claude-opus");
        assert_eq!(profile.provider, "anthropic");
        assert_eq!(profile.model, "claude-opus-4");
        assert!(profile.has_capability(Capability::Reasoning));
        assert!(profile.has_capability(Capability::CodeGeneration));
        assert!(!profile.has_capability(Capability::ImageUnderstanding));
        assert_eq!(profile.cost_tier, CostTier::High);
        assert_eq!(profile.latency_tier, LatencyTier::Slow);
        assert_eq!(profile.max_context, Some(200_000));
        assert!(!profile.local);
    }

    #[test]
    fn test_model_profile_local() {
        let profile = ModelProfile::new("ollama-llama", "ollama", "llama3.2")
            .with_capabilities(vec![Capability::LocalPrivacy, Capability::FastResponse])
            .with_cost_tier(CostTier::Free)
            .as_local();

        assert!(profile.local);
        assert!(profile.is_privacy_safe());
        assert_eq!(profile.cost_tier, CostTier::Free);
    }

    #[test]
    fn test_model_profile_primary_capability() {
        let profile = ModelProfile::new("test", "test", "test")
            .with_capabilities(vec![Capability::Reasoning, Capability::CodeGeneration]);

        assert_eq!(profile.primary_capability(), Some(Capability::Reasoning));

        let empty_profile = ModelProfile::new("empty", "test", "test");
        assert_eq!(empty_profile.primary_capability(), None);
    }

    #[test]
    fn test_model_profile_long_context() {
        let profile_with_flag = ModelProfile::new("gemini", "google", "gemini-pro")
            .with_capabilities(vec![Capability::LongContext]);
        assert!(profile_with_flag.supports_long_context());

        let profile_with_size =
            ModelProfile::new("gemini", "google", "gemini-pro").with_max_context(1_000_000);
        assert!(profile_with_size.supports_long_context());

        let profile_small =
            ModelProfile::new("haiku", "anthropic", "haiku").with_max_context(50_000);
        assert!(!profile_small.supports_long_context());
    }

    #[test]
    fn test_model_profile_capability_set() {
        let profile = ModelProfile::new("test", "test", "test").with_capabilities(vec![
            Capability::CodeGeneration,
            Capability::CodeReview,
            Capability::CodeGeneration, // duplicate
        ]);

        let set = profile.capability_set();
        assert_eq!(set.len(), 2); // duplicates removed
        assert!(set.contains(&Capability::CodeGeneration));
        assert!(set.contains(&Capability::CodeReview));
    }

    #[test]
    fn test_model_profile_serialization() {
        let profile = ModelProfile::new("claude-opus", "anthropic", "claude-opus-4")
            .with_capabilities(vec![Capability::Reasoning])
            .with_cost_tier(CostTier::High);

        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("claude-opus"));
        assert!(json.contains("reasoning"));
        assert!(json.contains("high"));

        let parsed: ModelProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, profile.id);
        assert_eq!(parsed.cost_tier, CostTier::High);
    }

    #[test]
    fn test_model_profile_deserialization_defaults() {
        let json = r#"{"id": "test", "provider": "test", "model": "test"}"#;
        let profile: ModelProfile = serde_json::from_str(json).unwrap();

        assert_eq!(profile.id, "test");
        assert!(profile.capabilities.is_empty());
        assert_eq!(profile.cost_tier, CostTier::Medium); // default
        assert_eq!(profile.latency_tier, LatencyTier::Medium); // default
        assert!(!profile.local);
    }

    #[test]
    fn test_model_profile_display() {
        let profile = ModelProfile::new("claude-opus", "anthropic", "claude-opus-4")
            .with_cost_tier(CostTier::High);

        let display = format!("{}", profile);
        assert!(display.contains("claude-opus"));
        assert!(display.contains("anthropic"));
        assert!(display.contains("High"));
    }
}
