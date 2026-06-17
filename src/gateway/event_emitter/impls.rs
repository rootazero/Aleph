//! `EventEmitter` implementations
//!
//! Concrete implementations of the `EventEmitter` trait:
//! - `GatewayEventEmitter` — broadcasts via the Gateway event bus
//! - `NoOpEventEmitter` — silent sink for testing / disabled streaming
//! - `CollectingEventEmitter` — collects events for test assertions
//! - `DynEventEmitter` — wrapper for trait objects

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicU64, Ordering};

use super::instant_buffer::{plan_instant, InstantOutcome};
use super::types::{EventEmitError, OutputMode, StreamEvent};
use super::EventEmitter;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

/// Gateway-based event emitter
///
/// Broadcasts events to all connected WebSocket clients via the event bus.
/// Respects `output_mode` configuration:
/// - Typewriter: stream chunks immediately as they arrive
/// - Instant: buffer all chunks, only emit on final
pub struct GatewayEventEmitter {
    pub(super) event_bus: Arc<GatewayEventBus>,
    pub(super) seq_counter: AtomicU64,
    // Instant mode buffer for accumulating all chunks
    pub(super) instant_buffer: Mutex<String>,
    /// Output mode: typewriter (streaming) or instant (all-at-once)
    pub(super) output_mode: OutputMode,
}

impl GatewayEventEmitter {
    #[must_use]
    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            event_bus,
            seq_counter: AtomicU64::new(0),
            instant_buffer: Mutex::new(String::new()),
            output_mode: OutputMode::Typewriter,
        }
    }

    /// Create with a specific output mode
    #[must_use]
    pub fn with_output_mode(event_bus: Arc<GatewayEventBus>, output_mode: OutputMode) -> Self {
        Self {
            event_bus,
            seq_counter: AtomicU64::new(0),
            instant_buffer: Mutex::new(String::new()),
            output_mode,
        }
    }

    /// Get the current output mode
    pub const fn output_mode(&self) -> &OutputMode {
        &self.output_mode
    }
}

#[async_trait]
impl EventEmitter for GatewayEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // Instant mode: coalesce streamed response text through the shared
        // planner (single-sourced with `InstantBufferingEmitter`) before it
        // reaches the bus. `Prepend`/`Forward` fall through to the common
        // broadcast below, so the original event is published exactly once.
        if self.output_mode == OutputMode::Instant {
            let outcome = {
                let mut buffer = self.instant_buffer.lock().await;
                plan_instant(&mut buffer, &event, || self.next_seq())
            };
            match outcome {
                InstantOutcome::Buffered => return Ok(()),
                InstantOutcome::Replace(events) => {
                    for e in events {
                        self.event_bus.publish_frame(&GatewayEventFrame::from(e))?;
                    }
                    return Ok(());
                }
                InstantOutcome::Prepend(events) => {
                    for e in events {
                        self.event_bus.publish_frame(&GatewayEventFrame::from(e))?;
                    }
                }
                InstantOutcome::Forward => {}
            }
        }

        // Typewriter mode, or instant-mode passthrough: broadcast immediately.
        let frame = GatewayEventFrame::from(event);
        self.event_bus.publish_frame(&frame)?;
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// No-op event emitter for testing or when streaming is disabled
pub struct NoOpEventEmitter {
    seq_counter: AtomicU64,
}

impl NoOpEventEmitter {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            seq_counter: AtomicU64::new(0),
        }
    }
}

impl Default for NoOpEventEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventEmitter for NoOpEventEmitter {
    async fn emit(&self, _event: StreamEvent) -> Result<(), EventEmitError> {
        // Do nothing
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Collecting event emitter for testing
///
/// Stores all emitted events for later inspection.
pub struct CollectingEventEmitter {
    events: tokio::sync::Mutex<Vec<StreamEvent>>,
    seq_counter: AtomicU64,
}

impl CollectingEventEmitter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: tokio::sync::Mutex::new(Vec::new()),
            seq_counter: AtomicU64::new(0),
        }
    }

    pub async fn events(&self) -> Vec<StreamEvent> {
        self.events.lock().await.clone()
    }

    pub async fn clear(&self) {
        self.events.lock().await.clear();
    }
}

impl Default for CollectingEventEmitter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventEmitter for CollectingEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.events.lock().await.push(event);
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }
}

/// Wrapper for dynamic `EventEmitter` trait objects
///
/// This wrapper allows passing `Arc<dyn EventEmitter + Send + Sync>` to generic
/// functions that require `E: EventEmitter + Send + Sync + 'static`.
/// The wrapper is Sized and delegates all calls to the inner trait object.
pub struct DynEventEmitter {
    inner: Arc<dyn EventEmitter + Send + Sync>,
}

impl DynEventEmitter {
    /// Create a new wrapper around a dynamic `EventEmitter`
    pub fn new(emitter: Arc<dyn EventEmitter + Send + Sync>) -> Self {
        Self { inner: emitter }
    }
}

#[async_trait]
impl EventEmitter for DynEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        self.inner.emit(event).await
    }

    fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }
}
