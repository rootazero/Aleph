//! Enum definitions for memory fact classification and metadata.
//!
//! In Aleph's memory system, a "Fact" ([`MemoryFact`](super::MemoryFact)) is the
//! universal unit of persisted knowledge — not limited to factual statements, but
//! encompassing preferences, reference documents, skills, transcripts, synthesized insights,
//! and agent experiences. Knowledge Notes are the primary structural layer, with
//! wikilink-based linking replacing the deprecated `graph_nodes/graph_edges` system.

use serde::{Deserialize, Serialize};

// ============================================================================
// NoteType
// ============================================================================

/// Type classification for memory facts
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum NoteType {
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
    /// Lesson learned from experience (symptom → cause → fix).
    Lesson,
    /// Reusable procedural knowledge extracted by LLM self-growth.
    Skill,
    /// Structured reference document (first-class knowledge document).
    Reference,
    /// User-taught correction / standing rule, distilled by `FeedbackDistill`.
    Feedback,
    /// Conversation transcript chunk (embedded for direct retrieval)
    Transcript,
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

impl NoteType {
    /// Convert to string representation
    #[must_use]
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Preference => "preference",
            Self::Plan => "plan",
            Self::Learning => "learning",
            Self::Project => "project",
            Self::Personal => "personal",
            Self::Tool => "tool",
            Self::Lesson => "lesson",
            Self::Skill => "skill",
            Self::Reference => "reference",
            Self::Feedback => "feedback",
            Self::Transcript => "transcript",
            Self::Other => "other",
            Self::SubagentRun => "subagent_run",
            Self::SubagentSession => "subagent_session",
            Self::SubagentCheckpoint => "subagent_checkpoint",
            Self::SubagentTranscript => "subagent_transcript",
        }
    }

    /// Return the filesystem category directory name for this note type.
    ///
    /// Matches the names in `CATEGORY_DIRS` (uses hyphens for subagent variants,
    /// not underscores as `as_str()` does).
    #[must_use]
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub const fn to_category_dir(&self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Plan => "plan",
            Self::Learning => "learning",
            Self::Project => "project",
            Self::Personal => "personal",
            Self::Tool => "tool",
            Self::Lesson => "lesson",
            Self::Skill => "skill",
            Self::Reference => "reference",
            Self::Feedback => "feedback",
            Self::Transcript => "transcript",
            Self::Other => "other",
            Self::SubagentRun => "subagent-run",
            Self::SubagentSession => "subagent-session",
            Self::SubagentCheckpoint => "subagent-checkpoint",
            Self::SubagentTranscript => "subagent-transcript",
        }
    }

    /// Parse from string with fallback to Other
    #[must_use]
    pub fn from_str_or_other(s: &str) -> Self {
        s.parse().unwrap_or(Self::Other)
    }

    /// Get default aleph:// path for this fact type
    #[must_use]
    pub const fn default_path(&self) -> &str {
        match self {
            Self::Preference => "aleph://user/preferences/",
            Self::Personal => "aleph://user/personal/",
            Self::Plan => "aleph://user/plans/",
            Self::Learning => "aleph://knowledge/learning/",
            Self::Project => "aleph://knowledge/projects/",
            Self::Tool => "aleph://agent/tools/",
            Self::Lesson => "aleph://knowledge/lessons/",
            Self::Skill => "aleph://skills/",
            Self::Reference => "aleph://reference/",
            Self::Feedback => "aleph://feedback/",
            Self::Transcript => crate::memory::transcript_indexer::TRANSCRIPT_PATH_PREFIX,
            Self::Other => "aleph://knowledge/",
            Self::SubagentRun
            | Self::SubagentSession
            | Self::SubagentCheckpoint
            | Self::SubagentTranscript => "aleph://agent/experiences/",
        }
    }

    /// Map fact type to standardized memory category.
    #[must_use]
    pub const fn default_category(&self) -> MemoryCategory {
        match self {
            Self::Preference => MemoryCategory::Preferences,
            Self::Plan | Self::Personal => MemoryCategory::Profile,
            Self::Learning | Self::Project | Self::Other => MemoryCategory::Entities,
            Self::Tool | Self::Skill | Self::Reference | Self::Feedback => MemoryCategory::Patterns,
            Self::Lesson => MemoryCategory::Cases,
            Self::SubagentRun | Self::SubagentSession | Self::SubagentCheckpoint => {
                MemoryCategory::Cases
            }
            Self::SubagentTranscript | Self::Transcript => MemoryCategory::Events,
        }
    }
}

impl std::str::FromStr for NoteType {
    type Err = String;

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "preference" => Ok(Self::Preference),
            "plan" => Ok(Self::Plan),
            "learning" => Ok(Self::Learning),
            "project" => Ok(Self::Project),
            "personal" => Ok(Self::Personal),
            "tool" => Ok(Self::Tool),
            "lesson" => Ok(Self::Lesson),
            "skill" => Ok(Self::Skill),
            "reference" => Ok(Self::Reference),
            "feedback" => Ok(Self::Feedback),
            "subagent_run" => Ok(Self::SubagentRun),
            "subagent_session" => Ok(Self::SubagentSession),
            "subagent_checkpoint" => Ok(Self::SubagentCheckpoint),
            "subagent_transcript" => Ok(Self::SubagentTranscript),
            "transcript" => Ok(Self::Transcript),
            "other" => Ok(Self::Other),
            _ => Err(format!("Unknown fact type: {s}")),
        }
    }
}

impl std::fmt::Display for NoteType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// FactSource
// ============================================================================

/// Origin/type of a Fact
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FactSource {
    /// LLM-extracted from conversation (existing behavior)
    #[default]
    Extracted,
    /// L1 Overview generated by `CompressionDaemon`
    Summary,
    /// User-uploaded long document (Markdown-first)
    Document,
    /// User-created manually
    Manual,
    /// Compressed summary produced by `SessionCompactor` during an active session
    SessionCompressed,
    /// Synthesized from cross-session pattern extraction during weekly dream cycles
    Synthesis,
}

impl FactSource {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Extracted => "extracted",
            Self::Summary => "summary",
            Self::Document => "document",
            Self::Manual => "manual",
            Self::SessionCompressed => "session_compressed",
            Self::Synthesis => "synthesis",
        }
    }

    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "extracted" => Self::Extracted,
            "summary" => Self::Summary,
            "document" => Self::Document,
            "manual" => Self::Manual,
            "session_compressed" => Self::SessionCompressed,
            "synthesis" => Self::Synthesis,
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
            "session_compressed" => Ok(Self::SessionCompressed),
            "synthesis" => Ok(Self::Synthesis),
            _ => Err(format!("Unknown fact source: {s}")),
        }
    }
}

impl std::fmt::Display for FactSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// MemoryLayer
// ============================================================================

/// Tiered loading level for memory retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryLayer {
    /// Abstract summary for fast scanning.
    L0Abstract,
    /// Structured overview for navigation.
    L1Overview,
    /// Full-detail content.
    #[default]
    L2Detail,
}

impl MemoryLayer {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::L0Abstract => "l0_abstract",
            Self::L1Overview => "l1_overview",
            Self::L2Detail => "l2_detail",
        }
    }

    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::L2Detail)
    }
}

impl std::str::FromStr for MemoryLayer {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "l0_abstract" => Ok(Self::L0Abstract),
            "l1_overview" => Ok(Self::L1Overview),
            "l2_detail" => Ok(Self::L2Detail),
            _ => Err(format!("Unknown memory layer: {s}")),
        }
    }
}

impl std::fmt::Display for MemoryLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// CognitiveLayer
// ============================================================================

/// Human-memory-inspired cognitive layer of a recalled memory item, mirroring
/// the reference model's working → episodic → semantic → raw hierarchy.
///
/// This is a *view* over Aleph's existing storage tiers, not a new store: it is
/// derived at render time from an item's `ItemSource` / slot (see
/// `assembler::render::cognitive_layer`) and surfaced as a label so the model
/// perceives the layered structure ("operates in layers like a human cognitive system"). Nothing is
/// persisted — the classification is deterministic and free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveLayer {
    /// Current-task context (live session / scratchpad-equivalent).
    Working,
    /// Lived experiences and causal chains — session summaries, transcripts,
    /// sub-agent runs. Time/episode-bound.
    Episodic,
    /// Distilled facts, rules, preferences and skills — the durable knowledge.
    Semantic,
    /// Audit/forensic substrate — verbatim raw fragments behind the distillations.
    Raw,
}

impl CognitiveLayer {
    /// Stable machine tag (also the serde representation).
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::Raw => "raw",
        }
    }
}

impl std::fmt::Display for CognitiveLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// MemoryCategory
// ============================================================================

/// Standardized memory categories inspired by `OpenViking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Profile,
    Preferences,
    #[default]
    Entities,
    Events,
    Cases,
    Patterns,
}

impl MemoryCategory {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Profile => "profile",
            Self::Preferences => "preferences",
            Self::Entities => "entities",
            Self::Events => "events",
            Self::Cases => "cases",
            Self::Patterns => "patterns",
        }
    }

    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::Entities)
    }
}

impl std::str::FromStr for MemoryCategory {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "profile" => Ok(Self::Profile),
            "preferences" => Ok(Self::Preferences),
            "entities" => Ok(Self::Entities),
            "events" => Ok(Self::Events),
            "cases" => Ok(Self::Cases),
            "patterns" => Ok(Self::Patterns),
            _ => Err(format!("Unknown memory category: {s}")),
        }
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// FactSpecificity
// ============================================================================

/// Fact specificity level (prevents too vague or too detailed facts)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FactSpecificity {
    /// Abstract level: cross-session synthesized insight
    Abstract,
    /// Principle level: "User prefers functional programming"
    Principle,
    /// Pattern level: "User uses Result instead of panic for error handling"
    #[default]
    Pattern,
    /// Instance level: "User used anyhow in 2025-01-15 project"
    Instance,
}

impl FactSpecificity {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Abstract => "abstract",
            Self::Principle => "principle",
            Self::Pattern => "pattern",
            Self::Instance => "instance",
        }
    }

    /// Parse from string with fallback to Pattern
    #[must_use]
    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::Pattern)
    }
}

impl std::str::FromStr for FactSpecificity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "abstract" => Ok(Self::Abstract),
            "principle" => Ok(Self::Principle),
            "pattern" => Ok(Self::Pattern),
            "instance" => Ok(Self::Instance),
            _ => Err(format!("Unknown fact specificity: {s}")),
        }
    }
}

impl std::fmt::Display for FactSpecificity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// TemporalScope
// ============================================================================

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
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Permanent => "permanent",
            Self::Contextual => "contextual",
            Self::Ephemeral => "ephemeral",
        }
    }

    /// Parse from string with fallback to Contextual
    #[must_use]
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
            _ => Err(format!("Unknown temporal scope: {s}")),
        }
    }
}

impl std::fmt::Display for TemporalScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_note_type_roundtrips() {
        let ft = NoteType::Reference;
        assert_eq!(ft.as_str(), "reference");
        assert_eq!(ft.to_string(), "reference");
        let parsed: NoteType = "reference".parse().unwrap();
        assert_eq!(parsed, NoteType::Reference);
    }

    #[test]
    fn reference_default_path() {
        assert_eq!(NoteType::Reference.default_path(), "aleph://reference/");
    }

    #[test]
    fn reference_default_category() {
        assert_eq!(
            NoteType::Reference.default_category(),
            MemoryCategory::Patterns
        );
    }

    #[test]
    fn feedback_note_type_roundtrips() {
        let ft = NoteType::Feedback;
        assert_eq!(ft.as_str(), "feedback");
        assert_eq!(ft.to_category_dir(), "feedback");
        assert_eq!("feedback".parse::<NoteType>().unwrap(), NoteType::Feedback);
        // Round-trips cleanly now instead of mangling to Other.
        assert_eq!(NoteType::from_str_or_other("feedback"), NoteType::Feedback);
    }

    #[test]
    fn feedback_category_is_valid_dir() {
        use crate::memory::notes::CATEGORY_DIRS;
        assert!(
            CATEGORY_DIRS.contains(&NoteType::Feedback.to_category_dir()),
            "feedback must be a scannable category dir so cold rebuild_index keeps feedback notes"
        );
    }
}
