use super::MemoryExtensionRegistry;
use crate::capability::ownership::OwnershipTree;
use crate::error::AlephError;
use crate::memory::extensions::traits::MemoryExtension;
use crate::memory::extensions::types::DelegationCtx;
use crate::memory::namespace::NamespaceScope;
use crate::memory::runtime::{MemoryOperation, MemoryRuntime, MemoryRuntimeConfig, MemoryState};
use async_trait::async_trait;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

struct Delegation {
    entered: Notify,
    release: Arc<Semaphore>,
    admitted: AtomicBool,
}

#[async_trait]
impl MemoryExtension for Delegation {
    fn name(&self) -> &str {
        "delegation-lifecycle-effect"
    }

    async fn on_delegation(&self, _ctx: &DelegationCtx) -> Result<(), AlephError> {
        self.admitted
            .store(MemoryOperation::current().is_some(), Ordering::SeqCst);
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        let operation = MemoryOperation::current()
            .ok_or_else(|| AlephError::other("delegation callback has no source admission"))?;
        operation
            .backend()
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TABLE delegation_callback_effect(value INTEGER); \
                 INSERT INTO delegation_callback_effect VALUES(1);",
                )
                .map_err(|error| AlephError::other(error.to_string()))
            })
            .await
    }
}

struct Release(Arc<Semaphore>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.add_permits(1);
    }
}

fn context() -> DelegationCtx {
    DelegationCtx {
        agent_id: "owner".into(),
        namespace: NamespaceScope::Owner,
        parent_session_id: "parent".into(),
        child_session_id: "child".into(),
        task: "delegated work".into(),
        result_summary: "finished work".into(),
    }
}

#[tokio::test]
async fn delegation_capture_is_cold_off_and_drains_actual_callback() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("derived.db");
    let runtime = MemoryRuntime::new(
        MemoryRuntimeConfig {
            metadata_path: path.clone(),
            note_memory_dir: root.path().join("notes"),
        },
        Arc::new(OwnershipTree::new()),
    );
    let owner = Arc::downgrade(&runtime);
    let extension = Arc::new(Delegation {
        entered: Notify::new(),
        release: Arc::new(Semaphore::new(0)),
        admitted: AtomicBool::new(false),
    });
    let release = Release(extension.release.clone());
    let registry = Arc::new(MemoryExtensionRegistry::new());
    registry.register(extension.clone()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        registry.dispatch_owned_on_delegation(Some(&owner), context()),
    )
    .await
    .unwrap();
    assert!(!extension.admitted.load(Ordering::SeqCst));
    assert!(!path.exists());
    runtime.start().await.unwrap();
    let mut work = Box::pin(registry.dispatch_owned_on_delegation(Some(&owner), context()));
    tokio::select! {
        _ = &mut work => panic!("delegation finished before release"),
        _ = async {
            tokio::time::timeout(Duration::from_secs(10), extension.entered.notified())
                .await.unwrap();
        } => {}
    }
    assert!(extension.admitted.load(Ordering::SeqCst));
    drop(work);
    let mut stop = Box::pin(runtime.stop());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(stop.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert_eq!(runtime.status().state, MemoryState::Draining);
    drop(stop);
    drop(release);
    tokio::time::timeout(Duration::from_secs(10), runtime.stop())
        .await
        .unwrap()
        .unwrap();
    let connection =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let observed: i64 = connection
        .query_row(
            "SELECT SUM(value) FROM delegation_callback_effect",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(observed, 1);
    assert_eq!(runtime.status().state, MemoryState::Off);
}
