//! Core types for the Cortex evolution system
//!
//! Defines the data structures for experience replay, distillation,
//! and skill evolution.

use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// Enums
// =============================================================================

/// Evolution status of an experience
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionStatus {
    /// Newly created experience, not yet verified
    Candidate,
    /// Verified through multiple successful replays
    Verified,
    /// Distilled into executable code (Hard Skill)
    Distilled,
    /// Archived due to low usage or obsolescence
    Archived,
}

impl fmt::Display for EvolutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvolutionStatus::Candidate => write!(f, "candidate"),
            EvolutionStatus::Verified => write!(f, "verified"),
            EvolutionStatus::Distilled => write!(f, "distilled"),
            EvolutionStatus::Archived => write!(f, "archived"),
        }
    }
}

impl EvolutionStatus {
    /// Parse status from database string with fallback to Candidate
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(EvolutionStatus::Candidate)
    }

    /// Check if experience can be used for replay
    pub fn is_replayable(&self) -> bool {
        matches!(self, EvolutionStatus::Verified | EvolutionStatus::Distilled)
    }
}

impl std::str::FromStr for EvolutionStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "candidate" => Ok(EvolutionStatus::Candidate),
            "verified" => Ok(EvolutionStatus::Verified),
            "distilled" => Ok(EvolutionStatus::Distilled),
            "archived" => Ok(EvolutionStatus::Archived),
            _ => Err(format!("Unknown evolution status: {}", s)),
        }
    }
}

/// Distillation mode (realtime vs batch)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistillationMode {
    /// Realtime distillation (triggered immediately after task completion)
    RealTime,
    /// Batch distillation (triggered by Dreaming process)
    Batch,
}

impl fmt::Display for DistillationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DistillationMode::RealTime => write!(f, "realtime"),
            DistillationMode::Batch => write!(f, "batch"),
        }
    }
}

// =============================================================================
// Core Structs
// =============================================================================

/// Experience replay entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    /// Unique identifier
    pub id: String,

    /// Pattern hash (for deduplication and clustering)
    pub pattern_hash: String,

    /// Intent embedding vector
    pub intent_vector: Option<Vec<f32>>,

    /// Original user intent
    pub user_intent: String,

    /// Environment context (JSON)
    pub environment_context_json: Option<String>,

    /// Distilled thought trace
    pub thought_trace_distilled: Option<String>,

    /// Tool sequence (JSON)
    pub tool_sequence_json: String,

    /// Parameter mapping (JSON)
    pub parameter_mapping: Option<String>,

    /// Logic trace for code generation (JSON)
    pub logic_trace_json: Option<String>,

    /// Success score (0.0-1.0)
    pub success_score: f64,

    /// Token efficiency
    pub token_efficiency: Option<f64>,

    /// Latency in milliseconds
    pub latency_ms: Option<i64>,

    /// Novelty score (0.0-1.0)
    pub novelty_score: Option<f64>,

    /// Evolution status
    pub evolution_status: EvolutionStatus,

    /// Usage count
    pub usage_count: i64,

    /// Success count
    pub success_count: i64,

    /// Last success rate
    pub last_success_rate: Option<f64>,

    /// Created timestamp
    pub created_at: i64,

    /// Last used timestamp
    pub last_used_at: i64,

    /// Last evaluated timestamp
    pub last_evaluated_at: Option<i64>,
}

/// Distillation task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistillationTask {
    /// Task trace ID to distill
    pub trace_id: String,

    /// Distillation mode
    pub mode: DistillationMode,
}

/// Replay match result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayMatch {
    /// Experience ID
    pub experience_id: String,

    /// Tool sequence to execute
    pub tool_sequence: String,

    /// Confidence score (0.0-1.0)
    pub confidence: f64,
}

/// Parameter extraction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterConfig {
    /// Parameter type (path, string, number, etc.)
    #[serde(rename = "type")]
    pub param_type: String,

    /// Extraction rule (regex, keyword_after, etc.)
    pub extraction_rule: String,

    /// Default value if extraction fails
    pub default: Option<String>,
}

/// Parameter mapping for parametric replay
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterMapping {
    /// Variable name -> extraction config
    pub variables: std::collections::HashMap<String, ParameterConfig>,
}

/// Environment context snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentContext {
    /// Working directory
    pub working_directory: String,

    /// Platform (macos, linux, windows)
    pub platform: String,

    /// User permissions
    pub permissions: Vec<String>,

    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

// =============================================================================
// Builder Pattern
// =============================================================================

/// Builder for Experience
pub struct ExperienceBuilder {
    experience: Experience,
}

impl ExperienceBuilder {
    pub fn new(id: String, user_intent: String, tool_sequence_json: String) -> Self {
        Self {
            experience: Experience {
                id,
                pattern_hash: String::new(),
                intent_vector: None,
                user_intent,
                environment_context_json: None,
                thought_trace_distilled: None,
                tool_sequence_json,
                parameter_mapping: None,
                logic_trace_json: None,
                success_score: 0.0,
                token_efficiency: None,
                latency_ms: None,
                novelty_score: None,
                evolution_status: EvolutionStatus::Candidate,
                usage_count: 1,
                success_count: 0,
                last_success_rate: None,
                created_at: chrono::Utc::now().timestamp(),
                last_used_at: chrono::Utc::now().timestamp(),
                last_evaluated_at: None,
            },
        }
    }

    pub fn pattern_hash(mut self, hash: String) -> Self {
        self.experience.pattern_hash = hash;
        self
    }

    pub fn intent_vector(mut self, vector: Vec<f32>) -> Self {
        self.experience.intent_vector = Some(vector);
        self
    }

    pub fn success_score(mut self, score: f64) -> Self {
        self.experience.success_score = score;
        self
    }

    pub fn token_efficiency(mut self, efficiency: f64) -> Self {
        self.experience.token_efficiency = Some(efficiency);
        self
    }

    pub fn latency_ms(mut self, latency: i64) -> Self {
        self.experience.latency_ms = Some(latency);
        self
    }

    pub fn novelty_score(mut self, score: f64) -> Self {
        self.experience.novelty_score = Some(score);
        self
    }

    pub fn environment_context_json(mut self, json: String) -> Self {
        self.experience.environment_context_json = Some(json);
        self
    }

    pub fn parameter_mapping(mut self, mapping: String) -> Self {
        self.experience.parameter_mapping = Some(mapping);
        self
    }

    pub fn build(self) -> Experience {
        self.experience
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evolution_status_from_str() {
        assert_eq!(EvolutionStatus::from_str_or_default("candidate"), EvolutionStatus::Candidate);
        assert_eq!(EvolutionStatus::from_str_or_default("verified"), EvolutionStatus::Verified);
        assert_eq!(EvolutionStatus::from_str_or_default("distilled"), EvolutionStatus::Distilled);
        assert_eq!(EvolutionStatus::from_str_or_default("archived"), EvolutionStatus::Archived);
        assert_eq!(EvolutionStatus::from_str_or_default("unknown"), EvolutionStatus::Candidate);
    }

    #[test]
    fn test_evolution_status_is_replayable() {
        assert!(!EvolutionStatus::Candidate.is_replayable());
        assert!(EvolutionStatus::Verified.is_replayable());
        assert!(EvolutionStatus::Distilled.is_replayable());
        assert!(!EvolutionStatus::Archived.is_replayable());
    }

    #[test]
    fn test_experience_builder() {
        let exp = ExperienceBuilder::new(
            "test-id".to_string(),
            "test intent".to_string(),
            "{}".to_string(),
        )
        .pattern_hash("hash123".to_string())
        .success_score(0.95)
        .build();

        assert_eq!(exp.id, "test-id");
        assert_eq!(exp.pattern_hash, "hash123");
        assert_eq!(exp.success_score, 0.95);
        assert_eq!(exp.evolution_status, EvolutionStatus::Candidate);
    }
}
