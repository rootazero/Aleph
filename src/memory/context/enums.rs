//! Enum definitions for memory fact classification and metadata.
//!
//! In Aleph's memory system, a "Fact" ([`MemoryFact`](super::MemoryFact)) is the
//! universal unit of persisted knowledge — not limited to factual statements, but
//! encompassing preferences, wiki pages, skills, transcripts, synthesized insights,
//! and agent experiences. Knowledge Notes are the primary structural layer, with
//! wikilink-based linking replacing the deprecated graph_nodes/graph_edges system.

use schemars::JsonSchema;
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
    /// Structured Markdown wiki page (first-class knowledge document).
    Wiki,
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
    pub fn as_str(&self) -> &str {
        match self {
            NoteType::Preference => "preference",
            NoteType::Plan => "plan",
            NoteType::Learning => "learning",
            NoteType::Project => "project",
            NoteType::Personal => "personal",
            NoteType::Tool => "tool",
            NoteType::Lesson => "lesson",
            NoteType::Skill => "skill",
            NoteType::Wiki => "wiki",
            NoteType::Transcript => "transcript",
            NoteType::Other => "other",
            NoteType::SubagentRun => "subagent_run",
            NoteType::SubagentSession => "subagent_session",
            NoteType::SubagentCheckpoint => "subagent_checkpoint",
            NoteType::SubagentTranscript => "subagent_transcript",
        }
    }

    /// Parse from string with fallback to Other
    pub fn from_str_or_other(s: &str) -> Self {
        s.parse().unwrap_or(NoteType::Other)
    }

    /// Get default aleph:// path for this fact type
    pub fn default_path(&self) -> &str {
        match self {
            NoteType::Preference => "aleph://user/preferences/",
            NoteType::Personal => "aleph://user/personal/",
            NoteType::Plan => "aleph://user/plans/",
            NoteType::Learning => "aleph://knowledge/learning/",
            NoteType::Project => "aleph://knowledge/projects/",
            NoteType::Tool => "aleph://agent/tools/",
            NoteType::Lesson => "aleph://knowledge/lessons/",
            NoteType::Skill => "aleph://skills/",
            NoteType::Wiki => "aleph://wiki/",
            NoteType::Transcript => "aleph://transcript/",
            NoteType::Other => "aleph://knowledge/",
            NoteType::SubagentRun
            | NoteType::SubagentSession
            | NoteType::SubagentCheckpoint
            | NoteType::SubagentTranscript => "aleph://agent/experiences/",
        }
    }

    /// Map fact type to standardized memory category.
    pub fn default_category(&self) -> MemoryCategory {
        match self {
            NoteType::Preference => MemoryCategory::Preferences,
            NoteType::Plan | NoteType::Personal => MemoryCategory::Profile,
            NoteType::Learning | NoteType::Project | NoteType::Other => MemoryCategory::Entities,
            NoteType::Tool | NoteType::Skill | NoteType::Wiki => MemoryCategory::Patterns,
            NoteType::Lesson => MemoryCategory::Cases,
            NoteType::SubagentRun | NoteType::SubagentSession | NoteType::SubagentCheckpoint => {
                MemoryCategory::Cases
            }
            NoteType::SubagentTranscript | NoteType::Transcript => MemoryCategory::Events,
        }
    }
}

impl std::str::FromStr for NoteType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "preference" => Ok(NoteType::Preference),
            "plan" => Ok(NoteType::Plan),
            "learning" => Ok(NoteType::Learning),
            "project" => Ok(NoteType::Project),
            "personal" => Ok(NoteType::Personal),
            "tool" => Ok(NoteType::Tool),
            "lesson" => Ok(NoteType::Lesson),
            "skill" => Ok(NoteType::Skill),
            "wiki" => Ok(NoteType::Wiki),
            "subagent_run" => Ok(NoteType::SubagentRun),
            "subagent_session" => Ok(NoteType::SubagentSession),
            "subagent_checkpoint" => Ok(NoteType::SubagentCheckpoint),
            "subagent_transcript" => Ok(NoteType::SubagentTranscript),
            "transcript" => Ok(NoteType::Transcript),
            "other" => Ok(NoteType::Other),
            _ => Err(format!("Unknown fact type: {}", s)),
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
    /// L1 Overview generated by CompressionDaemon
    Summary,
    /// User-uploaded long document (Markdown-first)
    Document,
    /// User-created manually
    Manual,
    /// Compressed summary produced by SessionCompactor during an active session
    SessionCompressed,
    /// Synthesized from cross-session pattern extraction during weekly dream cycles
    Synthesis,
}

impl FactSource {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Extracted => "extracted",
            Self::Summary => "summary",
            Self::Document => "document",
            Self::Manual => "manual",
            Self::SessionCompressed => "session_compressed",
            Self::Synthesis => "synthesis",
        }
    }

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
            _ => Err(format!("Unknown fact source: {}", s)),
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
    pub fn as_str(&self) -> &str {
        match self {
            Self::L0Abstract => "l0_abstract",
            Self::L1Overview => "l1_overview",
            Self::L2Detail => "l2_detail",
        }
    }

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
            _ => Err(format!("Unknown memory layer: {}", s)),
        }
    }
}

impl std::fmt::Display for MemoryLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// MemoryCategory
// ============================================================================

/// Standardized memory categories inspired by OpenViking.
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
    pub fn as_str(&self) -> &str {
        match self {
            Self::Profile => "profile",
            Self::Preferences => "preferences",
            Self::Entities => "entities",
            Self::Events => "events",
            Self::Cases => "cases",
            Self::Patterns => "patterns",
        }
    }

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
            _ => Err(format!("Unknown memory category: {}", s)),
        }
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// MemoryTier
// ============================================================================

/// Memory tier for cognitive architecture.
///
/// Controls how a fact is treated during retrieval and decay:
/// - **Core**: always loaded, never decayed (identity-level knowledge)
/// - **ShortTerm**: active working memory, subject to rapid decay
/// - **LongTerm**: consolidated knowledge, slow decay curve
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    /// Identity-level facts: always loaded, never decayed.
    Core,
    /// Active working memory, subject to rapid decay.
    #[default]
    ShortTerm,
    /// Consolidated knowledge, slow decay curve.
    LongTerm,
}

impl MemoryTier {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Core => "core",
            Self::ShortTerm => "short_term",
            Self::LongTerm => "long_term",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::ShortTerm)
    }
}

impl std::str::FromStr for MemoryTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "core" => Ok(Self::Core),
            "short_term" => Ok(Self::ShortTerm),
            "long_term" => Ok(Self::LongTerm),
            _ => Err(format!("Unknown memory tier: {}", s)),
        }
    }
}

impl std::fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// MemoryScope
// ============================================================================

/// Visibility scope for a memory fact.
///
/// Controls which retrieval contexts can see a given fact:
/// - **Global**: visible everywhere
/// - **Agent**: visible only within a specific agent
/// - **Persona**: visible only to a specific persona
/// - **SessionLocal**: visible only within the current session (ephemeral, not persisted long-term)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Visible everywhere.
    #[default]
    Global,
    /// Visible only within a specific agent.
    Agent,
    /// Visible only to a specific persona.
    Persona,
    /// Visible only within the current session; used by SessionCompactor for intra-session facts.
    SessionLocal,
}

impl MemoryScope {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Global => "global",
            Self::Agent => "agent",
            Self::Persona => "persona",
            Self::SessionLocal => "session_local",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or(Self::Global)
    }
}

impl std::str::FromStr for MemoryScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "global" => Ok(Self::Global),
            "agent" => Ok(Self::Agent),
            "persona" => Ok(Self::Persona),
            "session_local" => Ok(Self::SessionLocal),
            _ => Err(format!("Unknown memory scope: {}", s)),
        }
    }
}

impl std::fmt::Display for MemoryScope {
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
    pub fn as_str(&self) -> &str {
        match self {
            Self::Abstract => "abstract",
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
            "abstract" => Ok(Self::Abstract),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_note_type_roundtrips() {
        let ft = NoteType::Wiki;
        assert_eq!(ft.as_str(), "wiki");
        assert_eq!(ft.to_string(), "wiki");
        let parsed: NoteType = "wiki".parse().unwrap();
        assert_eq!(parsed, NoteType::Wiki);
    }

    #[test]
    fn wiki_default_path() {
        assert_eq!(NoteType::Wiki.default_path(), "aleph://wiki/");
    }

    #[test]
    fn wiki_default_category() {
        assert_eq!(NoteType::Wiki.default_category(), MemoryCategory::Patterns);
    }
}
