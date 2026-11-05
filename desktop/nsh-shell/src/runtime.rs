use std::thread;
use tokio::task::JoinHandle;

/// Spawn a Tokio multi-thread runtime on a background thread.
///
/// The returned task is a Tokio-side join handle for the worker thread's
/// result. The worker owns its runtime so NSH's UI runtime is not required to
/// be Tokio and remains free to drive the window.
pub fn spawn_tokio_worker<F>(future: F) -> JoinHandle<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();

    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let result = runtime.block_on(future);
        let _ = tx.send(result);
    });

    tokio::task::spawn_blocking(move || rx.recv().expect("worker thread panicked"))
}
