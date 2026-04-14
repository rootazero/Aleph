//! Data model for the compound-ingest plan and its outputs.

use serde::{Deserialize, Serialize};

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
    },
    Append {
        note_path: String,
        #[serde(default)]
        new_facts: Vec<String>,
        #[serde(default)]
        new_links: Vec<String>,
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
    pub fn primary_path(&self) -> &str {
        match self {
            PageOp::Create { note_path, .. }
            | PageOp::Append { note_path, .. }
            | PageOp::Update { note_path, .. }
            | PageOp::Contradict { note_path, .. } => note_path,
            PageOp::Link { from, .. } => from,
            PageOp::Supersede { old_path, .. } => old_path,
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

/// Summary of what an apply pass produced. Returned to CompressionService.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ApplyReport {
    pub created: u32,
    pub appended: u32,
    pub updated: u32,
    pub contradicted: u32,
    pub linked: u32,
    pub superseded: u32,
    pub tx_id: String,
    pub touched_paths: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            PageOp::Append {
                note_path: "learning/rust-async".into(),
                new_facts: vec!["pin API".into()],
                new_links: vec![],
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
    }
}
