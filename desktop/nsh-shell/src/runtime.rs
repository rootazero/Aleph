use std::thread;

/// Spawn a Tokio multi-thread runtime on a background thread.
///
/// The returned handle allows the caller to wait for completion from any
/// thread context (Tokio or non-Tokio). The worker owns its runtime so NSH's
/// UI runtime is not required to be Tokio and remains free to drive the window.
///
/// **Lifetime Note:** The spawned thread runs until the future completes.
/// Dropping the returned `JoinHandle` abandons waiting but does not stop the
/// thread. Phase 2's connection loop has bounded retry logic (max 10 failures)
/// to prevent infinite resource consumption.
pub fn spawn_tokio_worker<F>(future: F) -> thread::JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        runtime.block_on(future)
    })
}
