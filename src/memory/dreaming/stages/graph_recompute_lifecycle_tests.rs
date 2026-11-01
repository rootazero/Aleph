use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

use crate::capability::ownership::OwnershipTree;
use crate::memory::notes::graph::{GraphEdge, GraphNode, GraphSnapshot};
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::sync_primitives::Arc;
use tokio::sync::oneshot;

#[tokio::test]
async fn timed_out_graph_compute_remains_owned_until_blocking_completion() {
    let root = tempfile::tempdir().unwrap();
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.path().join("memory/metadata.db"),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    runtime.start().await.unwrap();
    let operation = runtime.operation().unwrap();
    let (entered, started) = oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let (cancel, cancelled) = oneshot::channel();
    let completed = Arc::new(AtomicUsize::new(0));
    let computed = completed.clone();
    let mut request = Box::pin(operation.run(async move {
        tokio::select! {
            result = super::super::compute_owned("graph recompute", move || {
                let _ = entered.send(MemoryOperation::current().is_some());
                // Dropping the test's sender also releases this closure on assertion failure.
                if gate.recv().is_err() {
                    return 0;
                }
                let node = |path: &str| GraphNode {
                    path: path.into(),
                    category: "reference".into(),
                    sources: Vec::new(),
                };
                let result = super::compute(&GraphSnapshot {
                    nodes: vec![node("reference/a.md"), node("reference/b.md")],
                    edges: vec![GraphEdge {
                        from: "reference/a.md".into(),
                        to: "reference/b.md".into(),
                        rel_type: None,
                        confidence: 1.0,
                    }],
                });
                computed.store(result.cache.len(), Ordering::SeqCst);
                result.node_count
            }) => result.map(|_| ()),
            _ = cancelled => Ok(()),
        }
    }));
    assert!(matches!(futures::poll!(request.as_mut()), Poll::Pending));
    let admitted = tokio::time::timeout(std::time::Duration::from_secs(10), started)
        .await
        .unwrap()
        .unwrap();
    assert!(
        admitted,
        "the actual graph compute closure must carry admission"
    );
    cancel.send(()).unwrap();
    request.await.unwrap();
    drop(operation);
    let mut stop = Box::pin(runtime.stop());
    assert!(matches!(futures::poll!(stop.as_mut()), Poll::Pending));
    assert_eq!(runtime.status().state, MemoryState::Draining);
    drop(stop);
    assert_eq!(completed.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    assert_eq!(runtime.stop().await.unwrap().state, MemoryState::Off);
    assert_eq!(completed.load(Ordering::SeqCst), 2);
}
