use crate::sync_primitives::Arc;

/// Wait until the resettable deadline expires.
///
/// The deadline can be extended by compression tasks. This function re-checks
/// after waking to handle extensions that occurred during sleep.
pub(super) async fn wait_for_deadline(deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>) {
    loop {
        let dl = *deadline.lock().await;
        tokio::time::sleep_until(dl).await;
        // Re-check: deadline may have been extended while we slept.
        if tokio::time::Instant::now() >= *deadline.lock().await {
            break;
        }
        // Guard against theoretical busy-spin if deadline is in the past
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}
