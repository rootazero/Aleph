//! Perception Module
//!
//! Provides event classification, emission, and observation capabilities
//! for the Multi-Agent Resilience architecture.
//!
//! # Components
//!
//! - `classifier`: Skeleton & Pulse event classification
//! - `emitter`: Dual-write event emission with database persistence
//! - `observer`: Real-time event observation with gap-fill

mod classifier;
mod emitter;
mod observer;

pub use classifier::{EventClassifier, EventTier, EventType, PulseBuffer};
pub use emitter::{EmitterConfig, EventEmitter};
pub use observer::{GapFillResult, TaskObserver};
