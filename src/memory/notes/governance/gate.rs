//! Unified raw → note write gate. Concentrates Accept/Defer/Reject routing
//! plus the side effects of writing review queue / archive rows.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{FactProvenance, KnowledgeNote, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteWriteAction {
    Create,
    Update,
    Append,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateNote {
    pub agent_id: String,
    pub category: String,
    pub note: KnowledgeNote,
    pub source_path: Option<String>,
    pub fact_provenance: Vec<FactProvenance>,
    pub action: NoteWriteAction,
    pub bypass_review: bool,
    pub contradicts_existing: bool,
}

#[derive(Debug)]
pub enum GateOutcome {
    Accept(CandidateNote),
    Defer { queue_id: String, reason: String },
    Reject { archive_id: String, reason: String },
}

#[derive(Debug, Clone)]
pub struct GateThresholds {
    pub min_confidence: f32,
    pub high_severity_min_confidence: f32,
    pub critical_severity_floor: f32,
}

impl Default for GateThresholds {
    fn default() -> Self {
        Self {
            min_confidence: 0.5,
            high_severity_min_confidence: 0.8,
            critical_severity_floor: 0.85,
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
    use std::sync::Arc;

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
            source_path: None,
            fact_provenance: vec![],
            action: NoteWriteAction::Create,
            bypass_review: false,
            contradicts_existing: false,
        }
    }

    fn make_store() -> Arc<crate::memory::store::SqliteMemoryBackend> {
        Arc::new(
            crate::memory::store::SqliteMemoryBackend::new(
                &std::env::temp_dir().join(format!("aleph_gate_{}", uuid::Uuid::new_v4())),
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn defers_low_confidence() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let cand = make_candidate(Severity::Low, 0.4);
        match gate.evaluate(&cand).await.unwrap() {
            GateOutcome::Defer { .. } => {}
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admits_high_confidence_low_severity() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let cand = make_candidate(Severity::Low, 0.9);
        match gate.evaluate(&cand).await.unwrap() {
            GateOutcome::Accept(_) => {}
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn defers_high_severity_medium_confidence() {
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let cand = make_candidate(Severity::High, 0.7);
        assert!(matches!(
            gate.evaluate(&cand).await.unwrap(),
            GateOutcome::Defer { .. }
        ));
    }

    #[tokio::test]
    async fn bypass_review_admits_unconditionally() {
        let store = make_store();
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
        let store = make_store();
        let gate = DefaultNoteWriteGate::new(store, Default::default());
        let mut cand = make_candidate(Severity::Critical, 0.95);
        cand.action = NoteWriteAction::Delete;
        assert!(matches!(
            gate.evaluate(&cand).await.unwrap(),
            GateOutcome::Defer { .. }
        ));
    }
}
