//! Telegram long-polling lifecycle and watchdog health checks.
//!
//! Contains the main polling loop with:
//! - Dispatcher creation and dispatch
//! - Watchdog health check (`get_me()` every 120s)
//! - Auto-restart with exponential backoff (base 2s, factor 1.8, cap 30s, ±25% jitter)
//! - Auto-restart with exponential backoff (base 2s, factor 1.8, cap 30s, ±25% jitter)

use super::error_cooldown::ErrorCooldown;
use super::offset::OffsetTracker;
use crate::gateway::channel::ChannelStatus;
use crate::sync_primitives::Arc;
use rand::RngExt;
use std::time::Instant;
use teloxide::dispatching::UpdateHandler;
use teloxide::prelude::*;
use tokio::sync::{oneshot, RwLock};
use tokio_util::sync::CancellationToken;

/// Tracks polling loop state for restart decisions and watchdog logic.
struct PollingState {
    attempt: u32,
    healthy_since: Option<Instant>,
}

impl PollingState {
    const fn new() -> Self {
        Self {
            attempt: 0,
            healthy_since: None,
        }
    }

    /// Reset attempt counter if we were healthy for >5 minutes.
    fn maybe_reset_attempts(&mut self) {
        if self
            .healthy_since
            .is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(300))
        {
            self.attempt = 1;
        }
    }

    /// Calculate exponential backoff with jitter.
    ///
    /// Formula: `min(2.0 * 1.8^(attempt-1), 30.0) * (1.0 + jitter)`
    /// where jitter ∈ [-0.25, +0.25].
    fn backoff_secs(&self) -> f64 {
        let base = 2.0_f64 * 1.8_f64.powi(self.attempt.saturating_sub(1) as i32);
        let capped = base.min(30.0);
        let jitter = rand::rng().random_range(-0.25..=0.25);
        capped * (1.0 + jitter)
    }
}

/// Process in update order, and acknowledge only after the handler returns.
/// Cancellation before completion leaves the update pending for the next poll.
async fn poll_updates(
    bot: &Bot,
    handler: &UpdateHandler<std::convert::Infallible>,
    tracker: Option<&OffsetTracker>,
    last_processed: &mut i64,
) -> Result<(), teloxide::RequestError> {
    loop {
        let offset = i32::try_from(last_processed.saturating_add(1)).unwrap_or(i32::MAX);
        let updates = bot
            .get_updates()
            .offset(offset)
            .timeout(30)
            .allowed_updates([
                teloxide::types::AllowedUpdate::Message,
                teloxide::types::AllowedUpdate::CallbackQuery,
                teloxide::types::AllowedUpdate::PollAnswer,
                teloxide::types::AllowedUpdate::MessageReaction,
            ])
            .await?;
        for update in updates {
            let update_id = i64::from(update.id.0);
            let _ = handler.dispatch(dptree::deps![bot.clone(), update]).await;
            if let Some(tracker) = tracker {
                tracker.advance(update_id, "polling").await;
            }
            *last_processed = update_id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn polling_acknowledges_only_completed_handlers_in_update_order() {
        let server = MockServer::start().await;
        let updates: Vec<_> = [10, 11]
            .into_iter()
            .map(|id| {
                serde_json::json!({
                    "update_id": id, "message": {"message_id": id, "date": 1700000000,
                        "chat": {"id": 123, "type": "private"}, "text": "hello"}
                })
            })
            .collect();
        Mock::given(path("/bot123:test/GetUpdates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "result": updates
            })))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(path("/bot123:test/GetUpdates"))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(2)
            .mount(&server)
            .await;
        let tracker = Arc::new(
            OffsetTracker::new(
                Arc::new(crate::resilience::StateDatabase::in_memory().unwrap()),
                "ordered".into(),
            )
            .await,
        );
        let (entered, mut received) = tokio::sync::mpsc::channel(2);
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let gate = release.clone();
        let handler: UpdateHandler<std::convert::Infallible> =
            dptree::entry().endpoint(move |update: Update| {
                let entered = entered.clone();
                let gate = gate.clone();
                async move {
                    entered.send(update.id.0).await.unwrap();
                    gate.acquire().await.unwrap().forget();
                    Ok::<(), std::convert::Infallible>(())
                }
            });
        let bot = Bot::new("123:test").set_api_url(server.uri().parse().unwrap());
        let worker_tracker = tracker.clone();
        let task = tokio::spawn(async move {
            let mut processed = 0;
            let result = poll_updates(&bot, &handler, Some(&worker_tracker), &mut processed).await;
            (result, processed)
        });
        assert_eq!(received.recv().await, Some(10));
        assert_eq!(tracker.load(), 0, "in-flight update is not acknowledged");
        release.add_permits(1);
        assert_eq!(received.recv().await, Some(11));
        assert_eq!(
            tracker.load(),
            10,
            "second update cannot overtake its handler"
        );
        release.add_permits(1);
        let (result, processed) = task.await.unwrap();
        assert!(result.is_err());
        assert_eq!(processed, 11);
        assert_eq!(tracker.load(), 11);
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(body["offset"], 12);
    }
}

/// Run the Telegram long-polling loop with watchdog and auto-restart.
///
/// The caller builds the dptree `handler` (message + callback branches)
/// in `start()`, capturing all channel-specific Arc clones. This function
/// only manages the polling infrastructure: dispatcher loop, watchdog
/// health checks, and restart logic.
pub(crate) async fn run_polling_loop(
    bot: Bot,
    handler: UpdateHandler<std::convert::Infallible>,
    status: Arc<RwLock<ChannelStatus>>,
    mut shutdown_rx: oneshot::Receiver<()>,
    offset_tracker: Option<Arc<OffsetTracker>>,
    error_cooldown: Arc<ErrorCooldown>,
) {
    tracing::info!("Starting Telegram long-polling...");

    // Delete any existing webhook — polling and webhooks are mutually exclusive.
    // If a webhook is set, getUpdates silently returns empty results.
    //
    // A durable initialization fact, not offset == 0, owns the one-time drop.
    // Without persistence we retain the backlog rather than dropping on every restart.
    let first_start = match &offset_tracker {
        Some(tracker) => tracker.initialize("polling").await,
        None => false,
    };
    let persisted_offset = offset_tracker.as_ref().map_or(0, |t| t.load());
    if let Err(error) = bot.delete_webhook().drop_pending_updates(first_start).await {
        tracing::warn!(%error, "Failed to clear Telegram webhook");
    }

    // Diagnostic: manually test getUpdates to verify polling works.
    // When we have a persisted offset, use offset+1 so Telegram only returns
    // updates newer than what we already processed.
    let test_request = if persisted_offset > 0 {
        bot.get_updates()
            .offset(
                i32::try_from(persisted_offset)
                    .unwrap_or(i32::MAX)
                    .saturating_add(1),
            )
            .limit(1)
            .timeout(5)
    } else {
        bot.get_updates().limit(1).timeout(5)
    };
    match test_request.await {
        Ok(updates) => {
            tracing::info!(
                "Telegram getUpdates test: {} pending updates",
                updates.len()
            );
            // Observation is not processing. The normal loop handles this update.
        }
        Err(e) => tracing::error!(
            "Telegram getUpdates test FAILED: {} — polling will not work!",
            e
        ),
    }

    *status.write().await = ChannelStatus::Connected;

    let mut state = PollingState::new();
    let mut last_processed = persisted_offset;

    loop {
        state.attempt += 1;
        tracing::info!(
            attempt = state.attempt,
            "Telegram polling loop iteration starting"
        );

        let polling = poll_updates(
            &bot,
            &handler,
            offset_tracker.as_deref(),
            &mut last_processed,
        );

        // Watchdog: periodic health check via get_me() API call.
        // Previous approach tracked "last message received" which falsely
        // triggered restarts during idle periods (no users messaging).
        // Now we actively probe the API — only restart on real failures.
        const HEALTH_CHECK_INTERVAL_SECS: u64 = 120;
        const MAX_CONSECUTIVE_FAILURES: u32 = 3;

        let (stall_tx, mut stall_rx) = tokio::sync::mpsc::channel::<()>(1);
        let watchdog_cancel = CancellationToken::new();
        let watchdog_token = watchdog_cancel.clone();
        let watchdog_bot = bot.clone();

        // Set healthy_since when dispatcher starts (not after backoff sleep)
        // so maybe_reset_attempts() can fire on first crash after 5min healthy.
        state.healthy_since = Some(Instant::now());

        let mut watchdog_handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS));
            let mut consecutive_failures: u32 = 0;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Periodic health check via get_me() API call.
                        // Stall detection via last_response_time was removed because
                        // teloxide's Dispatcher manages its own getUpdates loop and
                        // provides no hook to track per-response timestamps.
                        // We rely on consecutive health check failures instead.
                        match watchdog_bot.get_me().await {
                            Ok(_) => {
                                if consecutive_failures > 0 {
                                    tracing::info!(
                                        "Telegram health check recovered after {} failures",
                                        consecutive_failures,
                                    );
                                }
                                consecutive_failures = 0;
                            }
                            Err(e) => {
                                consecutive_failures += 1;
                                tracing::warn!(
                                    failures = consecutive_failures,
                                    "Telegram health check failed: {}",
                                    e,
                                );
                                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                    tracing::error!(
                                        failures = consecutive_failures,
                                        "Telegram health check failed {} consecutive times — triggering restart",
                                        consecutive_failures,
                                    );
                                    let _ = stall_tx.send(()).await;
                                    break;
                                }
                            }
                        }
                    }
                    _ = watchdog_token.cancelled() => break,
                }
            }
        });

        // Background sweep: periodically purge expired error cooldown entries.
        // Shares the watchdog CancellationToken so it is cancelled on restart.
        let sweep_ec = error_cooldown.clone();
        let sweep_token = watchdog_cancel.clone();
        let _sweep = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30 * 60));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        sweep_ec.sweep_expired();
                    }
                    _ = sweep_token.cancelled() => break,
                }
            }
        });

        tracing::info!("Telegram getUpdates loop starting...");
        let which = tokio::select! {
            result = polling => {
                tracing::warn!(?result, "Telegram getUpdates loop stopped");
                "stopped"
            },
            _ = &mut shutdown_rx => "shutdown",
            _ = stall_rx.recv() => "stall",
            result = &mut watchdog_handle => {
                match result {
                    Err(e) if e.is_panic() => {
                        tracing::error!("Telegram watchdog task panicked: {:?}", e);
                        "watchdog_panic"
                    }
                    _ => {
                        tracing::error!("Telegram watchdog task exited unexpectedly");
                        "watchdog_exit"
                    }
                }
            }
        };
        watchdog_cancel.cancel();

        if which == "shutdown" {
            tracing::info!("Telegram channel shutdown requested");
            break;
        }

        // Dispatcher stopped unexpectedly or health check failed — auto-restart
        *status.write().await = ChannelStatus::Connecting;
        tracing::error!(
            attempt = state.attempt,
            reason = which,
            "Telegram polling {} — auto-restarting",
            which
        );

        state.maybe_reset_attempts();
        let delay = state.backoff_secs();
        tracing::info!(
            delay_secs = format!("{:.1}", delay),
            "Telegram backoff sleeping"
        );
        tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
        // healthy_since is set at dispatcher start (above), not here after backoff.

        tracing::info!(
            attempt = state.attempt,
            "Telegram reconnected, queued messages will be delivered"
        );
        *status.write().await = ChannelStatus::Connected;
    }

    *status.write().await = ChannelStatus::Disconnected;
}
