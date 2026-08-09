//! Integration test for the governance gate's `Defer → enqueue_review →
//! list_pending_review` round-trip on `SqliteMemoryBackend`. Confirms the
//! primitive used by `DefaultCompoundIngestor::filter_ops_through_gate`
//! actually side-effects the review queue end-to-end.

use std::sync::Arc;

use alephcore::memory::notes::governance::gate::{
    CandidateNote, DefaultNoteWriteGate, GateOutcome, GateThresholds, NoteWriteAction,
    NoteWriteGate,
};
use alephcore::memory::notes::store::NoteStore;
use alephcore::memory::notes::{KnowledgeNote, Severity};
use alephcore::memory::SqliteMemoryBackend;

#[tokio::test]
async fn ingest_low_confidence_lands_in_queue_not_markdown() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
    let store: Arc<dyn NoteStore + Send + Sync> = backend.clone();
    let gate = DefaultNoteWriteGate::new(store, GateThresholds::default());

    let cand = CandidateNote {
        agent_id: "default".into(),
        category: "preference".into(),
        note: KnowledgeNote {
            title: "untrusted".into(),
            category: "preference".into(),
            facts: vec!["x".into()],
            confidence: 0.3,
            severity: Severity::Low,
            ..Default::default()
        },
        action: NoteWriteAction::Create,
        bypass_review: false,
        contradicts_existing: false,
        replay_op: None,
    };

    let out = gate.evaluate(&cand).await.unwrap();
    let queue_id = match out {
        GateOutcome::Defer { queue_id, .. } => queue_id,
        other => panic!("expected Defer, got {other:?}"),
    };

    let pending = backend
        .list_pending_review("default", chrono::Utc::now().timestamp() + 60)
        .await
        .unwrap();
    assert!(
        pending.iter().any(|p| p.id == queue_id),
        "Defer must enqueue a row visible to list_pending_review; queue_id={queue_id} pending_len={}",
        pending.len()
    );
}
