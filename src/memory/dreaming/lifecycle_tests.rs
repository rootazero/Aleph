use super::DreamDaemon;
use crate::capability::ownership::OwnershipTree;
use crate::config::types::memory::MemoryConfig;
use crate::memory::runtime::{MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use crate::sync_primitives::{Arc, AtomicBool, Ordering};
use chrono::NaiveTime;
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

#[tokio::test]
async fn dream_manual_and_tick_share_admission() {
    for scheduled in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let runtime = MemoryRuntime::new(
            MemoryRuntimeConfig {
                metadata_path: directory.path().join("metadata.db"),
                note_memory_dir: directory.path().join("note"),
            },
            Arc::new(OwnershipTree::new()),
        );
        runtime.start().await.unwrap();
        let operation = runtime.operation().unwrap();
        let mut config = MemoryConfig::default();
        config.dreaming.enabled = true;
        config.dreaming.max_duration_seconds = 30;
        let mut daemon = operation
            .clone()
            .scope(async {
                DreamDaemon::from_config(operation.backend().clone(), &config)
                    .unwrap()
                    .with_note_memory_dir(directory.path().join("note"))
                    .with_activity_probe(Arc::new(|| false))
            })
            .await;
        daemon.window_start = NaiveTime::MIN;
        daemon.window_end = NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap();
        let gate = Arc::new((
            tokio::sync::Notify::new(),
            tokio::sync::Notify::new(),
            AtomicBool::new(false),
        ));
        daemon.lifecycle_gate = Some(gate.clone());
        let daemon = Arc::new(daemon);
        if scheduled {
            runtime.retain_worker(
                daemon
                    .clone()
                    .start_owned_background_task(runtime.clone(), runtime.worker_stop().unwrap()),
            );
            tokio::time::timeout(Duration::from_secs(10), gate.0.notified())
                .await
                .unwrap();
        } else {
            let mut request = Box::pin(operation.run(async move { daemon.run_now().await }));
            tokio::time::timeout(Duration::from_secs(10), async {
                tokio::select! {
                    result = &mut request => panic!("Dream returned before its gate: {result:?}"),
                    _ = gate.0.notified() => {}
                }
            })
            .await
            .unwrap();
            // Only the waiter disappears; the owner must retain the same cycle.
            drop(request);
        }
        assert!(
            gate.2.load(Ordering::SeqCst),
            "Dream entry must carry its source admission"
        );
        drop(operation);
        {
            let mut stop = pin!(runtime.stop());
            assert!(matches!(futures::poll!(&mut stop), Poll::Pending));
            assert_eq!(runtime.status().state, MemoryState::Draining);
        }
        let mut retry = pin!(runtime.stop());
        assert!(matches!(futures::poll!(&mut retry), Poll::Pending));
        gate.1.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), &mut retry)
                .await
                .unwrap()
                .unwrap()
                .state,
            MemoryState::Off
        );
        let connection = rusqlite::Connection::open(directory.path().join("metadata.db")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM dream_reports", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(runtime.status().first_error.is_none());
    }
}
