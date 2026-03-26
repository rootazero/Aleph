//! EventEmitter implementations
//!
//! Concrete implementations of the `EventEmitter` trait:
//! - `GatewayEventEmitter` — broadcasts via the Gateway event bus
//! - `NoOpEventEmitter` — silent sink for testing / disabled streaming
//! - `CollectingEventEmitter` — collects events for test assertions
//! - `DynEventEmitter` — wrapper for trait objects

use async_trait::async_trait;
use std::time::Instant;
use tokio::sync::Mutex;

use crate::sync_primitives::{AtomicU64, Ordering};
use crate::sync_primitives::Arc;

use super::types::{EventEmitError, OutputMode, StreamEvent};
use super::{event_method, EventEmitter};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::JsonRpcRequest;

/// Gateway-based event emitter
///
/// Broadcasts events to all connected WebSocket clients via the event bus.
/// Supports throttled response chunk emission (150ms) for smoother streaming.
/// Respects `output_mode` configuration:
/// - Typewriter: stream chunks with 150ms throttling
/// - Instant: buffer all chunks, only emit on final
pub struct GatewayEventEmitter {
    pub(super) event_bus: Arc<GatewayEventBus>,
    pub(super) seq_counter: AtomicU64,
    // Throttling state for response chunks (typewriter mode)
    pub(super) delta_buffer: Mutex<String>,
    pub(super) last_delta_at: Mutex<Instant>,
    // Instant mode buffer for accumulating all chunks
    pub(super) instant_buffer: Mutex<String>,
    /// Output mode: typewriter (streaming) or instant (all-at-once)
    pub(super) output_mode: OutputMode,
}

impl GatewayEventEmitter {
    /// Delta event throttle interval (150ms like OpenClaw)
    pub(super) const DELTA_THROTTLE_MS: u64 = 150;

    pub fn new(event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            event_bus,
            seq_counter: AtomicU64::new(0),
            delta_buffer: Mutex::new(String::new()),
            last_delta_at: Mutex::new(Instant::now()),
            instant_buffer: Mutex::new(String::new()),
            output_mode: OutputMode::Typewriter,
        }
    }

    /// Create with a specific output mode
    pub fn with_output_mode(event_bus: Arc<GatewayEventBus>, output_mode: OutputMode) -> Self {
        Self {
            event_bus,
            seq_counter: AtomicU64::new(0),
            delta_buffer: Mutex::new(String::new()),
            last_delta_at: Mutex::new(Instant::now()),
            instant_buffer: Mutex::new(String::new()),
            output_mode,
        }
    }

    /// Get the current output mode
    pub fn output_mode(&self) -> &OutputMode {
        &self.output_mode
    }

    /// Emit response chunk with 150ms throttling
    ///
    /// Buffers chunks within the throttle window, sends accumulated content on boundary.
    /// Final chunks are always sent immediately with any buffered content.
    pub async fn emit_response_chunk_throttled(
        &self,
        run_id: &str,
        content: &str,
        chunk_index: u32,
        is_final: bool,
    ) {
        if is_final {
            // Always send final chunk immediately with any buffered content
            let mut buffer = self.delta_buffer.lock().await;
            let full_content = if buffer.is_empty() {
                content.to_string()
            } else {
                let buffered = std::mem::take(&mut *buffer);
                format!("{}{}", buffered, content)
            };
            drop(buffer);

            self.emit_response_chunk(run_id, &full_content, chunk_index, true, false)
                .await;
            return;
        }

        let now = Instant::now();
        let mut last_at = self.last_delta_at.lock().await;
        let elapsed = now.duration_since(*last_at).as_millis() as u64;

        if elapsed < Self::DELTA_THROTTLE_MS {
            // Buffer the content, don't send yet
            self.delta_buffer.lock().await.push_str(content);
            return;
        }

        // Send buffered + new content
        let mut buffer = self.delta_buffer.lock().await;
        let full_content = if buffer.is_empty() {
            content.to_string()
        } else {
            let buffered = std::mem::take(&mut *buffer);
            format!("{}{}", buffered, content)
        };
        drop(buffer);

        *last_at = now;
        drop(last_at);

        self.emit_response_chunk(run_id, &full_content, chunk_index, false, false)
            .await;
    }
}

#[async_trait]
impl EventEmitter for GatewayEventEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // In instant mode, buffer non-final ResponseChunks and only emit on final
        if self.output_mode == OutputMode::Instant {
            if let StreamEvent::ResponseChunk {
                ref content,
                is_final,
                is_intermediate,
                ref run_id,
                ..
            } = event
            {
                if is_intermediate {
                    if content.is_empty() {
                        // Intermediate boundary marker: flush accumulated buffer
                        // as an intermediate message, then clear it
                        let mut buffer = self.instant_buffer.lock().await;
                        let accumulated = std::mem::take(&mut *buffer);
                        drop(buffer);
                        if !accumulated.is_empty() {
                            let flush_event = StreamEvent::ResponseChunk {
                                run_id: run_id.clone(),
                                seq: self.next_seq(),
                                content: accumulated,
                                chunk_index: 0,
                                is_final: false,
                                is_intermediate: true,
                            };
                            let flush_value = serde_json::to_value(&flush_event)?;
                            let flush_notification = JsonRpcRequest::notification(event_method(&flush_event), Some(flush_value));
                            let flush_json = serde_json::to_string(&flush_notification)?;
                            self.event_bus.publish(flush_json);
                        }
                    } else {
                        // Non-empty intermediate: emit immediately as standalone message
                        let event_value = serde_json::to_value(&event)?;
                        let notification = JsonRpcRequest::notification(event_method(&event), Some(event_value));
                        let json = serde_json::to_string(&notification)?;
                        self.event_bus.publish(json);
                    }
                    return Ok(());
                } else if !is_final {
                    // Buffer the chunk content, don't emit yet
                    self.instant_buffer.lock().await.push_str(content);
                    return Ok(());
                }

                // Final chunk: combine buffered content + this chunk, emit as single response
                let mut buffer = self.instant_buffer.lock().await;
                let full_content = if buffer.is_empty() {
                    content.clone()
                } else {
                    let buffered = std::mem::take(&mut *buffer);
                    format!("{}{}", buffered, content)
                };
                drop(buffer);

                let final_event = StreamEvent::ResponseChunk {
                    run_id: run_id.clone(),
                    seq: self.next_seq(),
                    content: full_content,
                    chunk_index: 0,
                    is_final: true,
                    is_intermediate: false,
                };
                let event_value = serde_json::to_value(&final_event)?;
                let notification =
                    JsonRpcRequest::notification(event_method(&final_event), Some(event_value));
                let json = serde_json::to_string(&notification)?;
                self.event_bus.publish(json);
                return Ok(());
            }
        }

        // In instant mode, flush any buffered content on RunComplete
        if self.output_mode == OutputMode::Instant {
            if let StreamEvent::RunComplete { ref run_id, ref summary, .. } = event {
                let mut buffer = self.instant_buffer.lock().await;
                if !buffer.is_empty() {
                    let buffered = std::mem::take(&mut *buffer);
                    drop(buffer);
                    let flush_event = StreamEvent::ResponseChunk {
                        run_id: run_id.clone(),
                        seq: self.next_seq(),
                        content: buffered,
                        chunk_index: 0,
                        is_final: true,
                        is_intermediate: false,
                    };
                    let flush_value = serde_json::to_value(&flush_event)?;
                    let flush_notification =
                        JsonRpcRequest::notification(event_method(&flush_event), Some(flush_value));
                    let flush_json = serde_json::to_string(&flush_notification)?;
                    self.event_bus.publish(flush_json);
                } else if let Some(ref final_response) = summary.final_response {
                    // Fallback: buffer was empty (race with fire-and-forget emit),
                    // use final_response from summary
                    if !final_response.is_empty() {
                        drop(buffer);
                        let fallback_event = StreamEvent::ResponseChunk {
                            run_id: run_id.clone(),
                            seq: self.next_seq(),
                            content: final_response.clone(),
                            chunk_index: 0,
                            is_final: true,
                            is_intermediate: false,
                        };
                        let fb_value = serde_json::to_value(&fallback_event)?;
                        let fb_notification =
                            JsonRpcRequest::notification(event_method(&fallback_event), Some(fb_value));
                        let fb_json = serde_json::to_string(&fb_notification)?;
                        self.event_bus.publish(fb_json);
                    }
                }
            }
        }

        // Default: broadcast immediately (typewriter mode or non-ResponseChunk events)
        let event_value = serde_json::to_value(&event)?;
        let notification = JsonRpcRequest::notification(event_method(&event), Some(event_value));
        let json = serde_json::to_string(&notification)?;
        self.event_bus.publish(json);
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
    pub fn new() -> Self {
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

/// Wrapper for dynamic EventEmitter trait objects
///
/// This wrapper allows passing `Arc<dyn EventEmitter + Send + Sync>` to generic
/// functions that require `E: EventEmitter + Send + Sync + 'static`.
/// The wrapper is Sized and delegates all calls to the inner trait object.
pub struct DynEventEmitter {
    inner: Arc<dyn EventEmitter + Send + Sync>,
}

impl DynEventEmitter {
    /// Create a new wrapper around a dynamic EventEmitter
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
