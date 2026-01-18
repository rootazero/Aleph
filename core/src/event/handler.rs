// Aether/core/src/event/handler.rs
//! Event handler trait and registry for component subscriptions.

use crate::event::bus::EventBus;
use crate::event::types::{AetherEvent, EventType};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace};

/// Context provided to event handlers
#[derive(Clone)]
pub struct EventContext {
    /// Event bus for publishing new events
    pub bus: EventBus,
    /// Abort signal for graceful shutdown
    pub abort_signal: Arc<AtomicBool>,
    /// Session ID for the current execution
    pub session_id: Arc<RwLock<Option<String>>>,
}

impl EventContext {
    /// Create a new event context
    pub fn new(bus: EventBus) -> Self {
        Self {
            bus,
            abort_signal: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Check if abort has been signaled
    pub fn is_aborted(&self) -> bool {
        self.abort_signal.load(Ordering::Relaxed)
    }

    /// Signal abort
    pub fn abort(&self) {
        self.abort_signal.store(true, Ordering::Relaxed);
    }

    /// Reset abort signal
    pub fn reset_abort(&self) {
        self.abort_signal.store(false, Ordering::Relaxed);
    }

    /// Set current session ID
    pub async fn set_session_id(&self, session_id: String) {
        *self.session_id.write().await = Some(session_id);
    }

    /// Get current session ID
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }
}

/// Trait for event handlers
///
/// Components implement this trait to receive and process events.
/// Each handler declares which events it subscribes to and how to handle them.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Get the handler's unique name (for logging/debugging)
    fn name(&self) -> &'static str;

    /// Get the list of event types this handler subscribes to
    fn subscriptions(&self) -> Vec<EventType>;

    /// Handle an event
    ///
    /// Returns a list of new events to publish (can be empty).
    /// Errors are logged but don't stop the event loop.
    async fn handle(
        &self,
        event: &AetherEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError>;
}

/// Error type for event handlers
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("Handler error: {message}")]
    Generic { message: String },

    #[error("Aborted by user")]
    Aborted,

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Registry for managing event handlers
pub struct EventHandlerRegistry {
    handlers: Vec<Arc<dyn EventHandler>>,
    running: AtomicBool,
}

impl EventHandlerRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
            running: AtomicBool::new(false),
        }
    }

    /// Register a handler
    pub fn register(&mut self, handler: Arc<dyn EventHandler>) {
        info!(
            handler_name = handler.name(),
            subscriptions = ?handler.subscriptions(),
            "Registering event handler"
        );
        self.handlers.push(handler);
    }

    /// Get the number of registered handlers
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Start all handlers listening for events
    ///
    /// This spawns a tokio task for each handler that listens for events
    /// and dispatches them to the handler.
    pub async fn start(&self, ctx: EventContext) -> Vec<tokio::task::JoinHandle<()>> {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            debug!("Registry already running");
            return vec![];
        }

        info!(
            handler_count = self.handlers.len(),
            "Starting event handler registry"
        );

        let mut handles = Vec::new();

        for handler in &self.handlers {
            let handler = Arc::clone(handler);
            let ctx = ctx.clone();
            let subscriptions = handler.subscriptions();

            let mut subscriber = if subscriptions.contains(&EventType::All) {
                ctx.bus.subscribe()
            } else {
                ctx.bus.subscribe_filtered(subscriptions)
            };

            let handle = tokio::spawn(async move {
                let handler_name = handler.name();
                debug!(handler_name, "Handler event loop started");

                loop {
                    // Check abort signal
                    if ctx.is_aborted() {
                        debug!(handler_name, "Handler received abort signal");
                        break;
                    }

                    match subscriber.recv().await {
                        Ok(timestamped_event) => {
                            trace!(
                                handler_name,
                                event_type = ?timestamped_event.event.event_type(),
                                "Handler received event"
                            );

                            // Handle the event
                            match handler.handle(&timestamped_event.event, &ctx).await {
                                Ok(new_events) => {
                                    // Publish any new events
                                    for new_event in new_events {
                                        trace!(
                                            handler_name,
                                            new_event_type = ?new_event.event_type(),
                                            "Handler publishing new event"
                                        );
                                        ctx.bus.publish(new_event).await;
                                    }
                                }
                                Err(HandlerError::Aborted) => {
                                    debug!(handler_name, "Handler aborted");
                                    break;
                                }
                                Err(e) => {
                                    error!(
                                        handler_name,
                                        error = %e,
                                        "Handler error"
                                    );
                                    // Continue processing other events
                                }
                            }
                        }
                        Err(e) => {
                            error!(
                                handler_name,
                                error = %e,
                                "Handler receive error, stopping"
                            );
                            break;
                        }
                    }
                }

                debug!(handler_name, "Handler event loop ended");
            });

            handles.push(handle);
        }

        handles
    }

    /// Stop all handlers
    pub fn stop(&self, ctx: &EventContext) {
        info!("Stopping event handler registry");
        ctx.abort();
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Default for EventHandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::types::{InputEvent, StopReason};
    use std::sync::atomic::AtomicUsize;

    /// Test handler that counts events
    struct CountingHandler {
        count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EventHandler for CountingHandler {
        fn name(&self) -> &'static str {
            "CountingHandler"
        }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::All]
        }

        async fn handle(
            &self,
            _event: &AetherEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AetherEvent>, HandlerError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(vec![])
        }
    }

    /// Test handler that produces new events
    struct ProducingHandler;

    #[async_trait]
    impl EventHandler for ProducingHandler {
        fn name(&self) -> &'static str {
            "ProducingHandler"
        }

        fn subscriptions(&self) -> Vec<EventType> {
            vec![EventType::InputReceived]
        }

        async fn handle(
            &self,
            _event: &AetherEvent,
            _ctx: &EventContext,
        ) -> Result<Vec<AetherEvent>, HandlerError> {
            // Produce a LoopStop event for each input
            Ok(vec![AetherEvent::LoopStop(StopReason::Completed)])
        }
    }

    #[tokio::test]
    async fn test_handler_registration() {
        let mut registry = EventHandlerRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        registry.register(Arc::new(CountingHandler { count: counter.clone() }));

        assert_eq!(registry.handler_count(), 1);
    }

    #[tokio::test]
    async fn test_handler_receives_events() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let mut registry = EventHandlerRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        registry.register(Arc::new(CountingHandler { count: counter.clone() }));

        let handles = registry.start(ctx.clone()).await;

        // Give handlers time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish event
        bus.publish(AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })).await;

        // Give handler time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Stop and wait
        registry.stop(&ctx);
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                handle
            ).await;
        }

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_handler_produces_events() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus.clone());

        let mut registry = EventHandlerRegistry::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // Register producing handler first, then counting handler
        registry.register(Arc::new(ProducingHandler));
        registry.register(Arc::new(CountingHandler { count: counter.clone() }));

        let handles = registry.start(ctx.clone()).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Publish input event
        bus.publish(AetherEvent::InputReceived(InputEvent {
            text: "test".to_string(),
            topic_id: None,
            context: None,
            timestamp: 0,
        })).await;

        // Give handlers time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        registry.stop(&ctx);
        for handle in handles {
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                handle
            ).await;
        }

        // CountingHandler should have received: InputReceived + LoopStop
        assert!(counter.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_event_context_abort() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        assert!(!ctx.is_aborted());

        ctx.abort();

        assert!(ctx.is_aborted());

        ctx.reset_abort();

        assert!(!ctx.is_aborted());
    }

    #[tokio::test]
    async fn test_event_context_session_id() {
        let bus = EventBus::new();
        let ctx = EventContext::new(bus);

        assert!(ctx.get_session_id().await.is_none());

        ctx.set_session_id("session-123".to_string()).await;

        assert_eq!(ctx.get_session_id().await, Some("session-123".to_string()));
    }
}
