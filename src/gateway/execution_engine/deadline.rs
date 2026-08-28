use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// Wait until the resettable deadline expires.
///
/// The deadline can be extended by compression tasks. This function re-checks
/// after waking to handle extensions that occurred during sleep.
///
/// **Audit fix**: the previous implementation read `dl` under the lock,
/// released the lock, then `sleep_until(dl)` against that exact instant.
/// A concurrent extension (`*deadline.lock().await += delta`) between the
/// release and the await left us sleeping to the OLD value. On wakeup the
/// next iteration saw `now >= dl_old` and broke out, firing `cancel_token`
/// against a healthy long-running tool that had legitimately been granted
/// more time.
///
/// Fix: compute the remaining duration WHILE holding the lock and sleep
/// for that duration. A `+= delta` between the read and the sleep extends
/// the *remaining* time by `delta` (the next iteration's `remaining` is
/// re-derived from the now-larger deadline), so a healthy tool is never
/// killed by an in-flight extension.
pub(super) async fn wait_for_deadline(deadline: Arc<tokio::sync::Mutex<Instant>>) {
    loop {
        let remaining: Duration = {
            let dl = *deadline.lock().await;
            let now = Instant::now();
            if now >= dl {
                break;
            }
            dl.saturating_duration_since(now)
        };
        tokio::time::sleep(remaining).await;
    }
}
