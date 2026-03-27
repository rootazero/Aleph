//! Telegram long-polling lifecycle and watchdog health checks.
//!
//! Contains the main polling loop with:
//! - Dispatcher creation and dispatch
//! - Watchdog health check (get_me() every 120s)
//! - Auto-restart with exponential backoff
//! - Stall detection

use crate::gateway::channel::ChannelStatus;
use std::sync::Arc;
use std::time::Instant;
use teloxide::dispatching::UpdateHandler;
use teloxide::prelude::*;
use tokio::sync::{oneshot, RwLock};
use tokio_util::sync::CancellationToken;

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
) {
    tracing::info!("Starting Telegram long-polling...");
    *status.write().await = ChannelStatus::Connected;

    let mut attempt = 0u32;
    let mut healthy_since: Option<Instant> = None;

    loop {
        attempt += 1;

        let mut dispatcher = Dispatcher::builder(bot.clone(), handler.clone())
            .build();

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
        let _watchdog = tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                std::time::Duration::from_secs(HEALTH_CHECK_INTERVAL_SECS),
            );
            let mut consecutive_failures: u32 = 0;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
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

        let which = tokio::select! {
            _ = dispatcher.dispatch() => "stopped",
            _ = &mut shutdown_rx => "shutdown",
            _ = stall_rx.recv() => "stall",
        };
        watchdog_cancel.cancel();

        if which == "shutdown" {
            tracing::info!("Telegram channel shutdown requested");
            break;
        }

        // Dispatcher stopped unexpectedly or health check failed — auto-restart
        *status.write().await = ChannelStatus::Connecting;
        tracing::error!(attempt = attempt, reason = which, "Telegram polling {} — auto-restarting", which);

        // Reset attempt counter if we were healthy for >5 minutes
        if healthy_since.is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(300)) {
            attempt = 1;
        }
        let delay = std::cmp::min(5 * 2u64.pow(attempt.saturating_sub(1).min(4)), 60);
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

        healthy_since = Some(Instant::now());

        tracing::info!(attempt = attempt, "Telegram reconnected, queued messages will be delivered");
        *status.write().await = ChannelStatus::Connected;
    }

    *status.write().await = ChannelStatus::Disconnected;
}
