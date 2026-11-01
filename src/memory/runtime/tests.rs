use std::task::Poll;

use crate::capability::facade::Reference;
use crate::capability::ownership::{OwnershipTree, VisibilityScope};
use crate::memory::notes::store::NoteStore;
use crate::sync_primitives::Arc;
use tokio::sync::oneshot;

use super::{resource_id, MemoryRuntime, MemoryRuntimeConfig, MemoryState};

fn runtime(root: &std::path::Path) -> Arc<MemoryRuntime> {
    MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: root.join("memory/metadata.db"),
            note_memory_dir: root.join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    )
}

fn reference() -> Reference {
    Reference {
        id: resource_id(),
        visibility: VisibilityScope::default(),
    }
}

async fn pending_child(runtime: &Arc<MemoryRuntime>) -> oneshot::Sender<()> {
    let operation = runtime.capture(&reference()).unwrap().admit().unwrap();
    let (ready, started) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    operation
        .spawn(async move {
            ready.send(()).unwrap();
            wait.await.unwrap();
        })
        .unwrap();
    drop(operation);
    started.await.unwrap();
    release
}

#[tokio::test]
async fn cold_off_opens_no_memory_resources() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert!(!runtime.config.metadata_path.exists());
    assert!(!runtime.config.note_memory_dir.exists());
    assert!(runtime.capture(&reference()).is_err());
    assert!(runtime.tasks.ids().is_empty());
    runtime.start().await.unwrap();
    let operation = runtime.capture(&reference()).unwrap().admit().unwrap();
    let backend = operation.backend().clone();
    assert!(backend.count_all_notes().await.is_err());
    assert_eq!(
        operation
            .clone()
            .scope(backend.count_all_notes())
            .await
            .unwrap(),
        0
    );
    drop(operation);
    assert_eq!(runtime.stop().await.unwrap().state, MemoryState::Off);
    assert!(backend.conn().lock().unwrap().is_none());
    assert!(
        backend.count_all_notes().await.is_err(),
        "Off must mean actual Memory connection close"
    );
}

#[tokio::test]
async fn cancelled_stop_keeps_transition_and_exact_task_ids() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.start().await.unwrap();
    let release = pending_child(&runtime).await;
    let ids = runtime.tasks.ids();
    assert_eq!(ids.len(), 1);
    let transition = runtime.inner.lock().unwrap().driver.as_ref().unwrap().id();
    let mut stop = Box::pin(runtime.stop());
    assert!(
        matches!(futures::poll!(stop.as_mut()), Poll::Pending),
        "a live child must keep stop pending"
    );
    assert_eq!(runtime.status().state, MemoryState::Draining);
    drop(stop);
    assert_eq!(
        runtime.tasks.ids(),
        ids,
        "cancellation must retain the real task"
    );
    assert_eq!(
        runtime.inner.lock().unwrap().driver.as_ref().unwrap().id(),
        transition
    );
    assert!(
        runtime.start().await.is_err(),
        "start during drain must be busy"
    );
    release.send(()).unwrap();
    assert_eq!(runtime.stop().await.unwrap().state, MemoryState::Off);
    assert!(runtime.tasks.ids().is_empty());
}

#[tokio::test]
async fn concurrent_stop_waiters_share_one_drain() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.start().await.unwrap();
    let release = pending_child(&runtime).await;
    let mut first = Box::pin(runtime.stop());
    let mut second = Box::pin(runtime.stop());
    assert!(matches!(futures::poll!(first.as_mut()), Poll::Pending));
    let ids = runtime.tasks.ids();
    assert!(matches!(futures::poll!(second.as_mut()), Poll::Pending));
    assert_eq!(runtime.tasks.ids(), ids);
    release.send(()).unwrap();
    let (a, b) = tokio::join!(first, second);
    assert_eq!(a.unwrap().state, MemoryState::Off);
    assert_eq!(b.unwrap().state, MemoryState::Off);
}

#[tokio::test]
async fn blocking_work_outlives_cancelled_request() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.start().await.unwrap();
    let operation = runtime.capture(&reference()).unwrap().admit().unwrap();
    let backend = operation.backend().clone();
    let (ready, started) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let release = ReleaseOnDrop(Some(release));
    let mut request = Box::pin(
        operation
            .clone()
            .scope(backend.with_conn(move |connection| {
                ready.send(()).unwrap();
                wait.recv().unwrap();
                connection
                    .execute("CREATE TABLE owned_effect (value INTEGER)", [])
                    .unwrap();
                connection
                    .execute("INSERT INTO owned_effect VALUES (1)", [])
                    .unwrap();
                Ok(())
            })),
    );
    assert!(matches!(futures::poll!(request.as_mut()), Poll::Pending));
    started.await.unwrap();
    drop(request);
    drop(operation);
    let ids = runtime.tasks.ids();
    assert_eq!(
        ids.len(),
        1,
        "the source must retain the real blocking join"
    );
    let mut stop = Box::pin(runtime.stop());
    assert!(matches!(futures::poll!(stop.as_mut()), Poll::Pending));
    assert_ne!(runtime.status().state, MemoryState::Off);
    drop(stop);
    assert_eq!(runtime.tasks.ids(), ids);
    release.release();
    runtime.stop().await.unwrap();
    assert!(backend.conn().lock().unwrap().is_none());
    let connection = rusqlite::Connection::open(&runtime.config.metadata_path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT SUM(value) FROM owned_effect", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

struct ReleaseOnDrop(Option<std::sync::mpsc::Sender<()>>);

impl ReleaseOnDrop {
    fn release(mut self) {
        self.0.take().unwrap().send(()).unwrap();
    }
}

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

struct BusyStatement(*mut rusqlite::ffi::sqlite3_stmt);

impl Drop for BusyStatement {
    fn drop(&mut self) {
        // The statement is local to this test and finalized before its connection drops.
        unsafe {
            rusqlite::ffi::sqlite3_finalize(self.0);
        }
    }
}

#[tokio::test]
async fn stop_error_is_sticky_but_cleanup_retry_reaches_off() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.start().await.unwrap();
    let operation = runtime.capture(&reference()).unwrap().admit().unwrap();
    let backend = operation.backend().clone();
    operation
        .scope(backend.with_conn(|connection| {
            connection
                .execute("CREATE TABLE once_only (value INTEGER)", [])
                .unwrap();
            connection
                .execute("INSERT INTO once_only VALUES (1)", [])
                .unwrap();
            Ok(())
        }))
        .await
        .unwrap();
    let statement = {
        let guard = backend.conn().lock().unwrap();
        let mut statement = std::ptr::null_mut();
        // A genuine unfinalized SQLite statement makes Connection::close return SQLITE_BUSY.
        let result = unsafe {
            rusqlite::ffi::sqlite3_prepare_v2(
                guard.as_ref().unwrap().handle(),
                c"SELECT 1".as_ptr(),
                -1,
                &mut statement,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, rusqlite::ffi::SQLITE_OK);
        BusyStatement(statement)
    };
    assert!(runtime.stop().await.is_err());
    let status = runtime.status();
    assert_eq!(status.state, MemoryState::Failed);
    assert!(status.first_error.as_ref().unwrap().contains("close"));
    assert_eq!(runtime.status(), status, "status must not retry cleanup");
    drop(statement);
    assert!(
        runtime.stop().await.is_err(),
        "cleanup must not erase the transition's first error"
    );
    let cleaned = runtime.status();
    assert_eq!(cleaned.state, MemoryState::Off);
    assert_eq!(cleaned.first_error, status.first_error);
    let connection = rusqlite::Connection::open(&runtime.config.metadata_path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT SUM(value) FROM once_only", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(runtime.start().await.unwrap().state, MemoryState::Running);
    assert!(runtime.stop().await.is_err());
    assert_eq!(runtime.status().state, MemoryState::Off);
}

#[tokio::test]
async fn restart_never_revives_old_handle() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.start().await.unwrap();
    let old = runtime.capture(&reference()).unwrap();
    let operation = old.admit().unwrap();
    let backend = operation.backend().clone();
    drop(operation);
    runtime.stop().await.unwrap();
    assert!(old.admit().is_err());
    runtime.start().await.unwrap();
    assert!(
        old.admit().is_err(),
        "restart must not refresh the captured Resource lease"
    );
    let new = runtime.capture(&reference()).unwrap().admit().unwrap();
    assert!(!Arc::ptr_eq(&backend, new.backend()));
    drop(new);
    runtime.stop().await.unwrap();
}

async fn partial_backend(
    runtime: &Arc<MemoryRuntime>,
) -> Arc<crate::memory::store::sqlite::SqliteMemoryBackend> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let changed = runtime.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let backend = runtime
                .inner
                .lock()
                .unwrap()
                .bundle
                .as_ref()
                .map(|bundle| bundle.backend.clone());
            if let Some(backend) = backend {
                return backend;
            }
            changed.await;
        }
    })
    .await
    .expect("startup did not acquire its real backend")
}

#[tokio::test]
async fn cancelled_start_keeps_partial_resources_until_stop_finishes() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let (release, gate) = oneshot::channel();
    *runtime.initialization_gate.lock().unwrap() = Some(gate);
    let mut start = Box::pin(runtime.start());
    assert!(matches!(futures::poll!(start.as_mut()), Poll::Pending));
    let backend = partial_backend(&runtime).await;
    assert_eq!(runtime.status().state, MemoryState::Starting);
    assert!(backend.conn().lock().unwrap().is_some());
    let transition = runtime.inner.lock().unwrap().driver.as_ref().unwrap().id();
    drop(start);
    let mut stop = Box::pin(runtime.stop());
    assert!(matches!(futures::poll!(stop.as_mut()), Poll::Pending));
    assert_eq!(runtime.status().state, MemoryState::Draining);
    drop(stop);
    assert_eq!(
        runtime.inner.lock().unwrap().driver.as_ref().unwrap().id(),
        transition
    );
    release.send(()).unwrap();
    runtime.stop().await.unwrap();
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert!(backend.conn().lock().unwrap().is_none());
}

#[tokio::test]
async fn failed_initialization_closes_the_partial_backend() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    std::fs::write(&runtime.config.note_memory_dir, "blocked directory").unwrap();
    let (release, gate) = oneshot::channel();
    *runtime.initialization_gate.lock().unwrap() = Some(gate);
    let mut start = Box::pin(runtime.start());
    assert!(matches!(futures::poll!(start.as_mut()), Poll::Pending));
    let backend = partial_backend(&runtime).await;
    release.send(()).unwrap();
    assert!(start
        .await
        .unwrap_err()
        .to_string()
        .contains("notes initialization"));
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert!(backend.conn().lock().unwrap().is_none());
    assert!(runtime.capture(&reference()).is_err());
    assert!(runtime.stop().await.is_err());
}

#[tokio::test]
async fn running_reaper_observes_children_and_clones_share_one_admission() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.start().await.unwrap();
    let operation = runtime.capture(&reference()).unwrap().admit().unwrap();
    let sibling = operation.clone();
    operation.spawn(async {}).unwrap();
    operation.spawn(async {}).unwrap();
    assert_eq!(runtime.inner.lock().unwrap().admitted, 1);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let changed = runtime.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if runtime.tasks.ids().is_empty() {
                break;
            }
            changed.await;
        }
    })
    .await
    .expect("running owner did not reap its real children");
    assert_eq!(runtime.status().state, MemoryState::Running);
    drop(operation);
    assert_eq!(runtime.inner.lock().unwrap().admitted, 1);
    drop(sibling);
    assert_eq!(runtime.inner.lock().unwrap().admitted, 0);
    runtime.stop().await.unwrap();
}

#[tokio::test]
async fn child_panic_keeps_the_first_error_and_observes_its_sibling() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    runtime.start().await.unwrap();
    let release = pending_child(&runtime).await;
    let operation = runtime.capture(&reference()).unwrap().admit().unwrap();
    let (ready, started) = oneshot::channel();
    operation
        .spawn(async move {
            ready.send(()).unwrap();
            panic!("owned runtime panic");
        })
        .unwrap();
    drop(operation);
    started.await.unwrap();
    let mut stop = Box::pin(runtime.stop());
    assert!(matches!(futures::poll!(stop.as_mut()), Poll::Pending));
    drop(stop);
    release.send(()).unwrap();
    let error = runtime.stop().await.unwrap_err().to_string();
    assert!(error.contains("owned runtime panic"));
    assert_eq!(runtime.status().state, MemoryState::Off);
    assert!(runtime.tasks.ids().is_empty());
    assert_eq!(runtime.stop().await.unwrap_err().to_string(), error);
}
