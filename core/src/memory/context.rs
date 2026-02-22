/// Context capture data structures for memory anchors
use serde::{Deserialize, Serialize};

use crate::domain::{AggregateRoot, Entity};

/// Context anchor that identifies when and where an interaction occurred
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAnchor {
    /// Application bundle ID (e.g., "com.apple.Notes")
    pub app_bundle_id: String,
    /// Window title (e.g., "Project Plan.txt")
    pub window_title: String,
    /// Unix timestamp when interaction occurred
    pub timestamp: i64,
    /// Topic ID for associating memories with conversation topics
    /// For multi-turn: specific topic UUID; For single-turn: "single-turn" constant
    pub topic_id: String,
}

/// Default topic ID for single-turn interactions
pub const SINGLE_TURN_TOPIC_ID: &str = "single-turn";

impl ContextAnchor {
    /// Create a new context anchor with current timestamp (for single-turn)
    pub fn now(app_bundle_id: String, window_title: String) -> Self {
        Self::with_topic(
            app_bundle_id,
            window_title,
            SINGLE_TURN_TOPIC_ID.to_string(),
        )
    }

    /// Create context anchor with specific timestamp (for single-turn)
    pub fn with_timestamp(app_bundle_id: String, window_title: String, timestamp: i64) -> Self {
        Self {
            app_bundle_id,
            window_title,
            timestamp,
            topic_id: SINGLE_TURN_TOPIC_ID.to_string(),
        }
    }

    /// Create context anchor with topic ID (for multi-turn conversations)
    pub fn with_topic(app_bundle_id: String, window_title: String, topic_id: String) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            app_bundle_id,
            window_title,
            timestamp,
            topic_id,
        }
    }
}

/// Memory entry representing a stored interaction
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    /// Unique identifier (UUID)
    pub id: String,
    /// Context anchor (app + window + time)
    pub context: ContextAnchor,
    /// Original user input
    pub user_input: String,
    /// AI response
    pub ai_output: String,
    /// Vector embedding (384-dim for multilingual-e5-small)
    pub embedding: Option<Vec<f32>>,
    /// Similarity score (when retrieved from search)
    pub similarity_score: Option<f32>,
}

impl MemoryEntry {
    /// Create new memory entry without embedding
    pub fn new(id: String, context: ContextAnchor, user_input: String, ai_output: String) -> Self {
        Self {
            id,
            context,
            user_input,
            ai_output,
            embedding: None,
            similarity_score: None,
        }
    }

    /// Create memory entry with embedding
    pub fn with_embedding(
        id: String,
        context: ContextAnchor,
        user_input: String,
        ai_output: String,
        embedding: Vec<f32>,
    ) -> Self {
        Self {
            id,
            context,
            user_input,
            ai_output,
            embedding: Some(embedding),
            similarity_score: None,
        }
    }

    /// Set similarity score (used during retrieval)
    pub fn with_score(mut self, score: f32) -> Self {
        self.similarity_score = Some(score);
        self
    }
}

// ============================================================================
// Memory Compression: Fact Types and Structures
// ============================================================================

/// Type classification for memory facts
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum FactType {
    /// User preferences (likes, habits, style choices)
    Preference,
    /// User plans, goals, or intentions
    Plan,
    /// Learning or skill-related information
    Learning,
    /// Project or work-related information
    Project,
    /// Personal information (non-sensitive)
    Personal,
    /// Tool/capability procedural knowledge (for tool-as-resource)
    Tool,
    /// Other facts that don't fit above categories
    #[default]
    Other,
    // Multi-Agent 2.0 fact types
    /// Sub-agent run record (task execution metadata)
    SubagentRun,
    /// Sub-agent session state
    SubagentSession,
    /// Sub-agent checkpoint for resumption
    SubagentCheckpoint,
    /// Sub-agent conversation transcript
    SubagentTranscript,
}

impl FactType {
    /// Convert to string representation
    pub fn as_str(&self) -> &str {
        match self {
            FactType::Preference => "preference",
            FactType::Plan => "plan",
            FactType::Learning => "learning",
            FactType::Project => "project",
            FactType::Personal => "personal",
            FactType::Tool => "tool",
            FactType::Other => "other",
            FactType::SubagentRun => "subagent_run",
            FactType::SubagentSession => "subagent_session",
            FactType::SubagentCheckpoint => "subagent_checkpoint",
            FactType::SubagentTranscript => "subagent_transcript",
        }
    }

    /// Parse from string with fallback to Other
    pub fn from_str_or_other(s: &str) -> Self {
        s.parse().unwrap_or(FactType::Other)
    }

    /// Get default aleph:// path for this fact type
    pub fn default_path(&self) -> &str {
        match self {
            FactType::Preference => "aleph://user/preferences/",
            FactType::Personal => "aleph://user/personal/",
            FactType::Plan => "aleph://user/plans/",
            FactType::Learning => "aleph://knowledge/learning/",
            FactType::Project => "aleph://knowledge/projects/",
            FactType::Tool => "aleph://agent/tools/",
            FactType::Other => "aleph://knowledge/",
            FactType::SubagentRun
            | FactType::SubagentSession
            | FactType::SubagentCheckpoint
            | FactType::SubagentTranscript => "aleph://agent/experiences/",
        }
    }
}

impl std::str::FromStr for FactType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "preference" => Ok(FactType::Preference),
            "plan" => Ok(FactType::Plan),
            "learning" => Ok(FactType::Learning),
            "project" => Ok(FactType::Project),
            "personal" => Ok(FactType::Personal),
            "tool" => Ok(FactType::Tool),
            "subagent_run" => Ok(FactType::SubagentRun),
            "subagent_session" => Ok(FactType::SubagentSession),
            "subagent_checkpoint" => Ok(FactType::SubagentCheckpoint),
            "subagent_transcript" => Ok(FactType::SubagentTranscript),
            "other" => Ok(FactType::Other),
            _ => Err(format!("Unknown fact type: {}", s)),
        }
    }
}

impl std::fmt::Display for FactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Origin/type of a Fact
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FactSource {
    /// LLM-extracted from conversation (existing behavior)
    #[default]
    Extracted,
    /// L1 Overview generated by CompressionDaemon
    Summary,
    /// User-uploaded long document (Markdown-first)
    Document,
    /// User-created manually
    Manual,
}

impl FactSource {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Extracted => "extracted",
            Self::Summary => "summary",
            Self::Document => "document",
            Self::Manual => "manual",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "extracted" => Self::Extracted,
            "summary" => Self::Summary,
            "document" => Self::Document,
            "manual" => Self::Manual,
            _ => Self::Extracted,
        }
    }
}

impl std::str::FromStr for FactSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "extracted" => Ok(Self::Extracted),
            "summary" => Ok(Self::Summary),
            "document" => Ok(Self::Document),
            "manual" => Ok(Self::Manual),
            _ => Err(format!("Unknown fact source: {}", s)),
        }
    }
}

impl std::fmt::Display for FactSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Preset directory paths for the aleph:// VFS
pub const PRESET_PATHS: &[(&str, &str)] = &[
    ("aleph://user/", "User domain root"),
    ("aleph://user/preferences/", "User preferences"),
    ("aleph://user/personal/", "Personal information"),
    ("aleph://user/plans/", "User plans and goals"),
    ("aleph://knowledge/", "Knowledge domain root"),
    ("aleph://knowledge/learning/", "Learning records"),
    ("aleph://knowledge/projects/", "Project knowledge"),
    ("aleph://agent/", "Agent domain root"),
    ("aleph://agent/tools/", "Tool usage experiences"),
    ("aleph://agent/experiences/", "Cortex experiences"),
    ("aleph://session/", "Session temporary data"),
];

/// Fact specificity level (prevents too vague or too detailed facts)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FactSpecificity {
    /// Principle level: "User prefers functional programming"
    Principle,
    /// Pattern level: "User uses Result instead of panic for error handling"
    #[default]
    Pattern,
    /// Instance level: "User used anyhow in 2025-01-15 project"
    Instance,
}

impl FactSpecificity {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Principle => "principle",
            Self::Pattern => "pattern",
            Self::Instance => "instance",
        }
    }

    /// Parse from string with fallback to Pattern
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::Pattern)
    }
}

impl std::str::FromStr for FactSpecificity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "principle" => Ok(Self::Principle),
            "pattern" => Ok(Self::Pattern),
            "instance" => Ok(Self::Instance),
            _ => Err(format!("Unknown fact specificity: {}", s)),
        }
    }
}

impl std::fmt::Display for FactSpecificity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Temporal scope of a fact
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TemporalScope {
    /// Long-term valid: "User's native language is Chinese"
    Permanent,
    /// Context-related: "User is working on Aleph project"
    #[default]
    Contextual,
    /// Short-term valid: "User wants to focus on docs today"
    Ephemeral,
}

impl TemporalScope {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Permanent => "permanent",
            Self::Contextual => "contextual",
            Self::Ephemeral => "ephemeral",
        }
    }

    /// Parse from string with fallback to Contextual
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::Contextual)
    }
}

impl std::str::FromStr for TemporalScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "permanent" => Ok(Self::Permanent),
            "contextual" => Ok(Self::Contextual),
            "ephemeral" => Ok(Self::Ephemeral),
            _ => Err(format!("Unknown temporal scope: {}", s)),
        }
    }
}

impl std::fmt::Display for TemporalScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}


/// Compute parent path from a VFS path
/// "aleph://user/preferences/coding/" -> "aleph://user/preferences/"
/// "aleph://user/preferences/" -> "aleph://user/"
/// "aleph://user/" -> "aleph://"
pub fn compute_parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(pos) => format!("{}/", &trimmed[..pos]),
        None => String::new(),
    }
}

/// A compressed memory fact extracted from conversations by LLM
///
/// Facts are third-person statements about the user, such as:
/// - "The user is learning Rust programming language"
/// - "The user prefers using Vim for coding"
/// - "The user plans to travel to Tokyo next week"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFact {
    /// Unique identifier (UUID)
    pub id: String,
    /// Fact content (third-person statement)
    pub content: String,
    /// Type classification
    pub fact_type: FactType,
    /// Vector embedding (384-dim for multilingual-e5-small)
    pub embedding: Option<Vec<f32>>,
    /// Source memory IDs for traceability
    pub source_memory_ids: Vec<String>,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
    /// Confidence score (0.0-1.0) from LLM
    pub confidence: f32,
    /// Whether this fact is still valid (soft delete)
    pub is_valid: bool,
    /// Reason for invalidation (if is_valid = false)
    pub invalidation_reason: Option<String>,
    /// Timestamp when fact was invalidated due to decay (Unix seconds)
    /// Used for recycle bin retention period
    pub decay_invalidated_at: Option<i64>,
    /// Fact specificity level
    pub specificity: FactSpecificity,
    /// Temporal scope
    pub temporal_scope: TemporalScope,
    /// Similarity score (when retrieved from search)
    #[serde(skip)]
    pub similarity_score: Option<f32>,
    /// VFS path for hierarchical organization (e.g., "aleph://user/preferences/coding")
    pub path: String,
    /// Fact origin/type
    pub fact_source: FactSource,
    /// Content hash for L1 staleness detection
    pub content_hash: String,
    /// Parent path for ls operations
    pub parent_path: String,
    /// Name of the embedding model that generated this fact's vector
    pub embedding_model: String,
}

impl Entity for MemoryFact {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl AggregateRoot for MemoryFact {}

impl MemoryFact {
    /// Create a new valid memory fact
    pub fn new(content: String, fact_type: FactType, source_ids: Vec<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let path = fact_type.default_path().to_string();
        let parent_path = compute_parent_path(&path);

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            fact_type,
            embedding: None,
            source_memory_ids: source_ids,
            created_at: now,
            updated_at: now,
            confidence: 1.0,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
            path,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path,
            embedding_model: String::new(),
        }
    }

    /// Create a new fact with specific ID (for database reconstruction)
    pub fn with_id(id: String, content: String, fact_type: FactType) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let path = fact_type.default_path().to_string();
        let parent_path = compute_parent_path(&path);

        Self {
            id,
            content,
            fact_type,
            embedding: None,
            source_memory_ids: Vec::new(),
            created_at: now,
            updated_at: now,
            confidence: 1.0,
            is_valid: true,
            invalidation_reason: None,
            decay_invalidated_at: None,
            specificity: FactSpecificity::default(),
            temporal_scope: TemporalScope::default(),
            similarity_score: None,
            path,
            fact_source: FactSource::Extracted,
            content_hash: String::new(),
            parent_path,
            embedding_model: String::new(),
        }
    }

    /// Add embedding to the fact
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set confidence score
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Set similarity score (used during retrieval)
    pub fn with_score(mut self, score: f32) -> Self {
        self.similarity_score = Some(score);
        self
    }

    /// Set specificity level
    pub fn with_specificity(mut self, specificity: FactSpecificity) -> Self {
        self.specificity = specificity;
        self
    }

    /// Set temporal scope
    pub fn with_temporal_scope(mut self, scope: TemporalScope) -> Self {
        self.temporal_scope = scope;
        self
    }

    /// Set VFS path
    pub fn with_path(mut self, path: String) -> Self {
        self.parent_path = compute_parent_path(&path);
        self.path = path;
        self
    }

    /// Set fact source
    pub fn with_fact_source(mut self, source: FactSource) -> Self {
        self.fact_source = source;
        self
    }

    /// Invalidate this fact with a reason
    pub fn invalidate(mut self, reason: &str) -> Self {
        self.is_valid = false;
        self.invalidation_reason = Some(reason.to_string());
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        self
    }
}

/// Record of a compression session for auditing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionSession {
    /// Session ID (UUID)
    pub id: String,
    /// Source memory IDs that were compressed
    pub source_memory_ids: Vec<String>,
    /// Extracted fact IDs
    pub extracted_fact_ids: Vec<String>,
    /// Compression timestamp
    pub compressed_at: i64,
    /// AI provider used for extraction
    pub provider_used: String,
    /// Compression duration in milliseconds
    pub duration_ms: u64,
}

impl CompressionSession {
    /// Create a new compression session record
    pub fn new(
        source_memory_ids: Vec<String>,
        extracted_fact_ids: Vec<String>,
        provider_used: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            source_memory_ids,
            extracted_fact_ids,
            compressed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            provider_used,
            duration_ms,
        }
    }
}

/// Statistics for memory facts
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FactStats {
    /// Total number of facts
    pub total_facts: u64,
    /// Number of valid (non-invalidated) facts
    pub valid_facts: u64,
    /// Breakdown by fact type
    pub facts_by_type: std::collections::HashMap<String, u64>,
    /// Oldest fact timestamp
    pub oldest_fact_timestamp: Option<i64>,
    /// Newest fact timestamp
    pub newest_fact_timestamp: Option<i64>,
}

/// Result of a compression operation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompressionResult {
    /// Number of memories processed
    pub memories_processed: u32,
    /// Number of facts extracted
    pub facts_extracted: u32,
    /// Number of old facts invalidated due to conflicts
    pub facts_invalidated: u32,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

impl CompressionResult {
    /// Create an empty result (no work done)
    pub fn empty() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_anchor_now() {
        let anchor = ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string());
        assert_eq!(anchor.app_bundle_id, "com.apple.Notes");
        assert_eq!(anchor.window_title, "Test.txt");
        assert!(anchor.timestamp > 0);
    }

    #[test]
    fn test_context_anchor_with_timestamp() {
        let anchor = ContextAnchor::with_timestamp(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
            1234567890,
        );
        assert_eq!(anchor.timestamp, 1234567890);
    }

    #[test]
    fn test_memory_entry_new() {
        let context = ContextAnchor::now("app".to_string(), "window".to_string());
        let entry = MemoryEntry::new(
            "id-123".to_string(),
            context.clone(),
            "user input".to_string(),
            "ai output".to_string(),
        );
        assert_eq!(entry.id, "id-123");
        assert_eq!(entry.context, context);
        assert!(entry.embedding.is_none());
        assert!(entry.similarity_score.is_none());
    }

    #[test]
    fn test_memory_entry_with_embedding() {
        let context = ContextAnchor::now("app".to_string(), "window".to_string());
        let embedding = vec![0.1, 0.2, 0.3];
        let entry = MemoryEntry::with_embedding(
            "id-123".to_string(),
            context,
            "input".to_string(),
            "output".to_string(),
            embedding.clone(),
        );
        assert_eq!(entry.embedding, Some(embedding));
    }

    #[test]
    fn test_memory_entry_with_score() {
        let context = ContextAnchor::now("app".to_string(), "window".to_string());
        let entry = MemoryEntry::new(
            "id".to_string(),
            context,
            "in".to_string(),
            "out".to_string(),
        )
        .with_score(0.85);
        assert_eq!(entry.similarity_score, Some(0.85));
    }

    #[test]
    fn test_context_anchor_serialization() {
        let anchor = ContextAnchor::with_timestamp(
            "com.apple.Notes".to_string(),
            "Test.txt".to_string(),
            1234567890,
        );
        let json = serde_json::to_string(&anchor).unwrap();
        let deserialized: ContextAnchor = serde_json::from_str(&json).unwrap();
        assert_eq!(anchor, deserialized);
    }

    #[test]
    fn test_fact_specificity() {
        let fact = MemoryFact::new(
            "User prefers Rust".to_string(),
            FactType::Preference,
            vec!["mem-1".to_string()],
        )
        .with_specificity(FactSpecificity::Pattern)
        .with_temporal_scope(TemporalScope::Permanent);

        assert_eq!(fact.specificity, FactSpecificity::Pattern);
        assert_eq!(fact.temporal_scope, TemporalScope::Permanent);
    }

    #[test]
    fn test_specificity_from_str() {
        assert_eq!(
            FactSpecificity::from_str_or_default("principle"),
            FactSpecificity::Principle
        );
        assert_eq!(
            FactSpecificity::from_str_or_default("PATTERN"),
            FactSpecificity::Pattern
        );
        assert_eq!(
            FactSpecificity::from_str_or_default("instance"),
            FactSpecificity::Instance
        );
        assert_eq!(
            FactSpecificity::from_str_or_default("unknown"),
            FactSpecificity::Pattern
        ); // default
    }

    #[test]
    fn test_temporal_scope_from_str() {
        assert_eq!(
            TemporalScope::from_str_or_default("permanent"),
            TemporalScope::Permanent
        );
        assert_eq!(
            TemporalScope::from_str_or_default("CONTEXTUAL"),
            TemporalScope::Contextual
        );
        assert_eq!(
            TemporalScope::from_str_or_default("ephemeral"),
            TemporalScope::Ephemeral
        );
        assert_eq!(
            TemporalScope::from_str_or_default("unknown"),
            TemporalScope::Contextual
        ); // default
    }

    #[test]
    fn test_fact_specificity_default() {
        let fact = MemoryFact::new(
            "User likes coding".to_string(),
            FactType::Preference,
            vec![],
        );
        // Default should be Pattern and Contextual
        assert_eq!(fact.specificity, FactSpecificity::Pattern);
        assert_eq!(fact.temporal_scope, TemporalScope::Contextual);
    }

    #[test]
    fn test_fact_specificity_as_str() {
        assert_eq!(FactSpecificity::Principle.as_str(), "principle");
        assert_eq!(FactSpecificity::Pattern.as_str(), "pattern");
        assert_eq!(FactSpecificity::Instance.as_str(), "instance");
    }

    #[test]
    fn test_temporal_scope_as_str() {
        assert_eq!(TemporalScope::Permanent.as_str(), "permanent");
        assert_eq!(TemporalScope::Contextual.as_str(), "contextual");
        assert_eq!(TemporalScope::Ephemeral.as_str(), "ephemeral");
    }

    #[test]
    fn test_fact_type_tool() {
        assert_eq!(FactType::Tool.as_str(), "tool");
        assert_eq!(FactType::from_str_or_other("tool"), FactType::Tool);
    }

    #[test]
    fn test_subagent_fact_types() {
        assert_eq!(FactType::from_str_or_other("subagent_run"), FactType::SubagentRun);
        assert_eq!(FactType::from_str_or_other("subagent_session"), FactType::SubagentSession);
        assert_eq!(FactType::from_str_or_other("subagent_checkpoint"), FactType::SubagentCheckpoint);
        assert_eq!(FactType::from_str_or_other("subagent_transcript"), FactType::SubagentTranscript);
        assert_eq!(FactType::SubagentRun.as_str(), "subagent_run");
        assert_eq!(FactType::SubagentSession.as_str(), "subagent_session");
        assert_eq!(FactType::SubagentCheckpoint.as_str(), "subagent_checkpoint");
        assert_eq!(FactType::SubagentTranscript.as_str(), "subagent_transcript");
    }

    #[test]
    fn test_fact_source_roundtrip() {
        assert_eq!(FactSource::Extracted.as_str(), "extracted");
        assert_eq!(FactSource::Summary.as_str(), "summary");
        assert_eq!(FactSource::Document.as_str(), "document");
        assert_eq!(FactSource::Manual.as_str(), "manual");
        assert_eq!(FactSource::from_str_or_default("summary"), FactSource::Summary);
        assert_eq!(FactSource::from_str_or_default("unknown"), FactSource::Extracted);
    }

    #[test]
    fn test_fact_type_default_path() {
        assert_eq!(FactType::Preference.default_path(), "aleph://user/preferences/");
        assert_eq!(FactType::Personal.default_path(), "aleph://user/personal/");
        assert_eq!(FactType::Plan.default_path(), "aleph://user/plans/");
        assert_eq!(FactType::Learning.default_path(), "aleph://knowledge/learning/");
        assert_eq!(FactType::Project.default_path(), "aleph://knowledge/projects/");
        assert_eq!(FactType::Tool.default_path(), "aleph://agent/tools/");
        assert_eq!(FactType::Other.default_path(), "aleph://knowledge/");
        assert_eq!(FactType::SubagentRun.default_path(), "aleph://agent/experiences/");
    }

    #[test]
    fn test_memory_fact_new_has_path_fields() {
        let fact = MemoryFact::new(
            "User prefers Rust".to_string(),
            FactType::Preference,
            vec!["src-1".to_string()],
        );
        assert_eq!(fact.path, "aleph://user/preferences/");
        assert_eq!(fact.parent_path, "aleph://user/");
        assert_eq!(fact.fact_source, FactSource::Extracted);
        assert!(fact.content_hash.is_empty());
    }

    #[test]
    fn test_memory_fact_with_path() {
        let fact = MemoryFact::new(
            "Learning WebAssembly".to_string(),
            FactType::Learning,
            vec![],
        )
        .with_path("aleph://knowledge/learning/wasm/".to_string());
        assert_eq!(fact.path, "aleph://knowledge/learning/wasm/");
        assert_eq!(fact.parent_path, "aleph://knowledge/learning/");
    }

    #[test]
    fn test_compute_parent_path() {
        assert_eq!(compute_parent_path("aleph://user/preferences/coding/"), "aleph://user/preferences/");
        assert_eq!(compute_parent_path("aleph://user/preferences/"), "aleph://user/");
        assert_eq!(compute_parent_path("aleph://user/"), "aleph://");
        assert_eq!(compute_parent_path(""), "");
    }
}
