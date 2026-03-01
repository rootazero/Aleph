//! Projector runner -- background task that dispatches POE events to all projectors.
//!
//! Subscribes to PoeEventBus and fans out each event to:
//! - CrystallizationProjector (LanceDB experience store)
//! - TrustProjector (SQLite trust scores)
//! - MemoryProjector (Memory facts)
//!
//! Each projector failure is logged but doesn't stop the runner.

use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::poe::events::PoeEventEnvelope;
use super::crystallization::CrystallizationProjector;
use super::trust::TrustProjector;

/// Background runner that dispatches POE events to all registered projectors.
///
/// Created via builder pattern, then `run()` is called as a background task.
///
/// # Example
/// ```rust,ignore
/// let runner = ProjectorRunner::new()
///     .with_trust(trust_projector);
///
/// let rx = event_bus.subscribe();
/// tokio::spawn(runner.run(rx));
/// ```
pub struct ProjectorRunner {
    crystallization: Option<CrystallizationProjector>,
    trust: Option<TrustProjector>,
    // MemoryProjector is not directly typed here to avoid circular dependency.
    // Instead we use a boxed async handler.
    memory_handler: Option<Box<dyn ProjectorHandler>>,
}

/// Trait for generic projector handlers (allows adding projectors without changing the runner).
#[async_trait::async_trait]
pub trait ProjectorHandler: Send + Sync {
    /// Handle a POE event. Returns Ok(true) if processed, Ok(false) if skipped.
    async fn handle(&self, envelope: &PoeEventEnvelope) -> Result<bool, String>;
    /// Name of this handler (for logging).
    fn name(&self) -> &'static str;
}

impl ProjectorRunner {
    /// Create a new empty runner (no projectors registered).
    pub fn new() -> Self {
        Self {
            crystallization: None,
            trust: None,
            memory_handler: None,
        }
    }

    /// Register the CrystallizationProjector.
    pub fn with_crystallization(mut self, projector: CrystallizationProjector) -> Self {
        self.crystallization = Some(projector);
        self
    }

    /// Register the TrustProjector.
    pub fn with_trust(mut self, projector: TrustProjector) -> Self {
        self.trust = Some(projector);
        self
    }

    /// Register a generic projector handler (e.g., MemoryProjector).
    pub fn with_handler(mut self, handler: Box<dyn ProjectorHandler>) -> Self {
        self.memory_handler = Some(handler);
        self
    }

    /// Run the projector loop, consuming events from the broadcast receiver.
    ///
    /// This method runs forever (until the sender is dropped or the task is cancelled).
    /// Should be spawned as a background tokio task.
    pub async fn run(self, mut rx: broadcast::Receiver<PoeEventEnvelope>) {
        info!("ProjectorRunner started");

        loop {
            match rx.recv().await {
                Ok(envelope) => {
                    let event_type = envelope.event.event_type_tag();
                    debug!(event_type = event_type, "ProjectorRunner: dispatching event");

                    // Dispatch to crystallization projector
                    if let Some(ref projector) = self.crystallization {
                        if let Err(e) = projector.handle(&envelope).await {
                            warn!(
                                event_type = event_type,
                                error = %e,
                                "CrystallizationProjector error"
                            );
                        }
                    }

                    // Dispatch to trust projector
                    if let Some(ref projector) = self.trust {
                        if let Err(e) = projector.handle(&envelope).await {
                            warn!(
                                event_type = event_type,
                                error = %e,
                                "TrustProjector error"
                            );
                        }
                    }

                    // Dispatch to memory handler
                    if let Some(ref handler) = self.memory_handler {
                        if let Err(e) = handler.handle(&envelope).await {
                            warn!(
                                event_type = event_type,
                                handler = handler.name(),
                                error = %e,
                                "ProjectorHandler error"
                            );
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "ProjectorRunner: lagged behind, skipped events");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("ProjectorRunner: event bus closed, shutting down");
                    break;
                }
            }
        }
    }
}

impl Default for ProjectorRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::events::{PoeEvent, PoeEventEnvelope, PoeOutcomeKind};
    use crate::poe::event_bus::PoeEventBus;
    use crate::sync_primitives::{AtomicUsize, Ordering};
    use crate::sync_primitives::Arc;

    struct CountingHandler {
        count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ProjectorHandler for CountingHandler {
        async fn handle(&self, _envelope: &PoeEventEnvelope) -> Result<bool, String> {
            self.count.fetch_add(1, Ordering::Relaxed);
            Ok(true)
        }
        fn name(&self) -> &'static str {
            "CountingHandler"
        }
    }

    #[tokio::test]
    async fn test_runner_dispatches_to_handler() {
        let bus = PoeEventBus::new(64);
        let count = Arc::new(AtomicUsize::new(0));

        let runner = ProjectorRunner::new()
            .with_handler(Box::new(CountingHandler { count: count.clone() }));

        let rx = bus.subscribe();
        let handle = tokio::spawn(runner.run(rx));

        // Emit an event
        bus.emit(PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::OutcomeRecorded {
                task_id: "t1".into(),
                outcome: PoeOutcomeKind::Success,
                attempts: 1,
                total_tokens: 1000,
                duration_ms: 500,
                best_distance: 0.0,
            },
            None,
        ));

        // Give the runner time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(count.load(Ordering::Relaxed), 1);

        // Clean up
        handle.abort();
    }

    #[tokio::test]
    async fn test_runner_stops_when_bus_closed() {
        let bus = PoeEventBus::new(64);
        let rx = bus.subscribe();

        let runner = ProjectorRunner::new();
        let handle = tokio::spawn(runner.run(rx));

        // Drop the bus (closes the sender)
        drop(bus);

        // Runner should stop
        let result = tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            handle,
        )
        .await;

        assert!(result.is_ok(), "Runner should stop when bus is closed");
    }

    #[tokio::test]
    async fn test_runner_with_no_projectors() {
        let bus = PoeEventBus::new(64);
        let rx = bus.subscribe();

        let runner = ProjectorRunner::new(); // No projectors

        let handle = tokio::spawn(runner.run(rx));

        // Emit an event -- should not panic
        bus.emit(PoeEventEnvelope::new(
            "t1".into(),
            0,
            PoeEvent::ManifestCreated {
                task_id: "t1".into(),
                objective: "test".into(),
                hard_constraints_count: 1,
                soft_metrics_count: 0,
            },
            None,
        ));

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        handle.abort();
    }

    #[tokio::test]
    async fn test_runner_handles_multiple_events() {
        let bus = PoeEventBus::new(64);
        let count = Arc::new(AtomicUsize::new(0));

        let runner = ProjectorRunner::new()
            .with_handler(Box::new(CountingHandler { count: count.clone() }));

        let rx = bus.subscribe();
        let handle = tokio::spawn(runner.run(rx));

        // Emit multiple events
        for i in 0..5 {
            bus.emit(PoeEventEnvelope::new(
                format!("t{}", i),
                i as u32,
                PoeEvent::OperationAttempted {
                    task_id: format!("t{}", i),
                    attempt: 1,
                    tokens_used: 1000,
                },
                None,
            ));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert_eq!(count.load(Ordering::Relaxed), 5);

        handle.abort();
    }
}
