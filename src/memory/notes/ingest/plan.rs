//! Data model for the compound-ingest plan and its outputs.

use crate::memory::notes::note::types::Severity;
use crate::memory::notes::note::Relation;
use serde::{Deserialize, Serialize};

/// Back-compat default for `PageOp::Create.confidence`: plans emitted before the
/// planner learned to rate its output carry no `confidence`, and must keep
/// admitting at the governance gate exactly as they did before the field
/// existed. Mirrors `KnowledgeNote::default().confidence` (1.0).
fn default_confidence() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPlan {
    /// Free-form LLM rationale. Truncated to 240 chars before the log line.
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub ops: Vec<PageOp>,
    /// New tag / rule proposals the LLM wants — logged but never auto-applied.
    #[serde(default)]
    pub schema_proposals: Vec<SchemaProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// rust-doctor-disable-next-line large-enum-variant
pub enum PageOp {
    Create {
        note_path: String,
        title: String,
        summary: String,
        #[serde(default)]
        facts: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        relations: Vec<Relation>,
        /// Raw-memory ids (or prior-note paths) this page was distilled from.
        /// LLM-attributed; empty means the apply layer falls back to the batch set.
        #[serde(default)]
        source_ids: Vec<String>,
        /// LLM self-assessed confidence in this page's facts, in `[0,1]`. The
        /// governance gate defers creations below `min_confidence`. Plans emitted
        /// before this field existed default to `1.0` (see `default_confidence`)
        /// so they admit exactly as before — no ingest regression.
        #[serde(default = "default_confidence")]
        confidence: f32,
        /// LLM-judged importance. `High`/`Critical` pages must clear a higher
        /// confidence bar at the gate. Defaults to `Low` for older plans.
        #[serde(default)]
        severity: Severity,
    },
    Append {
        note_path: String,
        #[serde(default)]
        new_facts: Vec<String>,
        #[serde(default)]
        new_links: Vec<String>,
        #[serde(default)]
        new_relations: Vec<Relation>,
        #[serde(default)]
        source_ids: Vec<String>,
    },
    Update {
        note_path: String,
        /// Hash the LLM saw when it read the target. Verified at apply time.
        expected_content_hash: String,
        new_facts: Vec<String>,
        reason: String,
    },
    Contradict {
        note_path: String,
        new_claim: String,
        #[serde(default)]
        evidence_source_ids: Vec<String>,
    },
    Link {
        from: String,
        to: String,
    },
    Supersede {
        old_path: String,
        new_path: String,
    },
}

impl PageOp {
    /// Primary path this op touches — used for tx-scope + dedup.
    #[must_use]
    pub fn primary_path(&self) -> &str {
        match self {
            Self::Create { note_path, .. }
            | Self::Append { note_path, .. }
            | Self::Update { note_path, .. }
            | Self::Contradict { note_path, .. } => note_path,
            Self::Link { from, .. } => from,
            Self::Supersede { old_path, .. } => old_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaProposal {
    NewTag { tag: String, rationale: String },
    NewRule { rule: String, rationale: String },
    DomainUpdate { text: String },
}

/// Summary of what an apply pass produced. Returned to `CompressionService`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyReport {
    pub created: u32,
    pub appended: u32,
    pub updated: u32,
    pub contradicted: u32,
    /// Link edges written by `PageOp::Link`. Each edge rewrites BOTH endpoint
    /// notes on disk (`add_link` runs in both directions), so it is apply-pass
    /// work, not bookkeeping — `compress_to_notes` folds it into
    /// `CompressionResult::facts_extracted`. Kept a distinct field rather than
    /// pre-summed so the apply layer stays free to report edges separately.
    pub linked: u32,
    pub superseded: u32,
    pub tx_id: String,
    pub touched_paths: Vec<String>,
    /// True when the PLANNER failed to produce a usable plan — the LLM returned
    /// nothing parseable, or every operation it emitted was dropped — as opposed
    /// to deliberately planning nothing.
    ///
    /// Load-bearing because all four source prompts instruct the planner to
    /// "emit an empty plan" when nothing clears the bar, so an empty report is
    /// the EXPECTED outcome for an un-noteworthy batch. `CompressionService`
    /// used to read every empty report as a transient failure and hold the rows
    /// in a 6h retry grace, re-running the same LLM call over the same
    /// transcript on every tick until the rows aged out — punishing the planner
    /// for doing exactly what its prompt asked. Only a degraded planner earns
    /// the retry; a model-intended empty plan consumes its rows immediately.
    pub planner_degraded: bool,
}

impl ApplyReport {
    /// True when the apply pass wrote no notes. `touched_paths` is the
    /// authoritative record of what landed on disk, so an empty list means the
    /// plan produced nothing (empty plan, all ops filtered, or no-op apply).
    /// Used by `CompressionService` to decide whether to defer marking the raw
    /// batch processed (give a transiently-failed extraction a retry instead of
    /// discarding the knowledge forever).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.touched_paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_report_is_empty_tracks_touched_paths() {
        assert!(ApplyReport::default().is_empty());
        let wrote = ApplyReport {
            touched_paths: vec!["learning/tokio".into()],
            ..Default::default()
        };
        assert!(!wrote.is_empty());
    }

    #[test]
    fn page_op_roundtrip_json() {
        let ops = vec![
            PageOp::Create {
                note_path: "learning/tokio".into(),
                title: "Tokio".into(),
                summary: "Async runtime".into(),
                facts: vec!["event-driven".into()],
                links: vec!["learning/rust-async".into()],
                tags: vec!["rust".into()],
                relations: vec![],
                source_ids: vec![],
                confidence: 0.9,
                severity: Severity::Med,
            },
            PageOp::Append {
                note_path: "learning/rust-async".into(),
                new_facts: vec!["pin API".into()],
                new_links: vec![],
                new_relations: vec![],
                source_ids: vec![],
            },
            PageOp::Update {
                note_path: "preference/runtime".into(),
                expected_content_hash: "abc123".into(),
                new_facts: vec!["updated fact".into()],
                reason: "supersede old".into(),
            },
            PageOp::Contradict {
                note_path: "learning/rust-async".into(),
                new_claim: "use tokio 1.x".into(),
                evidence_source_ids: vec!["raw-1".into()],
            },
            PageOp::Link {
                from: "learning/tokio".into(),
                to: "learning/rust-async".into(),
            },
            PageOp::Supersede {
                old_path: "learning/old".into(),
                new_path: "learning/new".into(),
            },
        ];
        let plan = IngestPlan {
            reasoning: "test".into(),
            ops: ops.clone(),
            schema_proposals: vec![SchemaProposal::NewTag {
                tag: "async".into(),
                rationale: "used in 3 notes".into(),
            }],
        };
        // rust-doctor-disable-next-line unwrap-in-production
        let j = serde_json::to_string(&plan).unwrap();
        let back: IngestPlan = serde_json::from_str(&j).unwrap();
        assert_eq!(back.ops.len(), ops.len());
        assert_eq!(back.schema_proposals.len(), 1);
    }

    #[test]
    fn page_op_primary_path_matches_variant() {
        let p = PageOp::Create {
            note_path: "a/b".into(),
            title: "".into(),
            summary: "".into(),
            facts: vec![],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec![],
            confidence: 1.0,
            severity: Severity::Low,
        };
        assert_eq!(p.primary_path(), "a/b");

        let p = PageOp::Link {
            from: "x/y".into(),
            to: "z/w".into(),
        };
        assert_eq!(p.primary_path(), "x/y");

        let p = PageOp::Supersede {
            old_path: "old".into(),
            new_path: "new".into(),
        };
        assert_eq!(p.primary_path(), "old");
    }

    #[test]
    fn apply_report_default_is_zero() {
        let r = ApplyReport::default();
        assert_eq!(r.created, 0);
        assert!(r.tx_id.is_empty());
        assert!(r.touched_paths.is_empty());
        // Several early-return paths hand back `ApplyReport::default()` for
        // outcomes that are NOT planner failures (empty raw batch, all creates
        // deduped as no-ops, every op deferred by the governance gate). If the
        // default ever flipped to `true` those would all start burning a 6h
        // retry grace on rows nothing will ever extract a note from.
        assert!(!r.planner_degraded);
    }

    #[test]
    fn create_op_parses_relations_and_defaults_when_absent() {
        let j = r#"{"kind":"create","note_path":"entity/alice","title":"Alice","summary":"","facts":[],"links":[],"tags":[],"relations":[{"to":"entity/acme","type":"works_at","confidence":0.9}]}"#;
        let op: PageOp = serde_json::from_str(j).unwrap();
        match op {
            PageOp::Create { relations, .. } => {
                assert_eq!(relations.len(), 1);
                assert_eq!(relations[0].rel_type, "works_at");
            }
            _ => panic!("expected create"),
        }
        let j2 = r#"{"kind":"create","note_path":"learning/x","title":"X","summary":"","facts":[],"links":[],"tags":[]}"#;
        let op2: PageOp = serde_json::from_str(j2).unwrap();
        match op2 {
            PageOp::Create { relations, .. } => assert!(relations.is_empty()),
            _ => panic!("expected create"),
        }
    }

    #[test]
    fn append_op_parses_new_relations() {
        let j = r#"{"kind":"append","note_path":"entity/alice","new_facts":[],"new_links":[],"new_relations":[{"to":"entity/bob","type":"colleague"}]}"#;
        let op: PageOp = serde_json::from_str(j).unwrap();
        match op {
            PageOp::Append { new_relations, .. } => {
                assert_eq!(new_relations.len(), 1);
                assert_eq!(new_relations[0].rel_type, "colleague");
                assert_eq!(new_relations[0].to, "entity/bob");
                assert_eq!(new_relations[0].confidence, 1.0);
            }
            _ => panic!("expected append"),
        }
    }

    #[test]
    fn create_and_append_carry_source_ids_with_serde_default() {
        // New field round-trips.
        let op = PageOp::Create {
            note_path: "preference/typescript".into(),
            title: "TypeScript".into(),
            summary: "User prefers TypeScript".into(),
            facts: vec!["The user prefers TypeScript.".into()],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec!["raw-uuid-1".into(), "raw-uuid-2".into()],
            confidence: 1.0,
            severity: Severity::Low,
        };
        let j = serde_json::to_string(&op).unwrap();
        let back: PageOp = serde_json::from_str(&j).unwrap();
        match back {
            PageOp::Create { source_ids, .. } => {
                assert_eq!(source_ids, vec!["raw-uuid-1", "raw-uuid-2"])
            }
            _ => panic!("expected create"),
        }

        // Old JSON WITHOUT source_ids / confidence / severity still parses, and
        // the gate-facing fields default to the pre-field behaviour (1.0 / Low)
        // so a plan emitted before the planner rated its output admits unchanged.
        let legacy = r#"{"kind":"create","note_path":"a/b","title":"T","summary":"","facts":[],"links":[],"tags":[]}"#;
        let op2: PageOp = serde_json::from_str(legacy).unwrap();
        match op2 {
            PageOp::Create {
                source_ids,
                confidence,
                severity,
                ..
            } => {
                assert!(source_ids.is_empty());
                assert_eq!(confidence, 1.0);
                assert_eq!(severity, Severity::Low);
            }
            _ => panic!("expected create"),
        }

        let legacy_append =
            r#"{"kind":"append","note_path":"a/b","new_facts":["x"],"new_links":[]}"#;
        let op3: PageOp = serde_json::from_str(legacy_append).unwrap();
        match op3 {
            PageOp::Append { source_ids, .. } => assert!(source_ids.is_empty()),
            _ => panic!("expected append"),
        }
    }
}
