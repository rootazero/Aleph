//! Wire contract for the Memory Wiki evidence trace and curated-write ledger.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Selects the kind of memory target to inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    /// Trace a note by its exact `category/name` path.
    Note,
    /// Trace a raw memory by ID and find citing notes.
    Raw,
    /// Trace a `USER.md` section through its recorded sessions.
    ProfileSection,
    /// Look up curated-memory write attempts, including refusals.
    WriteDecision,
}

/// `memory.trace` RPC request. The builtin tool does not expose partition selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub target: String,
    pub kind: TraceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
    /// Read `agent_id` as a storage partition rather than composing a caller scope.
    /// Requires `agent_id`; does not grant access to a partition.
    #[serde(default, skip_serializing_if = "is_false")]
    pub exact_partition: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceResult {
    pub target: String,
    /// The requested note, or projected reverse matches for raw/profile traces.
    pub notes: Vec<String>,
    pub evidence: Vec<EvidenceItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub write_decisions: Vec<WriteDecisionRow>,
    /// Fact annotations on the requested note, not on intermediate source notes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<FactOrigin>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub note_sources: Vec<NoteSource>,
    /// A processing or output budget omitted part of the trace.
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// Recorded evidence could not be read or resolved.
    #[serde(skip_serializing_if = "is_false")]
    pub incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub raw_id: String,
    #[serde(default)]
    pub via_note: Option<String>,
    #[serde(default)]
    pub via_session: Option<String>,
    /// Up to 800 characters of raw content; absent when pruned.
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub pruned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactOrigin {
    pub note_path: String,
    /// Zero-based position in the note.
    pub fact_idx: usize,
    pub text: String,
    /// `raw_source`, `prior_note`, `inferred`, `system`, or `legacy`.
    pub origin: String,
    /// A recorded annotation, not a finding about whether a user ever said this.
    pub inferred: bool,
    #[serde(default)]
    pub source_id: Option<String>,
}

/// One curated write attempt. Reason is the persisted token, not prose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WriteDecisionRow {
    pub action: String,
    pub reason: String,
    /// Bounded excerpt of the attempted subject.
    pub subject: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteSource {
    pub note_path: String,
    pub source_path: String,
    pub status: NoteSourceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteSourceStatus {
    /// Source Markdown was readable and parseable.
    Available,
    /// Source Markdown could not be read or resolved.
    Unavailable,
    /// A budget stopped expansion before determining availability.
    NotVisited,
}

fn is_false(value: &bool) -> bool {
    !value
}
