use super::SqliteMemoryBackend;
use crate::memory::notes::store::NoteStore;
use crate::sync_primitives::Arc;
use rusqlite::ffi;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Keeps a genuine SQLite statement alive across close, without leaking it.
struct PendingStatement(*mut ffi::sqlite3_stmt);

impl Drop for PendingStatement {
    fn drop(&mut self) {
        // SAFETY: sqlite3_prepare_v2 created this statement on the retained
        // connection. The fixture drops the statement before the backend.
        assert_eq!(unsafe { ffi::sqlite3_finalize(self.0) }, ffi::SQLITE_OK);
    }
}

#[tokio::test]
async fn closed_backend_rejects_sync_and_blocking_access() {
    let backend = SqliteMemoryBackend::in_memory().unwrap();
    assert_eq!(backend.count_all_notes().await.unwrap(), 0);
    backend.close().unwrap();
    let error = backend
        .count_all_notes()
        .await
        .expect_err("sync access after close");
    assert!(error.to_string().contains("closed"), "{error}");

    let calls = Arc::new(AtomicUsize::new(0));
    let closure_calls = Arc::clone(&calls);
    let error = backend
        .with_conn(move |_| {
            closure_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .expect_err("blocking access after close");
    assert!(error.to_string().contains("closed"), "{error}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    backend.close().unwrap();
}

#[tokio::test]
async fn failed_close_retains_same_connection_for_retry() {
    let backend = SqliteMemoryBackend::in_memory().unwrap();
    let (handle, statement) = {
        let connection = backend.conn.lock().unwrap();
        // SAFETY: access is serialized by the backend's connection mutex.
        let handle = unsafe { connection.as_ref().unwrap().handle() };
        let mut statement = std::ptr::null_mut();
        // SAFETY: the connection is live, SQL is NUL-terminated, and SQLite
        // initializes statement. PendingStatement finalizes it before backend.
        assert_eq!(
            unsafe {
                ffi::sqlite3_prepare_v2(
                    handle,
                    c"SELECT 1".as_ptr(),
                    -1,
                    &mut statement,
                    std::ptr::null_mut(),
                )
            },
            ffi::SQLITE_OK,
        );
        (handle, PendingStatement(statement))
    };
    let error = backend
        .close()
        .expect_err("live statement prevents actual close");
    assert!(error.to_string().contains("close"), "{error}");
    {
        let connection = backend.conn.lock().unwrap();
        // SAFETY: the failed close must retain exactly the original connection.
        assert_eq!(unsafe { connection.as_ref().unwrap().handle() }, handle);
    }
    assert_eq!(backend.count_all_notes().await.unwrap(), 0);
    drop(statement);
    backend.close().unwrap();
    backend.close().unwrap();
    assert!(backend.count_all_notes().await.is_err());
}
