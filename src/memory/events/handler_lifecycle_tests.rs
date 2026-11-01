use super::MemoryCommandHandler;
use crate::memory::context::{FactSource, NoteType};
use crate::memory::events::commands::CreateNoteCommand;
use crate::memory::events::EventActor;
use crate::memory::notes::NoteIndexer;
use crate::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::resilience::database::StateDatabase;
use crate::sync_primitives::{Arc, AtomicBool, Ordering};
use std::future::Future;
use std::time::Duration;
use tokio::sync::Notify;

#[tokio::test]
async fn reconciler_stop_observes_current_scan() {
    let root = tempfile::TempDir::new().unwrap();
    let note_dir = root.path().join("notes");
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("metadata.db"),
            note_memory_dir: note_dir.clone(),
        },
        Arc::new(crate::capability::ownership::OwnershipTree::new()),
    );
    runtime.start().await.unwrap();
    let operation = runtime.operation().unwrap();
    let events = Arc::new(StateDatabase::new(root.path().join("events.db")).unwrap());
    let gate = Arc::new((Notify::new(), Notify::new(), AtomicBool::new(false)));
    let mut handler = MemoryCommandHandler::new(events.clone()).with_note_indexer(Arc::new(
        NoteIndexer::new(note_dir.clone(), operation.backend().clone()),
    ));
    handler.reconcile_gate = Some(gate.clone());
    let handler = Arc::new(handler);
    let seed = handler.clone();
    let fact_id = operation
        .run(async move {
            seed.create_fact(CreateNoteCommand {
                content: "A reconciler must finish its admitted scan.".into(),
                note_type: NoteType::Preference,
                path: "/user/preferences/lifecycle".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
                actor: EventActor::Agent,
                correlation_id: None,
            })
            .await
        })
        .await
        .unwrap();
    let missing = note_dir
        .join("default")
        .join(NoteType::Preference.to_category_dir())
        .join(format!("{fact_id}.md"));
    assert!(missing.is_file());
    tokio::fs::remove_file(&missing).await.unwrap();
    drop(operation);
    runtime.retain_worker(handler.clone().spawn_owned_reconciler_daemon(
        Duration::from_secs(3600),
        runtime.clone(),
        runtime.worker_stop().unwrap(),
    ));
    tokio::time::timeout(Duration::from_secs(10), gate.0.notified())
        .await
        .unwrap();
    assert!(
        gate.2.load(Ordering::SeqCst),
        "the scan must carry source admission"
    );
    assert!(handler.last_reconcile_report().is_none());
    {
        let mut stop = Box::pin(runtime.stop());
        assert!(stop
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
            .is_pending());
        assert_eq!(runtime.status().state, MemoryState::Draining);
    }
    {
        let mut retry = Box::pin(runtime.stop());
        assert!(retry
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
            .is_pending());
    }
    gate.1.notify_one();
    let stopped = tokio::time::timeout(Duration::from_secs(10), runtime.stop())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.state, MemoryState::Off);
    let report = handler.last_reconcile_report().unwrap();
    assert_eq!(report.scanned_facts, 1);
    assert_eq!(report.missing_files.len(), 1);
    assert_eq!(report.missing_files[0].fact_id, fact_id);
    assert_eq!(report.missing_files[0].expected_path, missing);
    assert!(!missing.exists());
    assert_eq!(
        events
            .get_memory_events_for_fact_unscoped(&fact_id)
            .await
            .unwrap()
            .len(),
        1
    );
}
