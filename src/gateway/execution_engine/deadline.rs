use crate::sync_primitives::Arc;

/// Wait until the resettable deadline expires.
///
/// The deadline can be extended by compression tasks. This function re-checks
/// after waking to handle extensions that occurred during sleep.
pub(super) async fn wait_for_deadline(deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>) {
    loop {
        let dl = *deadline.lock().await;
        if tokio::time::Instant::now() >= dl {
            break;
        }
        // Sleep precisely to the current deadline. If it was extended while we
        // slept, the next iteration re-reads the new value and sleeps again —
        // no fixed-interval poll, no busy-spin.
        tokio::time::sleep_until(dl).await;
    }
}
