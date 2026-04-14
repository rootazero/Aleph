//! Memory Compression Module
//!
//! Orchestrates compression of raw conversation memories into Knowledge Notes
//! via [`CompoundIngestor`](crate::memory::notes::ingest::CompoundIngestor).
//!
//! ## Components
//!
//! - [`CompressionService`]: Main service that orchestrates compression
//! - [`CompressionScheduler`]: Determines when to trigger compression
//! - [`SignalDetector`]: Keyword-based detection for smart compression triggers

mod extractor;
mod scheduler;
mod service;
pub mod signal_detector;
pub mod source_prompts;
mod trigger;

pub use extractor::FactExtractor;
pub use scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
pub use service::{CompressionConfig, CompressionService};
pub use signal_detector::{
    CompressionPriority, CompressionSignal, DetectionResult, SignalDetector, SignalKeywords,
};
pub use trigger::{CompressionAggressiveness, HybridTrigger, TriggerConfig, TriggerReason};
