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
    // Offset-aware startup:
    //   offset == 0 (first startup): drop pending updates to avoid replaying
    //     a potentially huge backlog accumulated before the bot was connected.
    //   offset >  0 (restart): do NOT drop — resume from the persisted offset
    //     so no messages are lost between restarts.
    let persisted_offset = offset_tracker.as_ref().map_or(0, |t| t.load());

    if persisted_offset > 0 {
        // Restart path: clear webhook but keep pending updates
        match bot.delete_webhook().await {
            Ok(_) => tracing::info!(
                offset = persisted_offset,
                "Telegram webhook cleared (keeping pending updates, resuming from offset)"
            ),
            Err(e) => tracing::warn!(
                "Failed to delete Telegram webhook: {} (polling may not work)",
                e
            ),
        }
    } else {
        // First startup: clear webhook AND drop pending updates
        match bot.delete_webhook().drop_pending_updates(true).await {
            Ok(_) => {
                tracing::info!("Telegram webhook cleared + pending updates dropped (first startup)")
            }
            Err(e) => tracing::warn!(
                "Failed to delete Telegram webhook: {} (polling may not work)",
                e
            ),
        }
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
            // Persist the offset from the diagnostic fetch if we got updates
            if let (Some(tracker), Some(last)) = (&offset_tracker, updates.last()) {
                tracker.advance(i64::from(last.id.0), "boot").await;
            }
        }
        Err(e) => tracing::error!(
            "Telegram getUpdates test FAILED: {} — polling will not work!",
            e
        ),
    }

    *status.write().await = ChannelStatus::Connected;

    let mut state = PollingState::new();

    loop {
        state.attempt += 1;
        tracing::info!(
            attempt = state.attempt,
            "Telegram polling loop iteration starting"
        );

        let mut dispatcher = Dispatcher::builder(bot.clone(), handler.clone()).build();

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

        tracing::info!("Telegram dispatcher.dispatch() starting...");
        let which = tokio::select! {
            _ = dispatcher.dispatch() => {
                tracing::warn!("Telegram dispatcher.dispatch() returned (stopped)");
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
