//! Note types — enums and small structs for knowledge notes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// LLM-judged importance of a distilled note. Used by retrieval re-ranking.
///
/// Default is `Low` so legacy notes (no `severity:` in frontmatter) get
/// `severity_boost = 1.0` and rank exactly as before. See
/// `docs/superpowers/plans/2026-04-29-aleph-self-evolution.md` Phase 2 Decision 4.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Low,
    Med,
    High,
    Critical,
}

/// Governance status of a note. Used by Phase C2 supersession / contradiction
/// handling. `Active` is the default so legacy notes (no `status:` in
/// frontmatter) behave exactly as before.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
pub enum NoteStatus {
    #[default]
    Active,
    Deprecated,
    Contradicted,
}

/// Provenance origin of an individual fact-bullet within a note. Phase C2
/// paragraph-level provenance. `Legacy` is the default for facts that have
/// no `<!-- ... -->` marker, preserving backward compatibility with
/// pre-C2.2 notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceOrigin {
    RawSource,
    PriorNote,
    Inferred,
    /// System-generated structural fact (e.g. the `[title]`/`[summary]` lines
    /// synthesized during note creation). Deterministic scaffolding, not
    /// user/LLM-derived content — distinct from `Legacy` (no marker at all).
    System,
    Legacy,
}

/// Per-fact provenance metadata extracted from inline HTML comments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactProvenance {
    pub origin: ProvenanceOrigin,
    pub source_id: Option<String>,
    pub inferred: bool,
}

impl Default for FactProvenance {
    fn default() -> Self {
        Self {
            origin: ProvenanceOrigin::Legacy,
            source_id: None,
            inferred: false,
        }
    }
}
