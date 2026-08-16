//! Unified raw → note write gate. Concentrates Accept/Defer/Reject routing
//! plus the side effects of writing review queue / archive rows.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{KnowledgeNote, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteWriteAction {
    Create,
    Update,
    Delete,
    Supersede,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateNote {
    pub agent_id: String,
    pub category: String,
    pub note: KnowledgeNote,
    pub action: NoteWriteAction,
    pub bypass_review: bool,
    pub contradicts_existing: bool,
    /// When set, this candidate is a **deferred non-Create op** (e.g.
    /// `Contradict`) carried as a serialized `PageOp`, not a materialized note.
    /// On review approval the reviewer replays this op through the apply
    /// transaction instead of `write_note`-ing `note` — a plain write would
    /// overwrite the target note with the delta. Kept as an opaque
    /// `serde_json::Value` so the governance layer takes no dependency on the
    /// ingest plan types. `None` = Create / materialized note (back-compat
    /// default for rows enqueued before this field existed).
    #[serde(default)]
    pub replay_op: Option<serde_json::Value>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum GateOutcome {
    Accept(CandidateNote),
    Defer { queue_id: String, reason: String },
    // NOTE: the gate is deliberately binary — Accept or Defer. It never rejects
    // at admission time; a deferred candidate's discard/rewrite decision is made
    // later by the LLM `NoteReviewStage`. (A former `Reject { archive_id, .. }`
    // variant was dead — never produced, no `archive_rejected` store method — so
    // it was removed per R10/YAGNI. Re-add if a hard admission-time reject is
    // ever genuinely needed.)
}

#[derive(Debug, Clone)]
pub struct GateThresholds {
    pub min_confidence: f32,
    pub high_severity_min_confidence: f32,
}

impl Default for GateThresholds {
    fn default() -> Self {
        Self {
            min_confidence: 0.5,
            high_severity_min_confidence: 0.8,
        }
    }
}

#[async_trait]
pub trait NoteWriteGate: Send + Sync {
    async fn evaluate(&self, candidate: &CandidateNote) -> Result<GateOutcome, AlephError>;
}

pub struct DefaultNoteWriteGate {
    store: Arc<dyn NoteStore + Send + Sync>,
    thresholds: GateThresholds,
}

impl DefaultNoteWriteGate {
    pub fn new(store: Arc<dyn NoteStore + Send + Sync>, thresholds: GateThresholds) -> Self {
        Self { store, thresholds }
    }
}

#[async_trait]
impl NoteWriteGate for DefaultNoteWriteGate {
    async fn evaluate(&self, candidate: &CandidateNote) -> Result<GateOutcome, AlephError> {
        if candidate.bypass_review {
            return Ok(GateOutcome::Accept(candidate.clone()));
        }

        // Delete of a Critical-severity note: defer.
        if matches!(candidate.action, NoteWriteAction::Delete)
            && candidate.note.severity == Severity::Critical
        {
            return self
                .defer(candidate, "delete of critical note requires review")
                .await;
        }

        // Supersede of a High/Critical note deprecates load-bearing knowledge —
        // defer for review. The candidate's severity is the *superseded* note's,
        // loaded by the ingestor (it is not indexed). Confidence-independent: the
        // risk is in retiring an important note, not in the writer's certainty.
        if matches!(candidate.action, NoteWriteAction::Supersede)
            && candidate.note.severity >= Severity::High
        {
            return self
                .defer(candidate, "supersede of high/critical note requires review")
                .await;
        }

        if candidate.note.confidence < self.thresholds.min_confidence {
            return self.defer(candidate, "confidence below minimum").await;
        }

        if candidate.note.severity >= Severity::High
            && candidate.note.confidence < self.thresholds.high_severity_min_confidence
        {
            return self
                .defer(candidate, "high severity needs higher confidence")
                .await;
        }

        if candidate.contradicts_existing {
            return self.defer(candidate, "contradicts existing note").await;
        }

        Ok(GateOutcome::Accept(candidate.clone()))
    }
}

impl DefaultNoteWriteGate {
    async fn defer(
        &self,
        candidate: &CandidateNote,
        reason: &str,
    ) -> Result<GateOutcome, AlephError> {
        let json = serde_json::to_string(candidate)
            .map_err(|e| AlephError::config(format!("candidate serialize: {e}")))?;
        let severity_str = format!("{:?}", candidate.note.severity).to_lowercase();
        let queue_id = self
            .store
            .enqueue_review(
                &candidate.agent_id,
                &json,
                &severity_str,
                candidate.note.confidence,
                reason,
            )
            .await?;
        Ok(GateOutcome::Defer {
            queue_id,
            reason: reason.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::sync_primitives::Arc;

    fn make_candidate(severity: Severity, confidence: f32) -> CandidateNote {
        CandidateNote {
            agent_id: "default".into(),
            category: "preference".into(),
            note: KnowledgeNote {
                title: "x".into(),
                category: "preference".into(),
                facts: vec!["body".into()],
                severity,
                confidence,
                ..Default::default()
            },
            action: NoteWriteAction::Create,
            bypass_review: false,
            contradicts_existing: false,
            replay_op: None,
        }
    }

    fn make_store() -> (
        tempfile::TempDir,
        Arc<crate::memory::store::SqliteMemoryBackend>,
    ) {
        let (scratch, path) = crate::utils::scratch::scratch_root();
        (
            scratch,
            Arc::new(crate::memory::store::SqliteMemoryBackend::new(&path).unwrap()),
        )
    }

    #[tokio::test]
    async fn defers_low_confidence() {
        let (_scratch, store) = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let cand = make_candidate(Severity::Low, 0.4);
        match gate.evaluate(&cand).await.unwrap() {
            GateOutcome::Defer { .. } => {}
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admits_high_confidence_low_severity() {
        let (_scratch, store) = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let cand = make_candidate(Severity::Low, 0.9);
        match gate.evaluate(&cand).await.unwrap() {
            GateOutcome::Accept(_) => {}
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn defers_high_severity_medium_confidence() {
        let (_scratch, store) = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let cand = make_candidate(Severity::High, 0.7);
        assert!(matches!(
            gate.evaluate(&cand).await.unwrap(),
            GateOutcome::Defer { .. }
        ));
    }

    #[tokio::test]
    async fn bypass_review_admits_unconditionally() {
        let (_scratch, store) = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let mut cand = make_candidate(Severity::Critical, 0.1);
        cand.bypass_review = true;
        assert!(matches!(
            gate.evaluate(&cand).await.unwrap(),
            GateOutcome::Accept(_)
        ));
    }

    #[tokio::test]
    async fn delete_critical_defers() {
        let (_scratch, store) = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let mut cand = make_candidate(Severity::Critical, 0.95);
        cand.action = NoteWriteAction::Delete;
        assert!(matches!(
            gate.evaluate(&cand).await.unwrap(),
            GateOutcome::Defer { .. }
        ));
    }

    #[tokio::test]
    async fn supersede_high_severity_defers() {
        // High confidence (0.95) so neither confidence rule fires — only the
        // Supersede-of-High/Critical rule can produce this Defer.
        let (_scratch, store) = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let mut cand = make_candidate(Severity::High, 0.95);
        cand.action = NoteWriteAction::Supersede;
        assert!(matches!(
            gate.evaluate(&cand).await.unwrap(),
            GateOutcome::Defer { .. }
        ));
    }

    #[tokio::test]
    async fn supersede_low_severity_admits() {
        let (_scratch, store) = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let mut cand = make_candidate(Severity::Low, 0.9);
        cand.action = NoteWriteAction::Supersede;
        assert!(matches!(
            gate.evaluate(&cand).await.unwrap(),
            GateOutcome::Accept(_)
        ));
    }
}
