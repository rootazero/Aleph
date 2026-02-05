//! Graceful Shutdown Handler
//!
//! Handles SIGTERM/SIGINT signals to perform graceful checkpoint
//! of running tasks before shutdown.

use crate::error::AlephError;
use crate::memory::database::VectorDatabase;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Shutdown signal type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    /// SIGTERM received
    Term,
    /// SIGINT received (Ctrl+C)
    Interrupt,
    /// Programmatic shutdown request
    Requested,
}

/// Graceful shutdown handler for task checkpoint
pub struct GracefulShutdown {
    db: Arc<VectorDatabase>,
    shutdown_tx: broadcast::Sender<ShutdownSignal>,
}

impl GracefulShutdown {
    /// Create a new graceful shutdown handler
    pub fn new(db: Arc<VectorDatabase>) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self { db, shutdown_tx }
    }

    /// Get a receiver for shutdown signals
    pub fn subscribe(&self) -> broadcast::Receiver<ShutdownSignal> {
        self.shutdown_tx.subscribe()
    }

    /// Trigger a graceful shutdown
    pub fn trigger(&self, signal: ShutdownSignal) {
        let _ = self.shutdown_tx.send(signal);
    }

    /// Perform graceful checkpoint of all running tasks
    ///
    /// This should be called when a shutdown signal is received.
    /// It marks all running tasks as interrupted so they can be
    /// recovered on next startup.
    pub async fn checkpoint(&self) -> Result<u64, AlephError> {
        info!("Performing graceful checkpoint...");

        // Mark all running tasks as interrupted
        let count = self.db.mark_running_as_interrupted().await?;

        if count > 0 {
            info!(
                interrupted_tasks = count,
                "Marked running tasks as interrupted for recovery"
            );
        } else {
            info!("No running tasks to checkpoint");
        }

        Ok(count)
    }

    /// Start listening for shutdown signals
    ///
    /// This spawns a background task that listens for OS signals
    /// and triggers graceful shutdown when received.
    #[cfg(unix)]
    pub fn start_signal_handler(self: Arc<Self>) {
        use tokio::signal::unix::{signal, SignalKind};

        let handler = self.clone();

        tokio::spawn(async move {
            let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM");
            let mut sigint = signal(SignalKind::interrupt()).expect("Failed to register SIGINT");

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, initiating graceful shutdown");
                    handler.trigger(ShutdownSignal::Term);
                    if let Err(e) = handler.checkpoint().await {
                        warn!(error = %e, "Failed to checkpoint during shutdown");
                    }
                }
                _ = sigint.recv() => {
                    info!("Received SIGINT, initiating graceful shutdown");
                    handler.trigger(ShutdownSignal::Interrupt);
                    if let Err(e) = handler.checkpoint().await {
                        warn!(error = %e, "Failed to checkpoint during shutdown");
                    }
                }
            }
        });
    }

    /// Placeholder for non-Unix platforms
    #[cfg(not(unix))]
    pub fn start_signal_handler(self: Arc<Self>) {
        let handler = self.clone();

        tokio::spawn(async move {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                info!("Received Ctrl+C, initiating graceful shutdown");
                handler.trigger(ShutdownSignal::Interrupt);
                if let Err(e) = handler.checkpoint().await {
                    warn!(error = %e, "Failed to checkpoint during shutdown");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shutdown_signal_eq() {
        assert_eq!(ShutdownSignal::Term, ShutdownSignal::Term);
        assert_ne!(ShutdownSignal::Term, ShutdownSignal::Interrupt);
    }
}
